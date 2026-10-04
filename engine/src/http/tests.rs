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
    SESSION_TTL_MS, check_password, create_session_token, parse_cookies, session_cookie,
    verify_session_token,
};
use super::client_ip::{Trust, get_client_ip, is_https};
use super::{AppState, HttpConfig, build_router};
use crate::db::{
    AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueItem, QueueRepo, SettingsPatch,
    SettingsRepo, open_database,
};
use crate::engine::{Command, EngineHandle, Snapshot};
use crate::library::MediaLibrary;
use crate::pipeline::channel::{ChannelState, ChannelStatus, NowPlaying};
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

fn config() -> HttpConfig {
    HttpConfig {
        admin_password: "tajne".to_string(),
        session_secret: "s".repeat(64),
        public_url: String::new(),
        trust_cloudflare: false,
        trust_proxy: false,
    }
}

fn idle_state() -> ChannelState {
    ChannelState {
        status: ChannelStatus::Idle,
        current: None,
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
                    },
                    PlaylistEntry {
                        url: "https://youtu.be/p2".into(),
                        title: "P2".into(),
                        duration_s: Some(20.0),
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
            repo_public_dir(),
        )
        .unwrap()
        .with_library(Some(Arc::new(MediaLibrary::new(vec![
            media_dir.path().to_path_buf(),
        ]))))
        .with_log(Arc::new(move |m: &str| {
            sink.lock().unwrap().push(m.to_string())
        }));
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
        let mut builder = Request::builder().method(c.method).uri(&c.url);
        for (k, v) in &c.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let mut req = builder
            .body(Body::from(c.body.unwrap_or_default()))
            .unwrap();
        let addr = SocketAddr::new(c.ip.parse().unwrap(), 40_000);
        req.extensions_mut().insert(ConnectInfo(addr));
        let res = self.router.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        Resp {
            status: parts.status,
            headers: parts.headers,
            body: body.collect().await.unwrap().to_bytes().to_vec(),
        }
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
            Some(n) => json!({"url": url, "name": n}),
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
    assert_eq!(missing.json(), json!({"error": "Brak fragmentu"}));
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

// ---------- zgłoszenia publiczne ----------

#[tokio::test]
async fn adds_a_film_to_the_end_of_the_queue_with_a_nickname() {
    let h = Harness::new();
    let res = h
        .request("https://youtu.be/a", "1.1.1.1", Some("  Ola  "))
        .await;
    assert_eq!(res.status, StatusCode::CREATED);
    assert_eq!(res.json()["status"], "queued");
    assert_eq!(
        res.json()["item"],
        json!({"id": 1, "title": "A", "durationS": 100, "requesterName": "Ola", "isLive": false})
    );
    let items = h.list(ItemStatus::Queued);
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.title, "A");
    assert_eq!(item.requester_name.as_deref(), Some("Ola"));
    assert_eq!(item.requester_ip.as_deref(), Some("1.1.1.1"));
    assert_eq!(item.added_by, AddedBy::Public);
    assert_eq!(
        h.logs(),
        vec!["Zgłoszenie od 1.1.1.1: \"A\" (queued)".to_string()]
    );
}

#[tokio::test]
async fn nickname_is_cleaned_and_shortened() {
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
        vec![Some("Ola".to_string()), None, Some("x".repeat(32))]
    );
}

#[tokio::test]
async fn disallowed_domain_is_400_without_asking_the_resolver() {
    let h = Harness::new();
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
        "Masz już 1 filmów w kolejce. Poczekaj, aż się odtworzą."
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
    assert_eq!(
        body["queue"][0],
        json!({"id": 1, "title": "A", "durationS": 100, "requesterName": "Ola", "isLive": false})
    );
    assert!(!res.text().contains("1.1.1.1"));
    assert_eq!(body["channelUrl"], "http://tv.local:8080/live/main.m3u8");
    assert_eq!(body["requestUrl"], "http://tv.local:8080/");
    assert_eq!(body["status"], "idle");
    assert_eq!(body["current"], Value::Null);
    assert_eq!(
        body["rules"],
        json!({
            "allowedDomains": ["youtube.com", "youtu.be", "pixeldrain.com", "cda.pl"],
            "maxDurationS": 7200,
            "moderation": false
        })
    );
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "status",
            "current",
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
                is_live: false,
            }),
        },
    ));
    let body = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(body["status"], "playing");
    assert_eq!(
        body["current"],
        json!({
            "itemId": 7, "title": "Film", "durationS": 100, "positionS": 8,
            "requesterName": "Ola", "isLive": false
        })
    );
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
    assert_eq!(res.json(), json!({"error": "Zaloguj się."}));
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
        assert_eq!(res.json(), json!({"error": "Złe hasło."}));
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
        repo_public_dir(),
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
}

#[tokio::test]
async fn serves_static_files_and_favicon() {
    let h = Harness::new();
    let css = h.send(get("/static/style.css")).await;
    assert_eq!(css.status, StatusCode::OK);
    assert!(css.header("content-type").starts_with("text/css"));
    assert_eq!(
        h.send(get("/static/nope.css")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(h.send(get("/static/")).await.status, StatusCode::NOT_FOUND);
    let fav = h.send(get("/favicon.ico")).await;
    assert_eq!(fav.status, StatusCode::NO_CONTENT);
    assert!(fav.body.is_empty());
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
