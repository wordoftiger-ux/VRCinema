//! The permanent-link wizard (spec section 9) over the `/api/desktop/tunnel/named/*` routes, with
//! the fake cloudflared's management commands (`examples/fake_cloudflared.rs`) on temporary
//! files. The login's "user folder" is the test's own (`<tmp>/user/.cloudflared`): never the
//! owner's real one. Never the real cloudflared, never the internet.

#![cfg(windows)]

use std::time::Instant;

use super::tunnel::{
    NAMED_HOST, QUICK_HOST, Setup, TUNNEL_ID, args_of, assert_never_two, fake_starts, start,
    wait_online,
};
use super::*;

const LOGIN_URL: &str = "https://dash.cloudflare.com/argotunnel?aud=&callback=https%3A%2F%2Flogin.cloudflareaccess.org%2Fproba-logowania";
/// Unique markers in the fake's certificate and credentials: they must never leave the files.
const CERT_SECRET: &str = "SEKRET-CERT-5d1f0c";
const CREDENTIALS_SECRET: &str = "SEKRET-POSW-a37e92";
/// Another tunnel's id (sorts before [`TUNNEL_ID`]).
const OTHER_ID: &str = "0aa42ae2-765d-4adf-8112-31c55c1551ef";

const LOGIN: &str = "/api/desktop/tunnel/named/login";
const PROGRESS: &str = "/api/desktop/tunnel/named/progress";
const CREATE: &str = "/api/desktop/tunnel/named/create";
const QUICK: &str = "/api/desktop/tunnel/named/quick";
const DELETE: &str = "/api/desktop/tunnel/named/delete";

fn user_dir(app: &App) -> PathBuf {
    app.tmp.path().join("user").join(".cloudflared")
}

fn tunnel_dir(app: &App) -> PathBuf {
    app.home.join("tunnel")
}

/// The certificate in `<home>/tunnel`, as a finished login leaves it.
fn logged_in(app: &App) {
    std::fs::create_dir_all(tunnel_dir(app)).unwrap();
    std::fs::write(tunnel_dir(app).join("cert.pem"), CERT_SECRET).unwrap();
}

