//! The app engine and the `/api/desktop/*` routes over real HTTP: a temporary home, port 0,
//! tools and releases from local test servers (never the internet).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;
use crate::tools::test_server::{TestServer, build_zip};

/// Nothing listens on the discard port: requests there fail at once.
const NOWHERE: &str = "http://127.0.0.1:9";

const ROUTES: [(&str, &str); 22] = [
    ("GET", "/api/desktop/info"),
    ("GET", "/api/desktop/config"),
    ("PUT", "/api/desktop/config"),
    ("GET", "/api/desktop/tools"),
    ("POST", "/api/desktop/tools/install"),
    ("POST", "/api/desktop/tools/update"),
    ("POST", "/api/desktop/tools/repair"),
    ("GET", "/api/desktop/tools/progress"),
    ("POST", "/api/desktop/encoder/detect"),
    ("POST", "/api/desktop/pick-cookies"),
    ("GET", "/api/desktop/log"),
    ("GET", "/api/desktop/update"),
    ("POST", "/api/desktop/update/apply"),
    ("POST", "/api/desktop/wizard/complete"),
    ("POST", "/api/desktop/restart"),
    ("GET", "/api/desktop/tunnel"),
    ("POST", "/api/desktop/tunnel/restart"),
    ("POST", "/api/desktop/tunnel/named/login"),
    ("GET", "/api/desktop/tunnel/named/progress"),
    ("POST", "/api/desktop/tunnel/named/create"),
    ("POST", "/api/desktop/tunnel/named/quick"),
    ("POST", "/api/desktop/tunnel/named/delete"),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn test_options(tmp: &Path) -> AppOptions {
    let fake = tmp.join("fake.json");
    std::fs::write(&fake, "{}").unwrap();
    AppOptions {
        version: Version::new(0, 2, 0),
        update_api_url: format!("{NOWHERE}/api/latest"),
        check_updates: false,
        tool_sources: ToolSources {
            ytdlp_base: NOWHERE.into(),
            ffmpeg_base: NOWHERE.into(),
            deno_base: NOWHERE.into(),
            cloudflared_base: NOWHERE.into(),
            cloudflared_release_api: format!("{NOWHERE}/release-api"),
        },
        fake_resolver: Some(fake),
        // Never the owner's real `%USERPROFILE%\.cloudflared`.
        named_tunnel: NamedOptions {
            user_dir: Some(tmp.join("user").join(".cloudflared")),
            ..NamedOptions::default()
        },
        ..AppOptions::default()
    }
}

/// Hard links (no disk space) of the repo's ffmpeg/ffprobe into `<home>/tools`.
fn link_real_tools(home: &Path) -> bool {
    let bin = repo_root().join("bin");
    ["ffmpeg.exe", "ffprobe.exe"].iter().all(|name| {
        bin.join(name).is_file()
            && std::fs::hard_link(bin.join(name), home.join("tools").join(name)).is_ok()
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn cookie_of(res: &reqwest::Response) -> String {
    res.headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

struct App {
    engine: Option<AppEngine>,
    hooks: Arc<FakeHooks>,
    home: PathBuf,
    tmp: tempfile::TempDir,
    http: reqwest::Client,
}

impl App {
    async fn start() -> Self {
        Self::start_with(|_, _| {}).await
    }

    async fn start_with(prepare: impl FnOnce(&Path, &mut AppOptions)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        crate::home::ensure_home(&home).unwrap();
        // Port 0: never the owner's 8080, never a fixed test port. The tunnel is off unless a
        // test switches it on (`tunnel` tests): the manual address and trust apply.
        std::fs::write(
            home.join("config.json"),
            "{\"port\": 0, \"tunnelEnabled\": false}",
        )
        .unwrap();
        let mut options = test_options(tmp.path());
        prepare(&home, &mut options);
        let hooks = Arc::new(FakeHooks::default());
        let engine = tokio::time::timeout(
            Duration::from_secs(60),
            AppEngine::start_with(
                home.clone(),
                Arc::clone(&hooks) as Arc<dyn DesktopHooks>,
                options,
            ),
        )
        .await
        .expect("the app did not start in 60 s")
        .unwrap();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0)
            .build()
            .unwrap();
        Self {
            engine: Some(engine),
            hooks,
            home,
            tmp,
            http,
        }
    }

    fn engine(&self) -> &AppEngine {
        self.engine.as_ref().unwrap()
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.engine().port())
    }

    async fn window_cookie(&self) -> String {
        let key = self.engine().new_desktop_key();
        let res = self
            .http
            .get(self.url(&format!("/desktop-login?key={key}")))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FOUND);
        cookie_of(&res)
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut req = self.http.request(method, self.url(path));
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        let status = res.status();
        let text = res.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn get(&self, path: &str, cookie: &str) -> Value {
        let (status, body) = self.call("GET", path, Some(cookie), None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        body
    }

    async fn put_config(&self, cookie: &str, body: Value) -> (StatusCode, Value) {
        self.call("PUT", "/api/desktop/config", Some(cookie), Some(body))
            .await
    }

    async fn remote_login(&self, password: &str) -> (StatusCode, String) {
        let res = self
            .http
            .post(self.url("/api/admin/login"))
            .json(&json!({ "password": password }))
            .send()
            .await
            .unwrap();
        (res.status(), cookie_of(&res))
    }

    fn saved(&self) -> Value {
        let text = std::fs::read_to_string(self.home.join("config.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn log_text(&self) -> String {
        self.engine().log_tail().join("\n")
    }

    async fn stop(mut self) {
        let engine = self.engine.take().unwrap();
        tokio::time::timeout(Duration::from_secs(30), engine.shutdown())
            .await
            .expect("shutdown took longer than 30 s");
    }
}

/// Polls `f` every 50 ms for up to `secs` seconds.
async fn eventually<F, Fut>(secs: u64, what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !f().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------- Start ----------

#[tokio::test]
async fn without_tools_the_server_starts_and_the_settings_are_saved_at_once() {
    let app = App::start().await;
    let port = app.engine().port();
    assert_ne!(port, 0);
    assert!(!app.engine().channel_running());
    assert_eq!(
        app.engine().channel_loop_counter().load(Ordering::SeqCst),
        0
    );
    let saved = app.saved();
    assert_eq!(saved["port"], json!(port), "the bound port is written back");
    assert_eq!(saved["sessionSecret"].as_str().unwrap().len(), 64);
    let page = app.http.get(app.url("/")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(app.home.join("logs").join("vrcinema.log").is_file());
    assert!(app.log_text().contains("bez kanału"), "{}", app.log_text());
    app.stop().await;
}

// ---------- Access ----------

#[tokio::test]
async fn every_desktop_route_is_401_without_a_session_and_403_for_a_remote_one() {
    let app = App::start().await;
    let window = app.window_cookie().await;
    let (status, _) = app
        .put_config(
            &window,
            json!({"remoteAdmin": true, "password": "haslo-zdalne-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, remote) = app.remote_login("haslo-zdalne-1").await;
    assert_eq!(status, StatusCode::OK);
    *app.hooks.cookies_file.lock().unwrap() = Some(app.tmp.path().join("c.txt"));

    for (method, path) in ROUTES {
        let (status, body) = app.call(method, path, None, Some(json!({}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        assert_eq!(body["error"], "Zaloguj się.", "{method} {path}");
        let (status, body) = app.call(method, path, Some(&remote), Some(json!({}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(
            body["error"], "Ta funkcja jest dostępna tylko w oknie aplikacji.",
            "{method} {path}"
        );
    }
    // Nothing behind the routes ran.
    let saved = app.saved();
    assert_eq!(saved["firstRunCompleted"], false);
    assert_eq!(saved["ytdlpCookies"], Value::Null);
    // The remote session still opens the ordinary admin API.
    let (status, _) = app
        .call("GET", "/api/admin/channels/main", Some(&remote), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    app.stop().await;
}

#[tokio::test]
async fn info_describes_the_app() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["version"], "0.2.0");
    assert_eq!(info["port"], json!(app.engine().port()));
    assert_eq!(info["publicUrl"], "");
    assert_eq!(info["firstRunCompleted"], false);
    assert_eq!(info["toolsOk"], false);
    assert_eq!(info["channelRunning"], false);
    // Missing tools are not a failed start.
    assert_eq!(info["channelError"], Value::Null);
    assert_eq!(info["restarting"], false);
    assert_eq!(info["encoder"], "libx264");
    assert_eq!(info["update"], Value::Null);
    assert_eq!(info["tunnel"]["state"], "off");
    assert_eq!(info["tunnel"]["url"], Value::Null);
    assert_eq!(info["paused"], false);
    assert_eq!(info["pauseReasons"], json!([]));
    app.stop().await;
}

// ---------- Settings ----------

#[tokio::test]
async fn put_config_rejects_bad_values_and_saves_nothing() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let before = app.saved();
    let cases = [
        (
            json!({"port": 80}),
            "Port musi być liczbą od 1024 do 65535.",
        ),
        (
            json!({"port": 70000}),
            "Port musi być liczbą od 1024 do 65535.",
        ),
        (
            json!({"port": "8081"}),
            "Port musi być liczbą od 1024 do 65535.",
        ),
        (
            json!({"port": 2000.5}),
            "Port musi być liczbą od 1024 do 65535.",
        ),
        (
            json!({"lanAccess": "tak"}),
            "Pole lanAccess musi mieć wartość true albo false.",
        ),
        (
            json!({"publicUrl": "ftp://tv.example.com"}),
            "Adres publiczny musi zaczynać się od http:// lub https://.",
        ),
        (json!({"videoEncoder": "h265"}), "Nieznany enkoder: h265."),
        (
            json!({"password": "krótkie"}),
            "Hasło musi mieć co najmniej 8 znaków.",
        ),
        (
            json!({"remoteAdmin": true}),
            "Najpierw ustaw hasło panelu zdalnego.",
        ),
        (
            json!({"ytdlpCookies": "C:/cookies.txt"}),
            "Plik cookies wybierz w oknie wyboru pliku.",
        ),
        (
            json!({"pixeldrainApiKey": 5}),
            "Pole pixeldrainApiKey musi być tekstem.",
        ),
        (json!([1, 2]), "Niepoprawne ustawienia."),
        (
            json!({"tunnelEnabled": "tak"}),
            "Pole tunnelEnabled musi mieć wartość true albo false.",
        ),
        (
            json!({"tunnelMode": "inny"}),
            "Tryb tunelu musi mieć wartość quick albo named.",
        ),
        (
            json!({"tunnelMode": 1}),
            "Tryb tunelu musi mieć wartość quick albo named.",
        ),
        (
            json!({"namedTunnelHostname": "https://kanal.example.com"}),
            "Niepoprawna nazwa hosta stałego linku (np. kanal.example.com).",
        ),
        (
            json!({"namedTunnelHostname": "bez-kropki"}),
            "Niepoprawna nazwa hosta stałego linku (np. kanal.example.com).",
        ),
        (
            json!({"namedTunnelHostname": "zły host.example.com"}),
            "Niepoprawna nazwa hosta stałego linku (np. kanal.example.com).",
        ),
        (
            json!({"namedTunnelHostname": "-a.example.com"}),
            "Niepoprawna nazwa hosta stałego linku (np. kanal.example.com).",
        ),
        (
            json!({"namedTunnelHostname": "192.168.0.1"}),
            "Niepoprawna nazwa hosta stałego linku (np. kanal.example.com).",
        ),
        (
            json!({"namedTunnelHostname": "kanal.example.123"}),
            "Niepoprawna nazwa hosta stałego linku (np. kanal.example.com).",
        ),
        (
            json!({"namedTunnelHostname": 5}),
            "Pole namedTunnelHostname musi być tekstem.",
        ),
        (
            json!({"pauseOnTunnelLoss": "tak"}),
            "Pole pauseOnTunnelLoss musi mieć wartość true albo false.",
        ),
        (
            json!({"pauseWhenNoViewers": 1}),
            "Pole pauseWhenNoViewers musi mieć wartość true albo false.",
        ),
        // One bad field: the good one next to it is not saved either.
        (
            json!({"publicUrl": "https://tv.example.com", "port": 1}),
            "Port musi być liczbą od 1024 do 65535.",
        ),
    ];
    for (body, error) in cases {
        let (status, res) = app.put_config(&cookie, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(res["error"], error, "{body}");
    }
    assert_eq!(app.saved(), before);
    assert!(app.hooks.autostart_calls.lock().unwrap().is_empty());
    app.stop().await;
}

#[tokio::test]
async fn put_config_saves_valid_values_and_never_shows_secrets() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let (status, res) = app
        .put_config(
            &cookie,
            json!({
                "publicUrl": " https://tv.example.com/ ",
                "pixeldrainApiKey": "klucz-pd",
                "trustCloudflare": true,
                "autostart": true,
                "skippedUpdateVersion": "0.3.0",
                "videoEncoder": "libx264",
                "lanAccess": false,
                // Read-only fields of the view are ignored.
                "hasPassword": true,
                "firstRunCompleted": true,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["publicUrl"], "https://tv.example.com");
    assert_eq!(res["restarted"], false, "only port and lanAccess restart");
    assert_eq!(res["port"], json!(app.engine().port()));
    assert_eq!(*app.hooks.autostart_calls.lock().unwrap(), vec![true]);
    let saved = app.saved();
    assert_eq!(saved["publicUrl"], "https://tv.example.com");
    assert_eq!(saved["pixeldrainApiKey"], "klucz-pd");
    assert_eq!(saved["trustCloudflare"], true);
    assert_eq!(saved["autostart"], true);
    assert_eq!(saved["skippedUpdateVersion"], "0.3.0");
    assert_eq!(saved["videoEncoder"], "libx264");
    assert_eq!(saved["firstRunCompleted"], false);

    let view = app.get("/api/desktop/config", &cookie).await;
    assert_eq!(view["hasPassword"], false);
    assert_eq!(view["pixeldrainApiKey"], "klucz-pd");
    for secret in ["adminPasswordHash", "sessionSecret", "remoteSessionEpoch"] {
        assert!(view.get(secret).is_none(), "{secret} leaked: {view}");
    }

    // Unchanged autostart: the shell is not asked again; an empty key clears it; no restart
    // for settings that need none.
    let (status, res) = app
        .put_config(
            &cookie,
            json!({"autostart": true, "skippedUpdateVersion": null}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res["restarted"], false);
    assert_eq!(app.hooks.autostart_calls.lock().unwrap().len(), 1);
    let (status, _) = app
        .put_config(&cookie, json!({"pixeldrainApiKey": "  "}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.saved()["pixeldrainApiKey"], Value::Null);
    assert_eq!(app.saved()["skippedUpdateVersion"], Value::Null);
    app.stop().await;
}

#[tokio::test]
async fn the_tunnel_settings_are_saved_shown_and_need_no_restart() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let view = app.get("/api/desktop/config", &cookie).await;
    assert_eq!(view["tunnelEnabled"], false, "the harness switches it off");
    assert_eq!(view["tunnelMode"], "quick");
    assert_eq!(view["namedTunnelHostname"], Value::Null);
    assert_eq!(view["pauseOnTunnelLoss"], true);
    assert_eq!(view["pauseWhenNoViewers"], false);
    let (status, res) = app
        .put_config(
            &cookie,
            json!({
                "tunnelMode": "named",
                "namedTunnelHostname": " Kanal.Example.com ",
                "pauseOnTunnelLoss": false,
                "pauseWhenNoViewers": true,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["restarted"], false);
    assert_eq!(res["tunnelMode"], "named");
    assert_eq!(res["namedTunnelHostname"], "kanal.example.com");
    assert_eq!(res["pauseOnTunnelLoss"], false);
    assert_eq!(res["pauseWhenNoViewers"], true);
    let saved = app.saved();
    assert_eq!(saved["tunnelMode"], "named");
    assert_eq!(saved["namedTunnelHostname"], "kanal.example.com");
    assert_eq!(saved["pauseWhenNoViewers"], true);
    let (status, res) = app
        .put_config(&cookie, json!({"namedTunnelHostname": null}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(app.saved()["namedTunnelHostname"], Value::Null);
    assert_eq!(app.engine().server_starts(), 1);
    app.stop().await;
}

#[tokio::test]
async fn a_password_opens_the_remote_panel_and_a_new_one_ends_old_remote_sessions() {
    let app = App::start().await;
    let window = app.window_cookie().await;
    let (status, res) = app
        .put_config(
            &window,
            json!({"remoteAdmin": true, "password": "haslo-zdalne-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["hasPassword"], true);
    assert_eq!(res["remoteAdmin"], true);
    assert_eq!(res["restarted"], false);
    let saved = app.saved();
    assert!(
        saved["adminPasswordHash"]
            .as_str()
            .unwrap()
            .starts_with("$argon2id$")
    );
    assert!(!app.log_text().contains("haslo-zdalne-1"));

    let (status, first) = app.remote_login("haslo-zdalne-1").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .call("GET", "/api/admin/channels/main", Some(&first), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = app
        .put_config(&window, json!({"password": "haslo-zdalne-2"}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .call("GET", "/api/admin/channels/main", Some(&first), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "old remote session ended");
    assert_eq!(
        app.remote_login("haslo-zdalne-1").await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, second) = app.remote_login("haslo-zdalne-2").await;
    assert_eq!(status, StatusCode::OK);

    // Off and on again: the session from before does not come back.
    app.put_config(&window, json!({"remoteAdmin": false})).await;
    let (status, _) = app.remote_login("haslo-zdalne-2").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    app.put_config(&window, json!({"remoteAdmin": true})).await;
    let (status, _) = app
        .call("GET", "/api/admin/channels/main", Some(&second), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The window session was never affected.
    app.get("/api/desktop/info", &window).await;
    app.stop().await;
}

#[tokio::test]
async fn wizard_complete_is_saved() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let (status, res) = app
        .call("POST", "/api/desktop/wizard/complete", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res, json!({"ok": true}));
    assert_eq!(app.saved()["firstRunCompleted"], true);
    assert!(app.engine().settings().first_run_completed);
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["firstRunCompleted"], true);
    app.stop().await;
}

#[tokio::test]
async fn pick_cookies_uses_the_native_dialog_of_the_shell() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let (status, res) = app
        .call("POST", "/api/desktop/pick-cookies", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res, json!({"picked": false, "ytdlpCookies": null}));

    let file = app.tmp.path().join("cookies.txt");
    *app.hooks.cookies_file.lock().unwrap() = Some(file.clone());
    let (status, res) = app
        .call("POST", "/api/desktop/pick-cookies", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res["picked"], true);
    assert_eq!(res["ytdlpCookies"], json!(file.to_string_lossy()));
    assert_eq!(app.saved()["ytdlpCookies"], json!(file.to_string_lossy()));
    // The next yt-dlp call uses it; the server was not restarted for it.
    assert_eq!(
        app.engine().core.live_settings.ytdlp_cookies.get(),
        Some(file.clone())
    );
    assert_eq!(app.engine().server_starts(), 1);

    let (status, _) = app.put_config(&cookie, json!({"ytdlpCookies": null})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.saved()["ytdlpCookies"], Value::Null);
    app.stop().await;
}

#[tokio::test]
async fn the_log_route_returns_the_tail_without_the_window_key() {
    let app = App::start().await;
    let key = app.engine().new_desktop_key();
    let res = app
        .http
        .get(app.url(&format!("/desktop-login?key={key}")))
        .send()
        .await
        .unwrap();
    let cookie = cookie_of(&res);
    app.engine().log("Wpis z powłoki");
    let log = app.get("/api/desktop/log", &cookie).await;
    let lines: Vec<&str> = log["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(lines.iter().any(|l| l.ends_with("Wpis z powłoki")));
    assert!(lines.iter().all(|l| !l.contains(&key)));
    let file = std::fs::read_to_string(app.home.join("logs").join("vrcinema.log")).unwrap();
    assert!(file.contains("Wpis z powłoki") && !file.contains(&key));
    app.stop().await;
}

#[tokio::test]
async fn the_wizard_page_is_served_to_the_window_only() {
    let app = App::start().await;
    let res = app.http.get(app.url("/wizard")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let cookie = app.window_cookie().await;
    let res = app
        .http
        .get(app.url("/wizard"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ctype = res.headers()["content-type"].to_str().unwrap().to_string();
    assert!(ctype.starts_with("text/html"), "{ctype}");
    assert!(res.text().await.unwrap().contains("id=\"wizard\""));
    app.stop().await;
}

// ---------- Restart (Review Focus 5) ----------

#[tokio::test]
async fn a_new_port_restarts_the_server_there_with_exactly_one_channel_loop() {
    let mut linked = false;
    let app = App::start_with(|home, _| linked = link_real_tools(home)).await;
    if !linked {
        eprintln!("pomijam: brak bin/ffmpeg.exe lub nie można utworzyć dowiązania");
        app.stop().await;
        return;
    }
    assert!(app.engine().channel_running());
    let loops = app.engine().channel_loop_counter();
    assert_eq!(loops.load(Ordering::SeqCst), 1);
    let peak = Arc::new(AtomicUsize::new(0));
    let watching = Arc::new(AtomicBool::new(true));
    let watcher = {
        let (loops, peak, watching) =
            (Arc::clone(&loops), Arc::clone(&peak), Arc::clone(&watching));
        tokio::spawn(async move {
            while watching.load(Ordering::SeqCst) {
                peak.fetch_max(loops.load(Ordering::SeqCst), Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
    };

    let old = app.engine().port();
    let cookie = app.window_cookie().await;
    let new = free_port();
    let (status, res) = app.put_config(&cookie, json!({"port": new})).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["port"], json!(new));
    assert_eq!(res["restarted"], true);
    assert_eq!(app.engine().port(), new);
    assert_eq!(app.saved()["port"], json!(new));

    let playlist = reqwest::get(format!("http://127.0.0.1:{new}/live/main.m3u8"))
        .await
        .unwrap();
    assert_eq!(playlist.status(), StatusCode::OK);
    assert!(playlist.text().await.unwrap().contains(".ts"));
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", old)).is_err(),
        "the old port still accepts connections"
    );
    // The window session survives (same secret).
    app.get("/api/desktop/info", &cookie).await;

    watching.store(false, Ordering::SeqCst);
    watcher.await.unwrap();
    assert_eq!(peak.load(Ordering::SeqCst), 1, "two channel loops at once");
    assert_eq!(loops.load(Ordering::SeqCst), 1);
    app.stop().await;
    assert_eq!(loops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_restarts_leave_one_server_holding_the_data_lock() {
    let app = App::start().await;
    let (a, b) = tokio::join!(app.engine().restart(), app.engine().restart());
    a.unwrap();
    b.unwrap();
    let cookie = app.window_cookie().await;
    app.get("/api/desktop/info", &cookie).await;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(app.home.join("data").join("vrcinema.lock"))
        .unwrap();
    assert!(matches!(
        lock.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    let port = app.engine().port();
    app.stop().await;
    lock.try_lock().unwrap();
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}

// ---------- Files dropped on the window ----------

#[tokio::test]
async fn dropped_files_are_queued_and_the_tray_helpers_work_without_a_channel() {
    let app = App::start_with(|_, o| o.fake_resolver = None).await;
    let film = app.tmp.path().join("Film 1.mkv");
    std::fs::write(&film, "x").unwrap();
    let note = app.tmp.path().join("notatka.txt");
    std::fs::write(&note, "x").unwrap();
    let report = app.engine().add_files(vec![film, note]).await.unwrap();
    assert_eq!(report.added.len(), 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.added[0].title, "Film 1");
    let cookie = app.window_cookie().await;
    let panel = app.get("/api/admin/channels/main", &cookie).await;
    assert_eq!(panel["queue"][0]["title"], "Film 1");
    assert_eq!(app.engine().now_playing(), None);
    app.engine().skip();
    app.stop().await;
}

// ---------- Tools ----------

#[tokio::test]
async fn a_second_tools_job_is_409_while_one_runs() {
    // A server that accepts connections and never answers: the first job hangs.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", silent.local_addr().unwrap());
    let app = App::start_with(|_, o| {
        o.tool_sources = ToolSources {
            ytdlp_base: base.clone(),
            ffmpeg_base: base.clone(),
            deno_base: base.clone(),
            cloudflared_base: base.clone(),
            cloudflared_release_api: format!("{base}/release-api"),
        };
    })
    .await;
    let cookie = app.window_cookie().await;
    let (status, res) = app
        .call(
            "POST",
            "/api/desktop/tools/update",
            Some(&cookie),
            Some(json!({"tool": "vlc"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["error"], "Nieznane narzędzie: vlc.");

    let (status, res) = app
        .call("POST", "/api/desktop/tools/install", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{res}");
    assert_eq!(res, json!({"started": true}));
    for (path, body) in [
        ("/api/desktop/tools/repair", None),
        ("/api/desktop/tools/install", None),
        ("/api/desktop/tools/update", Some(json!({"tool": "ffmpeg"}))),
    ] {
        let (status, res) = app.call("POST", path, Some(&cookie), body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}");
        assert_eq!(res["error"], "Trwa już pobieranie narzędzi.");
    }
    let progress = app.get("/api/desktop/tools/progress", &cookie).await;
    assert_eq!(progress["running"], true);
    assert_eq!(progress["done"], false);
    let tools = app.get("/api/desktop/tools", &cookie).await;
    assert_eq!(tools["running"], true);
    assert_eq!(tools["ok"], false);
    // Shutting down does not wait for the hanging download.
    app.stop().await;
    drop(silent);
}

#[tokio::test]
async fn installing_tools_from_the_local_server_updates_status_and_settings() {
    let server = TestServer::start().await;
    let ytdlp: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
    let ffmpeg_zip = build_zip(&[
        (
            "ffmpeg-master-latest-win64-gpl-shared/bin/ffmpeg.exe",
            b"FFMPEG",
        ),
        (
            "ffmpeg-master-latest-win64-gpl-shared/bin/ffprobe.exe",
            b"FFPROBE",
        ),
        (
            "ffmpeg-master-latest-win64-gpl-shared/bin/avcodec-62.dll",
            b"DLL",
        ),
    ]);
    let deno_zip = build_zip(&[("deno.exe", b"DENO")]);
    server.put("yt/yt-dlp.exe", ytdlp.clone());
    server.put("yt/SHA2-256SUMS", format!("{}  yt-dlp.exe\n", hex(&ytdlp)));
    server.put(
        "ff/ffmpeg-master-latest-win64-gpl-shared.zip",
        ffmpeg_zip.clone(),
    );
    server.put(
        "ff/checksums.sha256",
        format!(
            "{}  ffmpeg-master-latest-win64-gpl-shared.zip\n",
            hex(&ffmpeg_zip)
        ),
    );
    server.put("dn/deno-x86_64-pc-windows-msvc.zip", deno_zip.clone());
    server.put(
        "dn/deno-x86_64-pc-windows-msvc.zip.sha256sum",
        format!("{}\n", hex(&deno_zip)),
    );
    server.put("cf/cloudflared-windows-amd64.exe", b"CLOUDFLARED".to_vec());
    let base = server.base.clone();
    let app = App::start_with(|_, o| {
        o.tool_sources = ToolSources {
            ytdlp_base: format!("{base}/yt"),
            ffmpeg_base: format!("{base}/ff"),
            deno_base: format!("{base}/dn"),
            cloudflared_base: format!("{base}/cf"),
            cloudflared_release_api: format!("{base}/cf-api"),
        };
    })
    .await;
    let cookie = app.window_cookie().await;
    let (status, _) = app
        .call("POST", "/api/desktop/tools/install", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    // The job ends with a server restart: a poll may meet the closed port (no answer, retry).
    eventually(30, "the tools job to finish", || async {
        let res = app
            .http
            .get(app.url("/api/desktop/tools"))
            .header("cookie", &cookie)
            .send()
            .await;
        match res {
            Ok(res) => res
                .json::<Value>()
                .await
                .is_ok_and(|v| v["running"] == false),
            Err(_) => false,
        }
    })
    .await;
    let progress = app.get("/api/desktop/tools/progress", &cookie).await;
    assert_eq!(progress["done"], true);
    assert_eq!(progress["error"], Value::Null);
    let tools = app.get("/api/desktop/tools", &cookie).await;
    assert_eq!(tools["ok"], true, "{tools}");
    assert_eq!(tools["missing"], json!([]));
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["yt-dlp", "ffmpeg", "deno", "cloudflared"]);
    assert!(tools["tools"][0]["installedAt"].is_string());
    assert_eq!(app.saved()["toolsInstalled"], true);
    // The fake ffmpeg cannot make slates: the server came back without the channel.
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["toolsOk"], true);
    assert_eq!(info["channelRunning"], false);
    // The window shows why (the tools are there, but the channel failed to start).
    let why = info["channelError"].as_str().unwrap_or_default();
    assert!(!why.is_empty(), "{info}");
    assert!(app.log_text().contains(why), "{why}");
    assert_eq!(info["restarting"], false);
    assert!(
        app.log_text().contains("Kanał nie wystartował"),
        "{}",
        app.log_text()
    );
    app.stop().await;
}

#[tokio::test]
async fn encoder_detection_needs_ffmpeg_and_falls_back_to_libx264() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let (status, res) = app
        .call("POST", "/api/desktop/encoder/detect", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(res["error"], "Brak ffmpeg — najpierw pobierz narzędzia.");

    // An ffmpeg that cannot run: every hardware test fails.
    std::fs::write(app.home.join("tools").join("ffmpeg.exe"), "to nie program").unwrap();
    let (status, res) = app
        .call("POST", "/api/desktop/encoder/detect", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        res,
        json!({"detectedEncoder": "libx264", "encoder": "libx264"})
    );
    assert_eq!(app.saved()["detectedEncoder"], "libx264");
    assert_eq!(
        app.engine().server_starts(),
        1,
        "detection never restarts implicitly"
    );
    app.stop().await;
}

// ---------- Program updates ----------

fn publish_release(server: &TestServer, tag: &str, exe: &[u8]) -> String {
    server.put("rel/VRCinema.exe", exe);
    server.put(
        "rel/VRCinema.exe.sha256",
        format!("{}  VRCinema.exe\n", hex(exe)),
    );
    let body = json!({
        "tag_name": tag,
        "body": "Nowości",
        "draft": false,
        "prerelease": false,
        "assets": [
            {"name": "VRCinema.exe", "browser_download_url": format!("{}/rel/VRCinema.exe", server.base)},
            {"name": "VRCinema.exe.sha256", "browser_download_url": format!("{}/rel/VRCinema.exe.sha256", server.base)},
        ],
    });
    server.put("api/latest", body.to_string());
    format!("{}/api/latest", server.base)
}

#[tokio::test]
async fn the_update_check_offers_a_newer_release_and_the_info_shows_it() {
    let server = TestServer::start().await;
    let api = publish_release(&server, "v99.0.0", b"NOWY PROGRAM");
    let app = App::start_with(|_, o| o.update_api_url = api.clone()).await;
    let cookie = app.window_cookie().await;
    let update = app.get("/api/desktop/update", &cookie).await;
    assert_eq!(update["current"], "0.2.0");
    assert_eq!(update["available"]["version"], "99.0.0");
    assert_eq!(update["available"]["notes"], "Nowości");
    assert!(update.get("error").is_none_or(Value::is_null), "{update}");
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["update"]["version"], "99.0.0");
    app.stop().await;
}

#[tokio::test]
async fn an_unreachable_or_private_release_is_quiet() {
    let server = TestServer::start().await;
    let api = format!("{}/api/brak", server.base);
    let app = App::start_with(|_, o| o.update_api_url = api.clone()).await;
    let cookie = app.window_cookie().await;
    let update = app.get("/api/desktop/update", &cookie).await;
    assert_eq!(update["available"], Value::Null);
    assert!(update.get("error").is_none_or(Value::is_null), "{update}");
    assert!(
        app.log_text().contains("Sprawdzanie aktualizacji"),
        "{}",
        app.log_text()
    );
    app.stop().await;
}

#[tokio::test]
async fn the_update_is_checked_at_start() {
    let server = TestServer::start().await;
    let api = publish_release(&server, "v99.0.0", b"NOWY PROGRAM");
    let app = App::start_with(|_, o| {
        o.update_api_url = api.clone();
        o.check_updates = true;
    })
    .await;
    let cookie = app.window_cookie().await;
    eventually(10, "the start-time update check", || async {
        app.call("GET", "/api/desktop/info", Some(&cookie), None)
            .await
            .1["update"]["version"]
            == "99.0.0"
    })
    .await;
    app.stop().await;
}

#[tokio::test]
async fn applying_an_update_swaps_the_exe_and_asks_the_shell_to_restart() {
    let server = TestServer::start().await;
    let api = publish_release(&server, "v99.0.0", b"NOWY PROGRAM");
    let app = App::start_with(|_, o| o.update_api_url = api.clone()).await;
    let cookie = app.window_cookie().await;

    let (status, res) = app
        .call("POST", "/api/desktop/update/apply", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(res["error"], "Brak dostępnej aktualizacji.");
    app.get("/api/desktop/update", &cookie).await;
    let (status, res) = app
        .call("POST", "/api/desktop/update/apply", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        res["error"],
        "Aktualizacja nie jest dostępna w tej wersji programu."
    );

    let dir = app.tmp.path().join("program");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("VRCinema.exe");
    std::fs::write(&exe, "STARY PROGRAM").unwrap();
    *app.hooks.exe.lock().unwrap() = Some(exe.clone());
    let (status, res) = app
        .call("POST", "/api/desktop/update/apply", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res, json!({"ok": true, "version": "99.0.0"}));
    assert_eq!(std::fs::read(&exe).unwrap(), b"NOWY PROGRAM");
    assert_eq!(
        std::fs::read(dir.join("VRCinema.old.exe")).unwrap(),
        b"STARY PROGRAM"
    );
    eventually(5, "the shell to be asked to restart", || async {
        app.hooks.update_restarts.load(Ordering::SeqCst) == 1
    })
    .await;
    app.stop().await;
}

// ---------- Live settings, explicit restart (fix round 1) ----------

#[test]
fn only_the_port_and_network_access_restart_the_server() {
    let base = AppSettings::default();
    let changed = |f: &dyn Fn(&mut AppSettings)| {
        let mut s = base.clone();
        f(&mut s);
        needs_restart(&base, &s)
    };
    assert!(changed(&|s| s.port = 9000));
    assert!(changed(&|s| s.lan_access = true));
    assert!(!changed(&|s| s.public_url = "https://tv.example.com".into()));
    assert!(!changed(&|s| s.trust_cloudflare = true));
    assert!(!changed(&|s| s.video_encoder = "h264_nvenc".into()));
    assert!(!changed(&|s| s.detected_encoder = Some("h264_qsv".into())));
    assert!(!changed(&|s| s.ytdlp_cookies = Some(PathBuf::from("c.txt"))));
    assert!(!changed(&|s| s.pixeldrain_api_key = Some("k".into())));
    assert!(!changed(&|s| s.remote_admin = true));
}

#[tokio::test]
async fn minor_settings_take_effect_without_a_restart() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let port = app.engine().port();
    assert_eq!(app.engine().server_starts(), 1);
    for body in [
        json!({"trustCloudflare": true}),
        json!({"pixeldrainApiKey": "klucz-pd"}),
        json!({"videoEncoder": "h264_nvenc"}),
        json!({"autostart": true}),
        json!({"remoteAdmin": true, "password": "haslo-zdalne-1"}),
        json!({"publicUrl": "https://tv.example.com"}),
    ] {
        let (status, res) = app.put_config(&cookie, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}: {res}");
        assert_eq!(res["restarted"], false, "{body}");
    }
    assert_eq!(app.engine().server_starts(), 1, "no implicit restart");
    assert_eq!(app.engine().port(), port);
    let live = &app.engine().core.live_settings;
    assert!(live.trust_cloudflare.get());
    assert_eq!(live.encoder.get(), VideoEncoder::H264Nvenc);
    assert_eq!(live.pixeldrain_api_key.get(), Some("klucz-pd".to_string()));
    // Links and slates use the new address at once (the slates in the background).
    let panel = app.get("/api/admin/channels/main", &cookie).await;
    assert_eq!(panel["channelUrl"], "https://tv.example.com/live/main.m3u8");

    let (status, res) = app
        .call("POST", "/api/desktop/restart", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["ok"], true);
    assert_eq!(res["port"], json!(app.engine().port()));
    assert_eq!(app.engine().server_starts(), 2);
    app.stop().await;
}

#[tokio::test]
async fn trusting_cloudflare_applies_to_the_next_request() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let channel_url = || async {
        let res = app
            .http
            .get(app.url("/api/admin/channels/main"))
            .header("cookie", &cookie)
            .header("x-forwarded-host", "tv.example.org")
            .send()
            .await
            .unwrap();
        let body: Value = res.json().await.unwrap();
        body["channelUrl"].as_str().unwrap().to_string()
    };
    assert!(!channel_url().await.contains("tv.example.org"));
    let (status, _) = app
        .put_config(&cookie, json!({"trustCloudflare": true}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(channel_url().await.contains("tv.example.org"));
    assert_eq!(app.engine().server_starts(), 1);
    app.stop().await;
}

#[tokio::test]
async fn a_minor_change_keeps_the_stream_running() {
    let mut linked = false;
    let app = App::start_with(|home, _| linked = link_real_tools(home)).await;
    if !linked {
        eprintln!("pomijam: brak bin/ffmpeg.exe lub nie można utworzyć dowiązania");
        app.stop().await;
        return;
    }
    let cookie = app.window_cookie().await;
    // Number of the newest segment: a restarted broadcaster numbers from 0 again.
    let newest = || async {
        let text = app
            .http
            .get(app.url("/live/main.m3u8"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        text.lines()
            .filter_map(|l| {
                l.rsplit('/')
                    .next()?
                    .strip_suffix(".ts")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap()
    };
    // Let the stream run well past what a fresh start publishes.
    eventually(60, "the stream to advance", || async {
        newest().await >= 6
    })
    .await;
    let before = newest().await;
    for body in [
        json!({"videoEncoder": "libx264"}),
        json!({"publicUrl": "https://tv.example.com"}),
        json!({"trustCloudflare": true}),
    ] {
        let (status, res) = app.put_config(&cookie, body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["restarted"], false);
    }
    assert!(newest().await >= before, "the stream started over");
    assert_eq!(
        app.engine().channel_loop_counter().load(Ordering::SeqCst),
        1
    );
    assert_eq!(app.engine().server_starts(), 1);
    app.stop().await;
}

#[tokio::test]
async fn a_new_public_address_replaces_the_slates_live() {
    let mut linked = false;
    let app = App::start_with(|home, _| linked = link_real_tools(home)).await;
    if !linked {
        eprintln!("pomijam: brak bin/ffmpeg.exe lub nie można utworzyć dowiązania");
        app.stop().await;
        return;
    }
    let cookie = app.window_cookie().await;
    let slates = app.home.join("data").join("slates");
    // The tag line (not `#EXT-X-DISCONTINUITY-SEQUENCE`).
    const TAG: &str = "#EXT-X-DISCONTINUITY\n";
    let playlist = || async {
        app.http
            .get(app.url("/live/main.m3u8"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    };
    eventually(60, "the stream to start", || async {
        playlist().await.contains("#EXTINF")
    })
    .await;
    assert!(!playlist().await.contains(TAG));
    assert!(
        !slates.join("idle").join("qr.png").exists(),
        "no address, no code"
    );

    let (status, res) = app
        .put_config(&cookie, json!({"publicUrl": "https://tv.example.com"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["restarted"], false);
    // The new set appears in a folder of its own and goes on air as one discontinuity.
    eventually(90, "the new slate set", || async {
        slates.join("g1").join("idle").join("qr.png").is_file()
    })
    .await;
    // The code file is written before ffmpeg makes the set: under a full parallel test run
    // (many ffmpeg processes) the set may take a while.
    eventually(180, "the new set on air", || async {
        playlist().await.contains(TAG)
    })
    .await;
    // A second change makes a third set (and no code without an address).
    let (status, _) = app.put_config(&cookie, json!({"publicUrl": ""})).await;
    assert_eq!(status, StatusCode::OK);
    eventually(90, "the second slate set", || async {
        slates.join("g2").join("idle").join("title.txt").is_file()
    })
    .await;
    assert!(!slates.join("g2").join("idle").join("qr.png").exists());
    assert_eq!(app.engine().server_starts(), 1, "no restart");
    assert_eq!(
        app.engine().channel_loop_counter().load(Ordering::SeqCst),
        1
    );
    app.stop().await;
}

#[tokio::test]
async fn info_says_when_a_restart_is_in_progress() {
    let app = App::start().await;
    let core = Arc::clone(&app.engine().core);
    let cookie = app.window_cookie().await;
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["restarting"], false);
    let (reached, release) = core.pause_next_start();
    let restart = tokio::spawn({
        let core = Arc::clone(&core);
        async move { core.restart_server().await }
    });
    // The new server already answers, but the restart has not finished.
    reached.await.unwrap();
    let cookie = app.window_cookie().await;
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["restarting"], true);
    release.send(()).unwrap();
    restart.await.unwrap().unwrap();
    let info = app.get("/api/desktop/info", &cookie).await;
    assert_eq!(info["restarting"], false);
    app.stop().await;
}

/// Quitting must never hang on a restart that takes long (the window is already hidden and a
/// second launch only pokes this process): the shutdown gives up after its limit.
#[tokio::test]
async fn shutdown_gives_up_waiting_for_a_stuck_restart() {
    let mut app = App::start().await;
    let core = Arc::clone(&app.engine().core);
    let (reached, release) = core.pause_next_start();
    let restart = tokio::spawn({
        let core = Arc::clone(&core);
        async move { core.restart_server().await }
    });
    reached.await.unwrap();
    let engine = app.engine.take().unwrap();
    let clean = tokio::time::timeout(
        Duration::from_secs(10),
        engine.shutdown_within(Duration::from_millis(500)),
    )
    .await
    .expect("the shutdown waited for the stuck restart");
    assert!(!clean);
    let log = std::fs::read_to_string(app.home.join("logs").join("vrcinema.log")).unwrap();
    assert!(log.contains("Zatrzymywanie trwa za długo"), "{log}");
    // The restart that finishes after the shutdown gave up does not leave a server behind.
    release.send(()).unwrap();
    let _ = restart.await.unwrap();
    assert!(
        core.live().is_none(),
        "a server was published after the shutdown"
    );
    assert!(core.server.lock().await.is_none());
}

#[tokio::test]
async fn a_remote_switch_during_a_restart_reaches_the_new_server() {
    let app = App::start().await;
    let core = Arc::clone(&app.engine().core);
    let (reached, release) = core.pause_next_start();
    let restart = tokio::spawn({
        let core = Arc::clone(&core);
        async move { core.restart_server().await }
    });
    // The new server is up but not yet published: settings changed now must still reach it.
    reached.await.unwrap();
    let restarted = core
        .apply_config(ConfigPatch {
            remote_admin: Some(true),
            password: Some("haslo-zdalne-1".into()),
            trust_cloudflare: Some(true),
            ..ConfigPatch::default()
        })
        .await
        .unwrap();
    assert!(!restarted);
    release.send(()).unwrap();
    restart.await.unwrap().unwrap();
    let (status, _) = app.remote_login("haslo-zdalne-1").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the new server enforces the new switch"
    );
    assert!(core.live_settings.trust_cloudflare.get());
    app.stop().await;
}

mod named;
mod tunnel;
