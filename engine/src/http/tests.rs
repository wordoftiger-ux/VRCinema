//! Port of `tests/http.test.ts` (requests through `tower::ServiceExt::oneshot`, client address
//! through `ConnectInfo`, engine replaced by a hand-made channel/watch pair), plus a few extras.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tower::ServiceExt;

use super::auth::{
    AdminAuth, DesktopKey, SESSION_TTL_MS, SessionKind, check_password,
    create_desktop_session_token, create_session_token, hash_password, parse_cookies,
    remote_session_secret, session_cookie, session_kind, verify_password, verify_session_token,
};
use super::client_ip::{Trust, get_client_ip, is_https};
use super::{AppState, DesktopSession, HttpConfig, build_router, request_lang};
use crate::db::{
    AddedBy, ItemStatus, LinkMode, MAIN_CHANNEL_ID, NewItem, QueueItem, QueueRepo, SettingsPatch,
    SettingsRepo, open_database,
};
use crate::engine::{Command, EngineHandle, Snapshot};
use crate::i18n::{Lang, Msg};
use crate::library::MediaLibrary;
use crate::pipeline::channel::{
    ChannelState, ChannelStatus, ControlState, NowPlaying, PauseReason,
};
use crate::pipeline::monitor::{MonitorFeed, MonitorRow};
use crate::resolver::{MediaMeta, MediaResolver, PlaylistEntry, ResolveError, ResolvedStream};

type Metas = Arc<Mutex<HashMap<String, Result<MediaMeta, String>>>>;

struct TestResolver {
    metas: Metas,
}

fn video(title: &str, duration_s: f64) -> MediaMeta {
    MediaMeta {
        title: title.to_string(),
        duration_s: Some(duration_s),
        is_live: false,
        is_playlist: false,
        entries: None,
    }
}

#[async_trait]
impl MediaResolver for TestResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        if url.starts_with("local:") {
            let (_, path) = crate::library::parse_source_url(url)
                .ok_or_else(|| ResolveError::new("Unsupported URL"))?;
            let file = path.rsplit('/').next().unwrap_or("");
            let name = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
            return Ok(video(name, 60.0));
        }
        let found = self.metas.lock().unwrap().get(url).cloned();
        match found {
            None => Err(ResolveError::new("Unsupported URL")),
            Some(Err(message)) => Err(ResolveError::new(message)),
            Some(Ok(meta)) => Ok(meta),
        }
    }

    async fn resolve(&self, _url: &str) -> Result<ResolvedStream, ResolveError> {
        Err(ResolveError::new("nie w testach"))
    }
}

/// What the names of the tests resolve to (never the real DNS).
fn host_table() -> crate::rules::TableHostResolver {
    crate::rules::TableHostResolver::new(&[
        ("youtu.be", &["142.250.1.1"]),
        ("youtube.com", &["142.250.1.2"]),
        ("vimeo.com", &["151.101.1.1"]),
        ("cdn.example", &["93.184.216.34"]),
        ("bad.example", &["93.184.216.35"]),
        ("evil.example", &["192.168.0.10"]),
        ("mixed.example", &["93.184.216.34", "10.0.0.7"]),
        ("v6.example", &["fd00::5"]),
    ])
}

fn config() -> HttpConfig {
    HttpConfig {
        admin_password: "tajne".to_string(),
        session_secret: "s".repeat(64),
        public_url: String::new(),
        trust_cloudflare: false,
        trust_proxy: false,
        // The tests below read the Polish texts.
        language: Lang::Pl,
    }
}

fn idle_state() -> ChannelState {
    ChannelState {
        status: ChannelStatus::Idle,
        current: None,
        paused: false,
        pause_reasons: Vec::new(),
        control: Default::default(),
    }
}

fn snapshot(live_dir: &Path, segments: &[&str], state: ChannelState) -> Arc<Snapshot> {
    Arc::new(Snapshot {
        playlist: "#EXTM3U\n".to_string(),
        segments: segments
            .iter()
            .map(|s| s.to_string())
            .collect::<HashSet<_>>(),
        live_dir: live_dir.to_path_buf(),
        state,
        prefetched_item_id: None,
        stats: Default::default(),
        monitor: Default::default(),
        air: Default::default(),
    })
}

struct Harness {
    router: Router,
    db: Arc<Mutex<Connection>>,
    metas: Metas,
    commands: mpsc::Receiver<Command>,
    snapshots: watch::Sender<Arc<Snapshot>>,
    logs: Arc<Mutex<Vec<String>>>,
    live_dir: tempfile::TempDir,
    media_dir: tempfile::TempDir,
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("JSON body")
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }
    fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or("").to_string()
    }
}

/// A request: `ip` is the socket address, `body` is sent as JSON.
struct Call {
    method: &'static str,
    url: String,
    ip: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

fn call(method: &'static str, url: &str) -> Call {
    Call {
        method,
        url: url.to_string(),
        ip: "127.0.0.1".to_string(),
        headers: Vec::new(),
        body: None,
    }
}

fn get(url: &str) -> Call {
    call("GET", url)
}

impl Call {
    fn ip(mut self, ip: &str) -> Self {
        self.ip = ip.to_string();
        self
    }
    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
    fn cookie(self, cookie: &str) -> Self {
        self.header("cookie", cookie)
    }
    fn json(mut self, body: Value) -> Self {
        self.body = Some(body.to_string().into_bytes());
        self.header("content-type", "application/json")
    }
    fn raw(mut self, content_type: &str, body: &[u8]) -> Self {
        self.body = Some(body.to_vec());
        self.header("content-type", content_type)
    }
}

impl Harness {
    fn new() -> Self {
        Self::with_config(config())
    }

    fn with_config(config: HttpConfig) -> Self {
        Self::with_state(config, |s| s)
    }

    fn with_state(config: HttpConfig, tweak: impl FnOnce(AppState) -> AppState) -> Self {
        let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
        let live_dir = tempfile::tempdir().unwrap();
        let media_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(media_dir.path().join("Seria")).unwrap();
        for n in [10, 2, 1] {
            std::fs::write(
                media_dir.path().join("Seria").join(format!("Odc {n}.mkv")),
                "x",
            )
            .unwrap();
        }
        std::fs::write(media_dir.path().join("Film.mp4"), "x").unwrap();
        std::fs::write(live_dir.path().join("5.ts"), [0x47u8; 188]).unwrap();

        let mut metas: HashMap<String, Result<MediaMeta, String>> = HashMap::new();
        for (url, title) in [("a", "A"), ("b", "B"), ("c", "C"), ("d", "D")] {
            metas.insert(format!("https://youtu.be/{url}"), Ok(video(title, 100.0)));
        }
        metas.insert("https://youtu.be/long".into(), Ok(video("Long", 99_999.0)));
        metas.insert(
            "https://youtu.be/live".into(),
            Ok(MediaMeta {
                title: "Live".into(),
                duration_s: None,
                is_live: true,
                is_playlist: false,
                entries: None,
            }),
        );
        metas.insert(
            "https://youtube.com/playlist?list=1".into(),
            Ok(MediaMeta {
                title: "PL".into(),
                duration_s: None,
                is_live: false,
                is_playlist: true,
                entries: Some(vec![
                    PlaylistEntry {
                        url: "https://youtu.be/p1".into(),
                        title: "P1".into(),
                        duration_s: Some(10.0),
                        is_live: false,
                    },
                    PlaylistEntry {
                        url: "https://youtu.be/p2".into(),
                        title: "P2".into(),
                        duration_s: Some(20.0),
                        is_live: false,
                    },
                ]),
            }),
        );
        let metas: Metas = Arc::new(Mutex::new(metas));

        let (commands_tx, commands) = mpsc::channel(64);
        let (snapshots, snapshot_rx) =
            watch::channel(snapshot(live_dir.path(), &["5.ts"], idle_state()));
        let engine = EngineHandle {
            commands: commands_tx,
            snapshot: snapshot_rx,
        };
        let logs = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&logs);
        let state = AppState::new(
            config,
            MAIN_CHANNEL_ID,
            Arc::clone(&db),
            Arc::new(TestResolver {
                metas: Arc::clone(&metas),
            }),
            engine,
        )
        .unwrap()
        .with_library(Some(Arc::new(MediaLibrary::new(vec![
            media_dir.path().to_path_buf(),
        ]))))
        .with_log(Arc::new(move |m: Msg| {
            sink.lock().unwrap().push(m.to_string())
        }));
        let state = tweak(state.with_host_resolver(Arc::new(host_table())));
        Self {
            router: build_router(state),
            db,
            metas,
            commands,
            snapshots,
            logs,
            live_dir,
            media_dir,
        }
    }

    async fn send(&self, c: Call) -> Resp {
        send_to(self.router.clone(), c).await
    }

    fn with_db<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.db.lock().unwrap())
    }

    fn list(&self, status: ItemStatus) -> Vec<QueueItem> {
        self.with_db(|c| {
            QueueRepo::new(c)
                .list_by_status(MAIN_CHANNEL_ID, &[status])
                .unwrap()
        })
    }

    fn item(&self, id: i64) -> QueueItem {
        self.with_db(|c| QueueRepo::new(c).get(id).unwrap().unwrap())
    }

    fn add(&self, title: &str, status: ItemStatus) -> QueueItem {
        self.with_db(|c| {
            QueueRepo::new(c)
                .add(&NewItem {
                    channel_id: MAIN_CHANNEL_ID,
                    source_url: "u".into(),
                    title: title.into(),
                    duration_s: Some(1.0),
                    is_live: false,
                    status,
                    added_by: AddedBy::Admin,
                    requester_name: None,
                    requester_ip: None,
                    requester_key_hash: None,
                })
                .unwrap()
        })
    }

    fn update_settings(&self, patch: SettingsPatch) {
        self.with_db(|c| {
            SettingsRepo::new(c)
                .update(MAIN_CHANNEL_ID, &patch)
                .unwrap()
        });
    }

    /// Commands the engine would have received so far.
    fn commands(&mut self) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(c) = self.commands.try_recv() {
            out.push(c);
        }
        out
    }

    fn logs(&self) -> Vec<String> {
        self.logs.lock().unwrap().clone()
    }

    async fn login(&self) -> String {
        let res = self
            .send(call("POST", "/api/admin/login").json(json!({"password": "tajne"})))
            .await;
        assert_eq!(res.status, StatusCode::OK);
        res.header("set-cookie")
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    /// Public request of a film (like the `request()` helper of the TS test).
    async fn request(&self, url: &str, ip: &str, name: Option<&str>) -> Resp {
        let body = match name {
            // A nick always comes with a device key (the page sends both).
            Some(n) => json!({"url": url, "name": n, "deviceKey": KEY_A}),
            None => json!({"url": url}),
        };
        self.send(
            call("POST", "/api/channels/main/requests")
                .ip(ip)
                .json(body),
        )
        .await
    }

    async fn request_default(&self, url: &str) -> Resp {
        self.request(url, "1.1.1.1", None).await
    }
}

async fn send_to(router: Router, c: Call) -> Resp {
    let mut builder = Request::builder().method(c.method).uri(&c.url);
    for (k, v) in &c.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let mut req = builder
        .body(Body::from(c.body.unwrap_or_default()))
        .unwrap();
    let addr = SocketAddr::new(c.ip.parse().unwrap(), 40_000);
    req.extensions_mut().insert(ConnectInfo(addr));
    let res = router.oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    }
}

fn repo_public_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("public")
}

fn queued_titles(h: &Harness) -> Vec<String> {
    h.list(ItemStatus::Queued)
        .into_iter()
        .map(|i| i.title)
        .collect()
}

fn names(v: &Value) -> Vec<String> {
    v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect()
}

// ---------- strumień ----------

#[tokio::test]
async fn playlist_without_cache_and_with_cors() {
    let h = Harness::new();
    let res = h.send(get("/live/main.m3u8")).await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.header("content-type").contains("mpegurl"));
    assert_eq!(res.header("content-type"), "application/vnd.apple.mpegurl");
    assert_eq!(res.header("cache-control"), "no-cache");
    assert_eq!(res.header("access-control-allow-origin"), "*");
    assert_eq!(res.text(), "#EXTM3U\n");
}