/// The fake's management commands so far.
fn commands(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(home.join("tools").join("fake_cloudflared.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["event"] == "command")
        .collect()
}

/// The arguments of the last `name` command.
fn command_args(home: &Path, name: &str) -> Vec<String> {
    let last = commands(home)
        .into_iter()
        .rev()
        .find(|c| c["command"] == name)
        .unwrap_or_else(|| panic!("no {name} command ran"));
    args_of(&last)
}

/// A fake login holds `login.lock` exclusively for its whole life (a PID can be reused, a lock
/// held by a dead process cannot).
fn login_running(home: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    let lock = home.join("tools").join("login.lock");
    lock.exists()
        && std::fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(&lock)
            .is_err()
}

async fn wait_progress(
    app: &App,
    cookie: &str,
    what: &str,
    done: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let progress = app.get(PROGRESS, cookie).await;
        if done(&progress) {
            return progress;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {progress}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_tunnel(app: &App, cookie: &str, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let tunnel = app.get("/api/desktop/tunnel", cookie).await;
        if done(&tunnel) {
            return tunnel;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {tunnel}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn create(app: &App, cookie: &str, body: Value) -> (StatusCode, Value) {
    app.call("POST", CREATE, Some(cookie), Some(body)).await
}

// ---------- Step 1: log in to Cloudflare ----------

#[tokio::test]
async fn logging_in_shows_the_login_page_and_moves_the_certificate_into_the_app() {
    let app = start(Setup {
        commands: json!({"login": {"url": LOGIN_URL, "delay_ms": 1500, "cert": CERT_SECRET}}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    let before = app.get(PROGRESS, &cookie).await;
    assert_eq!(before["state"], "idle");
    assert_eq!(before["loggedIn"], false);

    let (status, body) = app.call("POST", LOGIN, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["operation"], "login");
    assert_eq!(body["state"], "running");
    let waiting = wait_progress(&app, &cookie, "the login page", |p| {
        p["loginUrl"] == LOGIN_URL
    })
    .await;
    assert_eq!(waiting["step"], "waiting_for_browser", "{waiting}");
    // "Please open the following URL": cloudflared did not open it, the window must.
    assert_eq!(waiting["browserOpened"], false, "{waiting}");
    assert!(
        waiting["message"]
            .as_str()
            .unwrap()
            .contains("przeglądarce")
    );

    let done = wait_progress(&app, &cookie, "the end of the login", |p| {
        p["state"] != "running"
    })
    .await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["loggedIn"], true);
    assert_eq!(
        done["loginUrl"],
        Value::Null,
        "the page is gone with the login"
    );
    assert_eq!(done["code"], Value::Null);
    // Moved, not copied: nothing of this login stays in cloudflared's folder.
    assert_eq!(
        std::fs::read_to_string(tunnel_dir(&app).join("cert.pem")).unwrap(),
        CERT_SECRET
    );
    assert!(!user_dir(&app).join("cert.pem").exists());
    assert_eq!(
        command_args(&app.home, "login"),
        ["tunnel", "--no-autoupdate", "login"]
    );
    assert!(!login_running(&app.home));
    // The login page carries the way to the certificate: never in the log.
    let log = app.log_text();
    assert!(!log.contains("argotunnel"), "{log}");
    assert!(!log.contains(CERT_SECRET), "{log}");
    // Nothing else changed.
    assert_eq!(app.saved()["tunnelMode"], "quick");
    app.stop().await;
}

#[tokio::test]
async fn an_owners_existing_cloudflared_certificate_is_copied_and_left_in_place() {
    let app = start(Setup::default()).await.unwrap();
    std::fs::create_dir_all(user_dir(&app)).unwrap();
    std::fs::write(user_dir(&app).join("cert.pem"), "certyfikat-wlasciciela").unwrap();
    let cookie = app.window_cookie().await;
    let (status, _) = app.call("POST", LOGIN, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let done = wait_progress(&app, &cookie, "the end of the login", |p| {
        p["state"] != "running"
    })
    .await;
    assert_eq!(done["state"], "done", "{done}");
    // The owner learns that an earlier login (its account and domain) is used.
    let message = done["message"].as_str().unwrap();
    assert!(message.contains("wcześniejsze logowanie"), "{message}");
    assert_eq!(
        std::fs::read_to_string(tunnel_dir(&app).join("cert.pem")).unwrap(),
        "certyfikat-wlasciciela"
    );
    assert_eq!(
        std::fs::read_to_string(user_dir(&app).join("cert.pem")).unwrap(),
        "certyfikat-wlasciciela",
        "the owner's own file stays"
    );
    app.stop().await;
}

/// The grandchild the fake login starts (stands for the browser cloudflared opens) holds
/// `child.lock` while it lives.
fn child_running(home: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    let lock = home.join("tools").join("child.lock");
    lock.exists()
        && std::fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(&lock)
            .is_err()
}

/// Review item: the browser cloudflared opens must never be in a job of ours (closing the job
/// or quitting the app would kill the owner's browser); cloudflared itself dies with the login.
#[tokio::test]
async fn the_login_cloudflared_dies_with_the_login_but_the_browser_it_opens_does_not() {
    let app = start(Setup {
        commands: json!({"login": {
            "url": LOGIN_URL,
            "browser": true,
            "wait_forever": true,
            "spawn_child_ms": 6000,
        }}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    app.call("POST", LOGIN, Some(&cookie), None).await;
    let waiting = wait_progress(&app, &cookie, "the login page", |p| {
        p["loginUrl"] == LOGIN_URL
    })
    .await;
    assert_eq!(waiting["browserOpened"], true, "{waiting}");
    super::eventually(10, "the browser stand-in", || async {
        child_running(&app.home)
    })
    .await;
    let pid: u32 = std::fs::read_to_string(app.home.join("tools").join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        crate::process::in_program_job(pid),
        Some(false),
        "the browser must not be in the program's job"
    );
    let (status, _) = app
        .call("POST", LOGIN, Some(&cookie), Some(json!({"cancel": true})))
        .await;
    assert_eq!(status, StatusCode::OK);
    super::eventually(10, "the login's exit", || async {
        !login_running(&app.home)
    })
    .await;
    assert!(child_running(&app.home), "the browser stand-in was killed");
    let home = app.home.clone();
    // The temporary folder cannot go entirely while the stand-in runs from it: quit the app
    // without `App::stop`, which would wait for the folder (until the stand-in ends).
    let mut app = app;
    let engine = app.engine.take().unwrap();
    tokio::time::timeout(Duration::from_secs(30), engine.shutdown())
        .await
        .expect("shutdown took longer than 30 s");
    assert!(child_running(&home), "quitting killed the browser stand-in");
    // It ends on its own; then what is left of the temporary folder goes.
    let deadline = Instant::now() + Duration::from_secs(20);
    while child_running(&home) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[tokio::test]
async fn quitting_the_app_during_a_login_stops_its_cloudflared() {
    let app = start(Setup {
        commands: json!({"login": {"url": LOGIN_URL, "wait_forever": true}}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    app.call("POST", LOGIN, Some(&cookie), None).await;
    wait_progress(&app, &cookie, "the login page", |p| {
        p["loginUrl"] == LOGIN_URL
    })
    .await;
    assert!(login_running(&app.home));
    let home = app.home.clone();
    app.stop().await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while login_running(&home) {
        assert!(
            Instant::now() < deadline,
            "the login's cloudflared outlived the app"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A cancel that comes after cloudflared already wrote the certificate: the certificate is not
/// left in cloudflared's folder (where it would pass for the owner's next time).
#[tokio::test]
async fn a_certificate_written_just_before_a_cancel_is_not_left_behind() {
    let app = start(Setup {
        commands: json!({"login": {"url": LOGIN_URL, "cert": CERT_SECRET, "wait_after_cert": true}}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    app.call("POST", LOGIN, Some(&cookie), None).await;
    let user_cert = user_dir(&app).join("cert.pem");
    super::eventually(10, "the certificate", || async { user_cert.is_file() }).await;
    let (status, _) = app
        .call("POST", LOGIN, Some(&cookie), Some(json!({"cancel": true})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let cancelled = wait_progress(&app, &cookie, "the cancel", |p| p["state"] != "running").await;
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    assert!(!user_cert.exists(), "left in cloudflared's folder");
    assert_eq!(
        std::fs::read_to_string(tunnel_dir(&app).join("cert.pem")).unwrap(),
        CERT_SECRET
    );
    assert_eq!(cancelled["loggedIn"], true);
    app.stop().await;
}

/// A cloudflared update or repair replaces the program the wizard runs: never both at once.
#[tokio::test]
async fn a_cloudflared_update_and_the_wizard_never_run_together() {
    // During a login: no update or repair of cloudflared.
    let app = start(Setup {
        commands: json!({"login": {"url": LOGIN_URL, "wait_forever": true}}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    app.call("POST", LOGIN, Some(&cookie), None).await;
    wait_progress(&app, &cookie, "the login page", |p| {
        p["loginUrl"] == LOGIN_URL
    })
    .await;
    for (path, body) in [
        (
            "/api/desktop/tools/update",
            Some(json!({"tool": "cloudflared"})),
        ),
        ("/api/desktop/tools/repair", None),
    ] {
        let (status, res) = app.call("POST", path, Some(&cookie), body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {res}");
        assert!(
            res["error"].as_str().unwrap().contains("stałego linku"),
            "{path}: {res}"
        );
    }
    app.stop().await;

    // During an update of cloudflared (hanging on a silent server): no wizard operation.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", silent.local_addr().unwrap());
    let app = App::start_with(|_, o| {
        o.tool_sources.cloudflared_base = base.clone();
        o.tool_sources.cloudflared_release_api = format!("{base}/release-api");
    })
    .await;
    let cookie = app.window_cookie().await;
    let (status, res) = app
        .call(
            "POST",
            "/api/desktop/tools/update",
            Some(&cookie),
            Some(json!({"tool": "cloudflared"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{res}");
    for (path, body) in [
        (LOGIN, None),
        (CREATE, Some(json!({"hostname": NAMED_HOST}))),
        (QUICK, None),
        (DELETE, None),
    ] {
        let (status, res) = app.call("POST", path, Some(&cookie), body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {res}");
        assert_eq!(res["code"], "busy", "{path}: {res}");
        assert!(
            res["error"].as_str().unwrap().contains("cloudflared"),
            "{path}: {res}"
        );
    }
    assert_eq!(app.get(PROGRESS, &cookie).await["state"], "idle");
    app.stop().await;
    drop(silent);
}

#[tokio::test]
async fn a_login_can_be_cancelled_times_out_and_only_one_operation_runs_at_a_time() {
    let app = start(Setup {
        commands: json!({"login": {"url": LOGIN_URL, "wait_forever": true}}),
        login_timeout: Duration::from_secs(3),
        ..Setup::default()
    })
    .await
    .unwrap();
    logged_in(&app);
    let cookie = app.window_cookie().await;
    let (status, _) = app.call("POST", LOGIN, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    wait_progress(&app, &cookie, "the login page", |p| {
        p["loginUrl"] == LOGIN_URL
    })
    .await;
    assert!(login_running(&app.home));

    // One operation at a time.
    for (path, body) in [
        (LOGIN, None),
        (CREATE, Some(json!({"hostname": NAMED_HOST}))),
        (QUICK, None),
        (DELETE, None),
    ] {
        let (status, body) = app.call("POST", path, Some(&cookie), body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {body}");
        assert_eq!(body["code"], "busy", "{path}: {body}");
        assert!(body["error"].as_str().unwrap().contains("Trwa"), "{body}");
    }
    assert!(!commands(&app.home).iter().any(|c| c["command"] != "login"));

    // Cancelled: the window says so and cloudflared is gone.
    let (status, body) = app
        .call("POST", LOGIN, Some(&cookie), Some(json!({"cancel": true})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cancelled = wait_progress(&app, &cookie, "the cancel", |p| p["state"] != "running").await;
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    assert_eq!(cancelled["code"], "cancelled");
    assert_eq!(cancelled["loginUrl"], Value::Null);
    super::eventually(10, "the cancelled login's exit", || async {
        !login_running(&app.home)
    })
    .await;

    // "Spróbuj ponownie" — and this time nobody logs in: the time limit ends it.
    let started = Instant::now();
    let (status, _) = app.call("POST", LOGIN, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let timed_out =
        wait_progress(&app, &cookie, "the time limit", |p| p["state"] != "running").await;
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert_eq!(timed_out["state"], "failed", "{timed_out}");
    assert_eq!(timed_out["code"], "timeout");
    let message = timed_out["message"].as_str().unwrap();
    assert!(message.contains("Spróbuj ponownie"), "{message}");
    assert!(message.contains("domen"), "{message}");
    super::eventually(10, "the timed-out login's exit", || async {
        !login_running(&app.home)
    })
    .await;
    // Cancelling when nothing runs changes nothing.
    let (status, body) = app
        .call("POST", LOGIN, Some(&cookie), Some(json!({"cancel": true})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], "timeout");
    assert_eq!(app.saved()["tunnelMode"], "quick");
    app.stop().await;
}

#[tokio::test]
async fn a_failed_login_says_why_and_without_cloudflared_nothing_starts() {
    let app = start(Setup {
        commands: json!({"login": {"exit_code": 1, "stderr": ["failed to connect to the login service"]}}),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    app.call("POST", LOGIN, Some(&cookie), None).await;
    let failed = wait_progress(&app, &cookie, "the failure", |p| p["state"] != "running").await;
    assert_eq!(failed["state"], "failed", "{failed}");
    assert_eq!(failed["code"], "failed");
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .contains("failed to connect to the login service"),
        "{failed}"
    );
    assert_eq!(failed["loggedIn"], false);
    app.stop().await;

    let app = start(Setup {
        runs: None,
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    let (status, body) = app.call("POST", LOGIN, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "no_cloudflared");
    assert!(body["error"].as_str().unwrap().contains("cloudflared"));
    app.stop().await;
}

// ---------- Step 2: the address ----------

/// Review Focus 5 as well: after the login, the creation and the DNS route, neither the log nor
/// any answer of the window's API holds the certificate or the credentials.
#[tokio::test]
async fn creating_the_link_routes_the_host_and_switches_to_the_named_tunnel_without_showing_secrets()
 {
    let app = start(Setup {
        commands: json!({
            "login": {"url": LOGIN_URL, "delay_ms": 200, "cert": CERT_SECRET},
            "create": {"secret": CREDENTIALS_SECRET},
        }),
        ..Setup::default()
    })
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    let quick = wait_online(&app, &cookie).await;
    assert_eq!(quick["url"], format!("https://{QUICK_HOST}"));
    let mut answers = Vec::new();

    let (_, body) = app.call("POST", LOGIN, Some(&cookie), None).await;
    answers.push(body);
    let done = wait_progress(&app, &cookie, "the login", |p| p["state"] != "running").await;
    assert_eq!(done["state"], "done", "{done}");
    answers.push(done);

    let (status, body) = create(&app, &cookie, json!({"hostname": " Kanal.Example.com "})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert!(
        body["message"].as_str().unwrap().contains(NAMED_HOST),
        "{body}"
    );
    answers.push(body);

    // The settings: the named mode with its host and the tunnel's id.
    let saved = app.saved();
    assert_eq!(saved["tunnelMode"], "named");
    assert_eq!(saved["namedTunnelHostname"], NAMED_HOST);
    assert_eq!(saved["namedTunnelId"], TUNNEL_ID);
    // The files: the credentials under the tunnel's id, nothing temporary left.
    let credentials = tunnel_dir(&app).join(format!("{TUNNEL_ID}.json"));
    assert!(
        std::fs::read_to_string(&credentials)
            .unwrap()
            .contains(CREDENTIALS_SECRET)
    );
    let mut files: Vec<String> = std::fs::read_dir(tunnel_dir(&app))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["6ff42ae2-765d-4adf-8112-31c55c1551ef.json", "cert.pem"]
    );

    // The commands, all with the certificate in the app's folder.
    let cert = tunnel_dir(&app).join("cert.pem");
    let args = command_args(&app.home, "create");
    assert_eq!(
        &args[..4],
        [
            "tunnel",
            "--no-autoupdate",
            "--origincert",
            &cert.to_string_lossy()
        ]
    );
    assert_eq!(args[4], "create");
    assert_eq!(args[5], "--credentials-file");
    assert_eq!(PathBuf::from(&args[6]).parent().unwrap(), tunnel_dir(&app));
    assert_eq!(args[7..], ["vrcinema"]);
    assert_eq!(
        command_args(&app.home, "route"),
        [
            "tunnel",
            "--no-autoupdate",
            "--origincert",
            &cert.to_string_lossy(),
            "route",
            "dns",
            TUNNEL_ID,
            NAMED_HOST
        ]
    );
    assert!(!commands(&app.home).iter().any(|c| c["command"] == "token"));

    // The named tunnel runs.
    let tunnel = wait_tunnel(&app, &cookie, "the named tunnel online", |t| {
        t["state"] == "online" && t["mode"] == "named"
    })
    .await;
    assert_eq!(tunnel["url"], format!("https://{NAMED_HOST}"));
    assert_eq!(tunnel["hostname"], NAMED_HOST);
    let run = args_of(&fake_starts(&app.home).pop().unwrap());
    let at = run.iter().position(|a| a == "run").expect("tunnel run");
    assert_eq!(PathBuf::from(&run[at + 2]), credentials);
    assert_eq!(run.last().unwrap(), TUNNEL_ID);
    assert_never_two(&app.home);

    // Review Focus 5.
    answers.push(app.get(PROGRESS, &cookie).await);
    answers.push(app.get("/api/desktop/tunnel", &cookie).await);
    answers.push(app.get("/api/desktop/config", &cookie).await);
    answers.push(app.get("/api/desktop/info", &cookie).await);
    for answer in &answers {
        let text = answer.to_string();
        for secret in [
            CERT_SECRET,
            CREDENTIALS_SECRET,
            "cert.pem",
            ".json",
            "TunnelSecret",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }
    // The log names the files (paths only), never what is in them.
    let log_file = std::fs::read_to_string(app.home.join("logs").join("vrcinema.log")).unwrap();
    let log_route = app.get("/api/desktop/log", &cookie).await.to_string();
    for text in [app.log_text(), log_file, log_route] {
        for secret in [
            CERT_SECRET,
            CREDENTIALS_SECRET,
            "TunnelSecret",
            "argotunnel",
        ] {
            assert!(!text.contains(secret), "{secret} in the log: {text}");
        }
    }
    app.stop().await;
}

#[tokio::test]
async fn an_existing_tunnel_is_reused_with_its_credentials_fetched() {
    let app = start(Setup {
        commands: json!({
            "create": {"exists": true},
            "token": {"id": OTHER_ID, "secret": CREDENTIALS_SECRET},
        }),
        ..Setup::default()
    })
    .await
    .unwrap();
    logged_in(&app);
    let cookie = app.window_cookie().await;
    let (status, body) = create(
        &app,
        &cookie,
        json!({"hostname": NAMED_HOST, "overwrite": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = command_args(&app.home, "token");
    let at = token.iter().position(|a| a == "token").unwrap();
    assert_eq!(token[at + 1], "--cred-file");
    assert_eq!(
        PathBuf::from(&token[at + 2]).parent().unwrap(),
        tunnel_dir(&app)
    );
    assert_eq!(token[at + 3..], ["vrcinema"]);
    assert_eq!(app.saved()["namedTunnelId"], OTHER_ID);
    assert!(tunnel_dir(&app).join(format!("{OTHER_ID}.json")).is_file());
    assert!(command_args(&app.home, "route").contains(&OTHER_ID.to_string()));
    let tunnel = wait_tunnel(&app, &cookie, "the named tunnel online", |t| {
        t["mode"] == "named" && t["state"] == "online"
    })
    .await;
    assert_eq!(tunnel["hostname"], NAMED_HOST);
    assert_eq!(
        args_of(&fake_starts(&app.home).pop().unwrap())
            .last()
            .unwrap(),
        OTHER_ID
    );
    assert!(!app.log_text().contains(CREDENTIALS_SECRET));
    app.stop().await;
}

#[tokio::test]
async fn a_taken_dns_name_is_explained_and_can_be_overwritten() {
    let app = start(Setup {
        commands: json!({"route": {"taken": true}}),
        ..Setup::default()
    })
    .await
    .unwrap();
    logged_in(&app);
    let cookie = app.window_cookie().await;
    let (status, body) = create(&app, &cookie, json!({"hostname": NAMED_HOST})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "dns_taken");
    let error = body["error"].as_str().unwrap();
    assert!(
        error.contains(NAMED_HOST) && error.contains("zajęty"),
        "{error}"
    );
    assert!(!command_args(&app.home, "route").contains(&"--overwrite-dns".to_string()));
    // The mode in use did not change (spec section 11).
    assert_eq!(app.saved()["tunnelMode"], "quick");
    assert_eq!(app.saved()["namedTunnelHostname"], Value::Null);
    let progress = app.get(PROGRESS, &cookie).await;
    assert_eq!(progress["state"], "failed");
    assert_eq!(progress["code"], "dns_taken");
    assert_eq!(progress["operation"], "create");

    let (status, body) = create(
        &app,
        &cookie,
        json!({"hostname": NAMED_HOST, "overwrite": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let route = command_args(&app.home, "route");
    assert_eq!(
        route[4..],
        ["route", "dns", "--overwrite-dns", TUNNEL_ID, NAMED_HOST]
    );
    assert_eq!(app.saved()["tunnelMode"], "named");
    assert_eq!(app.get(PROGRESS, &cookie).await["state"], "done");
    app.stop().await;
}

#[tokio::test]
async fn a_host_outside_the_accounts_domain_asks_for_a_domain() {
    let app = start(Setup {
        commands: json!({"route": {"zone": "moja-domena.pl"}}),
        ..Setup::default()
    })
    .await
    .unwrap();
    logged_in(&app);
    let cookie = app.window_cookie().await;
    let (status, body) = create(&app, &cookie, json!({"hostname": NAMED_HOST})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "no_domain");
    let error = body["error"].as_str().unwrap();
    assert!(
        error.contains("Dodaj domenę do Cloudflare, zanim ustawisz stały link."),
        "{error}"
    );
    assert!(
        error.contains("kanal.example.com.moja-domena.pl"),
        "{error}"
    );
    assert!(body["helpUrl"].as_str().unwrap().starts_with("https://"));
    assert_eq!(app.saved()["tunnelMode"], "quick");
    assert_eq!(app.get(PROGRESS, &cookie).await["code"], "no_domain");

    // A retry finds the stray record already there (or updates it): cloudflared succeeds with
    // another summary, and the host is still not routed. (The retry also replaces the
    // read-only credentials of the first attempt.)
    for result in ["unchanged", "updated"] {
        std::fs::write(
            app.home
                .join("tools")
                .join("fake_cloudflared_commands.json"),
            json!({"route": {"zone": "moja-domena.pl", "result": result}}).to_string(),
        )
        .unwrap();
        let (status, body) = create(&app, &cookie, json!({"hostname": NAMED_HOST})).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{result}: {body}");
        assert_eq!(body["code"], "no_domain", "{result}: {body}");
        assert_eq!(app.saved()["tunnelMode"], "quick", "{result}");
    }
    // In the domain, an existing record of this tunnel is fine.
    std::fs::write(
        app.home
            .join("tools")
            .join("fake_cloudflared_commands.json"),
        json!({"route": {"zone": "example.com", "result": "unchanged"}}).to_string(),
    )
    .unwrap();
    let (status, body) = create(&app, &cookie, json!({"hostname": NAMED_HOST})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(app.saved()["tunnelMode"], "named");
    app.stop().await;
}

#[tokio::test]
async fn the_address_is_validated_and_needs_the_login_first() {
    let app = start(Setup::default()).await.unwrap();
    let cookie = app.window_cookie().await;
    for body in [
        json!({}),
        json!({"hostname": 5}),
        json!({"hostname": "https://kanal.example.com"}),
        json!({"hostname": "kanal"}),
        json!({"hostname": "127.0.0.1"}),
        json!({"hostname": NAMED_HOST, "overwrite": "tak"}),
        json!([NAMED_HOST]),
    ] {
        let (status, answer) = create(&app, &cookie, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
        assert_eq!(answer["code"], "invalid", "{body}: {answer}");
    }
    let (status, answer) = app
        .call("POST", LOGIN, Some(&cookie), Some(json!({"cancel": "tak"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    // No certificate yet: log in first.
    let (status, answer) = create(&app, &cookie, json!({"hostname": NAMED_HOST})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "not_logged_in");
    let (status, answer) = app.call("POST", DELETE, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "not_logged_in");
    assert!(commands(&app.home).is_empty(), "nothing ran");
    assert_eq!(app.saved()["tunnelMode"], "quick");
    app.stop().await;
}

#[tokio::test]
async fn the_wizard_answers_in_the_language_of_the_request() {
    let app = start(Setup::default()).await.unwrap();
    let cookie = app.window_cookie().await;
    let create_in = |lang: Option<&'static str>, body: Value| {
        let mut req = app
            .http
            .post(app.url(CREATE))
            .header("cookie", cookie.as_str())
            .json(&body);
        if let Some(lang) = lang {
            req = req.header("x-vrcinema-lang", lang);
        }
        async move {
            let res = req.send().await.unwrap();
            (res.status(), res.json::<Value>().await.unwrap())
        }
    };
    // A failure of the wizard (its own answer, with `helpUrl`).
    let (status, answer) = create_in(Some("en"), json!({"hostname": NAMED_HOST})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "not_logged_in");
    assert_eq!(
        answer["error"],
        "Connect to Cloudflare first (step “Connect to Cloudflare”)."
    );
    // Without a language of its own: the app's (Polish until the app gets its setting).
    let (_, answer) = create_in(None, json!({"hostname": NAMED_HOST})).await;
    assert_eq!(
        answer["error"],
        "Najpierw połącz się z Cloudflare (krok „Połącz z Cloudflare”)."
    );
    // A bad request keeps the wizard's code `invalid`, its text in the request's language.
    let (status, answer) = create_in(Some("en"), json!({"hostname": "kanal"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(
        answer,
        json!({
            "error": "Invalid host name for the permanent link (e.g. channel.example.com).",
            "code": "invalid",
        })
    );
    app.stop().await;
}

// ---------- Back to quick, delete ----------

fn named_setup(commands: Value) -> Setup {
    Setup {
        settings: json!({
            "tunnelMode": "named",
            "namedTunnelHostname": NAMED_HOST,
            "namedTunnelId": TUNNEL_ID,
        }),
        // The named tunnel is online once `/ready` answers; the quick one gets this address.
        runs: Some(json!([{ "hostname": QUICK_HOST }])),
        named_files: true,
        commands,
        ..Setup::default()
    }
}

#[tokio::test]
async fn back_to_quick_keeps_the_files_and_deleting_removes_the_link() {
    let app = start(named_setup(json!({}))).await.unwrap();
    let cookie = app.window_cookie().await;
    let named = wait_online(&app, &cookie).await;
    assert_eq!(named["mode"], "named");
    let credentials = tunnel_dir(&app).join(format!("{TUNNEL_ID}.json"));

    let (status, body) = app.call("POST", QUICK, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["tunnel"]["mode"], "quick", "{body}");
    assert_eq!(app.saved()["tunnelMode"], "quick");
    assert_eq!(
        app.saved()["namedTunnelHostname"],
        NAMED_HOST,
        "kept for the way back"
    );
    assert!(credentials.is_file() && tunnel_dir(&app).join("cert.pem").is_file());
    super::eventually(20, "a quick cloudflared", || async {
        fake_starts(&app.home).len() == 2
    })
    .await;
    assert!(!args_of(&fake_starts(&app.home)[1]).contains(&"run".to_string()));
    assert_eq!(app.get(PROGRESS, &cookie).await["operation"], "quick");

    let (status, body) = app.call("POST", DELETE, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let message = body["message"].as_str().unwrap();
    assert!(message.contains("panelu Cloudflare"), "{message}");
    assert!(message.contains(NAMED_HOST), "{message}");
    let cert = tunnel_dir(&app).join("cert.pem");
    assert_eq!(
        command_args(&app.home, "delete"),
        [
            "tunnel",
            "--no-autoupdate",
            "--origincert",
            &cert.to_string_lossy(),
            "delete",
            "-f",
            "vrcinema"
        ]
    );
    assert!(!credentials.exists() && !cert.exists());
    let saved = app.saved();
    assert_eq!(saved["tunnelMode"], "quick");
    assert_eq!(saved["namedTunnelHostname"], Value::Null);
    assert_eq!(saved["namedTunnelId"], Value::Null);
    assert_eq!(app.get(PROGRESS, &cookie).await["loggedIn"], false);
    assert_never_two(&app.home);
    app.stop().await;
}

#[tokio::test]
async fn deleting_the_running_named_link_switches_to_quick_and_a_failure_changes_nothing() {
    let app = start(named_setup(
        json!({"delete": {"error": "Unauthorized: Invalid origin certificate"}}),
    ))
    .await
    .unwrap();
    let cookie = app.window_cookie().await;
    wait_online(&app, &cookie).await;
    let (status, body) = app.call("POST", DELETE, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], "failed");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("Invalid origin certificate"),
        "{body}"
    );
    assert_eq!(app.saved()["tunnelMode"], "named");
    assert!(tunnel_dir(&app).join(format!("{TUNNEL_ID}.json")).is_file());
    assert_eq!(
        app.get("/api/desktop/tunnel", &cookie).await["mode"],
        "named"
    );

    // The tunnel is already gone in Cloudflare: that counts as deleted.
    std::fs::write(
        app.home
            .join("tools")
            .join("fake_cloudflared_commands.json"),
        json!({"delete": {"missing": true}}).to_string(),
    )
    .unwrap();
    let (status, body) = app.call("POST", DELETE, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tunnel = wait_tunnel(&app, &cookie, "the quick tunnel", |t| {
        t["mode"] == "quick" && t["state"] == "online"
    })
    .await;
    assert_eq!(tunnel["url"], format!("https://{QUICK_HOST}"), "{tunnel}");
    assert!(!tunnel_dir(&app).join(format!("{TUNNEL_ID}.json")).exists());
    app.stop().await;
}
