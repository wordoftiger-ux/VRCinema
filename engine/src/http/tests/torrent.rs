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
    // Resolving starts it and gives the loopback address.
    let resolver = TorrentResolver::new(Arc::clone(&hub));
    let stream = resolver.resolve(MAGNET).await.unwrap();
    assert!(
        stream.inputs[0]
            .url
            .ends_with(&format!("/internal/torrent/{}/{HASH}", hub.token()))
    );
    hub.manager_if_running().unwrap().sample().await;
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