#[tokio::test]
async fn known_segment_is_served_unknown_is_404() {
    let h = Harness::new();
    let ok = h.send(get("/live/main/5.ts")).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.header("content-type"), "video/mp2t");
    assert_eq!(ok.header("cache-control"), "public, max-age=60");
    assert_eq!(ok.header("access-control-allow-origin"), "*");
    assert_eq!(ok.body.len(), 188);
    let missing = h.send(get("/live/main/6.ts")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        missing.json(),
        json!({"error": "Brak fragmentu", "code": "no_segment"})
    );
    let traversal = h.send(get("/live/main/..%2F..%2Fpackage.json")).await;
    assert_eq!(traversal.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn segment_must_be_in_the_snapshot_and_on_disk() {
    let h = Harness::new();
    // On disk, but not in the snapshot.
    std::fs::write(h.live_dir.path().join("7.ts"), [0x47u8; 188]).unwrap();
    assert_eq!(
        h.send(get("/live/main/7.ts")).await.status,
        StatusCode::NOT_FOUND
    );
    // In the snapshot, but gone from disk; and a name that does not look like a segment.
    h.snapshots.send_replace(snapshot(
        h.live_dir.path(),
        &["5.ts", "8.ts", "x.ts"],
        idle_state(),
    ));
    std::fs::write(h.live_dir.path().join("x.ts"), [0x47u8; 188]).unwrap();
    assert_eq!(
        h.send(get("/live/main/8.ts")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.send(get("/live/main/x.ts")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(h.send(get("/live/main/5.ts")).await.status, StatusCode::OK);
}

/// Fix round 1: the tunnel's Cloudflare trust counts only for loopback peers (cloudflared); a
/// LAN client forging `CF-Connecting-IP` is still limited by its own address.
#[tokio::test]
async fn tunnel_trust_ignores_cf_connecting_ip_from_other_peers() {
    let settings = crate::app_settings::AppSettings {
        trust_cloudflare: true,
        ..crate::app_settings::AppSettings::default()
    };
    let live = crate::live::LiveSettings::from_config(&crate::app_settings::app_config(
        &settings,
        Path::new("H"),
    ));
    live.cloudflare_from_loopback_only.set(true);
    let h = Harness::with_state(config(), move |s| s.with_live(live));
    let login = |peer: &str, n: usize| {
        call("POST", "/api/admin/login")
            .ip(peer)
            .header("cf-connecting-ip", &format!("1.1.1.{n}"))
            .json(json!({"password": "x"}))
    };
    // Through cloudflared: every viewer has its own address, none is blocked.
    for n in 0..7 {
        let res = h.send(login("127.0.0.1", n)).await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED, "loopback attempt {n}");
    }
    // From the LAN: a new forged address per attempt does not help.
    for n in 0..5 {
        let res = h.send(login("192.168.1.5", 100 + n)).await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED, "LAN attempt {n}");
    }
    let res = h.send(login("192.168.1.5", 200)).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
}

/// Spec section 8: a served segment is a viewer; the playlist and a missing segment are not.
#[tokio::test]
async fn a_served_segment_counts_as_a_viewer() {
    let viewers = Arc::new(crate::auto_pause::ViewerActivity::new());
    let h = Harness::with_state(config(), {
        let viewers = Arc::clone(&viewers);
        move |s| s.with_viewers(viewers)
    });
    assert_eq!(h.send(get("/live/main.m3u8")).await.status, StatusCode::OK);
    assert_eq!(
        h.send(get("/live/main/6.ts")).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(viewers.last_fetch().is_none());
    assert_eq!(h.send(get("/live/main/5.ts")).await.status, StatusCode::OK);
    assert!(viewers.last_fetch().is_some());
}

// ---------- zgłoszenia publiczne ----------

#[tokio::test]
async fn adds_a_film_to_the_end_of_the_queue_with_a_nickname() {
    let h = Harness::new();
    let res = h
        .request("https://youtu.be/a", "1.1.1.1", Some("  Ola  "))
        .await;
    assert_eq!(res.status, StatusCode::CREATED);
    assert_eq!(res.json()["status"], "queued");
    // The helper sends a device key with a nick, so the item has an icon (shape checked elsewhere).
    let mut item = res.json()["item"].clone();
    assert!(item["requesterBadge"].is_object());
    item["requesterBadge"] = Value::Null;
    assert_eq!(
        item,
        json!({"id": 1, "title": "A", "durationS": 100, "requesterName": "Ola", "requesterBadge": null, "isLive": false})
    );
    let items = h.list(ItemStatus::Queued);
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.title, "A");
    assert_eq!(item.requester_name.as_deref(), Some("Ola"));
    assert_eq!(item.requester_ip.as_deref(), Some("1.1.1.1"));
    assert_eq!(item.added_by, AddedBy::Public);
    let logs = h.logs();
    assert_eq!(logs.len(), 1);
    let tag = logs[0]
        .strip_prefix("Request from viewer ")
        .and_then(|rest| rest.split(':').next())
        .expect("an anonymous viewer tag")
        .to_string();
    assert_eq!(tag.len(), 8);
    assert!(tag.bytes().all(|b| b.is_ascii_hexdigit()), "{tag}");
    assert_eq!(
        logs[0],
        format!("Request from viewer {tag}: \"A\" (queued)")
    );
}

/// A viewer address never reaches the log (spec section 8): only a tag, the same for the same
/// viewer during one run, different for another one.
#[tokio::test]
async fn the_log_has_a_tag_of_the_viewer_never_the_address() {
    let h = Harness::new();
    h.update_settings(SettingsPatch {
        max_active_per_requester: Some(9),
        request_limit_count: Some(99),
        ..SettingsPatch::default()
    });
    for (url, ip) in [
        ("https://youtu.be/a", "1.1.1.1"),
        ("https://youtu.be/b", "1.1.1.1"),
        ("https://youtu.be/c", "2001:db8::7a"),
        ("https://youtu.be/d", "::ffff:9.8.7.6"),
    ] {
        assert_eq!(
            h.request(url, ip, None).await.status,
            StatusCode::CREATED,
            "{ip}"
        );
    }
    let logs = h.logs();
    assert_eq!(logs.len(), 4, "{logs:?}");
    let tags: Vec<&str> = logs
        .iter()
        .map(|l| {
            l.strip_prefix("Request from viewer ")
                .unwrap()
                .split(':')
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(tags[0], tags[1], "the same viewer, the same tag");
    assert_ne!(tags[0], tags[2]);
    assert_ne!(tags[2], tags[3]);
    let all = logs.join(
        "
",
    );
    for ip in ["1.1.1.1", "2001:db8", "7a\"", "9.8.7.6"] {
        assert!(!all.contains(ip), "{ip} in {all}");
    }
    // A new run has a new salt: the tag of the same address differs.
    let other = Harness::new();
    assert_eq!(
        other
            .request("https://youtu.be/a", "1.1.1.1", None)
            .await
            .status,
        StatusCode::CREATED
    );
    assert_ne!(other.logs()[0], logs[0]);
}

#[tokio::test]
async fn nickname_is_cleaned_and_shortened_to_24_characters() {
    let h = Harness::new();
    h.request("https://youtu.be/a", "1.1.1.1", Some("\u{1}O\u{7f}la\n"))
        .await;
    h.request("https://youtu.be/b", "2.2.2.2", Some("   "))
        .await;
    let long = "x".repeat(50);
    h.request("https://youtu.be/c", "3.3.3.3", Some(&long))
        .await;
    let names: Vec<Option<String>> = h
        .list(ItemStatus::Queued)
        .into_iter()
        .map(|i| i.requester_name)
        .collect();
    assert_eq!(
        names,
        vec![Some("Ola".to_string()), None, Some("x".repeat(24))]
    );
}

#[tokio::test]
async fn disallowed_domain_is_400_without_asking_the_resolver() {
    let h = Harness::new();
    h.update_settings(SettingsPatch {
        link_mode: Some(LinkMode::Allowlist),
        ..Default::default()
    });
    let res = h.request_default("https://vimeo.com/1").await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert!(res.error().to_lowercase().contains("serwis"));
    assert_eq!(
        res.error(),
        "Ten serwis nie jest dozwolony. Dozwolone: youtube.com, youtu.be, pixeldrain.com, cda.pl."
    );
}

#[tokio::test]
async fn invalid_link_is_400() {
    let h = Harness::new();
    let res = h.request_default("to nie link").await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.error(), "To nie wygląda na poprawny link (http/https).");
    // No url at all, and a non-string url.
    let res = h
        .send(call("POST", "/api/channels/main/requests").json(json!({})))
        .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    let res = h
        .send(call("POST", "/api/channels/main/requests").json(json!({"url": 5})))
        .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}

const INVALID_LINK_EN: &str = "That does not look like a valid link (http/https).";
const INVALID_LINK_PL: &str = "To nie wygląda na poprawny link (http/https).";

fn english_config() -> HttpConfig {
    HttpConfig {
        language: Lang::En,
        ..config()
    }
}

fn bad_request_call() -> Call {
    call("POST", "/api/channels/main/requests").json(json!({"url": "to nie link"}))
}

#[tokio::test]
async fn errors_have_a_code_and_are_in_the_apps_language_by_default() {
    let h = Harness::with_config(english_config());
    let res = h.send(bad_request_call()).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        res.json(),
        json!({"error": INVALID_LINK_EN, "code": "invalid_link"})
    );
    let res = h.send(get("/api/admin/channels/main")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        res.json(),
        json!({"error": "Log in.", "code": "login_required"})
    );
    // Errors of the body extractor too.
    let res = h
        .send(call("POST", "/api/channels/main/requests").raw("application/json", b"{x"))
        .await;
    assert_eq!(res.json()["code"], "invalid_json");

    let h = Harness::new();
    let res = h.send(bad_request_call()).await;
    assert_eq!(
        res.json(),
        json!({"error": INVALID_LINK_PL, "code": "invalid_link"})
    );
}

#[tokio::test]
async fn the_request_picks_the_language_by_header_then_accept_language() {
    let h = Harness::with_config(english_config());
    let error = |c: Call| {
        let h = &h;
        async move { h.send(c).await.error() }
    };
    assert_eq!(
        error(bad_request_call().header("x-vrcinema-lang", "pl")).await,
        INVALID_LINK_PL
    );
    assert_eq!(
        error(bad_request_call().header("accept-language", "pl-PL,pl;q=0.9,en;q=0.8")).await,
        INVALID_LINK_PL
    );
    // The header wins over Accept-Language; an unknown header value does not count.
    assert_eq!(
        error(
            bad_request_call()
                .header("x-vrcinema-lang", "en")
                .header("accept-language", "pl")
        )
        .await,
        INVALID_LINK_EN
    );
    assert_eq!(
        error(
            bad_request_call()
                .header("x-vrcinema-lang", "xx")
                .header("accept-language", "pl")
        )
        .await,
        INVALID_LINK_PL
    );
    // Nothing supported: the app's language.
    assert_eq!(
        error(bad_request_call().header("accept-language", "zh-CN,*")).await,
        INVALID_LINK_EN
    );
    let polish_app = Harness::new();
    assert_eq!(
        polish_app
            .send(bad_request_call().header("accept-language", "zh"))
            .await
            .error(),
        INVALID_LINK_PL
    );
}

#[test]
fn request_lang_takes_the_header_then_accept_language_then_the_app() {
    let headers = |pairs: &[(&'static str, &str)]| {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    };
    assert_eq!(
        request_lang(&headers(&[("x-vrcinema-lang", "de")]), Lang::Pl),
        Lang::De
    );
    assert_eq!(
        request_lang(&headers(&[("accept-language", "de-DE,de;q=0.9")]), Lang::Pl),
        Lang::De
    );
    assert_eq!(
        request_lang(&headers(&[("accept-language", "zh")]), Lang::Pl),
        Lang::Pl
    );
    assert_eq!(request_lang(&headers(&[]), Lang::Ja), Lang::Ja);
}

#[tokio::test]
async fn read_failure_is_400_with_the_reason() {
    let h = Harness::new();
    let res = h.request_default("https://youtu.be/zzz").await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert!(res.error().contains("Unsupported URL"));
    assert_eq!(res.error(), "Nie udało się odczytać filmu: Unsupported URL");
}

#[tokio::test]
async fn too_long_live_and_playlist_are_400() {
    let h = Harness::new();
    assert!(
        h.request_default("https://youtu.be/long")
            .await
            .error()
            .contains("za długi")
    );
    assert!(
        h.request("https://youtu.be/live", "2.2.2.2", None)
            .await
            .error()
            .contains("na żywo")
    );
    assert!(
        h.request("https://youtube.com/playlist?list=1", "3.3.3.3", None)
            .await
            .error()
            .contains("Playlist")
    );
}

#[tokio::test]
async fn limit_of_3_requests_per_10_minutes_per_ip_is_429() {
    let h = Harness::new();
    for x in ["a", "b", "c"] {
        let res = h.request_default(&format!("https://youtu.be/{x}")).await;
        assert_eq!(res.status, StatusCode::CREATED);
    }
    let res = h.request_default("https://youtu.be/d").await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        res.error(),
        "Za dużo zgłoszeń w krótkim czasie. Spróbuj za kilka minut."
    );
    let other = h.request("https://youtu.be/d", "9.9.9.9", None).await;
    assert_eq!(other.status, StatusCode::CREATED);
}

#[tokio::test]
async fn limit_of_active_films_per_person_is_429() {
    let h = Harness::new();
    h.update_settings(SettingsPatch {
        max_active_per_requester: Some(1),
        request_limit_count: Some(100),
        ..Default::default()
    });
    assert_eq!(
        h.request_default("https://youtu.be/a").await.status,
        StatusCode::CREATED
    );
    let res = h.request_default("https://youtu.be/b").await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(res.error().contains("w kolejce"));
    assert_eq!(
        res.error(),
        "Masz już 1 film w kolejce. Poczekaj, aż się odtworzy."
    );
}

#[tokio::test]
async fn moderation_mode_holds_a_request_for_approval() {
    let h = Harness::new();
    h.update_settings(SettingsPatch {
        moderation: Some(true),
        ..Default::default()
    });
    let res = h.request_default("https://youtu.be/a").await;
    assert_eq!(res.json()["status"], "pending_approval");
    assert_eq!(h.list(ItemStatus::PendingApproval).len(), 1);
}

#[tokio::test]
async fn channel_state_shows_the_queue_without_ip_addresses() {
    let h = Harness::new();
    h.request("https://youtu.be/a", "1.1.1.1", Some("Ola"))
        .await;
    let res = h
        .send(get("/api/channels/main/state").header("host", "tv.local:8080"))
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.header("content-type").starts_with("application/json"));
    let body = res.json();
    assert_eq!(body["queue"].as_array().unwrap().len(), 1);
    let mut first = body["queue"][0].clone();
    assert!(first["requesterBadge"].is_object());
    first["requesterBadge"] = Value::Null;
    assert_eq!(
        first,
        json!({"id": 1, "title": "A", "durationS": 100, "requesterName": "Ola", "requesterBadge": null, "isLive": false})
    );
    assert!(!res.text().contains("1.1.1.1"));
    assert_eq!(body["channelUrl"], "http://tv.local:8080/live/main.m3u8");
    assert_eq!(body["requestUrl"], "http://tv.local:8080/");
    assert_eq!(body["status"], "idle");
    assert_eq!(body["current"], Value::Null);
    assert_eq!(
        body["rules"],
        json!({
            "linkMode": "all",
            "allowedDomains": ["youtube.com", "youtu.be", "pixeldrain.com", "cda.pl"],
            "maxDurationS": 7200,
            "moderation": false,
            "viewerLists": false,
            "viewerListMax": 10
        })
    );
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "status",
            "current",
            "paused",
            "pauseReasons",
            "queue",
            "channelUrl",
            "requestUrl",
            "rules"
        ]
    );
}

#[tokio::test]
async fn channel_state_serializes_the_current_film_like_the_ts_state() {
    let h = Harness::new();
    h.snapshots.send_replace(snapshot(
        h.live_dir.path(),
        &["5.ts"],
        ChannelState {
            status: ChannelStatus::Playing,
            current: Some(NowPlaying {
                item_id: 7,
                title: "Film".into(),
                duration_s: Some(100.0),
                position_s: 8,
                requester_name: Some("Ola".into()),
                added_by: AddedBy::Public,
                requester_key_hash: crate::nicks::hash_device_key(KEY_A),
                is_live: false,
            }),
            paused: false,
            pause_reasons: Vec::new(),
            control: Default::default(),
        },
    ));
    let body = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(body["status"], "playing");
    assert_eq!(body["paused"], false);
    assert_eq!(body["pauseReasons"], json!([]));
    let badge = crate::nicks::badge(&crate::nicks::hash_device_key(KEY_A).unwrap());
    assert_eq!(
        body["current"],
        json!({
            "itemId": 7, "title": "Film", "durationS": 100, "positionS": 8,
            "requesterName": "Ola", "requesterBadge": {"hue": badge.hue, "shape": badge.shape},
            "isLive": false
        })
    );
}

#[tokio::test]
async fn a_paused_channel_reports_status_paused_reasons_and_the_paused_film() {
    let h = Harness::new();
    h.snapshots.send_replace(snapshot(
        h.live_dir.path(),
        &["5.ts"],
        ChannelState {
            status: ChannelStatus::Paused,
            current: Some(NowPlaying {
                item_id: 7,
                title: "Film".into(),
                duration_s: Some(100.0),
                position_s: 12,
                requester_name: None,
                added_by: AddedBy::Admin,
                requester_key_hash: None,
                is_live: false,
            }),
            paused: true,
            pause_reasons: vec![PauseReason::Tunnel, PauseReason::NoViewers],
            control: Default::default(),
        },
    ));
    let body = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(body["status"], "paused");
    assert_eq!(body["paused"], true);
    assert_eq!(body["pauseReasons"], json!(["tunnel", "no_viewers"]));
    assert_eq!(body["current"]["positionS"], 12);
    assert_eq!(body["current"]["requesterBadge"], "host");
}

#[tokio::test]
async fn channel_url_uses_public_url_and_forwarded_headers_only_when_trusted() {
    let mut cfg = config();
    cfg.public_url = "https://tv.example.com".into();
    let h = Harness::with_config(cfg);
    let body = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(body["channelUrl"], "https://tv.example.com/live/main.m3u8");
    assert_eq!(body["requestUrl"], "https://tv.example.com/");

    let h = Harness::new();
    async fn forwarded(h: &Harness) -> Resp {
        h.send(
            get("/api/channels/main/state")
                .header("host", "internal:8080")
                .header("x-forwarded-host", "tv.example.com")
                .header("x-forwarded-proto", "https"),
        )
        .await
    }
    assert_eq!(
        forwarded(&h).await.json()["channelUrl"],
        "http://internal:8080/live/main.m3u8"
    );
    let mut cfg = config();
    cfg.trust_proxy = true;
    let h = Harness::with_config(cfg);
    assert_eq!(
        forwarded(&h).await.json()["channelUrl"],
        "https://tv.example.com/live/main.m3u8"
    );
}

// ---------- panel administratora ----------

#[tokio::test]
async fn without_a_session_the_admin_api_is_401() {
    let mut h = Harness::new();
    let res = h.send(get("/api/admin/channels/main")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        res.json(),
        json!({"error": "Zaloguj się.", "code": "login_required"})
    );
    let res = h.send(call("POST", "/api/admin/channels/main/skip")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert!(h.commands().is_empty());
    // A bad cookie is no session either (and 401 comes before body parsing).
    let res = h
        .send(
            call("PATCH", "/api/admin/items/1")
                .cookie("vrcinema_session=1.abc")
                .raw("application/json", b"{not json"),
        )
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_password_is_401_and_after_5_attempts_429() {
    let h = Harness::new();
    for _ in 0..5 {
        let res = h
            .send(call("POST", "/api/admin/login").json(json!({"password": "x"})))
            .await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            res.json(),
            json!({"error": "Złe hasło.", "code": "wrong_password"})
        );
    }
    let blocked = h
        .send(call("POST", "/api/admin/login").json(json!({"password": "tajne"})))
        .await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        blocked.error(),
        "Za dużo nieudanych prób logowania. Spróbuj za 15 minut."
    );
    // The block is per IP.
    let other = h
        .send(
            call("POST", "/api/admin/login")
                .ip("8.8.8.8")
                .json(json!({"password": "tajne"})),
        )
        .await;
    assert_eq!(other.status, StatusCode::OK);
}

/// The check (peek), the password comparison and the registration (hit) are one step: wrong
/// passwords sent at the same moment from several threads cannot slip past the limit of 5.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_wrong_passwords_never_get_more_than_5_attempts() {
    const PARALLEL: usize = 32;
    for round in 0..40 {
        let h = Harness::new();
        let barrier = Arc::new(tokio::sync::Barrier::new(PARALLEL));
        let tasks: Vec<_> = (0..PARALLEL)
            .map(|_| {
                let router = h.router.clone();
                let barrier = Arc::clone(&barrier);
                tokio::spawn(async move {
                    barrier.wait().await;
                    send_to(
                        router,
                        call("POST", "/api/admin/login").json(json!({"password": "x"})),
                    )
                    .await
                    .status
                })
            })
            .collect();
        let mut unauthorized = 0;
        let mut too_many = 0;
        for task in tasks {
            match task.await.unwrap() {
                StatusCode::UNAUTHORIZED => unauthorized += 1,
                StatusCode::TOO_MANY_REQUESTS => too_many += 1,
                other => panic!("unexpected status {other}"),
            }
        }
        assert_eq!(
            (unauthorized, too_many),
            (5, PARALLEL - 5),
            "round {round}: {unauthorized} wrong passwords were checked"
        );
    }
}

