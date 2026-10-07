//! The built-in tunnel inside the app (spec sections 7 and 8) with the fake cloudflared
//! (`examples/fake_cloudflared.rs`, copied into `<home>/tools` as `cloudflared.exe`). Never the
//! real cloudflared, never the internet.
//!
//! The fake is an example target: a plain `cargo test` builds it, a filtered `cargo test --lib`
//! does not — run `cargo build --example fake_cloudflared` first.

#![cfg(windows)]

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::*;
use crate::auto_pause::AutoPauseTimings;
use crate::tunnel::TunnelTimings;

const QUICK_HOST: &str = "proba-kanalu.trycloudflare.com";
const NAMED_HOST: &str = "kanal.example.com";
const TUNNEL_ID: &str = "6ff42ae2-765d-4adf-8112-31c55c1551ef";

fn fake_program() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    let target = exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target folder");
    let program = target.join("examples").join("fake_cloudflared.exe");
    assert!(
        program.exists(),
        "{} is missing: run `cargo build --example fake_cloudflared` (a plain `cargo test` builds it)",
        program.display()
    );
    program
}

/// The fake as `<home>/tools/cloudflared.exe`, with one script entry per start.
fn install_fake(home: &Path, runs: &Value) {
    let tools = home.join("tools");
    let program = tools.join("cloudflared.exe");
    if std::fs::hard_link(fake_program(), &program).is_err() {
        std::fs::copy(fake_program(), &program).unwrap();
    }
    std::fs::write(tools.join("fake_cloudflared.json"), runs.to_string()).unwrap();
}

