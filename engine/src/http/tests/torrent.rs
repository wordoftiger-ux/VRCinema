//! Magnet links, the loopback stream route and the lazily created torrent manager, with a fake
//! torrent backend (nothing touches the network).

use std::path::Path as FsPath;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::resolver::MediaResolver;
use crate::resolver::torrent::TorrentResolver;
use crate::torrent::hub::{ManagerFactory, TorrentHub};
use crate::torrent::settings::TorrentSettings;
use crate::torrent::{
    FileStream, MagnetRef, RawProgress, TorrentBackend, TorrentFiles, TorrentManager,
};

const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Film";

/// One torrent with one 100-byte file whose bytes are 0, 1, 2, ...
struct FakeBackend;

#[async_trait]
impl TorrentBackend for FakeBackend {
    async fn resolve(&self, _m: &MagnetRef) -> Result<TorrentFiles, Msg> {
        Ok(TorrentFiles {
            files: vec![("film.mp4".into(), 100)],
        })
    }
    async fn add(
        &self,
        _m: &MagnetRef,
        _idx: usize,
        dir: &FsPath,
        _bps: Option<u64>,
    ) -> Result<(), Msg> {
        std::fs::create_dir_all(dir).unwrap();
        Ok(())
    }
    async fn progress(&self, _hash: &str) -> Option<RawProgress> {
        Some(RawProgress {
            file_downloaded: 100,
            file_total: 100,
            peers: 1,
            complete: true,
            error: None,
        })
    }
    async fn open_file(&self, _hash: &str, _idx: usize) -> Result<Box<dyn FileStream>, Msg> {
        Ok(Box::new(std::io::Cursor::new(
            (0..100u8).collect::<Vec<_>>(),
        )))
    }
    async fn stop(&self, _hash: &str) {}
    async fn remove(&self, _hash: &str, _delete_files: bool) {}
    fn set_upload_limit(&self, _bps: Option<u64>) {}
}

fn hub(made: Arc<AtomicUsize>) -> Arc<TorrentHub> {
    let root = tempfile::tempdir().unwrap().keep();
    let factory: ManagerFactory = Arc::new(move |root, kbps, gb| {
        made.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(TorrentManager::new(Arc::new(FakeBackend), root, kbps, gb)) })
    });
    TorrentHub::with_factory(root, factory)
}

fn enabled() -> TorrentSettings {
    TorrentSettings {
        torrents_enabled: true,
        ..Default::default()
    }
}

fn harness(hub: &Arc<TorrentHub>) -> Harness {
    let hub = Arc::clone(hub);
    Harness::with_state(config(), move |mut s| {
        s.resolver = Arc::new(TorrentResolver::new(Arc::clone(&hub)));
        s.with_torrents(hub)
    })
}

async fn add_magnet(h: &Harness, cookie: &str) -> Resp {
    h.send(
        call("POST", "/api/admin/channels/main/items")
            .cookie(cookie)
            .json(json!({ "url": MAGNET })),
    )
    .await
}

fn stream_url(hub: &TorrentHub) -> String {
    format!("/internal/torrent/{}/{HASH}", hub.token())
}