#[tokio::test]
async fn successful_logins_do_not_count_towards_the_block() {
    let h = Harness::new();
    for _ in 0..8 {
        h.login().await;
    }
    // A missing or non-string password is a failure.
    let res = h.send(call("POST", "/api/admin/login")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let res = h
        .send(call("POST", "/api/admin/login").json(json!({"password": 123})))
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_sets_an_httponly_cookie_that_opens_the_panel() {
    let h = Harness::new();
    let res = h
        .send(call("POST", "/api/admin/login").json(json!({"password": "tajne"})))
        .await;
    assert_eq!(res.json(), json!({"ok": true}));
    let set_cookie = res.header("set-cookie");
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    assert!(set_cookie.contains("Path=/"));
    assert!(set_cookie.contains("Max-Age=2592000"));
    assert!(!set_cookie.contains("Secure"));
    let cookie = set_cookie.split(';').next().unwrap();
    let panel = h.send(get("/api/admin/channels/main").cookie(cookie)).await;
    assert_eq!(panel.status, StatusCode::OK);
    let body = panel.json();
    assert!(body.get("settings").is_some());
    assert_eq!(body["status"], "idle");
    assert_eq!(body["prefetchedItemId"], Value::Null);
    for key in ["playing", "queue", "pending", "history"] {
        assert_eq!(body[key], json!([]), "{key}");
    }
    assert_eq!(body["channelUrl"], "http://localhost/live/main.m3u8");
}

/// The reason a film failed or was skipped is kept as a message and told in the language of
/// the request; a plain text kept by an older version stays as it is.
#[tokio::test]
async fn history_reasons_are_told_in_the_language_of_the_request() {
    let h = Harness::new();
    let cookie = h.login().await;
    let skipped = h.add("Skipped", ItemStatus::Queued);
    let failed = h.add("Failed", ItemStatus::Queued);
    let old = h.add("Old", ItemStatus::Queued);
    h.with_db(|c| {
        let q = QueueRepo::new(c);
        let reason = Msg::new("item_errors.skipped").to_stored();
        q.finish(skipped.id, ItemStatus::Played, Some(&reason))
            .unwrap();
        let reason = Msg::error("fetch_failed")
            .arg_msg(
                "reason",
                Msg::error("source_message").arg("detail", "Video unavailable"),
            )
            .to_stored();
        q.finish(failed.id, ItemStatus::Failed, Some(&reason))
            .unwrap();
        q.finish(
            old.id,
            ItemStatus::Failed,
            Some("Pominięty przez właściciela"),
        )
        .unwrap();
    });
    let errors = |lang: &str| {
        let req = get("/api/admin/channels/main")
            .cookie(&cookie)
            .header("x-vrcinema-lang", lang);
        let h = &h;
        async move {
            let body = h.send(req).await.json();
            body["history"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| (i["title"].as_str().unwrap().to_string(), i["error"].clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        }
    };
    let pl = errors("pl").await;
    let en = errors("en").await;
    assert_eq!(pl["Skipped"], "Pominięty przez właściciela");
    assert_eq!(en["Skipped"], "Skipped by the owner");
    assert_eq!(
        pl["Failed"],
        "Nie udało się odczytać filmu: Video unavailable"
    );
    assert_eq!(en["Failed"], "Could not read the video: Video unavailable");
    // Plain text from before stays readable as it is, in any language.
    assert_eq!(pl["Old"], "Pominięty przez właściciela");
    assert_eq!(en["Old"], "Pominięty przez właściciela");
    // What is in the database is the message, not a text of one language.
    let raw = h.item(skipped.id).error.unwrap();
    assert!(
        !raw.contains("Skipped by the owner") && !raw.contains("Pominięty"),
        "{raw:?}"
    );
}

#[tokio::test]
async fn login_cookie_is_secure_behind_a_trusted_https_proxy() {
    let mut cfg = config();
    cfg.trust_proxy = true;
    let h = Harness::with_config(cfg);
    let res = h
        .send(
            call("POST", "/api/admin/login")
                .header("x-forwarded-proto", "https")
                .json(json!({"password": "tajne"})),
        )
        .await;
    assert!(res.header("set-cookie").ends_with("; Secure"));
}

#[tokio::test]
async fn admin_adding_a_playlist_expands_it_into_items() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    let res = h
        .send(
            call("POST", "/api/admin/channels/main/items")
                .cookie(&cookie)
                .json(json!({"url": "https://youtube.com/playlist?list=1"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED);
    assert_eq!(res.json(), json!({"added": 2, "title": "PL"}));
    assert_eq!(queued_titles(&h), ["P1", "P2"]);
    assert_eq!(h.list(ItemStatus::Queued)[0].duration_s, Some(10.0));
    assert_eq!(h.commands(), vec![Command::QueueChanged]);
}

#[tokio::test]
async fn file_urls_never_enter_through_the_public_request() {
    let h = Harness::new();
    h.metas.lock().unwrap().insert(
        "file:///C:/Filmy/a.mp4".into(),
        Ok(video("Plik z dysku", 60.0)),
    );
    for url in ["file:///C:/Filmy/a.mp4", "FILE:///C:/Filmy/a.mp4"] {
        let res = h.request_default(url).await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{url}");
    }
    assert!(queued_titles(&h).is_empty());
    assert!(h.list(ItemStatus::PendingApproval).is_empty());
}

#[tokio::test]
async fn file_urls_never_enter_through_the_admin_panel() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.metas.lock().unwrap().insert(
        "file:///C:/Filmy/a.mp4".into(),
        Ok(video("Plik z dysku", 60.0)),
    );
    let res = h
        .send(
            call("POST", "/api/admin/channels/main/items")
                .cookie(&cookie)
                .json(json!({"url": "file:///C:/Filmy/a.mp4"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.error(), "To nie wygląda na poprawny link (http/https).");
    assert!(queued_titles(&h).is_empty());
}

#[tokio::test]
async fn playlist_entries_with_file_urls_are_dropped_when_expanding() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    let entry = |url: &str, title: &str| PlaylistEntry {
        url: url.into(),
        title: title.into(),
        duration_s: Some(5.0),
        is_live: false,
    };
    let playlist = |entries| {
        Ok(MediaMeta {
            title: "PL".into(),
            duration_s: None,
            is_live: false,
            is_playlist: true,
            entries: Some(entries),
        })
    };
    h.metas.lock().unwrap().insert(
        "https://youtube.com/playlist?list=mix".into(),
        playlist(vec![
            entry("file:///C:/Windows/system32/a.mp4", "Zly 1"),
            entry("https://youtu.be/p1", "Dobry"),
            entry("FILE:///C:/b.mp4", "Zly 2"),
            entry("local:0/Film.mp4", "Zly 3"),
        ]),
    );
    h.metas.lock().unwrap().insert(
        "https://youtube.com/playlist?list=files".into(),
        playlist(vec![entry("file:///C:/a.mp4", "Zly")]),
    );
    let add = |url: &str| {
        h.send(
            call("POST", "/api/admin/channels/main/items")
                .cookie(&cookie)
                .json(json!({"url": url})),
        )
    };
    let res = add("https://youtube.com/playlist?list=files").await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.error(), "Playlista jest pusta.");
    let res = add("https://youtube.com/playlist?list=mix").await;
    assert_eq!(res.status, StatusCode::CREATED);
    assert_eq!(res.json(), json!({"added": 1, "title": "PL"}));
    assert_eq!(queued_titles(&h), ["Dobry"]);
    assert!(
        h.list(ItemStatus::Queued)
            .iter()
            .all(|i| i.source_url.starts_with("https://"))
    );
    assert_eq!(h.commands(), vec![Command::QueueChanged]);
}
#[tokio::test]
async fn admin_can_add_a_live_stream_and_a_film_from_any_domain() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.metas
        .lock()
        .unwrap()
        .insert("https://vimeo.com/1".into(), Ok(video("Vimeo", 100.0)));
    for url in ["https://youtu.be/live", "https://vimeo.com/1"] {
        let res = h
            .send(
                call("POST", "/api/admin/channels/main/items")
                    .cookie(&cookie)
                    .json(json!({"url": url})),
            )
            .await;
        assert_eq!(res.status, StatusCode::CREATED);
    }
    assert!(h.list(ItemStatus::Queued)[0].is_live);
}

#[tokio::test]
async fn admin_adding_reports_bad_links_and_read_failures() {
    let h = Harness::new();
    let cookie = h.login().await;
    let add = |body: Value| {
        h.send(
            call("POST", "/api/admin/channels/main/items")
                .cookie(&cookie)
                .json(body),
        )
    };
    let res = add(json!({"url": "nope"})).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.error(), "To nie wygląda na poprawny link (http/https).");
    let res = add(json!({"url": "https://youtu.be/zzz"})).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.error(), "Nie udało się odczytać: Unsupported URL");
    h.metas.lock().unwrap().insert(
        "https://youtu.be/empty".into(),
        Ok(MediaMeta {
            title: "E".into(),
            duration_s: None,
            is_live: false,
            is_playlist: true,
            entries: Some(vec![]),
        }),
    );
    let res = add(json!({"url": "https://youtu.be/empty"})).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.error(), "Playlista jest pusta.");
}

#[tokio::test]
async fn approve_move_remove_and_skip() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.update_settings(SettingsPatch {
        moderation: Some(true),
        ..Default::default()
    });
    h.request_default("https://youtu.be/a").await;
    let pending = h.list(ItemStatus::PendingApproval)[0].id;
    let patch = |id: i64, body: Value| {
        h.send(
            call("PATCH", &format!("/api/admin/items/{id}"))
                .cookie(&cookie)
                .json(body),
        )
    };
    let r = patch(pending, json!({"action": "approve"})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!({"ok": true}));
    assert_eq!(h.item(pending).status, ItemStatus::Queued);

    let b = h.add("B", ItemStatus::Queued);
    let r = patch(b.id, json!({"position": 0})).await;
    assert_eq!(r.status, StatusCode::OK);
    let first = h.with_db(|c| {
        QueueRepo::new(c)
            .next_queued(MAIN_CHANNEL_ID)
            .unwrap()
            .unwrap()
    });
    assert_eq!(first.id, b.id);

    let r = h
        .send(call("DELETE", &format!("/api/admin/items/{}", b.id)).cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(h.item(b.id).status, ItemStatus::Removed);
    let queue_changed = h
        .commands()
        .iter()
        .filter(|c| **c == Command::QueueChanged)
        .count();
    assert!(queue_changed >= 3, "{queue_changed}");

    let r = h
        .send(call("POST", "/api/admin/channels/main/skip").cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!({"ok": true}));
    assert_eq!(h.commands(), vec![Command::Skip]);
}

#[tokio::test]
async fn reject_and_item_errors() {
    let h = Harness::new();
    let cookie = h.login().await;
    let patch = |id: &str, body: Value| {
        h.send(
            call("PATCH", &format!("/api/admin/items/{id}"))
                .cookie(&cookie)
                .json(body),
        )
    };
    // Unknown, malformed and foreign items.
    let r = patch("999", json!({"action": "approve"})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.error(), "Nie ma takiej pozycji.");
    assert_eq!(
        patch("abc", json!({"action": "approve"})).await.status,
        StatusCode::NOT_FOUND
    );
    let del = h
        .send(call("DELETE", "/api/admin/items/999").cookie(&cookie))
        .await;
    assert_eq!(del.status, StatusCode::NOT_FOUND);

    let queued = h.add("Q", ItemStatus::Queued);
    let r = patch(&queued.id.to_string(), json!({"action": "reject"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.error(), "Ta pozycja nie czeka na akceptację.");
    let pending = h.add("P", ItemStatus::PendingApproval);
    let r = patch(&pending.id.to_string(), json!({"position": 0})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.error(), "Przesuwać można tylko pozycje w kolejce.");
    let r = patch(&pending.id.to_string(), json!({"action": "reject"})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(h.item(pending.id).status, ItemStatus::Rejected);
    let r = h
        .send(call("DELETE", &format!("/api/admin/items/{}", pending.id)).cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.error(), "Ta pozycja już się zakończyła.");
}

#[tokio::test]
async fn patch_needs_an_action_or_a_non_negative_integer_position() {
    let h = Harness::new();
    let cookie = h.login().await;
    let item = h.add("Q", ItemStatus::Queued);
    let url = format!("/api/admin/items/{}", item.id);
    for body in [
        json!({}),
        json!({"position": -1}),
        json!({"position": 1.5}),
        json!({"position": "0"}),
        json!({"position": null}),
        json!({"action": "explode"}),
    ] {
        let r = h
            .send(call("PATCH", &url).cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(r.error(), "Podaj action (approve/reject) albo position.");
    }
    // No body at all is the same as an empty object.
    let r = h.send(call("PATCH", &url).cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // A whole-number float and a huge position are fine (clamped by the queue).
    for body in [json!({"position": 0.0}), json!({"position": 1e300})] {
        let r = h.send(call("PATCH", &url).cookie(&cookie).json(body)).await;
        assert_eq!(r.status, StatusCode::OK);
    }
}

#[tokio::test]
async fn settings_valid_ones_are_saved_bad_ones_rejected() {
    let h = Harness::new();
    let cookie = h.login().await;
    let put = |body: Value| {
        h.send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(&cookie)
                .json(body),
        )
    };
    let r = put(
        json!({"moderation": true, "allowedDomains": ["https://www.Vimeo.com/", "youtube.com"]}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json()["allowedDomains"],
        json!(["vimeo.com", "youtube.com"])
    );
    assert_eq!(r.json()["moderation"], true);
    let r = put(json!({"moderation": "tak"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!r.error().is_empty());
    let r = h
        .send(get("/api/admin/channels/main/settings").cookie(&cookie))
        .await;
    assert_eq!(r.json()["moderation"], true);
    assert_eq!(r.json()["maxDurationS"], 7200);
    // An empty body means no change.
    let r = h
        .send(call("PUT", "/api/admin/channels/main/settings").cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["moderation"], true);
}

/// Spec section 7: a new look of the ticker restarts the film on air (one command), its texts
/// do not (the clip's text file follows them by itself).
#[tokio::test]
async fn only_a_new_look_of_the_ticker_restarts_the_film_on_air() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    for (body, restart) in [
        (
            json!({"tickerMessages": ["Witajcie!"], "tickerShowNext": false}),
            false,
        ),
        (
            json!({"tickerShowAddress": true, "moderation": true}),
            false,
        ),
        (json!({"tickerEnabled": true}), true),
        // The same value again changes nothing.
        (json!({"tickerEnabled": true}), false),
        (
            json!({"tickerMode": "continuous", "tickerMessages": []}),
            true,
        ),
        (json!({"tickerIntervalS": 300}), true),
        (json!({"tickerSpeed": "fast"}), true),
        (json!({"tickerQr": "always"}), true),
        (json!({"tickerProgress": false}), true),
        // Polish, German...: the same font, the text file follows by itself.
        (json!({"slateLanguage": "pl"}), false),
        (json!({"slateLanguage": "de"}), false),
        // To or from Japanese: another font, the film starts again with it.
        (json!({"slateLanguage": "ja"}), true),
        (json!({"slateLanguage": "ja"}), false),
        (json!({"slateLanguage": "en"}), true),
        (json!({"tickerEnabled": false}), true),
        // Without the ticker nothing is drawn.
        (json!({"slateLanguage": "ja"}), false),
        (json!({"slateLanguage": "en", "tickerEnabled": true}), true),
    ] {
        let r = h
            .send(
                call("PUT", "/api/admin/channels/main/settings")
                    .cookie(&cookie)
                    .json(body.clone()),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{body}");
        let expected = if restart {
            vec![Command::ApplyTicker]
        } else {
            vec![]
        };
        assert_eq!(h.commands(), expected, "{body}");
    }
    let r = h
        .send(get("/api/admin/channels/main/settings").cookie(&cookie))
        .await;
    let s = r.json();
    assert_eq!(
        (
            &s["tickerEnabled"],
            &s["tickerMode"],
            &s["tickerIntervalS"],
            &s["tickerSpeed"],
            &s["tickerQr"],
            &s["tickerProgress"],
            &s["tickerMessages"],
        ),
        (
            &json!(true),
            &json!("continuous"),
            &json!(300),
            &json!("fast"),
            &json!("always"),
            &json!(false),
            &json!([]),
        )
    );
    let r = h
        .send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(&cookie)
                .json(json!({"tickerMessages": vec!["x"; 11]})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["code"], "ticker_too_many_messages");
}

/// A new slate set for another public address: the QR code the ticker draws is out of date,
/// so the film on air starts again with the new one (only when the ticker draws a code).
#[tokio::test]
async fn a_new_public_address_restarts_the_film_when_the_ticker_draws_its_code() {
    let captured = Arc::new(Mutex::new(None::<AppState>));
    let keep = Arc::clone(&captured);
    let mut h = Harness::with_state(config(), move |s| {
        *keep.lock().unwrap() = Some(s.clone());
        s
    });
    let state = captured.lock().unwrap().clone().unwrap();
    let set = |h: &Harness, patch: Value| {
        h.with_db(|c| {
            let patch = crate::db::sanitize_settings_patch(&patch).unwrap();
            SettingsRepo::new(c)
                .update(MAIN_CHANNEL_ID, &patch)
                .unwrap();
        })
    };
    // The ticker is off by default: nothing to redraw.
    state.after_slates_replaced(true, false);
    assert!(h.commands().is_empty());
    set(&h, json!({"tickerEnabled": true}));
    state.after_slates_replaced(true, false);
    assert_eq!(
        h.commands(),
        [Command::ApplyTickerCode {
            public_url: String::new()
        }]
    );
    // The address went away while the tunnel gets a new one: the film waits for that one.
    state.after_slates_replaced(true, true);
    assert!(h.commands().is_empty());
    // Another language only: the code is the same.
    state.after_slates_replaced(false, false);
    assert!(h.commands().is_empty());
    set(&h, json!({"tickerQr": "off"}));
    state.after_slates_replaced(true, false);
    assert!(h.commands().is_empty());
}

#[tokio::test]
async fn saved_request_limit_applies_to_the_next_request() {
    let h = Harness::new();
    let cookie = h.login().await;
    let r = h
        .send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(&cookie)
                .json(json!({"requestLimitCount": 1})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        h.request_default("https://youtu.be/a").await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        h.request_default("https://youtu.be/b").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn logout_invalidates_the_cookie_in_the_browser() {
    let h = Harness::new();
    let cookie = h.login().await;
    let r = h
        .send(call("POST", "/api/admin/logout").cookie(&cookie))
        .await;
    assert!(r.header("set-cookie").contains("Max-Age=0"));
    assert_eq!(
        r.header("set-cookie"),
        "vrcinema_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    );
    assert_eq!(r.json(), json!({"ok": true}));
}

#[tokio::test]
async fn bodies_are_limited_and_must_be_json() {
    let h = Harness::new();
    let big = vec![b'x'; 17 * 1024];
    let r = h
        .send(call("POST", "/api/admin/login").raw("application/json", &big))
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.error(), "Request body is too large");
    let r = h
        .send(call("POST", "/api/admin/login").raw("application/json", b"{oops"))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        r.error(),
        "Body is not valid JSON but content-type is set to 'application/json'"
    );
    let r = h
        .send(call("POST", "/api/admin/login").raw("application/json", b""))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        r.error(),
        "Body cannot be empty when content-type is set to 'application/json'"
    );
    let r = h
        .send(call("POST", "/api/admin/login").raw("application/xml", b"<a/>"))
        .await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(r.error(), "Unsupported Media Type");
}

#[tokio::test]
async fn a_text_plain_login_body_is_a_wrong_password_and_counts_as_a_failure() {
    let h = Harness::new();
    for _ in 0..5 {
        let r = h
            .send(call("POST", "/api/admin/login").raw("text/plain", b"tajne"))
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert_eq!(r.error(), "Złe hasło.");
    }
    let r = h
        .send(call("POST", "/api/admin/login").json(json!({"password": "tajne"})))
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

// ---------- pliki z dysku ----------

#[tokio::test]
async fn library_without_login_is_401() {
    let h = Harness::new();
    assert_eq!(
        h.send(get("/api/admin/library?root=0&path=")).await.status,
        StatusCode::UNAUTHORIZED
    );
    let r = h
        .send(
            call("POST", "/api/admin/channels/main/local")
                .json(json!({"root": 0, "path": "Film.mp4"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn lists_the_root_folder_and_a_subfolder() {
    let h = Harness::new();
    let cookie = h.login().await;
    let root = h
        .send(get("/api/admin/library?root=0&path=").cookie(&cookie))
        .await
        .json();
    assert_eq!(root["enabled"], true);
    assert_eq!(root["roots"].as_array().unwrap().len(), 1);
    assert_eq!(root["roots"][0]["index"], 0);
    assert!(root["roots"][0].get("path").is_none(), "no disk path leaks");
    assert_eq!(names(&root), ["Seria", "Film.mp4"]);
    assert_eq!(root["parent"], Value::Null);
    let sub = h
        .send(get("/api/admin/library?root=0&path=Seria").cookie(&cookie))
        .await
        .json();
    assert_eq!(names(&sub), ["Odc 1.mkv", "Odc 2.mkv", "Odc 10.mkv"]);
    assert_eq!(sub["parent"], "");
    // The query defaults to the first root and its top folder.
    let default = h
        .send(get("/api/admin/library").cookie(&cookie))
        .await
        .json();
    assert_eq!(names(&default), ["Seria", "Film.mp4"]);
}

#[tokio::test]
async fn library_is_disabled_without_media_dirs() {
    let h = Harness::new();
    let cookie = h.login().await;
    // Rebuild the app without a library.
    let state = AppState::new(
        config(),
        MAIN_CHANNEL_ID,
        Arc::clone(&h.db),
        Arc::new(TestResolver {
            metas: Arc::clone(&h.metas),
        }),
        EngineHandle {
            commands: mpsc::channel(1).0,
            snapshot: h.snapshots.subscribe(),
        },
    )
    .unwrap();
    let router = build_router(state);
    let ask = |method: &'static str, url: &'static str, body: Option<Value>| {
        let router = router.clone();
        let cookie = cookie.clone();
        async move {
            let mut b = Request::builder()
                .method(method)
                .uri(url)
                .header("cookie", cookie);
            let body = match body {
                Some(v) => {
                    b = b.header("content-type", "application/json");
                    Body::from(v.to_string())
                }
                None => Body::empty(),
            };
            let mut req = b.body(body).unwrap();
            req.extensions_mut()
                .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
            let res = router.oneshot(req).await.unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let (status, body) = ask("GET", "/api/admin/library", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"enabled": false, "roots": []}));
    let (status, body) = ask(
        "POST",
        "/api/admin/channels/main/local",
        Some(json!({"root": 0, "path": "Film.mp4"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"],
        "Pliki z dysku są wyłączone. Ustaw MEDIA_DIRS w pliku .env."
    );
}

#[tokio::test]
async fn leaving_the_folder_is_400() {
    let h = Harness::new();
    let cookie = h.login().await;
    let r = h
        .send(get("/api/admin/library?root=0&path=..").cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.error(), "Ścieżka wychodzi poza folder z filmami.");
    let add = h
        .send(
            call("POST", "/api/admin/channels/main/local")
                .cookie(&cookie)
                .json(json!({"root": 0, "path": "../x.mp4"})),
        )
        .await;
    assert_eq!(add.status, StatusCode::BAD_REQUEST);
    // Unknown root.
    let r = h
        .send(get("/api/admin/library?root=5&path=").cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.error(), "Nie ma takiego folderu głównego.");
    let r = h
        .send(get("/api/admin/library?root=abc&path=").cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn adds_a_single_file_as_local_with_a_title_from_its_name() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    let r = h
        .send(
            call("POST", "/api/admin/channels/main/local")
                .cookie(&cookie)
                .json(json!({"root": 0, "path": "Film.mp4"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json(), json!({"added": 1, "title": "Film"}));
    let items = h.list(ItemStatus::Queued);
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.title, "Film");
    assert_eq!(item.source_url, "local:0/Film.mp4");
    assert_eq!(item.added_by, AddedBy::Admin);
    assert_eq!(item.duration_s, Some(60.0));
    assert!(!item.is_live);
    assert_eq!(h.commands(), vec![Command::QueueChanged]);
}

#[tokio::test]
async fn adds_a_whole_folder_in_natural_order() {
    let h = Harness::new();
    let cookie = h.login().await;
    let r = h
        .send(
            call("POST", "/api/admin/channels/main/local")
                .cookie(&cookie)
                .json(json!({"root": 0, "path": "Seria"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["added"], 3);
    assert_eq!(r.json()["title"], "Seria");
    assert_eq!(queued_titles(&h), ["Odc 1", "Odc 2", "Odc 10"]);
}

#[tokio::test]
async fn adding_from_disk_reports_empty_folders_and_missing_files() {
    let h = Harness::new();
    std::fs::create_dir(h.media_dir.path().join("Pusty")).unwrap();
    let cookie = h.login().await;
    let add = |body: Value| {
        h.send(
            call("POST", "/api/admin/channels/main/local")
                .cookie(&cookie)
                .json(body),
        )
    };
    let r = add(json!({"root": "0", "path": "Pusty"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.error(), "W tym folderze nie ma plików wideo.");
    let r = add(json!({"root": 0, "path": "Nie ma.mp4"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.error(), "Plik nie istnieje: Nie ma.mp4");
    let r = add(json!({"path": "Film.mp4"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_public_page_does_not_reveal_file_paths() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.send(
        call("POST", "/api/admin/channels/main/local")
            .cookie(&cookie)
            .json(json!({"root": 0, "path": "Film.mp4"})),
    )
    .await;
    let body = h.send(get("/api/channels/main/state")).await.text();
    assert!(body.contains("Film"));
    assert!(!body.contains("local:"));
    let base = h
        .media_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(!body.contains(&base));
}

#[tokio::test]
async fn a_viewer_cannot_request_a_disk_file() {
    let h = Harness::new();
    let r = h.request_default("local:0/Film.mp4").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

// ---------- strony ----------

#[tokio::test]
async fn serves_the_request_page_and_the_panel() {
    let h = Harness::new();
    assert!(
        h.send(get("/"))
            .await
            .header("content-type")
            .contains("text/html")
    );
    assert!(
        h.send(get("/admin"))
            .await
            .header("content-type")
            .contains("text/html")
    );
    let index = h.send(get("/")).await;
    assert_eq!(index.status, StatusCode::OK);
    assert_eq!(
        index.body,
        std::fs::read(repo_public_dir().join("index.html")).unwrap()
    );
    assert_eq!(index.header("content-type"), "text/html; charset=utf-8");
    assert_eq!(index.header("cache-control"), "public, max-age=0");
    // Stage 4: `/admin` is the new interface (the window and the remote panel alike).
    let admin = h.send(get("/admin")).await;
    assert_eq!(admin.status, StatusCode::OK);
    assert_eq!(admin.header("content-type"), "text/html; charset=utf-8");
    assert_eq!(admin.header("cache-control"), "public, max-age=0");
    assert_eq!(
        admin.body,
        std::fs::read(repo_public_dir().join("app.html")).unwrap()
    );
}

#[tokio::test]
async fn serves_static_files_and_favicon() {
    let h = Harness::new();
    let css = h.send(get("/static/app/theme.css")).await;
    assert_eq!(css.status, StatusCode::OK);
    assert_eq!(css.header("content-type"), "text/css; charset=utf-8");
    assert_eq!(css.header("cache-control"), "public, max-age=0");
    let js = h.send(get("/static/app/viewer/main.js")).await;
    assert_eq!(
        js.header("content-type"),
        "application/javascript; charset=utf-8"
    );
    assert_eq!(js.header("cache-control"), "public, max-age=0");
    let missing = h.send(get("/static/nope.css")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        missing.json(),
        json!({
            "message": "Route GET:/static/nope.css not found",
            "error": "Not Found",
            "statusCode": 404
        })
    );
    assert_eq!(h.send(get("/static/")).await.status, StatusCode::NOT_FOUND);
    let fav = h.send(get("/favicon.ico")).await;
    assert_eq!(fav.status, StatusCode::NO_CONTENT);
    assert!(fav.body.is_empty());
}

/// The new player interface (stage 4) is served at `/admin` (the temporary `/app` is gone);
/// its modules and the bundled libraries come from `/static` with a JavaScript type (`.mjs`
/// too: browsers refuse module scripts of any other type).
#[tokio::test]
async fn serves_the_new_interface_and_its_libraries() {
    let h = Harness::new();
    assert_eq!(h.send(get("/app")).await.status, StatusCode::NOT_FOUND);
    let app = h.send(get("/admin")).await;
    assert_eq!(app.status, StatusCode::OK);
    assert_eq!(app.header("content-type"), "text/html; charset=utf-8");
    assert_eq!(
        app.body,
        std::fs::read(repo_public_dir().join("app.html")).unwrap()
    );
    assert!(String::from_utf8_lossy(&app.body).contains("/static/app/main.js"));
    for path in [
        "/static/app/main.js",
        "/static/vendor/preact.mjs",
        "/static/vendor/hls.mjs",
    ] {
        let r = h.send(get(path)).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert_eq!(
            r.header("content-type"),
            "application/javascript; charset=utf-8",
            "{path}"
        );
    }
    let css = h.send(get("/static/app/theme.css")).await;
    assert_eq!(css.header("content-type"), "text/css; charset=utf-8");
    let en = h.send(get("/static/locales/en.json")).await;
    assert_eq!(en.header("content-type"), "application/json; charset=utf-8");
}

#[tokio::test]
async fn unknown_routes_are_404_like_fastify() {
    let h = Harness::new();
    let r = h.send(get("/nic?x=1")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        r.json(),
        json!({"message": "Route GET:/nic?x=1 not found", "error": "Not Found", "statusCode": 404})
    );
    // A known path with another method is just as unknown (no 405).
    let r = h.send(call("DELETE", "/api/channels/main/state")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

// ---------- getClientIp ----------

fn ip_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("cf-connecting-ip", "5.5.5.5".parse().unwrap());
    h.insert("x-forwarded-for", "6.6.6.6, 7.7.7.7".parse().unwrap());
    h
}

#[test]
fn client_ip_defaults_to_the_connection_address() {
    let trust = Trust {
        cloudflare: false,
        proxy: false,
    };
    assert_eq!(
        get_client_ip(&ip_headers(), "127.0.0.1", trust),
        "127.0.0.1"
    );
    assert_eq!(
        get_client_ip(&ip_headers(), "::ffff:10.0.0.1", trust),
        "10.0.0.1"
    );
}

#[test]
fn client_ip_cloudflare() {
    let trust = Trust {
        cloudflare: true,
        proxy: false,
    };
    assert_eq!(get_client_ip(&ip_headers(), "127.0.0.1", trust), "5.5.5.5");
    assert_eq!(
        get_client_ip(&HeaderMap::new(), "127.0.0.1", trust),
        "127.0.0.1"
    );
}

#[test]
fn client_ip_reverse_proxy_takes_the_first_forwarded_address() {
    let trust = Trust {
        cloudflare: false,
        proxy: true,
    };
    assert_eq!(get_client_ip(&ip_headers(), "127.0.0.1", trust), "6.6.6.6");
    let both = Trust {
        cloudflare: true,
        proxy: true,
    };
    assert_eq!(get_client_ip(&ip_headers(), "127.0.0.1", both), "5.5.5.5");
}

#[test]
fn https_directly_or_through_a_trusted_proxy() {
    let mut h = HeaderMap::new();
    h.insert("x-forwarded-proto", "https, http".parse().unwrap());
    assert!(is_https(&HeaderMap::new(), true, false));
    assert!(!is_https(&h, false, false));
    assert!(is_https(&h, false, true));
    assert!(!is_https(&HeaderMap::new(), false, true));
}

// ---------- sesje ----------

#[test]
fn token_is_valid_until_expiry_and_resists_forgery() {
    let t = create_session_token("sekret", 1000, 0);
    assert!(verify_session_token(Some(&t), "sekret", 999));
    assert!(!verify_session_token(Some(&t), "sekret", 1001));
    assert!(!verify_session_token(Some(&t), "inny", 0));
    let forged = t.replacen("1000", "99999", 1);
    assert!(!verify_session_token(Some(&forged), "sekret", 0));
    assert!(!verify_session_token(Some(""), "sekret", 0));
    assert!(!verify_session_token(None, "sekret", 0));
    assert!(!verify_session_token(Some("nodot"), "sekret", 0));
    assert!(!verify_session_token(Some("12.a b"), "sekret", 0));
}

#[test]
fn token_format_is_expiry_dot_base64url_hmac() {
    let t = create_session_token("sekret", SESSION_TTL_MS, 5);
    let (expires, mac) = t.split_once('.').unwrap();
    assert_eq!(expires, (SESSION_TTL_MS + 5).to_string());
    assert_eq!(mac.len(), 43);
    assert!(
        mac.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    // HMAC-SHA256("sekret", "<expires>"), computed with Node's crypto:
    // createHmac('sha256','sekret').update('2592000005').digest('base64url')
    assert_eq!(mac, "5uxwXSC8dIePno8eQ19WXuet2X-gFKy1OnNErVapnFE");
}

#[test]
fn password_check_and_cookies() {
    assert!(check_password(Some("tajne"), "tajne"));
    assert!(!check_password(Some("tajn"), "tajne"));
    assert!(!check_password(None, "tajne"));
    let c = parse_cookies(Some(
        "a=1; vrcinema_session=x%20y; broken; b=%E0%A4%A; empty=",
    ));
    assert_eq!(c["a"], "1");
    assert_eq!(c["vrcinema_session"], "x y");
    assert_eq!(c["b"], "%E0%A4%A");
    assert_eq!(c["empty"], "");
    assert!(!c.contains_key("broken"));
    assert!(parse_cookies(None).is_empty());
    assert_eq!(
        session_cookie("tok", true, 60),
        "vrcinema_session=tok; Path=/; HttpOnly; SameSite=Lax; Max-Age=60; Secure"
    );
}

// ---------- Panel access of the desktop app (window key, remote switch, Argon2) ----------

const SECRET: &str = "ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss";

fn remote_off() -> AdminAuth {
    AdminAuth::App {
        remote_enabled: false,
        password_hash: None,
        session_epoch: 0,
    }
}

fn remote_on(password: &str) -> AdminAuth {
    AdminAuth::App {
        remote_enabled: true,
        password_hash: Some(hash_password(password)),
        session_epoch: 0,
    }
}

#[test]
fn session_kinds_are_signed_into_the_token() {
    let now = 1_000;
    let remote = create_session_token(SECRET, 5_000, now);
    let desktop = create_desktop_session_token(SECRET, 5_000, now);
    assert!(desktop.contains(".d."));
    assert_eq!(
        session_kind(Some(&remote), SECRET, now),
        Some(SessionKind::Remote)
    );
    assert_eq!(
        session_kind(Some(&desktop), SECRET, now),
        Some(SessionKind::Desktop)
    );
    assert!(verify_session_token(Some(&desktop), SECRET, now));
    // Expired, wrong secret.
    assert_eq!(session_kind(Some(&desktop), SECRET, now + 5_000), None);
    assert_eq!(session_kind(Some(&desktop), "inny", now), None);
    // A remote MAC cannot be relabelled as a desktop session, nor the other way round.
    let (expires, mac) = remote.split_once('.').unwrap();
    let relabelled = format!("{expires}.d.{mac}");
    assert_eq!(session_kind(Some(&relabelled), SECRET, now), None);
    let dmac = desktop.rsplit('.').next().unwrap();
    let stripped = format!("{expires}.{dmac}");
    assert_eq!(session_kind(Some(&stripped), SECRET, now), None);
    assert_eq!(session_kind(Some("5000.d"), SECRET, now), None);
}

#[test]
fn password_hash_is_argon2id_and_verifies() {
    let phc = hash_password("sekret");
    assert!(phc.starts_with("$argon2id$"), "{phc}");
    assert_ne!(phc, hash_password("sekret"), "salt must differ");
    assert!(verify_password("sekret", &phc));
    assert!(!verify_password("Sekret", &phc));
    assert!(!verify_password("sekret", "nie-jest-hashem"));
    assert!(!verify_password("sekret", ""));
}

#[test]
fn desktop_key_is_one_time_and_rotates() {
    let (key, first) = DesktopKey::generate();
    assert!(first.len() >= 43, "32 random bytes in base64url");
    assert!(!key.redeem("zly"));
    assert!(!key.redeem(""));
    assert!(key.redeem(&first));
    assert!(!key.redeem(&first), "second use");
    let second = key.rotate();
    assert_ne!(first, second);
    assert!(!key.redeem(&first));
    let third = key.rotate();
    assert!(
        !key.redeem(&second),
        "rotating invalidates the previous key"
    );
    assert!(key.redeem(&third));
    assert_ne!(DesktopKey::generate().1, DesktopKey::generate().1);
}

struct AppHarness {
    h: Harness,
    key: Arc<DesktopKey>,
    state: AppState,
}

fn app_harness(auth: AdminAuth) -> (AppHarness, String) {
    let (key, first) = DesktopKey::generate();
    let key = Arc::new(key);
    let mut captured = None;
    let k = Arc::clone(&key);
    let mut cfg = config();
    cfg.session_secret = SECRET.to_string();
    let h = Harness::with_state(cfg, |s| {
        let s = s.with_admin_auth(auth).with_desktop_key(k);
        captured = Some(s.clone());
        s
    });
    let state = captured.unwrap();
    (AppHarness { h, key, state }, first)
}

fn cookie_of(res: &Resp) -> String {
    res.header("set-cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn window_cookie(a: &AppHarness, key: &str) -> String {
    let res = a.h.send(get(&format!("/desktop-login?key={key}"))).await;
    assert_eq!(res.status, StatusCode::FOUND);
    cookie_of(&res)
}

async fn remote_login(a: &AppHarness, password: &str) -> Resp {
    a.h.send(call("POST", "/api/admin/login").json(json!({ "password": password })))
        .await
}

#[tokio::test]
async fn window_key_logs_in_once_and_redirects_without_the_key() {
    let (a, key) = app_harness(remote_off());
    let res = a.h.send(get(&format!("/desktop-login?key={key}"))).await;
    assert_eq!(res.status, StatusCode::FOUND);
    assert_eq!(res.header("location"), "/admin");
    assert_eq!(res.header("referrer-policy"), "no-referrer");
    let set_cookie = res.header("set-cookie");
    assert!(set_cookie.starts_with("vrcinema_session="), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Lax"));
    assert!(!set_cookie.contains("Secure"));
    assert!(!res.header("location").contains(&key));
    // The cookie opens the panel even though the remote panel is off.
    let cookie = cookie_of(&res);
    let panel = a.h.send(get("/api/admin/library").cookie(&cookie)).await;
    assert_eq!(panel.status, StatusCode::OK);

    // Second use of the same key: the same redirect, no session (the interface then says
    // the window's session ended).
    let again = a.h.send(get(&format!("/desktop-login?key={key}"))).await;
    assert_no_window_session(&again);
    // Nothing the key was in ends up in the log.
    assert!(a.h.logs().iter().all(|l| !l.contains(&key)));
}

/// A refused window key: the redirect to the interface, without a session or the key.
fn assert_no_window_session(res: &Resp) {
    assert_eq!(res.status, StatusCode::FOUND);
    assert_eq!(res.header("location"), "/admin");
    assert_eq!(res.header("referrer-policy"), "no-referrer");
    assert_eq!(res.header("set-cookie"), "");
}

/// Task 15: the old pages' script-readable marker cookie is gone; only the session cookie.
#[tokio::test]
async fn window_login_sets_only_the_session_cookie() {
    let (a, key) = app_harness(remote_off());
    let res = a.h.send(get(&format!("/desktop-login?key={key}"))).await;
    assert_eq!(res.status, StatusCode::FOUND);
    let all: Vec<String> = res
        .headers
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(all[0].starts_with("vrcinema_session=") && all[0].contains("HttpOnly"));
}

#[tokio::test]
async fn headless_server_has_no_wizard_page() {
    let h = Harness::new();
    assert_eq!(h.send(get("/wizard")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn window_login_cookie_is_secure_behind_a_trusted_https_proxy() {
    let (key, first) = DesktopKey::generate();
    let k = Arc::new(key);
    let mut cfg = config();
    cfg.trust_proxy = true;
    let h = Harness::with_state(cfg, |s| s.with_desktop_key(Arc::clone(&k)));
    let res = h
        .send(get(&format!("/desktop-login?key={first}")).header("x-forwarded-proto", "https"))
        .await;
    assert_eq!(res.status, StatusCode::FOUND);
    assert!(res.header("set-cookie").ends_with("; Secure"));
}

#[tokio::test]
async fn wrong_missing_or_malformed_window_key_gets_no_session() {
    let (a, key) = app_harness(remote_off());
    for url in [
        "/desktop-login".to_string(),
        "/desktop-login?key=".to_string(),
        "/desktop-login?key=zly".to_string(),
        "/desktop-login?klucz=x".to_string(),
        format!("/desktop-login?key={key}x"),
        format!("/desktop-login?key=%zz{key}"),
    ] {
        let res = a.h.send(get(&url)).await;
        assert_no_window_session(&res);
    }
    // A failed attempt does not burn the real key.
    assert!(!window_cookie(&a, &key).await.is_empty());
}

#[tokio::test]
async fn a_rotated_window_key_replaces_the_previous_one() {
    let (a, old) = app_harness(remote_off());
    let new = a.key.rotate();
    let res = a.h.send(get(&format!("/desktop-login?key={old}"))).await;
    assert_no_window_session(&res);
    window_cookie(&a, &new).await;
}

#[tokio::test]
async fn headless_mode_has_no_usable_window_key() {
    let h = Harness::new();
    let res = h.send(get("/desktop-login?key=cokolwiek")).await;
    assert_no_window_session(&res);
}

#[tokio::test]
async fn disabled_remote_panel_refuses_login_without_counting_it() {
    let (a, _) = app_harness(AdminAuth::App {
        remote_enabled: false,
        password_hash: Some(hash_password("sekret")),
        session_epoch: 0,
    });
    for _ in 0..7 {
        let res = remote_login(&a, "sekret").await;
        assert_eq!(res.status, StatusCode::FORBIDDEN);
        assert_eq!(res.error(), "Panel zdalny jest wyłączony.");
        assert_eq!(res.header("set-cookie"), "");
    }
    // Switching on later: the refusals above did not fill the limiter.
    a.state.set_admin_auth(remote_on("sekret"));
    assert_eq!(remote_login(&a, "sekret").await.status, StatusCode::OK);
}

#[tokio::test]
async fn enabled_remote_panel_without_a_password_is_refused() {
    let (a, _) = app_harness(AdminAuth::App {
        remote_enabled: true,
        password_hash: None,
        session_epoch: 0,
    });
    let res = remote_login(&a, "").await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(res.error(), "Panel zdalny jest wyłączony.");
}

#[tokio::test]
async fn enabled_remote_panel_checks_the_argon2_hash_and_keeps_the_limiter() {
    let (a, _) = app_harness(remote_on("sekret"));
    let login = |ip: &'static str, password: Value| {
        a.h.send(
            call("POST", "/api/admin/login")
                .ip(ip)
                .json(json!({ "password": password })),
        )
    };
    let ok = login("9.9.9.9", json!("sekret")).await;
    assert_eq!(ok.status, StatusCode::OK);
    let cookie = cookie_of(&ok);
    let now = crate::db::now_ms() as u64;
    let AdminAuth::App { password_hash, .. } = a.state.admin_auth() else {
        panic!("app mode");
    };
    // Signed with the key derived from the password hash, not with the plain secret.
    let remote_key = remote_session_secret(SECRET, password_hash.as_deref(), 0);
    assert_eq!(
        session_kind(cookie.strip_prefix("vrcinema_session="), &remote_key, now),
        Some(SessionKind::Remote)
    );
    assert_eq!(
        session_kind(cookie.strip_prefix("vrcinema_session="), SECRET, now),
        None
    );
    for p in [json!("zle"), json!(5), json!(null), json!("tajne")] {
        let res = login("9.9.9.9", p).await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED);
        assert_eq!(res.error(), "Złe hasło.");
    }
    let fifth = login("9.9.9.9", json!("zle")).await;
    assert_eq!(fifth.status, StatusCode::UNAUTHORIZED);
    // Five failures: now even the right password is blocked.
    let blocked = login("9.9.9.9", json!("sekret")).await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    // Another address is unaffected.
    let other = login("8.8.8.8", json!("sekret")).await;
    assert_eq!(other.status, StatusCode::OK);
}

#[tokio::test]
async fn desktop_routes_need_a_window_session_and_admin_routes_take_both() {
    let (a, key) = app_harness(remote_on("sekret"));
    let desktop_cookie = window_cookie(&a, &key).await;
    let remote_cookie = cookie_of(&remote_login(&a, "sekret").await);

    let router = axum::Router::new()
        .route(
            "/api/desktop/ping",
            axum::routing::get(|_: DesktopSession| async { "pong" }),
        )
        .with_state(a.state.clone());
    let ping = |cookie: Option<&str>| {
        let mut c = get("/api/desktop/ping");
        if let Some(cookie) = cookie {
            c = c.cookie(cookie);
        }
        send_to(router.clone(), c)
    };
    assert_eq!(ping(Some(&desktop_cookie)).await.status, StatusCode::OK);
    assert_eq!(
        ping(Some(&remote_cookie)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(ping(None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        ping(Some("vrcinema_session=1.zly")).await.status,
        StatusCode::UNAUTHORIZED
    );

    for cookie in [&desktop_cookie, &remote_cookie] {
        let res = a.h.send(get("/api/admin/library").cookie(cookie)).await;
        assert_eq!(res.status, StatusCode::OK);
    }
    let anon = a.h.send(get("/api/admin/library")).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn switching_the_remote_panel_off_ends_remote_sessions_but_not_the_window() {
    let (a, key) = app_harness(remote_on("sekret"));
    let desktop_cookie = window_cookie(&a, &key).await;
    let remote_cookie = cookie_of(&remote_login(&a, "sekret").await);
    a.state.set_admin_auth(remote_off());
    let r =
        a.h.send(get("/api/admin/library").cookie(&remote_cookie))
            .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let d =
        a.h.send(get("/api/admin/library").cookie(&desktop_cookie))
            .await;
    assert_eq!(d.status, StatusCode::OK);
}

#[tokio::test]
async fn an_empty_configured_password_never_authenticates() {
    let mut cfg = config();
    cfg.admin_password = String::new();
    let h = Harness::with_config(cfg);
    for body in [json!({"password": ""}), json!({})] {
        let res = h.send(call("POST", "/api/admin/login").json(body)).await;
        assert_ne!(res.status, StatusCode::OK);
        assert_eq!(res.status, StatusCode::FORBIDDEN);
        assert_eq!(res.error(), "Panel zdalny jest wyłączony.");
        assert_eq!(res.header("set-cookie"), "");
    }
    assert!(!check_password(Some(""), ""));
}

// ---------- Remote sessions follow the password and the panel switch (app mode) ----------

#[test]
fn remote_session_key_depends_on_secret_hash_and_epoch() {
    let k = remote_session_secret(SECRET, Some("$argon2id$a"), 0);
    assert!(!k.is_empty());
    assert_ne!(k, SECRET);
    assert_eq!(k, remote_session_secret(SECRET, Some("$argon2id$a"), 0));
    assert_ne!(k, remote_session_secret(SECRET, Some("$argon2id$b"), 0));
    assert_ne!(k, remote_session_secret(SECRET, Some("$argon2id$a"), 1));
    assert_ne!(k, remote_session_secret("inny", Some("$argon2id$a"), 0));
    assert_ne!(k, remote_session_secret(SECRET, None, 0));
}

#[tokio::test]
async fn headless_remote_tokens_stay_signed_with_the_session_secret() {
    let h = Harness::new();
    let cookie = h.login().await;
    let now = crate::db::now_ms() as u64;
    assert_eq!(
        session_kind(
            cookie.strip_prefix("vrcinema_session="),
            &"s".repeat(64),
            now
        ),
        Some(SessionKind::Remote)
    );
}

#[tokio::test]
async fn a_new_password_ends_remote_sessions_but_not_the_window() {
    let (a, key) = app_harness(remote_on("sekret-1"));
    let desktop_cookie = window_cookie(&a, &key).await;
    let old_remote = cookie_of(&remote_login(&a, "sekret-1").await);
    let ok =
        a.h.send(get("/api/admin/library").cookie(&old_remote))
            .await;
    assert_eq!(ok.status, StatusCode::OK);

    a.state.set_admin_auth(remote_on("sekret-2"));
    let stale =
        a.h.send(get("/api/admin/library").cookie(&old_remote))
            .await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    let window =
        a.h.send(get("/api/admin/library").cookie(&desktop_cookie))
            .await;
    assert_eq!(window.status, StatusCode::OK);

    assert_eq!(
        remote_login(&a, "sekret-1").await.status,
        StatusCode::UNAUTHORIZED
    );
    let fresh = cookie_of(&remote_login(&a, "sekret-2").await);
    let res = a.h.send(get("/api/admin/library").cookie(&fresh)).await;
    assert_eq!(res.status, StatusCode::OK);
}

#[tokio::test]
async fn switching_the_panel_on_again_does_not_revive_old_remote_sessions() {
    let hash = hash_password("sekret");
    let auth = |remote_enabled: bool, session_epoch: u64| AdminAuth::App {
        remote_enabled,
        password_hash: Some(hash.clone()),
        session_epoch,
    };
    let (a, _) = app_harness(auth(true, 0));
    let old_remote = cookie_of(&remote_login(&a, "sekret").await);
    a.state.set_admin_auth(auth(false, 0));
    a.state.set_admin_auth(auth(true, 1));
    let res =
        a.h.send(get("/api/admin/library").cookie(&old_remote))
            .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let fresh = cookie_of(&remote_login(&a, "sekret").await);
    let res = a.h.send(get("/api/admin/library").cookie(&fresh)).await;
    assert_eq!(res.status, StatusCode::OK);
}

#[tokio::test]
async fn headless_mode_has_no_desktop_routes() {
    let h = Harness::new();
    let cookie = h.login().await;
    for (method, url) in [
        ("GET", "/api/desktop/info"),
        ("GET", "/api/desktop/config"),
        ("POST", "/api/desktop/wizard/complete"),
    ] {
        let res = h.send(call(method, url).cookie(&cookie)).await;
        assert_eq!(res.status, StatusCode::NOT_FOUND, "{method} {url}");
        assert_eq!(res.json()["error"], "Not Found");
    }
}

// ---------- The producer's monitor (window only, spec 2026-10-21) ----------

fn monitor_router(a: &AppHarness) -> Router {
    super::routes_monitor::router(a.state.clone()).with_state(a.state.clone())
}

fn monitor_row(seq: u64, clip: &str, name: &str, path: &Path, position_s: f64) -> MonitorRow {
    MonitorRow {
        seq,
        clip: clip.into(),
        name: name.into(),
        path: path.to_path_buf(),
        duration: 4.0,
        item_id: 7,
        position_s,
        discontinuity: false,
    }
}

impl AppHarness {
    fn set_monitor(&self, feed: MonitorFeed) {
        let mut s = (*snapshot(self.h.live_dir.path(), &["5.ts"], idle_state())).clone();
        s.monitor = Arc::new(feed);
        self.h.snapshots.send_replace(Arc::new(s));
    }
}

const MONITOR_LIST: &str = "/api/desktop/monitor.m3u8";

#[tokio::test]
async fn the_preview_plays_a_disk_film_with_ranges_for_the_window_only() {
    let (a, key) = app_harness(remote_on("sekret"));
    let desktop = window_cookie(&a, &key).await;
    let remote = cookie_of(&remote_login(&a, "sekret").await);
    let dir = tempfile::tempdir().unwrap();
    let film = dir.path().join("Film 1.mp4");
    std::fs::write(&film, (0u8..100).collect::<Vec<u8>>()).unwrap();
    let source = crate::resolver::file::file_source_url(&film).unwrap();
    let item = a.h.with_db(|c| {
        QueueRepo::new(c)
            .add(&NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url: source,
                title: "Film 1".into(),
                duration_s: Some(10.0),
                is_live: false,
                status: ItemStatus::Playing,
                added_by: AddedBy::Admin,
                requester_name: None,
                requester_ip: None,
                requester_key_hash: None,
            })
            .unwrap()
    });
    let resolver = crate::resolver::file::FileResolver::new(Box::new(|_| Box::pin(async { None })));
    let state = a
        .state
        .clone()
        .with_preview(crate::preview_source::PreviewSources::new(
            Arc::new(resolver),
            crate::pipeline::temp_downloads::TempDownloads::new(dir.path().join("temp")),
            None,
        ));
    let router = super::routes_monitor::router(state.clone()).with_state(state);
    let status_url = format!("/api/desktop/preview?item={}", item.id);
    let media_url = format!("/api/desktop/preview/{}/media", item.id);

    let res = send_to(router.clone(), get(&status_url).cookie(&remote)).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = send_to(router.clone(), get(&media_url)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);

    let res = send_to(router.clone(), get(&status_url).cookie(&desktop)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json()["state"], "ready");
    assert_eq!(res.json()["url"], media_url.as_str());

    let whole = send_to(router.clone(), get(&media_url).cookie(&desktop)).await;
    assert_eq!(whole.status, StatusCode::OK);
    assert_eq!(whole.body.len(), 100);
    assert_eq!(whole.header("accept-ranges"), "bytes");
    assert_eq!(whole.header("content-type"), "video/mp4");
    let part = send_to(
        router.clone(),
        get(&media_url)
            .cookie(&desktop)
            .header("range", "bytes=10-19"),
    )
    .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.body, (10u8..20).collect::<Vec<u8>>());
    assert_eq!(part.header("content-range"), "bytes 10-19/100");
    let tail = send_to(
        router.clone(),
        get(&media_url)
            .cookie(&desktop)
            .header("range", "bytes=95-"),
    )
    .await;
    assert_eq!(tail.body.len(), 5);
    let beyond = send_to(
        router.clone(),
        get(&media_url)
            .cookie(&desktop)
            .header("range", "bytes=100-"),
    )
    .await;
    assert_eq!(beyond.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(beyond.header("content-range"), "bytes */100");

    // Unknown items, requests waiting for approval and films never asked for have no media.
    let res = send_to(
        router.clone(),
        get("/api/desktop/preview?item=999").cookie(&desktop),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let waiting = a.h.add("request", ItemStatus::PendingApproval);
    let res = send_to(
        router.clone(),
        get(&format!("/api/desktop/preview?item={}", waiting.id)).cookie(&desktop),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let res = send_to(
        router,
        get("/api/desktop/preview/999/media").cookie(&desktop),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_monitor_is_for_the_window_only() {
    let (a, key) = app_harness(remote_on("sekret"));
    let desktop = window_cookie(&a, &key).await;
    let remote = cookie_of(&remote_login(&a, "sekret").await);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("seg_00000.ts");
    std::fs::write(&file, [0x47u8; 188]).unwrap();
    a.set_monitor(MonitorFeed {
        rows: vec![monitor_row(0, "clip-7-0", "seg_00000.ts", &file, 0.0)],
        next_seq: 1,
        active: true,
        ..MonitorFeed::default()
    });
    let router = monitor_router(&a);
    for url in [MONITOR_LIST, "/api/desktop/monitor/clip-7-0/seg_00000.ts"] {
        let none = send_to(router.clone(), get(url)).await;
        assert_eq!(none.status, StatusCode::UNAUTHORIZED, "{url}");
        let res = send_to(router.clone(), get(url).cookie(&remote)).await;
        assert_eq!(res.status, StatusCode::FORBIDDEN, "{url}");
        assert_eq!(res.json()["code"], "window_only");
        let ok = send_to(router.clone(), get(url).cookie(&desktop)).await;
        assert_eq!(ok.status, StatusCode::OK, "{url}");
    }
    // A server without a window has no monitor at all.
    let h = Harness::new();
    let cookie = h.login().await;
    let res = h.send(get(MONITOR_LIST).cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_monitor_list_is_the_feed_and_segments_come_only_from_it() {
    let (a, key) = app_harness(remote_off());
    let cookie = window_cookie(&a, &key).await;
    let router = monitor_router(&a);
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("seg_00000.ts");
    let second = dir.path().join("seg_00001.ts");
    std::fs::write(&first, [0x47u8; 188]).unwrap();
    std::fs::write(&second, [0x47u8; 376]).unwrap();
    std::fs::write(dir.path().join("secret.ts"), b"not listed").unwrap();
    let mut jump = monitor_row(4, "clip-7-0-r1", "seg_00000.ts", &second, 60.0);
    jump.discontinuity = true;
    a.set_monitor(MonitorFeed {
        rows: vec![
            monitor_row(3, "clip-7-0", "seg_00000.ts", &first, 8.0),
            jump,
        ],
        next_seq: 5,
        active: true,
        antenna_seq: Some(3),
        aired_seq: Some(3),
        ..MonitorFeed::default()
    });
    assert!(!a.state.monitor_watch.watched());
    let list = send_to(router.clone(), get(MONITOR_LIST).cookie(&cookie)).await;
    assert_eq!(list.status, StatusCode::OK);
    // The channel feeds the monitor ahead while the window reads it (review fix 13).
    assert!(a.state.monitor_watch.watched());
    assert_eq!(list.header("content-type"), "application/vnd.apple.mpegurl");
    assert_eq!(list.header("cache-control"), "no-store");
    let text = list.text();
    assert!(text.contains("#EXT-X-MEDIA-SEQUENCE:3\n"), "{text}");
    assert!(
        text.contains("#EXT-X-DISCONTINUITY\n#VRCINEMA-AT:7,60.000\n#EXTINF:4.000000,\nmonitor/clip-7-0-r1/seg_00000.ts\n"),
        "{text}"
    );
    assert!(text.contains("#VRCINEMA-ANTENNA:3\n"), "{text}");
    let from = send_to(
        router.clone(),
        get(&format!("{MONITOR_LIST}?from=4")).cookie(&cookie),
    )
    .await;
    assert!(
        from.text()
            .contains("#EXT-X-START:TIME-OFFSET=4.000,PRECISE=YES\n"),
        "{}",
        from.text()
    );
    let junk = send_to(
        router.clone(),
        get(&format!("{MONITOR_LIST}?from=x")).cookie(&cookie),
    )
    .await;
    assert_eq!(junk.status, StatusCode::OK, "a bad start is ignored");
    assert!(!junk.text().contains("EXT-X-START"));

    let seg = send_to(
        router.clone(),
        get("/api/desktop/monitor/clip-7-0-r1/seg_00000.ts").cookie(&cookie),
    )
    .await;
    assert_eq!(seg.status, StatusCode::OK);
    assert_eq!(seg.body.len(), 376, "the row's own file (the jump)");
    assert_eq!(seg.header("content-type"), "video/mp2t");
    assert_eq!(seg.header("cache-control"), "no-store");
    for url in [
        "/api/desktop/monitor/clip-7-0/seg_00001.ts",
        "/api/desktop/monitor/clip-7-0/secret.ts",
        "/api/desktop/monitor/clip-7-1/seg_00000.ts",
        "/api/desktop/monitor/clip-7-0/..%2F..%2Fvrcinema.db",
        "/api/desktop/monitor/..%5Cclip-7-0/seg_00000.ts",
        "/api/desktop/monitor/clip-7-0/seg_00000.ts%00.txt",
        "/api/desktop/monitor/clip-7-0/seg_00000",
    ] {
        let res = send_to(router.clone(), get(url).cookie(&cookie)).await;
        assert_eq!(res.status, StatusCode::NOT_FOUND, "{url}");
    }
    // A listed row whose file is gone is a 404 too.
    std::fs::remove_file(&first).unwrap();
    let gone = send_to(
        router.clone(),
        get("/api/desktop/monitor/clip-7-0/seg_00000.ts").cookie(&cookie),
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);

    // Nothing on air: an empty list (200), the player shows "no preview".
    a.set_monitor(MonitorFeed {
        next_seq: 9,
        ..MonitorFeed::default()
    });
    let empty = send_to(router.clone(), get(MONITOR_LIST).cookie(&cookie)).await;
    assert_eq!(empty.status, StatusCode::OK);
    assert!(empty.text().starts_with("#EXTM3U\n"));
    assert!(!empty.text().contains("#EXTINF"), "{}", empty.text());
}

// ---------- Channel controls (stage 4) ----------

const CONTROL_ROUTES: [&str; 5] = ["pause", "resume", "seek", "restart", "play-now"];

fn film_control(duration_s: Option<f64>, is_live: bool) -> ControlState {
    ControlState {
        item_id: Some(7),
        is_live,
        position_s: 12.0,
        duration_s,
        can_pause: !is_live,
        can_seek: !is_live,
        seek_pending: false,
        position_approx: false,
        pause_pending: false,
    }
}

fn control_route(route: &str) -> Call {
    call("POST", &format!("/api/admin/channels/main/{route}"))
}

fn playing_with(live_dir: &Path, control: ControlState) -> Arc<Snapshot> {
    let mut state = idle_state();
    state.status = ChannelStatus::Playing;
    state.control = control;
    snapshot(live_dir, &["5.ts"], state)
}

impl Harness {
    fn set_control(&self, control: ControlState) {
        self.snapshots
            .send_replace(playing_with(self.live_dir.path(), control));
    }
}

#[tokio::test(start_paused = true)]
async fn control_routes_need_a_session() {
    let mut h = Harness::new();
    h.set_control(film_control(Some(100.0), false));
    for route in CONTROL_ROUTES {
        let res = h
            .send(control_route(route).json(json!({"toS": 1, "itemId": 1})))
            .await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{route}");
    }
    assert!(h.commands().is_empty());
}

#[tokio::test(start_paused = true)]
async fn control_routes_work_with_a_window_session_and_a_remote_password_session() {
    let (a, key) = app_harness(remote_on("sekret"));
    a.h.set_control(film_control(Some(100.0), false));
    let window = window_cookie(&a, &key).await;
    let remote = cookie_of(&remote_login(&a, "sekret").await);
    for cookie in [window, remote] {
        let res = a.h.send(control_route("pause").cookie(&cookie)).await;
        assert_eq!(res.status, StatusCode::OK);
        assert_eq!(res.json()["control"]["canPause"], true);
    }
}

#[tokio::test(start_paused = true)]
async fn pause_and_resume_send_the_manual_reason_and_answer_with_the_control_state() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    let mut control = film_control(Some(100.0), false);
    control.position_approx = true;
    let mut state = idle_state();
    state.status = ChannelStatus::Paused;
    state.paused = true;
    state.pause_reasons = vec![PauseReason::Tunnel, PauseReason::Manual];
    state.control = control;
    h.snapshots
        .send_replace(snapshot(h.live_dir.path(), &["5.ts"], state));
    let res = h.send(control_route("pause").cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        res.json(),
        json!({"control": {
            "itemId": 7, "isLive": false, "positionS": 12, "durationS": 100,
            "canPause": true, "canSeek": true, "seekPending": false, "positionApprox": true,
            "pausePending": false, "paused": true, "pauseReasons": ["tunnel", "manual"]
        }})
    );
    assert_eq!(h.commands(), [Command::Pause(PauseReason::Manual)]);
    let res = h.send(control_route("resume").cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(h.commands(), [Command::Resume(PauseReason::Manual)]);
}

#[tokio::test(start_paused = true)]
async fn pause_takes_the_position_of_the_preview_and_refuses_a_bad_one() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    let mut control = film_control(Some(100.0), false);
    control.pause_pending = true;
    h.set_control(control);
    for (body, command) in [
        (json!({"atS": 17.5}), Command::PauseAt { at_s: 17.5 }),
        (json!({"atS": 0}), Command::PauseAt { at_s: 0.0 }),
        (json!({"atS": 100}), Command::PauseAt { at_s: 100.0 }),
        (json!({"atS": null}), Command::Pause(PauseReason::Manual)),
        (json!({}), Command::Pause(PauseReason::Manual)),
    ] {
        let res = h
            .send(control_route("pause").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::OK, "{body}");
        assert_eq!(res.json()["control"]["pausePending"], true, "{body}");
        assert_eq!(h.commands(), [command], "{body}");
    }
    for body in [
        json!({"atS": -1}),
        json!({"atS": 100.5}),
        json!({"atS": "17"}),
        json!({"atS": true}),
    ] {
        let res = h
            .send(control_route("pause").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(res.json()["code"], "bad_request", "{body}");
        assert!(!res.error().is_empty());
    }
    assert!(h.commands().is_empty());
    // A film of unknown length takes any position; a live stream none (409 as without one).
    h.set_control(film_control(None, false));
    let res = h
        .send(
            control_route("pause")
                .cookie(&cookie)
                .json(json!({"atS": 5000})),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(h.commands(), [Command::PauseAt { at_s: 5000.0 }]);
    h.set_control(film_control(None, true));
    let res = h
        .send(
            control_route("pause")
                .cookie(&cookie)
                .json(json!({"atS": 5})),
        )
        .await;
    assert_eq!(res.status, StatusCode::CONFLICT);
    assert_eq!(res.json()["code"], "live_item");
    assert!(h.commands().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_answer_waits_until_the_loop_has_applied_the_command() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(Some(100.0), false));
    let mut commands = std::mem::replace(&mut h.commands, mpsc::channel(1).1);
    let placeholder = playing_with(h.live_dir.path(), ControlState::default());
    let snapshots = std::mem::replace(&mut h.snapshots, watch::channel(placeholder).0);
    let live_dir = h.live_dir.path().to_path_buf();
    let engine = tokio::spawn(async move {
        let command = commands.recv().await.unwrap();
        // A tick that had drained the commands just before this one, then the one applying it.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        snapshots.send_replace(playing_with(&live_dir, film_control(Some(100.0), false)));
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let mut applied = film_control(Some(100.0), false);
        applied.seek_pending = true;
        applied.position_s = 40.0;
        snapshots.send_replace(playing_with(&live_dir, applied));
        (command, snapshots)
    });
    let res = h
        .send(
            control_route("seek")
                .cookie(&cookie)
                .json(json!({"toS": 40})),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    let control = &res.json()["control"];
    assert_eq!(control["seekPending"], true);
    assert_eq!(control["positionS"], 40);
    assert_eq!(engine.await.unwrap().0, Command::Seek { to_s: 40.0 });
}

#[tokio::test(start_paused = true)]
async fn seek_takes_to_s_or_by_s_and_restart_is_its_own_command() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(Some(100.0), false));
    for (body, command) in [
        (json!({"toS": 30}), Command::Seek { to_s: 30.0 }),
        (json!({"toS": 12.5}), Command::Seek { to_s: 12.5 }),
        (json!({"byS": -10}), Command::SeekBy { delta_s: -10.0 }),
        (json!({"byS": 60}), Command::SeekBy { delta_s: 60.0 }),
    ] {
        let res = h
            .send(control_route("seek").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::OK, "{body}");
        assert_eq!(h.commands(), [command]);
    }
    let res = h.send(control_route("restart").cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(h.commands(), [Command::Restart]);
}

#[tokio::test(start_paused = true)]
async fn seek_refuses_bad_bodies_with_bad_request() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(Some(100.0), false));
    for body in [
        json!({}),
        json!({"toS": 1, "byS": 2}),
        json!({"toS": "10"}),
        json!({"byS": null}),
        json!({"to": 3}),
        json!([1]),
        json!(5),
    ] {
        let res = h
            .send(control_route("seek").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(res.json()["code"], "bad_request", "{body}");
        assert!(!res.error().is_empty());
    }
    let res = h.send(control_route("seek").cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.json()["code"], "bad_request");
    assert!(h.commands().is_empty());
}

#[tokio::test(start_paused = true)]
async fn without_a_film_pause_and_seek_are_a_conflict_but_resume_works() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    for (route, body) in [
        ("pause", json!(null)),
        ("seek", json!({"byS": 10})),
        ("restart", json!(null)),
    ] {
        let res = h
            .send(control_route(route).cookie(&cookie).json(body))
            .await;
        assert_eq!(res.status, StatusCode::CONFLICT, "{route}");
        assert_eq!(res.json()["code"], "nothing_playing", "{route}");
        assert!(!res.error().is_empty());
    }
    assert!(h.commands().is_empty());
    let res = h.send(control_route("resume").cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json()["control"]["itemId"], Value::Null);
    assert_eq!(h.commands(), [Command::Resume(PauseReason::Manual)]);
}

#[tokio::test(start_paused = true)]
async fn a_live_stream_is_a_conflict_for_pause_and_seek_but_skip_works() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(None, true));
    for (route, body) in [
        ("pause", json!(null)),
        ("seek", json!({"byS": 10})),
        ("restart", json!(null)),
    ] {
        let res = h
            .send(control_route(route).cookie(&cookie).json(body))
            .await;
        assert_eq!(res.status, StatusCode::CONFLICT, "{route}");
        assert_eq!(res.json()["code"], "live_item", "{route}");
    }
    assert!(h.commands().is_empty());
    let res = h.send(control_route("skip").cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(h.commands(), [Command::Skip]);
}

#[tokio::test(start_paused = true)]
async fn a_film_of_unknown_length_seeks_only_forward_or_back_to_the_start() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(None, false));
    for body in [json!({"toS": 30}), json!({"byS": -10})] {
        let res = h
            .send(control_route("seek").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(res.json()["code"], "bad_request");
    }
    assert!(h.commands().is_empty());
    let res = h
        .send(
            control_route("seek")
                .cookie(&cookie)
                .json(json!({"byS": 10})),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    let res = h.send(control_route("restart").cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        h.commands(),
        [Command::SeekBy { delta_s: 10.0 }, Command::Restart]
    );
}

/// Fix round 1 (5): the unknown-length rule agrees with the engine (`toS ≤ 0` is the restart).
#[tokio::test(start_paused = true)]
async fn a_film_of_unknown_length_may_seek_to_the_start() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(None, false));
    for (body, command) in [
        (json!({"toS": 0}), Command::Seek { to_s: 0.0 }),
        (json!({"toS": -3}), Command::Seek { to_s: -3.0 }),
    ] {
        let res = h
            .send(control_route("seek").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::OK, "{body}");
        assert_eq!(h.commands(), [command]);
    }
}

/// Fix round 1 (5): |byS| ≤ 1 day, |toS| ≤ 7 days.
#[tokio::test(start_paused = true)]
async fn seek_values_are_bounded() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(Some(100.0), false));
    for body in [
        json!({"byS": 86_401}),
        json!({"byS": -86_401}),
        json!({"toS": 604_801}),
        json!({"toS": -604_801}),
        json!({"toS": 1e300}),
    ] {
        let res = h
            .send(control_route("seek").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(res.json()["code"], "bad_request", "{body}");
    }
    assert!(h.commands().is_empty());
    for (body, command) in [
        (
            json!({"byS": 86_400}),
            Command::SeekBy { delta_s: 86_400.0 },
        ),
        (
            json!({"byS": -86_400}),
            Command::SeekBy { delta_s: -86_400.0 },
        ),
        (json!({"toS": 604_800}), Command::Seek { to_s: 604_800.0 }),
    ] {
        let res = h
            .send(control_route("seek").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::OK, "{body}");
        assert_eq!(h.commands(), [command]);
    }
}

/// Fix round 1 (4): a command the engine loop cannot take is 503, not a stale 200.
#[tokio::test(start_paused = true)]
async fn a_control_command_without_an_engine_loop_is_503() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(Some(100.0), false));
    // No loop behind the queue any more.
    drop(std::mem::replace(&mut h.commands, mpsc::channel(1).1));
    for (route, body) in [
        ("pause", json!(null)),
        ("resume", json!(null)),
        ("seek", json!({"toS": 10})),
        ("restart", json!(null)),
    ] {
        let res = h
            .send(control_route(route).cookie(&cookie).json(body))
            .await;
        assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE, "{route}");
        assert_eq!(res.json()["code"], "engine_unavailable", "{route}");
        assert!(!res.error().is_empty());
    }
    let queued = h.add("Q", ItemStatus::Queued);
    let res = h
        .send(
            control_route("play-now")
                .cookie(&cookie)
                .json(json!({"itemId": queued.id})),
        )
        .await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test(start_paused = true)]
async fn play_now_takes_a_queued_or_finished_item() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    let queued = h.add("Q", ItemStatus::Queued);
    let played = h.add("P", ItemStatus::Played);
    let failed = h.add("F", ItemStatus::Failed);
    for item in [&queued, &played, &failed] {
        let res = h
            .send(
                control_route("play-now")
                    .cookie(&cookie)
                    .json(json!({"itemId": item.id})),
            )
            .await;
        assert_eq!(res.status, StatusCode::OK, "{}", item.title);
        assert!(res.json()["control"].is_object());
        assert_eq!(h.commands(), [Command::PlayNow { item_id: item.id }]);
    }
    for status in [
        ItemStatus::PendingApproval,
        ItemStatus::Removed,
        ItemStatus::Rejected,
        ItemStatus::Playing,
    ] {
        let item = h.add("X", status);
        let res = h
            .send(
                control_route("play-now")
                    .cookie(&cookie)
                    .json(json!({"itemId": item.id})),
            )
            .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{status:?}");
        assert_eq!(res.json()["code"], "bad_request");
    }
    let res = h
        .send(
            control_route("play-now")
                .cookie(&cookie)
                .json(json!({"itemId": 9_999})),
        )
        .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    for body in [json!({"itemId": "1"}), json!({"itemId": 1.5}), json!({})] {
        let res = h
            .send(control_route("play-now").cookie(&cookie).json(body.clone()))
            .await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(res.json()["code"], "bad_request");
    }
    assert!(h.commands().is_empty());
}

#[tokio::test]
async fn the_panel_shows_the_control_state_and_the_public_state_does_not() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.set_control(film_control(Some(100.0), false));
    let body = h
        .send(get("/api/admin/channels/main").cookie(&cookie))
        .await
        .json();
    assert_eq!(body["control"]["itemId"], 7);
    assert_eq!(body["control"]["canSeek"], true);
    assert_eq!(body["control"]["durationS"], 100);
    let public = h.send(get("/api/channels/main/state")).await.json();
    assert!(public.get("control").is_none());
}

// ---------- nerd stats ----------

const STATS_ROUTE: &str = "/api/admin/channels/main/stats";

fn stats_snapshot(h: &Harness) -> Arc<Snapshot> {
    use crate::stats::{
        ChannelStats, FfmpegInfo, HlsCounters, NextInfo, NextState, OnAirInfo, ProbeSummary,
        StreamMode,
    };
    let mut snapshot = (**h.snapshots.borrow()).clone();
    snapshot.stats = ChannelStats {
        on_air: Some(OnAirInfo {
            title: "Kot".into(),
            source: "https://youtu.be/a".into(),
            position_s: 42.5,
            attempt: 1,
            probe: Some(ProbeSummary {
                codec: "h264".into(),
                height: 720,
                fps: 25.0,
                pix_fmt: "yuv420p".into(),
                has_audio: true,
                duration_s: Some(100.0),
            }),
            mode: Some(StreamMode::Copy),
            encoder: None,
            ready_segments: 2,
        }),
        hls: Some(HlsCounters {
            sequence: 17,
            window: 10,
            segment_s: 4.0,
            discontinuities: 3,
            last_pts: Some(88.25),
            dts_continuous: true,
        }),
        next: Some(NextInfo {
            title: "Pies".into(),
            state: NextState::Preparing,
            ready_segments: 0,
        }),
        ffmpeg: Some(FfmpegInfo {
            pid: std::process::id(),
            speed: Some(1.9),
            cpu_percent: None,
            rss_bytes: None,
        }),
        ticker_slow: false,
    };
    Arc::new(snapshot)
}

#[tokio::test]
async fn the_stats_route_needs_a_session() {
    let h = Harness::new();
    let res = h.send(get(STATS_ROUTE)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_stats_route_reports_every_group_and_no_viewer_address() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.snapshots.send_replace(stats_snapshot(&h));
    for ip in ["203.0.113.7", "203.0.113.8", "203.0.113.7"] {
        let res = h.send(get("/live/main/5.ts").ip(ip)).await;
        assert_eq!(res.status, StatusCode::OK);
    }

    let res = h.send(get(STATS_ROUTE).cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    let text = String::from_utf8(res.body.clone()).unwrap();
    assert!(!text.contains("203.0.113"), "an address leaked: {text}");
    let body = res.json();
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["onAir", "hls", "next", "ffmpeg", "viewers", "tunnel"]
    );

    assert_eq!(body["onAir"]["title"], "Kot");
    assert_eq!(body["onAir"]["positionS"], 42.5);
    assert_eq!(body["onAir"]["mode"], "copy");
    assert_eq!(body["onAir"]["encoder"], Value::Null);
    assert_eq!(body["onAir"]["readySegments"], 2);
    assert_eq!(body["onAir"]["probe"]["codec"], "h264");
    assert_eq!(body["onAir"]["probe"]["pixFmt"], "yuv420p");
    assert_eq!(body["onAir"]["probe"]["hasAudio"], true);
    assert_eq!(
        body["hls"],
        json!({
            "sequence": 17, "window": 10, "segmentS": 4.0, "discontinuities": 3,
            "lastPts": 88.25, "dtsContinuous": true
        })
    );
    assert_eq!(body["next"]["state"], "preparing");
    assert_eq!(body["ffmpeg"]["pid"], std::process::id());
    assert_eq!(body["ffmpeg"]["speed"], 1.9);
    #[cfg(windows)]
    assert!(body["ffmpeg"]["rssBytes"].as_u64().unwrap() > 0);

    let viewers = &body["viewers"];
    assert_eq!(viewers["watching"], 2);
    assert_eq!(viewers["fetchesPerMin"], 3);
    assert_eq!(viewers["history"].as_array().unwrap().len(), 60);
    assert_eq!(
        viewers["history"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .sum::<u64>(),
        3 * 188
    );
    // No app engine behind this server: no tunnel.
    assert_eq!(body["tunnel"], Value::Null);
}

#[tokio::test]
async fn the_stats_route_has_null_for_what_is_missing() {
    let h = Harness::new();
    let cookie = h.login().await;
    let res = h.send(get(STATS_ROUTE).cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    let body = res.json();
    for key in ["onAir", "hls", "next", "ffmpeg", "tunnel"] {
        assert_eq!(body[key], Value::Null, "{key}");
    }
    assert_eq!(body["viewers"]["watching"], 0);
    assert_eq!(body["viewers"]["bytesPerS"], 0);
    assert_eq!(body["viewers"]["history"], json!(vec![0; 60]));
}

#[tokio::test]
async fn the_stats_route_works_with_a_remote_password_session_too() {
    let (a, _key) = app_harness(remote_on("sekret"));
    let remote = cookie_of(&remote_login(&a, "sekret").await);
    let res = a.h.send(get(STATS_ROUTE).cookie(&remote)).await;
    assert_eq!(res.status, StatusCode::OK);
}

// ---------- QR code of the request page ----------

const QR_ROUTE: &str = "/api/admin/qr";

#[tokio::test]
async fn the_panel_qr_route_needs_a_session() {
    let h = Harness::new();
    let res = h.send(get(QR_ROUTE)).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_panel_qr_route_has_nothing_to_draw_without_a_public_address() {
    let h = Harness::new();
    let cookie = h.login().await;
    let res = h.send(get(QR_ROUTE).cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    assert!(res.json()["error"].as_str().unwrap().contains("adres"));
}

#[tokio::test]
async fn the_panel_qr_route_draws_a_png_that_is_never_cached() {
    let mut cfg = config();
    cfg.public_url = "https://tv.example.com".into();
    let h = Harness::with_config(cfg);
    let cookie = h.login().await;
    let res = h.send(get(QR_ROUTE).cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.header("content-type"), "image/png");
    assert_eq!(res.header("cache-control"), "no-store");
    assert_eq!(&res.body[..8], b"\x89PNG\r\n\x1a\n");
}

#[tokio::test]
async fn the_panel_qr_route_is_open_to_a_remote_password_session_too() {
    let (a, _key) = app_harness(remote_on("sekret"));
    let remote = cookie_of(&remote_login(&a, "sekret").await);
    let res = a.h.send(get(QR_ROUTE).cookie(&remote)).await;
    // Not refused (no 401/403): there is just no public address in this harness.
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

// ---------- environment of the panel (window or server without one) ----------

const ENVIRONMENT_ROUTE: &str = "/api/admin/environment";

#[tokio::test]
async fn the_environment_route_needs_a_session() {
    let h = Harness::new();
    assert_eq!(
        h.send(get(ENVIRONMENT_ROUTE)).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_environment_route_tells_a_server_without_a_window() {
    let h = Harness::new();
    let cookie = h.login().await;
    let res = h.send(get(ENVIRONMENT_ROUTE).cookie(&cookie)).await;
    assert_eq!(res.status, StatusCode::OK);
    let body = res.json();
    assert_eq!(body["window"], false);
    assert_eq!(body["tickerSlow"], false);
    assert_eq!(body["encoder"], "libx264");
    // The panel's language when nobody picked one in the browser: the server's (`LANGUAGE`).
    assert_eq!(body["language"], "pl");
    assert_eq!(body["sessionViaTunnel"], false);
    // No window: no tunnel and no channel state of the app.
    assert_eq!(body["tunnelEnabled"], Value::Null);
    assert_eq!(body["tunnel"], Value::Null);
    assert_eq!(body["channelRunning"], Value::Null);
    assert_eq!(body["channelError"], Value::Null);
    assert_eq!(body["publicUrl"], "");
}

/// Spec section 10: a session that came through the built-in tunnel is told apart (the panel
/// asks before switching the tunnel off from it). Only requests of cloudflared — a loopback
/// peer with Cloudflare's header while the tunnel's trust is on — count as such.
#[tokio::test]
async fn a_session_through_the_tunnel_is_told_apart() {
    let tunnel_live = || {
        let settings = crate::app_settings::AppSettings {
            trust_cloudflare: true,
            ..crate::app_settings::AppSettings::default()
        };
        crate::live::LiveSettings::from_config(&crate::app_settings::app_config(
            &settings,
            Path::new("H"),
        ))
    };
    let live = tunnel_live();
    live.cloudflare_from_loopback_only.set(true);
    let h = Harness::with_state(config(), move |s| s.with_live(live));
    let cookie = h.login().await;
    let via = |peer: &str, header: Option<&str>| {
        let mut c = get(ENVIRONMENT_ROUTE).cookie(&cookie).ip(peer);
        if let Some(value) = header {
            c = c.header("cf-connecting-ip", value);
        }
        c
    };
    let flag = |res: Resp| res.json()["sessionViaTunnel"].clone();
    assert_eq!(flag(h.send(via("127.0.0.1", Some("1.2.3.4"))).await), true);
    assert_eq!(flag(h.send(via("::1", Some("1.2.3.4"))).await), true);
    // The program's own window, a LAN device forging the header, an empty header.
    assert_eq!(flag(h.send(via("127.0.0.1", None)).await), false);
    assert_eq!(
        flag(h.send(via("192.168.1.5", Some("1.2.3.4"))).await),
        false
    );
    assert_eq!(flag(h.send(via("127.0.0.1", Some(" "))).await), false);

    // The manual Cloudflare trust (tunnel off): the header may come from anywhere.
    let manual = tunnel_live();
    let h = Harness::with_state(config(), move |s| s.with_live(manual));
    let cookie = h.login().await;
    let res = h
        .send(
            get(ENVIRONMENT_ROUTE)
                .cookie(&cookie)
                .header("cf-connecting-ip", "1.2.3.4"),
        )
        .await;
    assert_eq!(res.json()["sessionViaTunnel"], false);
}

/// A server without a window has none of the program's panel routes (Fastify's 404), and
/// the environment says why; with a session or without.
#[tokio::test]
async fn a_server_without_a_window_has_no_program_routes_for_the_panel() {
    let h = Harness::new();
    let cookie = h.login().await;
    for (method, path) in [
        ("GET", "/api/admin/info"),
        ("GET", "/api/admin/config"),
        ("PUT", "/api/admin/config"),
        ("GET", "/api/admin/tools"),
        ("POST", "/api/admin/tunnel/restart"),
        ("GET", "/api/admin/log"),
    ] {
        let res = h
            .send(
                call(method, path)
                    .cookie(&cookie)
                    .json(json!({"tunnelEnabled": false})),
            )
            .await;
        assert_eq!(res.status, StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(res.json()["statusCode"], 404, "{method} {path}");
    }
    let env = h.send(get(ENVIRONMENT_ROUTE).cookie(&cookie)).await.json();
    assert_eq!(env["window"], false);
    assert_eq!(env["restarting"], Value::Null);
}

// ---------- nicki widzów ----------

const KEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const KEY_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

impl Harness {
    /// A request with a nick and/or a device key, from its own address.
    async fn request_as(
        &self,
        ip: &str,
        name: Option<&str>,
        key: Option<&str>,
        lang: Option<&str>,
    ) -> Resp {
        let mut body = json!({"url": "https://youtu.be/a"});
        if let Some(n) = name {
            body["name"] = json!(n);
        }
        if let Some(k) = key {
            body["deviceKey"] = json!(k);
        }
        let mut c = call("POST", "/api/channels/main/requests")
            .ip(ip)
            .json(body);
        if let Some(l) = lang {
            c = c.header("x-vrcinema-lang", l);
        }
        self.send(c).await
    }
}

#[tokio::test]
async fn a_nick_with_a_device_key_gets_an_icon_and_is_reserved() {
    let h = Harness::new();
    let res = h
        .request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
        .await;
    assert_eq!(res.status, StatusCode::CREATED);
    let badge = res.json()["item"]["requesterBadge"].clone();
    assert!(badge["hue"].as_u64().unwrap() < 360);
    assert!((1..=12).contains(&badge["shape"].as_u64().unwrap()));
    // The same device keeps its nick and icon.
    let again = h
        .request_as("1.1.1.1", Some("kásia"), Some(KEY_A), None)
        .await;
    assert_eq!(again.status, StatusCode::CREATED);
    assert_eq!(again.json()["item"]["requesterBadge"], badge);
    // Another device cannot take a look-alike spelling.
    let taken = h
        .request_as("2.2.2.2", Some("Ka5ia"), Some(KEY_B), None)
        .await;
    assert_eq!(taken.status, StatusCode::CONFLICT);
    assert_eq!(taken.json()["code"], "nick_taken");
    assert_eq!(h.list(ItemStatus::Queued).len(), 2);
    // The queue shown to viewers carries the icon, never the key, its hash or an address.
    let state = h.send(get("/api/channels/main/state")).await;
    assert_eq!(state.json()["queue"][0]["requesterBadge"], badge);
    let hash = crate::nicks::hash_device_key(KEY_A).unwrap();
    for text in [res.text(), again.text(), taken.text(), state.text()] {
        assert!(!text.contains(&hash) && !text.contains(KEY_A), "{text}");
        assert!(
            !text.contains("1.1.1.1") && !text.contains("2.2.2.2"),
            "{text}"
        );
    }
}

/// Review focus 4, after task 16: a taken nick is refused in the viewer's language; a nick sent
/// without a device key is refused too (the page always sends one), so nothing slips in under
/// someone else's nick; the same viewer without a nick goes through as an anonymous one.
#[tokio::test]
async fn a_taken_nick_is_refused_in_the_viewers_language_and_a_nick_needs_a_key() {
    let h = Harness::new();
    h.request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
        .await;
    let en = h
        .request_as("3.3.3.3", Some("KASIA"), Some(KEY_B), Some("en"))
        .await;
    let pl = h
        .request_as("3.3.3.3", Some("KASIA"), Some(KEY_B), Some("pl"))
        .await;
    for nick in ["KASIA", "Free"] {
        let keyless = h.request_as("3.3.3.3", Some(nick), None, Some("en")).await;
        assert_eq!(keyless.status, StatusCode::BAD_REQUEST, "{nick}");
        assert_eq!(keyless.json()["code"], "invalid_device_key", "{nick}");
    }
    assert_eq!(en.status, StatusCode::CONFLICT);
    assert_eq!(pl.status, StatusCode::CONFLICT);
    assert_eq!(en.json()["code"], "nick_taken");
    assert_eq!(
        en.error(),
        "This nick is already taken by someone else. Pick another one."
    );
    assert_eq!(
        pl.error(),
        "Ten nick jest już zajęty przez kogoś innego. Wybierz inny."
    );
    assert_eq!(h.list(ItemStatus::Queued).len(), 1);
    // The same viewer without a nick goes through as an anonymous one.
    let anon = h.request_as("3.3.3.3", None, None, Some("pl")).await;
    assert_eq!(anon.status, StatusCode::CREATED);
    assert_eq!(anon.json()["item"]["requesterName"], Value::Null);
    assert_eq!(anon.json()["item"]["requesterBadge"], Value::Null);
}

#[tokio::test]
async fn words_for_host_are_reserved_and_a_bad_key_is_refused() {
    let h = Harness::new();
    for (i, w) in [
        "Gospodarz",
        "Host",
        "Anfitrión",
        "Gastgeber",
        "Hôte",
        "ホスト",
    ]
    .into_iter()
    .enumerate()
    {
        let res = h
            .request_as(&format!("4.4.4.{i}"), Some(w), Some(KEY_A), Some("en"))
            .await;
        assert_eq!(res.status, StatusCode::CONFLICT, "{w}");
        assert_eq!(res.json()["code"], "nick_reserved", "{w}");
    }
    let keyless = h.request_as("5.5.5.5", Some("Host"), None, None).await;
    assert_eq!(keyless.json()["code"], "invalid_device_key");
    for key in ["short", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"] {
        let res = h.request_as("6.6.6.6", Some("Ola"), Some(key), None).await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST);
        assert_eq!(res.json()["code"], "invalid_device_key");
    }
    assert!(h.list(ItemStatus::Queued).is_empty());
}

#[tokio::test]
async fn owner_manages_nicks_and_blocks_devices() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
        .await;
    h.request_as("2.2.2.2", Some("Basia"), Some(KEY_B), None)
        .await;
    let list = h.send(get("/api/admin/nicks").cookie(&cookie)).await;
    assert_eq!(list.status, StatusCode::OK);
    let nicks = list.json()["nicks"].clone();
    assert_eq!(nicks.as_array().unwrap().len(), 2);
    let kasia = nicks
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["nick"] == "Kasia")
        .unwrap()
        .clone();
    assert_eq!(kasia["skeleton"], crate::nicks::skeleton("Kasia"));
    assert!(kasia["badge"]["hue"].is_number() && kasia["lastUsedAt"].is_number());
    assert_eq!(kasia["blocked"], false);
    let hash = crate::nicks::hash_device_key(KEY_A).unwrap();
    assert!(!list.text().contains(&hash));
    // Needs the owner.
    assert_eq!(
        h.send(get("/api/admin/nicks")).await.status,
        StatusCode::UNAUTHORIZED
    );

    // Blocking: the device's requests are refused with 403 from now on.
    let block = h
        .send(
            call("POST", "/api/admin/devices/block")
                .cookie(&cookie)
                .json(json!({"skeleton": kasia["skeleton"]})),
        )
        .await;
    assert_eq!(block.status, StatusCode::OK);
    let refused = h
        .request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), Some("en"))
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.json()["code"], "device_blocked");
    assert_eq!(refused.error(), "You cannot add videos from this device.");
    let no_nick = h.request_as("1.1.1.1", None, Some(KEY_A), None).await;
    assert_eq!(no_nick.status, StatusCode::FORBIDDEN);
    let list = h.send(get("/api/admin/nicks").cookie(&cookie)).await.json();
    let blocked: Vec<_> = list["nicks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["nick"].as_str().unwrap().to_string(),
                n["blocked"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(blocked.contains(&("Kasia".to_string(), true)));
    assert!(blocked.contains(&("Basia".to_string(), false)));

    // Releasing frees the nick for another device.
    let url = format!(
        "/api/admin/nicks/{}/release",
        kasia["skeleton"].as_str().unwrap()
    );
    assert_eq!(
        h.send(call("POST", &url).cookie(&cookie)).await.status,
        StatusCode::OK
    );
    let gone = h.send(call("POST", &url).cookie(&cookie)).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    assert_eq!(gone.json()["code"], "nick_not_found");
    let ok = h
        .request_as("2.2.2.2", Some("Kasia"), Some(KEY_B), None)
        .await;
    assert_eq!(ok.status, StatusCode::CREATED);
    let unknown = h
        .send(
            call("POST", "/api/admin/devices/block")
                .cookie(&cookie)
                .json(json!({"skeleton": "nobody"})),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn panel_items_have_badges_and_never_an_address() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.add("Mine", ItemStatus::Queued);
    h.request_as("9.9.9.9", Some("Kasia"), Some(KEY_A), None)
        .await;
    h.request_as("8.8.8.8", None, None, None).await;
    let panel = h
        .send(get("/api/admin/channels/main").cookie(&cookie))
        .await;
    let text = panel.text();
    assert!(!text.contains("requesterIp"), "{text}");
    assert!(
        !text.contains("9.9.9.9") && !text.contains("8.8.8.8"),
        "{text}"
    );
    let queue = panel.json()["queue"].clone();
    assert_eq!(queue[0]["requesterBadge"], "host");
    assert!(queue[1]["requesterBadge"]["hue"].is_number());
    assert_eq!(queue[2]["requesterBadge"], Value::Null);
    // The per-viewer limit still counts by address in the database.
    assert_eq!(h.item(2).requester_ip.as_deref(), Some("9.9.9.9"));
}

impl Harness {
    async fn request_url_as(&self, ip: &str, url: &str, name: &str, key: &str) -> Resp {
        self.send(
            call("POST", "/api/channels/main/requests")
                .ip(ip)
                .json(json!({"url": url, "name": name, "deviceKey": key})),
        )
        .await
    }

    fn nick_holder(&self, nick: &str) -> Option<(String, i64)> {
        self.with_db(|c| {
            c.query_row(
                "SELECT key_hash, last_used_at FROM viewer_nicks WHERE skeleton = ?",
                [crate::nicks::skeleton(nick)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
        })
    }
}

#[tokio::test]
async fn invisible_characters_do_not_get_around_a_taken_or_reserved_nick() {
    let h = Harness::new();
    h.request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
        .await;
    for (i, nick) in [
        "Kas\u{200B}ia",
        "Ka\u{200D}sia",
        "K\u{2060}asia",
        "Ka\u{ad}sia",
        "\u{202E}Kasia",
    ]
    .into_iter()
    .enumerate()
    {
        let res = h
            .request_as(&format!("7.7.7.{i}"), Some(nick), Some(KEY_B), None)
            .await;
        assert_eq!(res.json()["code"], "nick_taken", "{nick:?}");
    }
    let res = h
        .request_as("7.7.8.1", Some("Ho\u{200B}st"), Some(KEY_B), None)
        .await;
    assert_eq!(res.json()["code"], "nick_reserved");
    // The displayed name has no invisible characters either.
    let ok = h
        .request_as("7.7.8.2", Some("Ba\u{200B}sia"), Some(KEY_B), None)
        .await;
    assert_eq!(ok.json()["item"]["requesterName"], "Basia");
}

#[tokio::test]
async fn a_refused_request_does_not_claim_or_refresh_a_nick() {
    let h = Harness::new();
    let hash = crate::nicks::hash_device_key(KEY_A).unwrap();
    let old = crate::db::now_ms() - 10 * 24 * 60 * 60 * 1000;
    h.with_db(|c| {
        crate::nicks::NickStore::new(c)
            .claim("Kasia", &hash, old)
            .unwrap()
    });

    // Bad link: neither a new nick is reserved nor the old one refreshed.
    let bad = h
        .request_url_as("1.1.1.1", "https://youtu.be/unknown", "Kasia", KEY_A)
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let bad = h
        .request_url_as("1.1.1.1", "https://youtu.be/unknown", "Basia", KEY_A)
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(h.nick_holder("Kasia"), Some((hash.clone(), old)));
    assert_eq!(h.nick_holder("Basia"), None);

    // Limiter: three requests pass, the fourth is refused before it can claim.
    for _ in 0..3 {
        assert_eq!(
            h.request("https://youtu.be/a", "2.2.2.2", None)
                .await
                .status,
            StatusCode::CREATED
        );
    }
    let limited = h
        .request_url_as("2.2.2.2", "https://youtu.be/a", "Kasia", KEY_A)
        .await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(h.nick_holder("Kasia"), Some((hash.clone(), old)));

    // Too many active items.
    h.update_settings(SettingsPatch {
        max_active_per_requester: Some(1),
        ..Default::default()
    });
    let active = h
        .request_url_as("2.2.2.2", "https://youtu.be/a", "Kasia", KEY_A)
        .await;
    assert_eq!(active.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(h.nick_holder("Kasia"), Some((hash.clone(), old)));

    // A request that goes through refreshes it.
    let ok = h
        .request_url_as("3.3.3.3", "https://youtu.be/a", "Kasia", KEY_A)
        .await;
    assert_eq!(ok.status, StatusCode::CREATED);
    assert!(h.nick_holder("Kasia").unwrap().1 > old);
    // The early check still answers a taken nick before the limiter.
    let taken = h
        .request_url_as("2.2.2.2", "https://youtu.be/a", "KASIA", KEY_B)
        .await;
    assert_eq!(taken.json()["code"], "nick_taken");
}

#[tokio::test]
async fn a_block_is_listed_after_the_nick_is_released_and_can_be_lifted() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
        .await;
    let sk = crate::nicks::skeleton("Kasia");
    let block = |sk: String| {
        call("POST", "/api/admin/devices/block")
            .cookie(&cookie)
            .json(json!({ "skeleton": sk }))
    };
    assert_eq!(h.send(block(sk.clone())).await.status, StatusCode::OK);
    let release = format!("/api/admin/nicks/{sk}/release");
    assert_eq!(
        h.send(call("POST", &release).cookie(&cookie)).await.status,
        StatusCode::OK
    );
    let list = h.send(get("/api/admin/nicks").cookie(&cookie)).await;
    let body = list.json();
    assert!(body["nicks"].as_array().unwrap().is_empty());
    let blocked = body["blocked"].as_array().unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["nick"], "Kasia");
    assert!(blocked[0]["blockedAt"].is_number() && blocked[0]["badge"]["hue"].is_number());
    let hash = crate::nicks::hash_device_key(KEY_A).unwrap();
    assert!(!list.text().contains(&hash));
    // Still refused.
    assert_eq!(
        h.request_as("1.1.1.1", None, Some(KEY_A), None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // Unblock.
    let unblock = |id: Value| {
        call("POST", "/api/admin/devices/unblock")
            .cookie(&cookie)
            .json(json!({ "id": id }))
    };
    assert_eq!(
        h.send(unblock(blocked[0]["id"].clone())).await.status,
        StatusCode::OK
    );
    let again = h.send(unblock(blocked[0]["id"].clone())).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    assert_eq!(again.json()["code"], "block_not_found");
    assert_eq!(
        h.send(call("POST", "/api/admin/devices/unblock").json(json!({"id": 1})))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
            .await
            .status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn a_database_failure_is_a_500_not_a_taken_nick() {
    let h = Harness::new();
    h.with_db(|c| c.execute_batch("DROP TABLE blocked_devices").unwrap());
    let res = h
        .request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
        .await;
    assert_eq!(res.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(res.json()["code"], "internal");
}

// ---------- link modes and the protection of the home network ----------

fn set_links(h: &Harness, mode: LinkMode) {
    h.update_settings(SettingsPatch {
        link_mode: Some(mode),
        allowed_domains: Some(
            [
                "youtube.com",
                "youtu.be",
                "evil.example",
                "mixed.example",
                "v6.example",
            ]
            .map(String::from)
            .to_vec(),
        ),
        blocked_domains: Some(vec!["bad.example".into()]),
        ..Default::default()
    });
}

async fn refused(h: &Harness, url: &str, ip: &str) -> (String, String) {
    let res = h.request(url, ip, None).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{url}: {}", res.text());
    let body = res.json();
    (
        body["code"].as_str().unwrap().to_string(),
        body["error"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn the_all_mode_takes_any_public_site() {
    let h = Harness::new();
    h.metas
        .lock()
        .unwrap()
        .insert("https://vimeo.com/1".into(), Ok(video("Vimeo", 100.0)));
    let res = h.request_default("https://vimeo.com/1").await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
}

#[tokio::test]
async fn the_allowlist_refuses_other_sites_with_the_list_in_the_answer() {
    let h = Harness::new();
    set_links(&h, LinkMode::Allowlist);
    let (code, error) = refused(&h, "https://vimeo.com/1", "1.1.1.1").await;
    assert_eq!(code, "domain_not_allowed");
    assert!(error.starts_with("Ten serwis nie jest dozwolony. Dozwolone: youtube.com"));
}

#[tokio::test]
async fn the_blocklist_refuses_its_sites_and_subdomains_and_takes_the_rest() {
    let h = Harness::new();
    set_links(&h, LinkMode::Blocklist);
    for url in ["https://bad.example/x", "https://CDN.Bad.Example./x"] {
        let (code, error) = refused(&h, url, "1.1.1.1").await;
        assert_eq!(code, "domain_not_allowed", "{url}");
        assert_eq!(error, "Ten serwis jest zablokowany na tym kanale.", "{url}");
    }
    h.metas
        .lock()
        .unwrap()
        .insert("https://vimeo.com/1".into(), Ok(video("Vimeo", 100.0)));
    let res = h.request_default("https://vimeo.com/1").await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
}

#[tokio::test]
async fn the_home_network_is_refused_in_every_mode_without_asking_the_resolver() {
    for mode in LinkMode::ALL {
        let h = Harness::new();
        set_links(&h, mode);
        // The resolver knows all of these: any answer but the refusal would mean it was asked.
        for (n, url) in [
            "http://192.168.1.1/x",
            "http://127.0.0.1:8080/x",
            "http://[::1]/x",
            "http://[::ffff:192.168.1.1]/x",
            "http://localhost/x",
            "http://router.local/x",
            "http://nas.lan/x",
            "http://x.home.arpa/x",
            "http://user@127.0.0.1/x",
            "http://evil.example/x",
            "http://mixed.example/x",
            "http://v6.example/x",
        ]
        .into_iter()
        .enumerate()
        {
            h.metas
                .lock()
                .unwrap()
                .insert(url.into(), Ok(video("Domowy", 10.0)));
            let (code, error) = refused(&h, url, &format!("9.9.9.{n}")).await;
            assert_eq!(code, "local_address", "{url} {mode:?}");
            assert_eq!(
                error, "Linki do sieci lokalnej lub do tego komputera są niedozwolone.",
                "{url}"
            );
        }
        assert!(h.list(ItemStatus::Queued).is_empty());
    }
}

#[tokio::test]
async fn other_schemes_are_refused_but_the_internal_kind_stays_an_invalid_link() {
    let h = Harness::new();
    for url in [
        "file:///C:/Windows/win.ini",
        "ftp://youtu.be/x",
        "javascript:alert(1)",
    ] {
        let (code, error) = refused(&h, url, "1.1.1.1").await;
        assert_eq!(code, "bad_scheme", "{url}");
        assert_eq!(error, "Dozwolone są tylko linki http i https.");
    }
    for url in ["local:0/movie.mp4", "to nie link", ""] {
        let (code, _) = refused(&h, url, "1.1.1.1").await;
        assert_eq!(code, "invalid_link", "{url:?}");
    }
}

#[tokio::test]
async fn the_refusals_speak_the_language_of_the_request() {
    let h = Harness::new();
    set_links(&h, LinkMode::Blocklist);
    let ask = |url: &str, lang: &str| {
        h.send(
            call("POST", "/api/channels/main/requests")
                .header("x-vrcinema-lang", lang)
                .json(json!({"url": url})),
        )
    };
    for (url, en, pl) in [
        (
            "http://192.168.0.1/",
            "Links to a local network or to this computer are not allowed.",
            "Linki do sieci lokalnej lub do tego komputera są niedozwolone.",
        ),
        (
            "ftp://x.example/",
            "Only http and https links are allowed.",
            "Dozwolone są tylko linki http i https.",
        ),
        (
            "https://bad.example/",
            "This site is blocked on this channel.",
            "Ten serwis jest zablokowany na tym kanale.",
        ),
    ] {
        assert_eq!(ask(url, "en").await.error(), en);
        assert_eq!(ask(url, "pl").await.error(), pl);
    }
}

#[tokio::test]
async fn a_refusal_of_the_address_does_not_use_up_the_limit() {
    let h = Harness::new();
    // Five refusals from one address; the limit is three requests.
    for _ in 0..5 {
        let (code, _) = refused(&h, "http://192.168.1.1/", "5.5.5.5").await;
        assert_eq!(code, "local_address");
    }
    h.metas
        .lock()
        .unwrap()
        .insert("https://youtu.be/ok".into(), Ok(video("Ok", 10.0)));
    let res = h
        .request("https://youtu.be/ok", "5.5.5.5", Some("Ola"))
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
}

#[tokio::test]
async fn a_name_whose_lookup_times_out_is_refused_in_the_viewers_language() {
    let h = Harness::with_state(config(), |s| {
        s.with_host_resolver(Arc::new(host_table().timing_out("slow.example")))
    });
    let (code, error) = refused(&h, "https://slow.example/x", "1.1.1.1").await;
    assert_eq!(code, "lookup_failed");
    assert_eq!(
        error,
        "Nie udało się w porę sprawdzić adresu tej strony. Spróbuj za chwilę."
    );
    // A single-word name is the owner's network.
    let (code, _) = refused(&h, "http://nas/x", "1.1.1.2").await;
    assert_eq!(code, "local_address");
}

#[tokio::test]
async fn the_owner_adds_links_to_the_local_network_in_every_mode() {
    for mode in LinkMode::ALL {
        let h = Harness::new();
        set_links(&h, mode);
        let cookie = h.login().await;
        h.metas
            .lock()
            .unwrap()
            .insert("http://192.168.0.5/v.mp4".into(), Ok(video("Domowy", 10.0)));
        let res = h
            .send(
                call("POST", "/api/admin/channels/main/items")
                    .cookie(&cookie)
                    .json(json!({"url": "http://192.168.0.5/v.mp4"})),
            )
            .await;
        assert_eq!(res.status, StatusCode::CREATED, "{mode:?}: {}", res.text());
    }
}

#[tokio::test]
async fn the_settings_carry_the_modes_the_lists_and_the_popular_sites() {
    let h = Harness::new();
    let cookie = h.login().await;
    let get_settings = || h.send(get("/api/admin/channels/main/settings").cookie(&cookie));
    let body = get_settings().await.json();
    assert_eq!(body["linkMode"], "all");
    assert_eq!(body["blockedDomains"], json!([]));
    let popular = body["popularDomains"].as_array().unwrap();
    assert_eq!(popular.len(), 16);
    assert_eq!(popular[0], "youtube.com");
    let r = h
        .send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(&cookie)
                .json(json!({"linkMode": "blocklist", "blockedDomains": ["Bad.Example"]})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let body = get_settings().await.json();
    assert_eq!(body["linkMode"], "blocklist");
    assert_eq!(body["blockedDomains"], json!(["bad.example"]));
    assert_eq!(body["allowedDomains"][0], "youtube.com");
    // The public page learns the mode (to list the supported sites in the allow list mode).
    let state = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(state["rules"]["linkMode"], "blocklist");
    let r = h
        .send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(&cookie)
                .json(json!({"linkMode": "nothing"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn queue_tools_remove_a_viewer_or_everything_block_undo_and_play_next() {
    let mut h = Harness::new();
    let cookie = h.login().await;
    let viewer = |title: &str, key: &str, status: ItemStatus| {
        h.with_db(|c| {
            QueueRepo::new(c)
                .add(&NewItem {
                    channel_id: MAIN_CHANNEL_ID,
                    source_url: format!("https://youtu.be/{title}"),
                    title: title.into(),
                    duration_s: Some(1.0),
                    is_live: false,
                    status,
                    added_by: AddedBy::Public,
                    requester_name: Some("Ola".into()),
                    requester_ip: Some("1.1.1.1".into()),
                    requester_key_hash: crate::nicks::hash_device_key(key),
                })
                .unwrap()
        })
    };
    let on_air = viewer("on-air", KEY_A, ItemStatus::Queued);
    h.with_db(|c| QueueRepo::new(c).start_playing(on_air.id).unwrap());
    let a = viewer("a", KEY_A, ItemStatus::Queued);
    viewer("a2", KEY_A, ItemStatus::PendingApproval);
    let host = h.add("host", ItemStatus::Queued);
    let b = viewer("b", KEY_B, ItemStatus::Queued);
    h.commands();
    let post = |path: &str, body: Value| {
        h.send(
            call("POST", &format!("/api/admin/channels/main/{path}"))
                .cookie(&cookie)
                .json(body),
        )
    };

    let r = post("remove-requester", json!({"itemId": a.id, "dryRun": true})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json(),
        json!({"name": "Ola", "queued": 1, "pending": 1,
               "playing": {"itemId": on_air.id, "title": "on-air"}, "canBlock": true, "blocked": false})
    );
    let r = post("remove-requester", json!({"itemId": host.id})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.json()["code"], "no_requester");

    let r = post("remove-requester", json!({"itemId": a.id, "block": true})).await;
    assert_eq!(r.status, StatusCode::OK);
    let body = r.json();
    assert_eq!(
        (body["removed"].clone(), body["blocked"].clone()),
        (json!(2), json!(true))
    );
    assert_eq!(h.item(a.id).status, ItemStatus::Removed);
    assert_eq!(
        h.item(on_air.id).status,
        ItemStatus::Playing,
        "on air stays"
    );
    let key_a = crate::nicks::hash_device_key(KEY_A).unwrap();
    let blocked =
        |h: &Harness| h.with_db(|c| crate::nicks::NickStore::new(c).is_blocked(&key_a).unwrap());
    assert!(blocked(&h));

    let r = post("undo", json!({"undo": body["undo"]})).await;
    assert_eq!(r.json(), json!({"restored": 2}));
    assert_eq!(h.item(a.id).status, ItemStatus::Queued);
    assert!(!blocked(&h), "the undo lifts the block it made");
    let r = post("undo", json!({"undo": body["undo"]})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["code"], "undo_expired");

    // Clearing takes everything waiting, but not the film on air unless asked.
    let r = post("clear", json!({})).await;
    assert_eq!(r.json()["removed"], 4);
    assert_eq!(h.item(on_air.id).status, ItemStatus::Playing);
    assert_eq!(h.item(b.id).status, ItemStatus::Removed);
    let r = post("clear", json!({"includePlaying": true})).await;
    assert_eq!(r.json()["removed"], 1);
    assert_eq!(h.item(on_air.id).status, ItemStatus::Removed);
    let r = post("clear", json!({"includePlaying": "yes"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Play next: a queued video moves first, a played one comes back first.
    h.with_db(|c| {
        QueueRepo::new(c)
            .set_status(host.id, ItemStatus::Played, None)
            .unwrap()
    });
    let r = post("play-next", json!({"itemId": host.id})).await;
    assert_eq!(r.status, StatusCode::OK);
    let again = r.json()["itemId"].as_i64().unwrap();
    assert_ne!(again, host.id);
    let c = h.add("c", ItemStatus::Queued);
    assert_eq!(h.list(ItemStatus::Queued)[0].id, again);
    let r = post("play-next", json!({"itemId": c.id})).await;
    assert_eq!(r.json()["itemId"], c.id);
    assert_eq!(h.list(ItemStatus::Queued)[0].id, c.id);
    let r = post("play-next", json!({"itemId": a.id})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.json()["code"], "not_playable_next");
}
