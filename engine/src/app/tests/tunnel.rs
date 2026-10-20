//! The built-in tunnel inside the app (spec sections 7 and 8) with the fake cloudflared
//! (`examples/fake_cloudflared.rs`, copied into `<home>/tools` as `cloudflared.exe`). Never the
//! real cloudflared, never the internet.
//!
//! The fake is an example target: a plain `cargo test` builds it, a filtered `cargo test --lib`
//! does not — run `cargo build --example fake_cloudflared` first.

#![cfg(windows)]

use std::time::Instant;

use super::*;
use crate::auto_pause::AutoPauseTimings;
use crate::tunnel::TunnelTimings;

pub(super) const QUICK_HOST: &str = "proba-kanalu.trycloudflare.com";
pub(super) const NAMED_HOST: &str = "kanal.example.com";
pub(super) const TUNNEL_ID: &str = "6ff42ae2-765d-4adf-8112-31c55c1551ef";

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

pub(super) fn fake_starts(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(home.join("tools").join("fake_cloudflared.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["event"] == "start")
        .collect()
}

pub(super) fn args_of(start: &Value) -> Vec<String> {
    start["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

/// A fake instance holds `instance.lock` exclusively for its whole life.
pub(super) fn fake_running(home: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    let lock = home.join("tools").join("instance.lock");
    lock.exists()
        && std::fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(&lock)
            .is_err()
}

pub(super) fn assert_never_two(home: &Path) {
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

pub(super) struct Setup {
    /// The fake's script; `None`: no cloudflared in the tools.
    pub(super) runs: Option<Value>,
    /// Settings on top of `{"port": 0, "tunnelEnabled": true}`.
    pub(super) settings: Value,
    /// Link the repo's ffmpeg (the channel runs); the test is skipped without it.
    pub(super) tools: bool,
    pub(super) auto_pause: AutoPauseTimings,
    /// The named tunnel's files in `<home>/tunnel` (as the wizard leaves them).
    pub(super) named_files: bool,
    pub(super) tunnel: TunnelTimings,
    /// The fake's management commands (`fake_cloudflared_commands.json`); the login writes its
    /// certificate into the test's own cloudflared folder.
    pub(super) commands: Value,
    pub(super) login_timeout: Duration,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            runs: Some(json!([{ "hostname": QUICK_HOST }])),
            settings: json!({}),
            tools: false,
            auto_pause: AutoPauseTimings::default(),
            named_files: false,
            tunnel: fast_tunnel(),
            commands: json!({}),
            login_timeout: Duration::from_secs(30),
        }
    }
}

/// The app with the tunnel on; `None` when the test needs ffmpeg and it is not there.
pub(super) async fn start(setup: Setup) -> Option<App> {
    let mut linked = true;
    // The real ffmpeg: one of the few heavy apps at a time.
    let heavy = if setup.tools {
        Some(HEAVY.acquire().await.unwrap())
    } else {
        None
    };
    let mut app = App::start_with(|home, options| {
        let mut config = json!({"port": 0, "tunnelEnabled": true});
        for (k, v) in setup.settings.as_object().unwrap() {
            config[k] = v.clone();
        }
        std::fs::write(home.join("config.json"), config.to_string()).unwrap();
        if let Some(runs) = &setup.runs {
            install_fake(home, runs);
            let user_dir = options.named_tunnel.user_dir.clone().unwrap();
            let mut commands = setup.commands.clone();
            commands["login"]["cert_dir"] = json!(user_dir);
            std::fs::write(
                home.join("tools").join("fake_cloudflared_commands.json"),
                commands.to_string(),
            )
            .unwrap();
        }
        options.named_tunnel.login_timeout = setup.login_timeout;
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
        options.tunnel_timings = setup.tunnel;
        options.auto_pause_timings = setup.auto_pause;
    })
    .await;
    app.heavy = heavy;
    if !linked {
        eprintln!("pomijam: brak bin/ffmpeg.exe lub nie można utworzyć dowiązania");
        app.stop().await;
        return None;
    }
    Some(app)
}

pub(super) async fn wait_online(app: &App, cookie: &str) -> Value {
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
    assert_eq!(info["tunnelEnabled"], true);
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
            "Connecting to the internet…"
        );
    }
    wait_online(&app, &cookie).await;
    let tunnel_set = slates_for(&slates, &format!("https://{QUICK_HOST}")).await;

    let (status, res) = app
        .put_config(&cookie, json!({"tunnelEnabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["restarted"], false);
    let manual_set = slates_for(&slates, "https://reczny.example.com").await;
    assert_ne!(manual_set, tunnel_set);
    assert_eq!(app.engine().server_starts(), 1);
    assert_eq!(
        app.engine().channel_loop_counter().load(Ordering::SeqCst),
        1
    );
    app.stop().await;
}

/// Spec sections 2 and 10: the remote panel switches and reconnects the tunnel (the panel asks
/// first when the session came through it, which the environment and the panel's info tell);
/// the program's language and the tunnel's state reach a remote session.
#[tokio::test]
async fn a_remote_session_sees_the_tunnel_switches_it_and_knows_it_came_through_it() {
    let app = start(Setup::default()).await.unwrap();
    let window = app.window_cookie().await;
    wait_online(&app, &window).await;
    let (status, _) = app
        .put_config(
            &window,
            json!({"remoteAdmin": true, "password": "haslo-zdalne-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, remote) = app.remote_login("haslo-zdalne-1").await;
    assert_eq!(status, StatusCode::OK);
    let env = |cf: bool| {
        let mut req = app
            .http
            .get(app.url("/api/admin/environment"))
            .header("cookie", &remote);
        if cf {
            req = req.header("cf-connecting-ip", "203.0.113.7");
        }
        async move { req.send().await.unwrap().json::<Value>().await.unwrap() }
    };
    // Through cloudflared (a loopback peer with Cloudflare's header).
    let through = env(true).await;
    assert_eq!(through["sessionViaTunnel"], true, "{through}");
    assert_eq!(through["window"], true);
    assert_eq!(through["language"], "pl");
    assert_eq!(through["tunnelEnabled"], true);
    assert_eq!(through["tunnel"]["state"], "online");
    assert_eq!(through["tunnel"]["url"], format!("https://{QUICK_HOST}"));
    assert_eq!(through["channelRunning"], false);
    assert_eq!(through["restarting"], false);
    assert_eq!(env(false).await["sessionViaTunnel"], false);
    // The window's info says the same for the window: never through the tunnel.
    let info = app.get("/api/desktop/info", &window).await;
    assert_eq!(info["sessionViaTunnel"], false);
    // The panel's info (the same as the window's) tells a remote session too.
    let info = app
        .http
        .get(app.url("/api/admin/info"))
        .header("cookie", &remote)
        .header("cf-connecting-ip", "203.0.113.7")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(info["sessionViaTunnel"], true, "{info}");
    assert_eq!(info["tunnel"]["state"], "online");

    // Reconnecting from the panel: a new cloudflared, online again.
    let (status, res) = app
        .call("POST", "/api/admin/tunnel/restart", Some(&remote), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    wait_online(&app, &window).await;
    assert_eq!(fake_starts(&app.home).len(), 2);

    let (status, res) = app
        .call(
            "PUT",
            "/api/admin/config",
            Some(&remote),
            Some(json!({"tunnelEnabled": false})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["tunnelEnabled"], false);
    assert_eq!(
        app.get("/api/desktop/tunnel", &window).await["state"],
        "off"
    );
    assert_eq!(app.saved()["tunnelEnabled"], false);
    assert!(!fake_running(&app.home), "cloudflared still runs");
    // Without the tunnel the header proves nothing.
    assert_eq!(env(true).await["sessionViaTunnel"], false);

    let (status, res) = app
        .call(
            "PUT",
            "/api/admin/config",
            Some(&remote),
            Some(json!({"tunnelEnabled": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["tunnelEnabled"], true);
    assert_eq!(app.saved()["tunnelEnabled"], true);
    // Still nothing else of the window's settings for a remote session.
    let (status, _) = app
        .call(
            "PUT",
            "/api/desktop/config",
            Some(&remote),
            Some(json!({"tunnelEnabled": false})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
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
async fn the_qr_route_draws_the_request_page_of_the_address_in_effect() {
    let app = start(Setup {
        settings: json!({"publicUrl": "https://reczny.example.com"}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    let expected = format!("https://{QUICK_HOST}");
    assert_eq!(app.engine().public_url(), expected);
    let res = app
        .http
        .get(app.url("/api/desktop/tunnel/qr"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "image/png");
    let png = app.tmp.path().join("qr.png");
    std::fs::write(&png, res.bytes().await.unwrap()).unwrap();
    assert_eq!(decode_qr(&png), [format!("{expected}/")]);

    // Tunnel off: the manual address is in effect.
    let (status, _) = app
        .put_config(&cookie, json!({"tunnelEnabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.engine().public_url(), "https://reczny.example.com");
    app.stop().await;
}

#[tokio::test]
async fn the_qr_route_has_nothing_to_draw_without_an_address() {
    let app = start(Setup {
        runs: None,
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    assert_eq!(app.engine().public_url(), "");
    let (status, body) = app
        .call("GET", "/api/desktop/tunnel/qr", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"].as_str().unwrap().contains("adres"), "{body}");
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

/// Fix round 1: a short loss of the connection (cloudflared alive, `Reconnecting`, back
/// `Online` with the same address) changes neither the public address nor the slates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_short_reconnect_keeps_the_address_and_the_slates() {
    let Some(app) = start(Setup {
        tools: true,
        // The connection is lost while the test keeps `offline` (its own clock, not the
        // fake's: however long the slates took under load).
        runs: Some(json!([{
            "hostname": QUICK_HOST,
            "ready_down_while": "offline",
        }])),
        ..Setup::default()
    })
    .await
    else {
        return;
    };
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    let url = format!("https://{QUICK_HOST}");
    let slates = app.home.join("data").join("slates");
    let set = slates_for(&slates, &url).await;
    let core = Arc::clone(&app.engine().core);
    let offline = app.home.join("tools").join("offline");
    std::fs::write(&offline, "").unwrap();
    // Lost (`Reconnecting`) and kept so for about 2 s; the address stays all along.
    let mut lost_at = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while lost_at.is_none_or(|t: Instant| t.elapsed() < Duration::from_secs(2)) {
        assert!(Instant::now() < deadline, "the loss did not happen");
        if lost_at.is_none() && core.tunnel_state() == TunnelState::Reconnecting {
            lost_at = Some(Instant::now());
        }
        assert_eq!(core.live_settings.public_url.get(), url);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    std::fs::remove_file(&offline).unwrap();
    let back = Instant::now() + Duration::from_secs(20);
    while core.tunnel_state() != (TunnelState::Online { url: url.clone() }) {
        assert!(Instant::now() < back, "the connection did not come back");
        assert_eq!(core.live_settings.public_url.get(), url);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        newest_set(&slates),
        Some(set),
        "a slate set was made for the short loss"
    );
    assert_eq!(fake_starts(&app.home).len(), 1);
    app.stop().await;
}

/// Fix round 1: a tunnel sync while the server restarts (no server published yet) does not
/// move cloudflared to the new port early; the restart's own sync does, once.
#[tokio::test]
async fn a_sync_during_a_server_restart_leaves_cloudflared_for_the_restart() {
    let app = start(Setup::default()).await.unwrap();
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    let core = Arc::clone(&app.engine().core);
    let new = free_port();
    core.update_settings(|s| s.port = new).unwrap();
    let (reached, release) = core.pause_next_start();
    let restart = tokio::spawn({
        let core = Arc::clone(&core);
        async move { core.restart_server().await }
    });
    reached.await.unwrap();
    let old = core.tunnel_running.lock().await.as_ref().unwrap().port;
    assert_ne!(old, new);
    // E.g. a settings change or a finished tools job meanwhile.
    core.sync_tunnel().await;
    assert_eq!(
        core.tunnel_running.lock().await.as_ref().unwrap().port,
        old,
        "cloudflared was moved before the new server was up"
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        fake_starts(&app.home).len(),
        1,
        "cloudflared restarted while the server was restarting"
    );
    release.send(()).unwrap();
    restart.await.unwrap().unwrap();
    eventually(20, "a cloudflared for the new port", || async {
        fake_starts(&app.home).len() >= 2
    })
    .await;
    wait_online(&app, &cookie).await;
    let starts = fake_starts(&app.home);
    assert_eq!(starts.len(), 2);
    assert!(args_of(&starts[1]).contains(&format!("http://127.0.0.1:{new}")));
    assert_never_two(&app.home);
    app.stop().await;
}

/// Fix round 1: updating cloudflared while the tunnel runs stops it first (the running exe
/// would keep the old program alive) and starts the new one after the swap.
#[tokio::test]
async fn updating_cloudflared_restarts_the_tunnel_on_the_new_program() {
    let server = TestServer::start().await;
    server.put(
        "cf/cloudflared-windows-amd64.exe",
        std::fs::read(fake_program()).unwrap(),
    );
    let base = server.base.clone();
    let app = App::start_with(|home, options| {
        std::fs::write(
            home.join("config.json"),
            json!({"port": 0, "tunnelEnabled": true}).to_string(),
        )
        .unwrap();
        // Start 1: the tunnel; start 2: `--version` after the install; start 3: the tunnel.
        install_fake(
            home,
            &json!([
                { "hostname": QUICK_HOST },
                { "serve_metrics": false, "exit_after_ms": 0 },
                { "hostname": QUICK_HOST }
            ]),
        );
        options.tunnel_timings = fast_tunnel();
        options.tool_sources.cloudflared_base = format!("{base}/cf");
        options.tool_sources.cloudflared_release_api = format!("{base}/cf-api");
    })
    .await;
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    let (status, res) = app
        .call(
            "POST",
            "/api/desktop/tools/update",
            Some(&cookie),
            Some(json!({"tool": "cloudflared"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{res}");
    eventually(60, "the tools job to finish", || async {
        app.call("GET", "/api/desktop/tools", Some(&cookie), None)
            .await
            .1["running"]
            == false
    })
    .await;
    let progress = app.get("/api/desktop/tools/progress", &cookie).await;
    assert_eq!(progress["error"], Value::Null, "{progress}");
    eventually(20, "the tunnel on the new program", || async {
        fake_starts(&app.home).len() >= 3
    })
    .await;
    wait_online(&app, &cookie).await;
    let starts = fake_starts(&app.home);
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_eq!(args_of(&starts[1]), ["--version"]);
    assert_eq!(args_of(&starts[2])[0], "tunnel");
    assert_never_two(&app.home);
    app.stop().await;
}

/// Fix round 2: a quick tunnel's address dies with its cloudflared: after the process exits
/// the public address is none (not the dead URL) until the new process is online.
#[tokio::test]
async fn a_quick_tunnel_that_exits_drops_its_dead_address() {
    let first = "pierwszy.trycloudflare.com";
    let second = "drugi.trycloudflare.com";
    let app = start(Setup {
        runs: Some(json!([
            { "hostname": first, "exit_after_ms": 2500, "exit_code": 1 },
            { "hostname": second, "delay_ms": 3000 }
        ])),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    let tunnel = wait_online(&app, &cookie).await;
    assert_eq!(tunnel["url"], format!("https://{first}"));
    let log = app.home.join("tools").join("fake_cloudflared.log");
    eventually(10, "the first cloudflared to exit", || async {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("\"event\":\"exit\"")
    })
    .await;
    let core = &app.engine().core;
    // The exit is noticed within moments (the new process waits 3 s before it has an address).
    eventually(2, "the exit noticed", || async {
        core.tunnel_state().url().is_none()
    })
    .await;
    assert_eq!(
        core.live_settings.public_url.get(),
        "",
        "the dead quick address is still in effect"
    );
    let tunnel = wait_online(&app, &cookie).await;
    assert_eq!(tunnel["url"], format!("https://{second}"));
    assert_eq!(
        core.live_settings.public_url.get(),
        format!("https://{second}")
    );
    app.stop().await;
}

/// Fix round 2: a reason active when a restarted server was built, cleared before that server
/// was published (a viewer fetched a segment from it), does not leave its channel paused.
#[tokio::test]
async fn a_reason_cleared_during_a_server_restart_is_resumed_on_the_new_channel() {
    let Some(app) = start(Setup {
        tools: true,
        runs: None,
        settings: json!({"tunnelEnabled": false, "pauseWhenNoViewers": true}),
        auto_pause: AutoPauseTimings {
            no_viewers: Duration::from_secs(6),
            recheck: Duration::from_secs(10),
            ..AutoPauseTimings::default()
        },
        ..Setup::default()
    })
    .await
    else {
        return;
    };
    let core = Arc::clone(&app.engine().core);
    eventually(30, "the no-viewers pause", || async {
        pause_reasons(&core) == ["no_viewers"]
    })
    .await;
    let (reached, release) = core.pause_next_start();
    let restart = tokio::spawn({
        let core = Arc::clone(&core);
        async move { core.restart_server().await }
    });
    // The new server serves (its channel started paused) but is not published yet.
    reached.await.unwrap();
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
    let res = app
        .http
        .get(app.url(&format!("/live/{segment}")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let fetched = Instant::now();
    tokio::time::sleep(Duration::from_millis(200)).await;
    release.send(()).unwrap();
    restart.await.unwrap().unwrap();
    eventually(3, "the new channel resumed", || async {
        pause_reasons(&core).is_empty()
    })
    .await;
    assert!(
        fetched.elapsed() < Duration::from_secs(6),
        "checked after the next no-viewers pause could come"
    );
    app.stop().await;
}

/// Fix round 3: the quick cloudflared dies while the state is already `Reconnecting` (killed
/// after `reconnect_limit`; the next one never gets an address): the state does not change,
/// but its address is dead — the public address is cleared and the slates say "Łączenie z
/// internetem…".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quick_tunnel_killed_while_reconnecting_drops_its_dead_address() {
    let Some(app) = start(Setup {
        tools: true,
        // The connection is lost once the test creates `offline` (never given back).
        runs: Some(json!([
            { "hostname": QUICK_HOST, "ready_down_while": "offline" },
            { "hostname": "" }
        ])),
        tunnel: TunnelTimings {
            reconnect_limit: Duration::from_millis(1500),
            ..fast_tunnel()
        },
        ..Setup::default()
    })
    .await
    else {
        return;
    };
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    let slates = app.home.join("data").join("slates");
    let first = slates_for(&slates, &format!("https://{QUICK_HOST}")).await;
    // Only now (however long the set took under load) the connection goes.
    std::fs::write(app.home.join("tools").join("offline"), "").unwrap();
    let core = Arc::clone(&app.engine().core);
    eventually(20, "the second cloudflared", || async {
        fake_starts(&app.home).len() >= 2
    })
    .await;
    eventually(5, "the dead address cleared", || async {
        core.live_settings.public_url.get().is_empty()
    })
    .await;
    assert_ne!(
        core.tunnel_state(),
        TunnelState::Online {
            url: format!("https://{QUICK_HOST}")
        }
    );
    let connecting = slates_without_code(&slates, &first).await;
    if std::path::Path::new("C:/Windows/Fonts/arial.ttf").exists() {
        assert_eq!(
            std::fs::read_to_string(connecting.join("idle").join("subtitle.txt")).unwrap(),
            "Connecting to the internet…"
        );
    }
    app.stop().await;
}

/// The info ticker on, with its QR code (the case where a new address restarts the film).
async fn ticker_with_code(app: &App, cookie: &str) {
    let (status, res) = app
        .call(
            "PUT",
            "/api/admin/channels/main/settings",
            Some(cookie),
            Some(json!({"tickerEnabled": true, "tickerQr": "always"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
}

/// How many times a new public address restarted the film for the ticker's code.
fn ticker_redraws(app: &App) -> usize {
    app.engine().core.live().unwrap().state.ticker_redraws()
}

/// The newest slate set, once it shows a QR code that decodes to `url`'s request page.
async fn slates_for(slates: &Path, url: &str) -> PathBuf {
    let page = format!("{url}/");
    eventually(90, &format!("the slates for {url}"), || async {
        newest_set(slates).is_some_and(|set| {
            let qr = set.join("idle").join("qr.png");
            qr.is_file() && decode_qr(&qr) == [page.clone()]
        })
    })
    .await;
    newest_set(slates).unwrap()
}

/// Waits until `set` was handed to the channel (the log names its number; whatever follows
/// the swap, such as the ticker's restart, runs right after).
async fn handed_over(app: &App, set: &Path) {
    let n = set.file_name().unwrap().to_str().unwrap()[1..].to_string();
    eventually(120, &format!("set {n} on air"), || async {
        app.log_text().contains(&format!("(set {n})"))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// The newest slate set, once it is a set without a code made after `previous`.
async fn slates_without_code(slates: &Path, previous: &Path) -> PathBuf {
    eventually(90, "the slates without a code", || async {
        newest_set(slates).is_some_and(|set| {
            set != previous
                && set.join("idle").join("title.txt").is_file()
                && !set.join("idle").join("qr.png").exists()
        })
    })
    .await;
    newest_set(slates).unwrap()
}

/// Final wave I1: with the ticker drawing its code, a quick tunnel that loses its address
/// and gets a new one restarts the film once (for the new address), not also for the gap in
/// between (the connecting slates).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnect_to_a_new_address_restarts_the_film_for_the_ticker_once() {
    let second = "drugi.trycloudflare.com";
    let Some(app) = start(Setup {
        tools: true,
        settings: json!({"pauseOnTunnelLoss": false}),
        // The connection is lost once the test creates `offline`; the next cloudflared gets
        // another address once the file is gone.
        runs: Some(json!([
            { "hostname": QUICK_HOST, "ready_down_while": "offline" },
            { "hostname": second, "no_hostname_while": "offline" }
        ])),
        tunnel: TunnelTimings {
            reconnect_limit: Duration::from_millis(1500),
            ..fast_tunnel()
        },
        ..Setup::default()
    })
    .await
    else {
        return;
    };
    let cookie = app.window_cookie().await;
    ticker_with_code(&app, &cookie).await;
    wait_online(&app, &cookie).await;
    let slates = app.home.join("data").join("slates");
    let first = slates_for(&slates, &format!("https://{QUICK_HOST}")).await;
    handed_over(&app, &first).await;
    let before = ticker_redraws(&app);
    let offline = app.home.join("tools").join("offline");
    std::fs::write(&offline, "").unwrap();
    eventually(20, "the second cloudflared", || async {
        fake_starts(&app.home).len() >= 2
    })
    .await;
    let gap = slates_without_code(&slates, &first).await;
    handed_over(&app, &gap).await;
    assert_eq!(
        ticker_redraws(&app),
        before,
        "the film restarted for the connecting gap"
    );
    std::fs::remove_file(&offline).unwrap();
    let tunnel = wait_online(&app, &cookie).await;
    assert_eq!(tunnel["url"], format!("https://{second}"));
    let new = slates_for(&slates, &format!("https://{second}")).await;
    handed_over(&app, &new).await;
    assert_eq!(ticker_redraws(&app) - before, 1, "one restart per address");
    assert!(pause_reasons(&app.engine().core).is_empty());
    app.stop().await;
}

/// Final wave I1: switching the tunnel off is no reconnect: the film restarts at once (one
/// time), so the code of the dead tunnel address does not stay on the stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switching_the_tunnel_off_restarts_the_film_for_the_ticker_once() {
    let Some(app) = start(Setup {
        tools: true,
        ..Setup::default()
    })
    .await
    else {
        return;
    };
    let cookie = app.window_cookie().await;
    ticker_with_code(&app, &cookie).await;
    wait_online(&app, &cookie).await;
    let slates = app.home.join("data").join("slates");
    let first = slates_for(&slates, &format!("https://{QUICK_HOST}")).await;
    handed_over(&app, &first).await;
    let before = ticker_redraws(&app);
    let (status, res) = app
        .put_config(&cookie, json!({"tunnelEnabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let manual = slates_without_code(&slates, &first).await;
    handed_over(&app, &manual).await;
    assert_eq!(ticker_redraws(&app) - before, 1);
    app.stop().await;
}

// ---------- Upgrade from stage 2 (final review C1) ----------

/// A home as stage 2 leaves it: the wizard done, every tool there except cloudflared (dummy
/// files: the channel cannot start, which does not matter here), the tunnel on (the new
/// default). The fake's script: start 1 is `--version` after the install, then the tunnel.
fn stage_two_home(home: &Path, first_run_completed: bool) {
    std::fs::write(
        home.join("config.json"),
        json!({
            "port": 0,
            "tunnelEnabled": true,
            "firstRunCompleted": first_run_completed,
            "toolsInstalled": true,
        })
        .to_string(),
    )
    .unwrap();
    let tools = home.join("tools");
    for name in [
        "yt-dlp.exe",
        "deno.exe",
        "ffmpeg.exe",
        "ffprobe.exe",
        "avcodec-62.dll",
    ] {
        std::fs::write(tools.join(name), "x").unwrap();
    }
    std::fs::write(
        tools.join("fake_cloudflared.json"),
        json!([
            { "serve_metrics": false, "exit_after_ms": 0 },
            { "hostname": QUICK_HOST }
        ])
        .to_string(),
    )
    .unwrap();
}

/// The fake cloudflared as a release on a local server (with its checksum in the notes).
fn publish_fake_cloudflared(server: &TestServer) {
    let program = std::fs::read(fake_program()).unwrap();
    server.put("cf/cloudflared-windows-amd64.exe", program.clone());
    server.put(
        "cf-api",
        json!({
            "tag_name": "2099.1.0",
            "body": format!(
                "## Release\n| File | SHA256 |\n|---|---|\n| cloudflared-windows-amd64.exe | {} |\n",
                hex(&program)
            ),
        })
        .to_string(),
    );
}

async fn tunnel_view(app: &App, cookie: &str) -> Value {
    app.get("/api/desktop/tunnel", cookie).await
}

/// Polls `GET tunnel` until online, for up to `secs`; a tools job may restart the server, so
/// a poll meeting the closed port is retried.
async fn wait_online_across_restarts(app: &App, cookie: &str, secs: u64) -> Value {
    let mut last = Value::Null;
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let res = app
            .http
            .get(app.url("/api/desktop/tunnel"))
            .header("cookie", cookie)
            .send()
            .await;
        if let Ok(res) = res
            && let Ok(body) = res.json::<Value>().await
        {
            last = body;
            if last["state"] == "online" {
                return last;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the tunnel did not come online in {secs} s: {last}");
}

#[tokio::test]
async fn an_upgrade_without_cloudflared_downloads_it_unprompted_and_goes_online() {
    let server = TestServer::start().await;
    publish_fake_cloudflared(&server);
    let base = server.base.clone();
    let app = App::start_with(|home, options| {
        stage_two_home(home, true);
        options.tunnel_timings = fast_tunnel();
        options.tool_sources.cloudflared_base = format!("{base}/cf");
        options.tool_sources.cloudflared_release_api = format!("{base}/cf-api");
    })
    .await;
    let cookie = app.window_cookie().await;
    // The job restarts the server (the dummy ffmpeg): a poll may meet the closed port.
    let last = wait_online_across_restarts(&app, &cookie, 90).await;
    assert_eq!(last["url"], format!("https://{QUICK_HOST}"), "{last}");
    assert!(app.home.join("tools").join("cloudflared.exe").is_file());
    let log = app.log_text();
    assert!(
        log.contains(
            "Tools: the cloudflared program is missing (new in this version) — downloading it in the background."
        ),
        "{log}"
    );
    assert!(log.contains("Tools (downloading): done."), "{log}");
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["toolsOk"], true, "{info}");
    assert_eq!(info["missingTools"], json!([]));
    let starts = fake_starts(&app.home);
    assert_eq!(args_of(&starts[0]), ["--version"]);
    assert_eq!(args_of(&starts[1])[0], "tunnel");
    assert_never_two(&app.home);
    app.stop().await;
}

#[tokio::test]
async fn while_cloudflared_downloads_the_tunnel_says_so() {
    // Accepts connections and never answers: the download hangs.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", silent.local_addr().unwrap());
    let app = App::start_with(|home, options| {
        stage_two_home(home, true);
        options.tool_sources.cloudflared_base = format!("{base}/cf");
        options.tool_sources.cloudflared_release_api = format!("{base}/cf-api");
    })
    .await;
    let cookie = app.window_cookie().await;
    const FETCHING: &str = "Pobieranie programu cloudflared — tunel połączy się sam po pobraniu.";
    // The job starts in the background right after the app.
    eventually(10, "the download shown", || async {
        tunnel_view(&app, &cookie).await["message"] == FETCHING
    })
    .await;
    assert_eq!(tunnel_view(&app, &cookie).await["state"], "error");
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["toolsRunning"], true, "{info}");
    assert_eq!(info["missingTools"], json!(["cloudflared"]));
    // Shutting down does not wait for the hanging download.
    app.stop().await;
    drop(silent);
}

#[tokio::test]
async fn an_upgrade_whose_download_fails_runs_on_and_shows_cloudflared_missing() {
    let app = App::start_with(|home, _| stage_two_home(home, true)).await;
    let cookie = app.window_cookie().await;
    eventually(60, "the failed download", || async {
        app.log_text().contains("Tools (downloading): error")
    })
    .await;
    eventually(10, "the job to end", || async {
        !app.engine().core.tools_job_running()
    })
    .await;
    let tunnel = tunnel_view(&app, &cookie).await;
    assert_eq!(tunnel["state"], "error", "{tunnel}");
    assert_eq!(
        tunnel["message"],
        crate::i18n::t(crate::i18n::Lang::Pl, "errors.no_cloudflared", &[]),
        "{tunnel}"
    );
    let page = app.http.get(app.url("/")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["missingTools"], json!(["cloudflared"]));
    assert_eq!(info["toolsRunning"], false);
    app.stop().await;
}

#[tokio::test]
async fn a_failed_cloudflared_download_is_tried_again_later() {
    let server = TestServer::start().await;
    let base = server.base.clone();
    let app = App::start_with(|home, options| {
        stage_two_home(home, true);
        options.tunnel_timings = fast_tunnel();
        options.tool_sources.cloudflared_base = format!("{base}/cf");
        options.tool_sources.cloudflared_release_api = format!("{base}/cf-api");
        options.cloudflared_retry = Duration::from_secs(1);
    })
    .await;
    let cookie = app.window_cookie().await;
    // Nothing published yet: the first download fails.
    eventually(60, "the failed download", || async {
        app.log_text().contains("Tools (downloading): error")
    })
    .await;
    publish_fake_cloudflared(&server);
    wait_online_across_restarts(&app, &cookie, 90).await;
    assert!(app.home.join("tools").join("cloudflared.exe").is_file());
    app.stop().await;
}

#[tokio::test]
async fn before_the_first_run_wizard_nothing_is_downloaded_by_itself() {
    let app = App::start_with(|home, _| stage_two_home(home, false)).await;
    let cookie = app.window_cookie().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!app.engine().core.tools_job_running());
    assert!(
        !app.log_text().contains("new in this version"),
        "{}",
        app.log_text()
    );
    let tunnel = tunnel_view(&app, &cookie).await;
    assert_eq!(
        tunnel["message"],
        crate::i18n::t(crate::i18n::Lang::Pl, "errors.no_cloudflared", &[]),
        "{tunnel}"
    );
    app.stop().await;
}

// ---------- Named tunnel ----------

#[tokio::test]
async fn the_named_mode_without_its_files_runs_the_quick_tunnel_with_a_warning() {
    let app = start(Setup {
        settings: json!({
            "tunnelMode": "named",
            "namedTunnelHostname": NAMED_HOST,
            "namedTunnelId": TUNNEL_ID,
        }),
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
        settings: json!({
            "tunnelMode": "named",
            "namedTunnelHostname": NAMED_HOST,
            "namedTunnelId": TUNNEL_ID,
        }),
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
    // And back ("Wróć do stałego linku"): allowed, its files are there.
    let (status, res) = app
        .put_config(&cookie, json!({"tunnelMode": "named"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["tunnelMode"], "named");
    eventually(20, "the named cloudflared again", || async {
        fake_starts(&app.home).len() == 3
    })
    .await;
    assert!(args_of(&fake_starts(&app.home)[2]).contains(&"run".to_string()));
    assert_never_two(&app.home);
    app.stop().await;
}

// ---------- Automatic pause (Review Focus 4) ----------

/// Review Focus 4 with the real channel and the fake cloudflared losing its connection twice:
/// a loss shorter than the limit does not pause; a longer one pauses; the return resumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_short_tunnel_loss_does_not_pause_a_long_one_does_and_the_return_resumes() {
    let Some(app) = start(Setup {
        tools: true,
        // `/ready` fails while the test's file `offline` exists; limit 4 s.
        runs: Some(json!([{
            "hostname": QUICK_HOST,
            "ready_down_while": "offline",
        }])),
        auto_pause: AutoPauseTimings {
            tunnel_loss: Duration::from_millis(4000),
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
    let core = Arc::clone(&app.engine().core);
    // A slow first start of cloudflared alone can already pause the channel for a moment
    // (limit of a few seconds): the losses begin once it is steadily on air.
    eventually(60, "the channel on air without a pause", || async {
        core.channel_running() && pause_reasons(&core).is_empty()
    })
    .await;
    let offline = app.home.join("tools").join("offline");
    // (ms since the loss began, tunnel state, pause reasons), every 25 ms, for `for_ms`.
    let sample = |since: Instant, for_ms: u64| {
        let core = Arc::clone(&core);
        async move {
            let mut samples: Vec<(u64, String, Vec<&str>)> = Vec::new();
            while since.elapsed() < Duration::from_millis(for_ms) {
                let state = format!("{:?}", core.tunnel_state());
                samples.push((
                    since.elapsed().as_millis() as u64,
                    state,
                    pause_reasons(&core),
                ));
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            samples
        }
    };
    let reconnecting = |samples: &[(u64, String, Vec<&str>)]| {
        samples
            .iter()
            .any(|(_, s, _)| s.starts_with("Reconnecting"))
    };
    let paused_at = |samples: &[(u64, String, Vec<&str>)]| {
        samples
            .iter()
            .find(|(_, _, r)| r.contains(&"tunnel"))
            .map(|(t, _, _)| *t)
    };

    // A loss of 1.5 s (well under the limit), watched until long after the limit.
    std::fs::write(&offline, "").unwrap();
    let short = Instant::now();
    let mut samples = sample(short, 1500).await;
    std::fs::remove_file(&offline).unwrap();
    samples.extend(sample(short, 6500).await);
    assert!(
        reconnecting(&samples),
        "the short loss did not happen: {samples:?}"
    );
    assert_eq!(
        paused_at(&samples),
        None,
        "the short loss paused: {samples:?}"
    );
    assert!(
        samples.last().unwrap().1.starts_with("Online"),
        "not back online: {samples:?}"
    );

    // A loss of 8 s: paused once the limit has passed since the loss began, not before.
    std::fs::write(&offline, "").unwrap();
    let long = Instant::now();
    let samples = sample(long, 8000).await;
    std::fs::remove_file(&offline).unwrap();
    assert!(
        reconnecting(&samples),
        "the long loss did not happen: {samples:?}"
    );
    let first_pause = paused_at(&samples).unwrap_or_else(|| panic!("never paused: {samples:?}"));
    assert!(
        (4000..8000).contains(&first_pause),
        "paused at {first_pause} ms: {samples:?}"
    );
    eventually(10, "the resume after the tunnel returned", || async {
        pause_reasons(&core).is_empty()
    })
    .await;
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["paused"], false);
    assert!(app.log_text().contains("Automatic pause"));
    app.stop().await;
}

/// Stage 4 fix round 1: the owner's manual pause survives a server restart (the new loop
/// starts with it), and the automatic pause never resumes it: a segment fetch clears only
/// `no_viewers`.
#[tokio::test]
async fn a_manual_pause_survives_a_server_restart_and_the_automatic_pause_leaves_it() {
    let Some(app) = start(Setup {
        tools: true,
        runs: None,
        settings: json!({"tunnelEnabled": false, "pauseWhenNoViewers": true}),
        auto_pause: AutoPauseTimings {
            no_viewers: Duration::from_secs(2),
            recheck: Duration::from_secs(30),
            ..AutoPauseTimings::default()
        },
        ..Setup::default()
    })
    .await
    else {
        return;
    };
    let core = Arc::clone(&app.engine().core);
    core.live()
        .unwrap()
        .state
        .engine
        .commands
        .send(crate::engine::Command::Pause(PauseReason::Manual))
        .await
        .unwrap();
    eventually(30, "the manual and no-viewers pause", || async {
        pause_reasons(&core) == ["no_viewers", "manual"]
    })
    .await;

    app.engine().restart().await.unwrap();
    assert_eq!(app.engine().server_starts(), 2);
    // The new loop starts with both reasons.
    assert_eq!(pause_reasons(&core), ["no_viewers", "manual"]);

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
    let res = app
        .http
        .get(app.url(&format!("/live/{segment}")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{segment}");
    eventually(5, "the no-viewers resume", || async {
        pause_reasons(&core) == ["manual"]
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(pause_reasons(&core), ["manual"]);
    app.stop().await;
}

/// No segment fetched for the limit: paused; the pause survives a server restart (the new
/// channel gets the reason again); the first segment fetch resumes without waiting for the
/// periodic check (set to 30 s here, so a resume well within 10 s cannot be that check).
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
        ..Setup::default()
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
    // Fix round 1: the new loop starts paused (its very first snapshot has the reason), not
    // only after the controller's command reaches it.
    assert_eq!(pause_reasons(&core), ["no_viewers"]);

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
    eventually(10, "the resume after a fetch", || async {
        pause_reasons(&core).is_empty()
    })
    .await;
    assert!(
        fetched.elapsed() < Duration::from_secs(10),
        "the resume waited {:?}",
        fetched.elapsed()
    );
    app.stop().await;
}