async fn put_enabled(h: &Harness, cookie: &str, on: bool) {
    let r = h
        .send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(cookie)
                .json(json!({ "torrentsEnabled": on })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
}

#[tokio::test]
async fn no_manager_created_while_disabled() {
    let made = Arc::new(AtomicUsize::new(0));
    let hub = hub(Arc::clone(&made));
    let h = harness(&hub);
    let cookie = h.login().await;
    assert!(hub.manager().await.is_err());
    assert_eq!(
        add_magnet(&h, &cookie).await.status,
        StatusCode::BAD_REQUEST
    );
    assert!(hub.manager_if_running().is_none());
    assert_eq!(made.load(Ordering::SeqCst), 0);

    // Created at the first need once on; turning it off drops it again.
    put_enabled(&h, &cookie, true).await;
    assert_eq!(made.load(Ordering::SeqCst), 0);
    hub.manager().await.unwrap();
    assert_eq!(made.load(Ordering::SeqCst), 1);
    assert!(hub.manager_if_running().is_some());
    put_enabled(&h, &cookie, false).await;
    assert!(hub.manager_if_running().is_none());
}

#[tokio::test]
async fn settings_take_a_checked_torrent_download_folder() {
    let hub = hub(Arc::default());
    // The app with a window: a window session and a remote one.
    let (desktop_key, first) = DesktopKey::generate();
    let mut cfg = config();
    cfg.session_secret = SECRET.to_string();
    let torrents = Arc::clone(&hub);
    let h = Harness::with_state(cfg, move |mut s| {
        s.resolver = Arc::new(TorrentResolver::new(Arc::clone(&torrents)));
        s.with_torrents(torrents)
            .with_admin_auth(remote_on("sekret"))
            .with_desktop_key(Arc::new(desktop_key))
    });
    let res = h.send(get(&format!("/desktop-login?key={first}"))).await;
    assert_eq!(res.status, StatusCode::FOUND);
    let cookie = cookie_of(&res);
    let remote = cookie_of(
        &h.send(call("POST", "/api/admin/login").json(json!({ "password": "sekret" })))
            .await,
    );
    let put_as = |cookie: &str, v: Value| {
        h.send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(cookie)
                .json(json!({ "torrentDir": v })),
        )
    };
    let put = |v: Value| put_as(&cookie, v);

    // A remote panel can neither set the folder nor see it or the drive's free space.
    let chosen = tempfile::tempdir().unwrap();
    let dir = chosen.path().to_string_lossy().into_owned();
    let r = put_as(&remote, json!(dir)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json()["code"], "window_only");
    assert!(!chosen.path().join("VRCinema-torrents").exists());

    // Network shares and device paths are refused.
    for unc in [
        r"\\server\share",
        r"\\?\C:\Films",
        r"\\.\PhysicalDrive0",
        "//server/share",
    ] {
        let r = put(json!(unc)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{unc}");
        assert_eq!(r.json()["code"], "torrent_dir_not_local", "{unc}");
    }

    let r = put(json!("relative/dir")).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["code"], "torrent_dir_not_absolute");
    let gone = chosen.path().join("gone");
    let r = put(json!(gone.to_string_lossy())).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["code"], "torrent_dir_missing");
    let r = put(json!(7)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let r = put(json!(format!("  {dir} "))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let body = r.json();
    assert_eq!(body["torrentDir"], dir.as_str());
    assert!(body["torrentDirDefault"].is_string());
    assert!(chosen.path().join("VRCinema-torrents").is_dir());
    assert_eq!(
        hub.root(),
        chosen.path().join("VRCinema-torrents"),
        "the hub uses the subfolder"
    );
    let got = h
        .send(call("GET", "/api/admin/channels/main/settings").cookie(&cookie))
        .await
        .json();
    assert_eq!(got["torrentDir"], dir.as_str());
    let seen = h
        .send(call("GET", "/api/admin/channels/main/settings").cookie(&remote))
        .await;
    assert_eq!(seen.status, StatusCode::OK);
    let seen = seen.json();
    for key in ["torrentDir", "torrentDirDefault", "torrentDriveFreeBytes"] {
        assert!(seen.get(key).is_none(), "remote JSON has no {key}");
    }
    assert!(!seen.to_string().contains(&*dir.replace('\\', "\\\\")));

    // Failed torrent films: a remote panel sees their reasons without the folder or the drive.
    let reasons = [
        crate::torrent::disk_full_msg(3 << 30, 21 << 30),
        Msg::error("torrent_dir_missing").arg("path", r"E:\Private\Films"),
        Msg::chain(vec![
            Msg::error("torrent_failed"),
            Msg::text(r"write failed: E:\Private\Films\x.mkv"),
        ]),
    ];
    for (n, reason) in reasons.iter().enumerate() {
        let item = h.add(&format!("t{n}"), ItemStatus::Playing);
        h.with_db(|c| {
            QueueRepo::new(c)
                .finish(item.id, ItemStatus::Failed, Some(&reason.to_stored()))
                .unwrap()
        });
    }
    let hr = &h;
    let history_errors = |cookie: &str| {
        let cookie = cookie.to_string();
        async move {
            let panel = hr
                .send(call("GET", "/api/admin/channels/main").cookie(&cookie))
                .await
                .json();
            panel["history"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|i| i["error"].as_str().map(str::to_string))
                .collect::<Vec<_>>()
        }
    };
    let full = history_errors(&cookie).await.join("\n");
    assert!(full.contains(r"E:\Private\Films"), "{full}");
    assert!(full.contains("3.0 GB"), "{full}");
    let general = history_errors(&remote).await;
    assert_eq!(general.len(), 3, "{general:?}");
    let joined = general.join("\n");
    assert!(
        !joined.contains("Private") && !joined.contains("GB"),
        "{joined}"
    );
    assert!(
        joined.contains("Za mało wolnego miejsca na dysku dla tego torrenta."),
        "{joined}"
    );
    assert!(joined.contains("Nie można użyć folderu pobierania torrentów."));

    // A remote change of other settings still answers without the folder.
    let r = h
        .send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(&remote)
                .json(json!({ "torrentUploadKbps": 100 })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.json().get("torrentDir").is_none());
    // Empty: the default folder again.
    let r = put(json!("")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(hub.root(), hub.default_root());
}

#[tokio::test]
async fn magnet_refused_when_disabled() {
    let hub = hub(Arc::default());
    let h = harness(&hub);
    let cookie = h.login().await;
    let r = add_magnet(&h, &cookie).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["code"], "torrents_disabled");
    assert!(h.list(ItemStatus::Queued).is_empty());
}

#[tokio::test]
async fn owner_adds_magnet_when_enabled_and_panel_shows_progress() {
    let hub = hub(Arc::default());
    let h = harness(&hub);
    let cookie = h.login().await;
    put_enabled(&h, &cookie, true).await;
    let r = add_magnet(&h, &cookie).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!(r.json()["title"], "Film");
    // An item that has not started yet is idle.
    let panel = h
        .send(get("/api/admin/channels/main").cookie(&cookie))
        .await
        .json();
    assert_eq!(panel["queue"][0]["torrent"]["state"], "idle");
    assert_eq!(panel["queue"][0]["torrent"]["ready"], false);
    // Playing never starts a download: it is refused until the download may play.
    let resolver = TorrentResolver::new(Arc::clone(&hub));
    let err = resolver.resolve(MAGNET).await.unwrap_err();
    assert_eq!(err.0.code(), Some("torrent_not_active"));
    assert!(
        hub.manager_if_running()
            .is_none_or(|m| m.status(HASH).is_none()),
        "nothing started"
    );
    hub.start(MAGNET).await.unwrap();
    hub.manager_if_running().unwrap().sample().await;
    // Complete now: resolving gives the loopback address.
    let stream = resolver.resolve(MAGNET).await.unwrap();
    assert!(
        stream.inputs[0]
            .url
            .ends_with(&format!("/internal/torrent/{}/{HASH}", hub.token()))
    );
    assert!(crate::resolver::torrent::is_stream_route_url(
        &stream.inputs[0].url
    ));
    let panel = h
        .send(get("/api/admin/channels/main").cookie(&cookie))
        .await
        .json();
    let t = &panel["queue"][0]["torrent"];
    assert_eq!(t["state"], "complete");
    assert_eq!(t["progress"], 1);
    assert_eq!(t["ready"], true);
    assert!(t["etaS"].is_null());
}

#[tokio::test]
async fn viewer_cannot_add_magnet() {
    let hub = hub(Arc::default());
    hub.apply(&enabled()).await;
    let h = harness(&hub);
    let r = h.request_default(MAGNET).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert!(h.list(ItemStatus::Queued).is_empty());
    assert!(h.list(ItemStatus::PendingApproval).is_empty());
    assert!(hub.manager_if_running().is_none());
}

async fn started() -> (Arc<TorrentHub>, Harness) {
    let hub = hub(Arc::default());
    let h = harness(&hub);
    hub.apply(&enabled()).await;
    hub.start(MAGNET).await.unwrap();
    (hub, h)
}

#[tokio::test]
async fn stream_route_serves_range_from_fake_backend() {
    let (hub, h) = started().await;
    let url = stream_url(&hub);
    let r = h.send(get(&url).header("range", "bytes=10-19")).await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.body, (10..20u8).collect::<Vec<_>>());
    assert_eq!(r.header("content-range"), "bytes 10-19/100");
    assert_eq!(r.header("content-length"), "10");
    assert_eq!(r.header("content-type"), "video/mp4");

    let r = h.send(get(&url).header("range", "bytes=95-")).await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.body, (95..100u8).collect::<Vec<_>>());
    assert_eq!(r.header("content-range"), "bytes 95-99/100");

    let r = h.send(get(&url).header("range", "bytes=100-")).await;
    assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE);

    let r = h.send(get(&url)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body.len(), 100);
    assert_eq!(r.header("content-length"), "100");
    assert_eq!(r.header("accept-ranges"), "bytes");

    // A range that is not valid syntax is ignored (RFC 9110): everything, 200.
    for bad in ["bytes=abc-", "bytes=5-3", "bytes=-x", "bytes=1-2x"] {
        let r = h.send(get(&url).header("range", bad)).await;
        assert_eq!(r.status, StatusCode::OK, "{bad}");
        assert_eq!(r.body.len(), 100, "{bad}");
    }
    // HEAD: the headers only.
    let r = h
        .send(call("HEAD", &url).header("range", "bytes=10-19"))
        .await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.header("content-length"), "10");
    assert!(r.body.is_empty());
}