fn fake_starts(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(home.join("tools").join("fake_cloudflared.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["event"] == "start")
        .collect()
}

fn args_of(start: &Value) -> Vec<String> {
    start["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

/// A fake instance holds `instance.lock` exclusively for its whole life.
fn fake_running(home: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    let lock = home.join("tools").join("instance.lock");
    lock.exists()
        && std::fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(&lock)
            .is_err()
}

fn assert_never_two(home: &Path) {
    let overlapping: Vec<Value> = fake_starts(home)
        .into_iter()
        .filter(|s| s["overlap"] != false)
        .collect();
    assert!(
        overlapping.is_empty(),
        "two cloudflared ran at once: {overlapping:?}"
    );
}

fn fast_tunnel() -> TunnelTimings {
    TunnelTimings {
        address_timeout: Duration::from_secs(10),
        poll_interval: Duration::from_millis(50),
        backoff_initial: Duration::from_millis(100),
        backoff_max: Duration::from_secs(2),
        stable_after: Duration::from_secs(60),
        rate_limit_wait: Duration::from_secs(60),
        health_grace: Duration::from_millis(200),
        reconnect_limit: Duration::from_secs(60),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

struct Setup {
    /// The fake's script; `None`: no cloudflared in the tools.
    runs: Option<Value>,
    /// Settings on top of `{"port": 0, "tunnelEnabled": true}`.
    settings: Value,
    /// Link the repo's ffmpeg (the channel runs); the test is skipped without it.
    tools: bool,
    auto_pause: AutoPauseTimings,
    /// The named tunnel's files in `<home>/tunnel` (as the wizard leaves them).
    named_files: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            runs: Some(json!([{ "hostname": QUICK_HOST }])),
            settings: json!({}),
            tools: false,
            auto_pause: AutoPauseTimings::default(),
            named_files: false,
        }
    }
}

/// The app with the tunnel on; `None` when the test needs ffmpeg and it is not there.
async fn start(setup: Setup) -> Option<App> {
    let mut linked = true;
    let app = App::start_with(|home, options| {
        let mut config = json!({"port": 0, "tunnelEnabled": true});
        for (k, v) in setup.settings.as_object().unwrap() {
            config[k] = v.clone();
        }
        std::fs::write(home.join("config.json"), config.to_string()).unwrap();
        if let Some(runs) = &setup.runs {
            install_fake(home, runs);
        }
        if setup.named_files {
            let dir = home.join("tunnel");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{TUNNEL_ID}.json")),
                "{\"TunnelSecret\":\"tajny-sekret\"}",
            )
            .unwrap();
            std::fs::write(dir.join("cert.pem"), "tajny-certyfikat").unwrap();
        }
        if setup.tools {
            linked = link_real_tools(home);
        }
        options.tunnel_timings = fast_tunnel();
        options.auto_pause_timings = setup.auto_pause;
    })
    .await;
    if !linked {
        eprintln!("pomijam: brak bin/ffmpeg.exe lub nie można utworzyć dowiązania");
        app.stop().await;
        return None;
    }
    Some(app)
}

async fn wait_online(app: &App, cookie: &str) -> Value {
    let mut last = Value::Null;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        last = app.get("/api/desktop/tunnel", cookie).await;
        if last["state"] == "online" {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the tunnel did not come online: {last}");
}

/// Pause reasons of the running channel (empty without one).
fn pause_reasons(core: &AppCore) -> Vec<&'static str> {
    core.live()
        .map(|l| {
            l.state
                .engine
                .snapshot
                .borrow()
                .state
                .pause_reasons
                .iter()
                .map(|r| r.as_str())
                .collect()
        })
        .unwrap_or_default()
}

fn decode_qr(png: &Path) -> Vec<String> {
    let image = image::open(png).unwrap().to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(image);
    prepared
        .detect_grids()
        .into_iter()
        .filter_map(|g| g.decode().ok().map(|(_, text)| text))
        .collect()
}

// ---------- Public address ----------

#[tokio::test]
async fn an_online_tunnel_gives_the_public_address_and_cloudflare_trust_live() {
    let app = start(Setup {
        settings: json!({"publicUrl": "https://reczny.example.com", "trustCloudflare": false}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    let tunnel = wait_online(&app, &cookie).await;
    let url = format!("https://{QUICK_HOST}");
    assert_eq!(tunnel["url"], url);
    assert_eq!(tunnel["mode"], "quick");
    assert_eq!(tunnel["hostname"], Value::Null);
    assert_eq!(tunnel["warning"], Value::Null);
    assert_eq!(tunnel["message"], Value::Null);
    // The tunnel's address and trust replace the manual ones while it is enabled.
    let live = &app.engine().core.live_settings;
    assert_eq!(live.public_url.get(), url);
    assert!(live.trust_cloudflare.get());
    let panel = app.get("/api/admin/channels/main", &cookie).await;
    assert_eq!(panel["channelUrl"], format!("{url}/live/main.m3u8"));
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["tunnel"]["state"], "online");
    assert_eq!(info["tunnel"]["url"], url);
    // The saved manual address is kept for when the tunnel is switched off.
    assert_eq!(app.saved()["publicUrl"], "https://reczny.example.com");
    // cloudflared runs for the local server, nothing restarted for the address.
    let starts = fake_starts(&app.home);
    assert_eq!(starts.len(), 1);
    let args = args_of(&starts[0]);
    assert_eq!(args[..3], ["tunnel", "--no-autoupdate", "--metrics"]);
    assert!(args[3].starts_with("127.0.0.1:"), "{args:?}");
    assert_eq!(
        args[4..],
        [
            "--url".to_string(),
            format!("http://127.0.0.1:{}", app.engine().port())
        ]
    );
    assert_eq!(app.engine().server_starts(), 1);
    app.stop().await;
}

/// The slates follow the effective address: "Łączenie z internetem…" while the tunnel has no
/// address yet, then the QR code of the tunnel's request page, then the manual address once
/// the tunnel is switched off.
#[tokio::test]
async fn the_slates_follow_the_tunnel_address() {
    let Some(app) = start(Setup {
        tools: true,
        settings: json!({"publicUrl": "https://reczny.example.com"}),
        ..Setup::default()
    })
    .await
    else {
        return;
    };
    let cookie = app.window_cookie().await;
    let slates = app.home.join("data").join("slates");
    let font = std::path::Path::new("C:/Windows/Fonts/arial.ttf").exists();
    // The start-up set was made before the tunnel had an address.
    assert!(!slates.join("idle").join("qr.png").exists());
    if font {
        assert_eq!(
            std::fs::read_to_string(slates.join("idle").join("subtitle.txt")).unwrap(),
            "Łączenie z internetem…"
        );
    }
    wait_online(&app, &cookie).await;
    eventually(90, "the slates for the tunnel address", || async {
        slates.join("g1").join("idle").join("qr.png").is_file()
    })
    .await;
    assert_eq!(
        decode_qr(&slates.join("g1").join("idle").join("qr.png")),
        [format!("https://{QUICK_HOST}/")]
    );

    let (status, res) = app
        .put_config(&cookie, json!({"tunnelEnabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["restarted"], false);
    eventually(90, "the slates for the manual address", || async {
        slates.join("g2").join("idle").join("qr.png").is_file()
    })
    .await;
    assert_eq!(
        decode_qr(&slates.join("g2").join("idle").join("qr.png")),
        ["https://reczny.example.com/".to_string()]
    );
    assert_eq!(app.engine().server_starts(), 1);
    assert_eq!(
        app.engine().channel_loop_counter().load(Ordering::SeqCst),
        1
    );
    app.stop().await;
}

#[tokio::test]
async fn switching_the_tunnel_off_stops_cloudflared_and_the_manual_address_applies() {
    let app = start(Setup {
        settings: json!({"publicUrl": "https://reczny.example.com", "trustCloudflare": false}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    assert!(fake_running(&app.home));

    let (status, res) = app
        .put_config(&cookie, json!({"tunnelEnabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["tunnelEnabled"], false);
    assert_eq!(res["restarted"], false);
    let tunnel = app.get("/api/desktop/tunnel", &cookie).await;
    assert_eq!(tunnel["state"], "off");
    assert_eq!(tunnel["url"], Value::Null);
    assert!(!fake_running(&app.home), "cloudflared still runs");
    let live = &app.engine().core.live_settings;
    assert_eq!(live.public_url.get(), "https://reczny.example.com");
    assert!(!live.trust_cloudflare.get());
    let panel = app.get("/api/admin/channels/main", &cookie).await;
    assert_eq!(
        panel["channelUrl"],
        "https://reczny.example.com/live/main.m3u8"
    );

    // On again: a new cloudflared, online again.
    let (status, _) = app
        .put_config(&cookie, json!({"tunnelEnabled": true}))
        .await;
    assert_eq!(status, StatusCode::OK);
    wait_online(&app, &cookie).await;
    assert_eq!(fake_starts(&app.home).len(), 2);
    assert_never_two(&app.home);
    assert_eq!(app.engine().server_starts(), 1);
    app.stop().await;
}

/// Settings that do not concern the tunnel never restart cloudflared (a quick tunnel would get
/// a new address); a new port re-points it, with one process at a time.
#[tokio::test]
async fn a_new_port_re_points_the_tunnel_and_other_settings_leave_it_alone() {
    let app = start(Setup::default()).await.unwrap();
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    for body in [
        json!({"pauseWhenNoViewers": true}),
        json!({"pauseOnTunnelLoss": false}),
        json!({"publicUrl": "https://reczny.example.com"}),
        json!({"tunnelEnabled": true}),
    ] {
        let (status, res) = app.put_config(&cookie, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}: {res}");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fake_starts(&app.home).len(), 1, "cloudflared was restarted");

    let new = free_port();
    let (status, res) = app.put_config(&cookie, json!({"port": new})).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["restarted"], true);
    eventually(20, "a cloudflared for the new port", || async {
        fake_starts(&app.home).len() >= 2
    })
    .await;
    wait_online(&app, &cookie).await;
    let starts = fake_starts(&app.home);
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert!(
        args_of(&starts[1]).contains(&format!("http://127.0.0.1:{new}")),
        "{starts:?}"
    );
    assert_never_two(&app.home);
    app.stop().await;
}

#[tokio::test]
async fn quitting_the_app_stops_cloudflared() {
    let mut app = start(Setup::default()).await.unwrap();
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    assert!(fake_running(&app.home));
    let engine = app.engine.take().unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(30),
            engine.shutdown_within(Duration::from_secs(20))
        )
        .await
        .unwrap()
    );
    assert!(!fake_running(&app.home), "cloudflared outlived the app");
    assert_eq!(fake_starts(&app.home).len(), 1);
}

#[tokio::test]
async fn without_cloudflared_the_tunnel_shows_why_and_the_app_runs_on() {
    let app = start(Setup {
        runs: None,
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    let tunnel = app.get("/api/desktop/tunnel", &cookie).await;
    assert_eq!(tunnel["state"], "error", "{tunnel}");
    assert!(
        tunnel["message"].as_str().unwrap().contains("cloudflared"),
        "{tunnel}"
    );
    assert_eq!(tunnel["url"], Value::Null);
    let page = app.http.get(app.url("/")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    // No address while enabled: links use the address the request came to.
    assert_eq!(app.engine().core.live_settings.public_url.get(), "");
    app.stop().await;
}

#[tokio::test]
async fn the_restart_route_starts_a_new_cloudflared_and_needs_the_tunnel_on() {
    let app = start(Setup::default()).await.unwrap();
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    let (status, res) = app
        .call("POST", "/api/desktop/tunnel/restart", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert!(res.get("state").is_some(), "{res}");
    wait_online(&app, &cookie).await;
    assert_eq!(fake_starts(&app.home).len(), 2);
    assert_never_two(&app.home);

    let (status, _) = app
        .put_config(&cookie, json!({"tunnelEnabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, res) = app
        .call("POST", "/api/desktop/tunnel/restart", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{res}");
    assert_eq!(fake_starts(&app.home).len(), 2);
    app.stop().await;
}

// ---------- Named tunnel ----------

#[tokio::test]
async fn the_named_mode_without_its_files_runs_the_quick_tunnel_with_a_warning() {
    let app = start(Setup {
        settings: json!({"tunnelMode": "named", "namedTunnelHostname": NAMED_HOST}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    let tunnel = wait_online(&app, &cookie).await;
    assert_eq!(tunnel["url"], format!("https://{QUICK_HOST}"));
    assert_eq!(tunnel["mode"], "quick");
    assert!(
        tunnel["warning"].as_str().unwrap().contains("szybki"),
        "{tunnel}"
    );
    assert!(!args_of(&fake_starts(&app.home)[0]).contains(&"run".to_string()));
    // The setting stays: the wizard's files may come back.
    assert_eq!(app.saved()["tunnelMode"], "named");
    app.stop().await;
}

#[tokio::test]
async fn the_named_mode_runs_the_named_tunnel_and_never_shows_its_credentials() {
    let app = start(Setup {
        settings: json!({"tunnelMode": "named", "namedTunnelHostname": NAMED_HOST}),
        // `/ready` answers 200: the named tunnel is connected.
        runs: Some(json!([{}])),
        named_files: true,
        ..Setup::default()
    })
    .await
    .unwrap();
    let credentials = app.home.join("tunnel").join(format!("{TUNNEL_ID}.json"));
    let cookie = app.window_cookie().await;
    let tunnel = wait_online(&app, &cookie).await;
    assert_eq!(tunnel["url"], format!("https://{NAMED_HOST}"));
    assert_eq!(tunnel["mode"], "named");
    assert_eq!(tunnel["hostname"], NAMED_HOST);
    assert_eq!(tunnel["warning"], Value::Null);
    let last = fake_starts(&app.home).pop().unwrap();
    let args = args_of(&last);
    let at = args.iter().position(|a| a == "run").expect("tunnel run");
    assert_eq!(args[at + 1], "--credentials-file");
    assert_eq!(PathBuf::from(&args[at + 2]), credentials);
    assert_eq!(args.last().unwrap(), TUNNEL_ID);
    for body in [
        app.get("/api/desktop/tunnel", &cookie).await,
        app.get("/api/desktop/info", &cookie).await,
        app.get("/api/desktop/config", &cookie).await,
    ] {
        let text = body.to_string();
        for secret in ["tajny", "credentials", ".json", "cert.pem"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }
    // Back to quick (the files stay): the mode change re-points the running tunnel.
    let (status, _) = app
        .put_config(&cookie, json!({"tunnelMode": "quick"}))
        .await;
    assert_eq!(status, StatusCode::OK);
    eventually(20, "a quick cloudflared", || async {
        fake_starts(&app.home).len() == 2
    })
    .await;
    assert!(!args_of(&fake_starts(&app.home)[1]).contains(&"run".to_string()));
    assert!(credentials.is_file());
    assert_never_two(&app.home);
    app.stop().await;
}

// ---------- Automatic pause (Review Focus 4) ----------

/// Review Focus 4 with the real channel and the fake cloudflared losing its connection twice:
/// a loss shorter than the limit does not pause; a longer one pauses; the return resumes.
#[tokio::test]
async fn a_short_tunnel_loss_does_not_pause_a_long_one_does_and_the_return_resumes() {
    let Some(app) = start(Setup {
        tools: true,
        // `/ready` fails 4.0–4.6 s (≈ 0.4 s "Reconnecting" after the 0.2 s grace) and 8–12 s.
        runs: Some(json!([{
            "hostname": QUICK_HOST,
            "ready_down": [[4000, 4600], [8000, 12000]],
        }])),
        auto_pause: AutoPauseTimings {
            tunnel_loss: Duration::from_millis(1500),
            ..AutoPauseTimings::default()
        },
        ..Setup::default()
    })
    .await
    else {
        return;
    };
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    let started_ms = fake_starts(&app.home)[0]["ms"].as_u64().unwrap();
    let core = Arc::clone(&app.engine().core);
    // (ms after the fake's start, tunnel state, pause reasons), every 25 ms.
    let mut samples: Vec<(u64, String, Vec<&str>)> = Vec::new();
    while now_ms() < started_ms + 14_000 {
        let state = core.tunnel_state();
        let name = format!("{state:?}");
        samples.push((now_ms() - started_ms, name, pause_reasons(&core)));
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let lost = |from: u64, to: u64| {
        samples
            .iter()
            .any(|(t, s, _)| (from..to).contains(t) && s.starts_with("Reconnecting"))
    };
    assert!(
        lost(4000, 5000),
        "the short loss did not happen: {samples:?}"
    );
    assert!(
        lost(8000, 12_000),
        "the long loss did not happen: {samples:?}"
    );
    let first_pause = samples
        .iter()
        .find(|(_, _, r)| r.contains(&"tunnel"))
        .map(|(t, _, _)| *t)
        .unwrap_or_else(|| panic!("never paused: {samples:?}"));
    // ≈ 8.0 s + 0.2 s grace + 1.5 s limit; the short loss would have paused near 5.7 s.
    assert!(
        (9_000..12_000).contains(&first_pause),
        "paused at {first_pause} ms: {samples:?}"
    );
    eventually(10, "the resume after the tunnel returned", || async {
        pause_reasons(&core).is_empty()
    })
    .await;
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["paused"], false);
    assert!(app.log_text().contains("Automatyczna pauza"));
    app.stop().await;
}

/// No segment fetched for the limit: paused; the pause survives a server restart (the new
/// channel gets the reason again); the first segment fetch resumes without waiting for the
/// periodic check (set to 10 s here).
#[tokio::test]
async fn no_viewers_pause_survive_a_restart_and_a_segment_fetch_resumes_at_once() {
    let Some(app) = start(Setup {
        tools: true,
        runs: None,
        settings: json!({"tunnelEnabled": false, "pauseWhenNoViewers": true}),
        auto_pause: AutoPauseTimings {
            no_viewers: Duration::from_secs(2),
            recheck: Duration::from_secs(10),
            ..AutoPauseTimings::default()
        },
        named_files: false,
    })
    .await
    else {
        return;
    };
    let cookie = app.window_cookie().await;
    let core = Arc::clone(&app.engine().core);
    eventually(30, "the no-viewers pause", || async {
        pause_reasons(&core) == ["no_viewers"]
    })
    .await;
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["paused"], true);
    assert_eq!(info["pauseReasons"], json!(["no_viewers"]));

    app.engine().restart().await.unwrap();
    assert_eq!(app.engine().server_starts(), 2);
    eventually(5, "the reason on the new channel", || async {
        pause_reasons(&core) == ["no_viewers"]
    })
    .await;

    let playlist = app
        .http
        .get(app.url("/live/main.m3u8"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let segment = playlist
        .lines()
        .rfind(|l| l.ends_with(".ts"))
        .unwrap()
        .to_string();
    let fetched = Instant::now();
    let res = app
        .http
        .get(app.url(&format!("/live/{segment}")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{segment}");
    eventually(5, "the resume after a fetch", || async {
        pause_reasons(&core).is_empty()
    })
    .await;
    assert!(
        fetched.elapsed() < Duration::from_secs(2),
        "the resume waited {:?}",
        fetched.elapsed()
    );
    app.stop().await;
}