#[tokio::test]
async fn stream_route_caps_open_streams() {
    let (hub, h) = started().await;
    let slots: Vec<_> = (0..crate::torrent::hub::MAX_STREAMS)
        .map(|_| hub.stream_slot().unwrap())
        .collect();
    let url = stream_url(&hub);
    assert_eq!(
        h.send(get(&url)).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    // Answered without a stream: not counted.
    let r = h.send(get(&url).header("range", "bytes=500-")).await;
    assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE);
    drop(slots);
    assert_eq!(h.send(get(&url)).await.status, StatusCode::OK);
    assert!(
        hub.stream_slot().is_some(),
        "a finished body frees its slot"
    );
}

#[tokio::test]
async fn local_url_uses_the_bound_address_unless_unspecified() {
    let hub = hub(Arc::default());
    hub.set_bound("0.0.0.0:8080".parse().unwrap());
    assert!(
        hub.local_url(HASH)
            .starts_with("http://127.0.0.1:8080/internal/torrent/")
    );
    assert!(!hub.is_this_computer("192.168.1.5".parse().unwrap()));
    hub.set_bound("192.168.1.5:8080".parse().unwrap());
    assert!(
        hub.local_url(HASH)
            .starts_with("http://192.168.1.5:8080/internal/torrent/")
    );
    assert!(crate::resolver::torrent::is_stream_route_url(
        &hub.local_url(HASH)
    ));
    assert!(hub.is_this_computer("192.168.1.5".parse().unwrap()));
    assert!(hub.is_this_computer("127.0.0.1".parse().unwrap()));
    assert!(!hub.is_this_computer("192.168.1.6".parse().unwrap()));
}

#[tokio::test]
async fn stream_route_refuses_non_loopback() {
    let (hub, h) = started().await;
    let url = stream_url(&hub);
    assert_eq!(h.send(get(&url)).await.status, StatusCode::OK);
    // Another computer, even with the token.
    assert_eq!(
        h.send(get(&url).ip("203.0.113.9")).await.status,
        StatusCode::NOT_FOUND
    );
    // The tunnel (cloudflared) arrives from loopback with its headers.
    for header in ["cf-connecting-ip", "x-forwarded-for", "cf-ray"] {
        let r = h.send(get(&url).header(header, "203.0.113.9")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{header}");
    }
    // Wrong token, unknown torrent.
    let r = h.send(get(&format!("/internal/torrent/nope/{HASH}"))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let other = format!("/internal/torrent/{}/{}", hub.token(), "f".repeat(40));
    assert_eq!(h.send(get(&other)).await.status, StatusCode::NOT_FOUND);
}
