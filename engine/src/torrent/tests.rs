use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::*;

const MB: u64 = 1_000_000;
const GB: u64 = 1024 * 1024 * 1024;
const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

#[derive(Default)]
struct Fake {
    files: Mutex<HashMap<String, TorrentFiles>>,
    progress: Mutex<HashMap<String, RawProgress>>,
    added: Mutex<Vec<(String, usize)>>,
    stopped: Mutex<Vec<String>>,
    removed: Mutex<Vec<(String, bool)>>,
    upload: Mutex<Vec<Option<u64>>>,
}

impl Fake {
    fn with_files(self, hash: &str, files: &[(&str, u64)]) -> Self {
        self.files.lock().unwrap().insert(
            hash.to_string(),
            TorrentFiles {
                files: files.iter().map(|(p, l)| (p.to_string(), *l)).collect(),
            },
        );
        self
    }

    fn set_progress(&self, hash: &str, downloaded: u64, total: u64) {
        self.progress.lock().unwrap().insert(
            hash.to_string(),
            RawProgress {
                file_downloaded: downloaded,
                file_total: total,
                peers: 3,
                complete: downloaded >= total,
            },
        );
    }
}

#[async_trait]
impl TorrentBackend for Fake {
    async fn resolve(&self, m: &MagnetRef) -> Result<TorrentFiles, Msg> {
        self.files
            .lock()
            .unwrap()
            .get(&m.info_hash)
            .cloned()
            .ok_or_else(|| Msg::error("torrent_metadata"))
    }
    async fn add(
        &self,
        m: &MagnetRef,
        file_idx: usize,
        dir: &Path,
        _upload_bps: Option<u64>,
    ) -> Result<(), Msg> {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("part"), b"data").unwrap();
        self.added
            .lock()
            .unwrap()
            .push((m.info_hash.clone(), file_idx));
        Ok(())
    }
    async fn progress(&self, hash: &str) -> Option<RawProgress> {
        self.progress.lock().unwrap().get(hash).cloned()
    }
    async fn open_file(&self, _hash: &str, _idx: usize) -> Result<Box<dyn FileStream>, Msg> {
        Ok(Box::new(std::io::Cursor::new(b"video".to_vec())))
    }
    async fn stop(&self, hash: &str) {
        self.stopped.lock().unwrap().push(hash.to_string());
    }
    async fn remove(&self, hash: &str, delete_files: bool) {
        self.removed
            .lock()
            .unwrap()
            .push((hash.to_string(), delete_files));
    }
    fn set_upload_limit(&self, bps: Option<u64>) {
        self.upload.lock().unwrap().push(bps);
    }
}

fn magnet(hash: &str) -> MagnetRef {
    MagnetRef {
        info_hash: hash.to_string(),
        name: None,
        trackers: vec![],
        file_idx: None,
    }
}

fn manager(fake: Arc<Fake>, root: &Path, cap_gb: u64) -> Arc<TorrentManager> {
    TorrentManager::new(fake, root.to_path_buf(), 200, cap_gb)
}

#[test]
fn parse_magnet_reads_hash_name_trackers_and_so() {
    let m = parse_magnet(
        "magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567&dn=My%20Film&tr=udp%3A%2F%2Ft1%3A80&tr=udp://t2:80&so=3,5",
    )
    .unwrap();
    assert_eq!(m.info_hash, HASH);
    assert_eq!(m.name.as_deref(), Some("My Film"));
    assert_eq!(m.trackers, vec!["udp://t1:80", "udp://t2:80"]);
    assert_eq!(m.file_idx, Some(3));
    // base32 form of the same hash
    let b32 = parse_magnet("magnet:?xt=urn:btih:AERUKZ4JVPG66AJDIVTYTK6N54ASGRLH").unwrap();
    assert_eq!(b32.info_hash, HASH);
    assert!(parse_magnet("https://example.com/a.mp4").is_none());
    assert!(parse_magnet("magnet:?xt=urn:btih:short").is_none());
    assert_eq!(parse_magnet(&m.to_link()).unwrap(), m);
}

#[test]
fn magnet_from_stream_keeps_only_tracker_sources() {
    let link = magnet_from_stream(
        HASH,
        Some(2),
        "A b",
        &[
            "tracker:udp://t1:80/announce".into(),
            format!("dht:{HASH}"),
            "tracker:http://t2/announce".into(),
        ],
    );
    let m = parse_magnet(&link).unwrap();
    assert_eq!(m.info_hash, HASH);
    assert_eq!(m.name.as_deref(), Some("A b"));
    assert_eq!(
        m.trackers,
        vec!["udp://t1:80/announce", "http://t2/announce"]
    );
    assert_eq!(m.file_idx, Some(2));
    assert!(!link.contains("dht"));
}

#[tokio::test]
async fn largest_video_file_is_picked_without_file_idx() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(
        Fake::default().with_files(HASH, &[("a.txt", 9 * GB), ("b.mkv", 2 * GB), ("c.mp4", GB)]),
    );
    let m = manager(fake.clone(), dir.path(), 50);
    m.start(&magnet(HASH)).await.unwrap();
    assert_eq!(*fake.added.lock().unwrap(), vec![(HASH.to_string(), 1)]);
    let st = m.status(HASH).unwrap();
    assert_eq!(st.state, TorrentState::Downloading);
    assert_eq!(st.file_name, "b.mkv");
    assert_eq!(st.total, 2 * GB);

    // a given file index wins
    let other = "1111111111111111111111111111111111111111";
    let fake2 = Arc::new(Fake::default().with_files(other, &[("x.mkv", GB), ("y.mkv", 10)]));
    let m2 = manager(fake2.clone(), dir.path(), 50);
    let mut mr = magnet(other);
    mr.file_idx = Some(1);
    m2.start(&mr).await.unwrap();
    assert_eq!(*fake2.added.lock().unwrap(), vec![(other.to_string(), 1)]);

    // no video file → failed
    let bad = "2222222222222222222222222222222222222222";
    let fake3 = Arc::new(Fake::default().with_files(bad, &[("readme.txt", 10)]));
    let m3 = manager(fake3, dir.path(), 50);
    let err = m3.start(&magnet(bad)).await.unwrap_err();
    assert_eq!(err.code(), Some("torrent_no_video"));
    assert!(matches!(
        m3.status(bad).unwrap().state,
        TorrentState::Failed(_)
    ));
}

#[test]
fn estimate_ready_with_margin() {
    assert_eq!(estimate(600 * MB, MB as f64, Some(700.0)), Readiness::Ready);
    match estimate(600 * MB, MB as f64, Some(650.0)) {
        Readiness::WaitS(s) => assert!(s > 0 && s <= 40, "{s}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(estimate(600 * MB, 0.0, Some(700.0)), Readiness::Unknown);
}

#[tokio::test(start_paused = true)]
async fn estimate_ready_with_margin_through_the_manager() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 1000 * MB)]));
    let m = manager(fake.clone(), dir.path(), 50);
    m.start(&magnet(HASH)).await.unwrap();
    fake.set_progress(HASH, 370 * MB, 1000 * MB);
    m.sample().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    fake.set_progress(HASH, 400 * MB, 1000 * MB);
    m.sample().await;
    let st = m.status(HASH).unwrap();
    assert!((st.rate_bps - MB as f64).abs() < 1.0, "{}", st.rate_bps);
    assert_eq!(st.peers, 3);
    assert_eq!(m.will_make_it(HASH, Some(700.0)), Readiness::Ready);
    assert!(matches!(
        m.will_make_it(HASH, Some(650.0)),
        Readiness::WaitS(s) if s > 0
    ));
}

#[tokio::test(start_paused = true)]
async fn unknown_duration_waits_until_complete() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 1000 * MB)]));
    let m = manager(fake.clone(), dir.path(), 50);
    m.start(&magnet(HASH)).await.unwrap();
    fake.set_progress(HASH, 10 * MB, 1000 * MB);
    m.sample().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    fake.set_progress(HASH, 990 * MB, 1000 * MB);
    m.sample().await;
    assert_eq!(m.will_make_it(HASH, None), Readiness::Unknown);
    fake.set_progress(HASH, 1000 * MB, 1000 * MB);
    m.sample().await;
    assert_eq!(m.will_make_it(HASH, None), Readiness::Ready);
}

#[tokio::test]
async fn complete_torrent_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 100)]));
    let m = manager(fake.clone(), dir.path(), 50);
    m.start(&magnet(HASH)).await.unwrap();
    fake.set_progress(HASH, 50, 100);
    m.sample().await;
    assert!(fake.stopped.lock().unwrap().is_empty());
    fake.set_progress(HASH, 100, 100);
    m.sample().await;
    m.sample().await;
    assert_eq!(*fake.stopped.lock().unwrap(), vec![HASH.to_string()]);
    let st = m.status(HASH).unwrap();
    assert_eq!(st.state, TorrentState::Complete);
    assert_eq!(st.downloaded, 100);
    assert_eq!(st.rate_bps, 0.0);
    assert!(m.open(HASH).await.is_ok());
}

#[tokio::test]
async fn disk_cap_makes_it_wait_not_fail() {
    let dir = tempfile::tempdir().unwrap();
    let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let fake = Arc::new(
        Fake::default()
            .with_files(a, &[("a.mkv", 3 * GB)])
            .with_files(HASH, &[("f.mkv", 2 * GB)]),
    );
    let m = manager(fake.clone(), dir.path(), 4);
    m.start(&magnet(a)).await.unwrap();
    m.start(&magnet(HASH)).await.unwrap();
    let st = m.status(HASH).unwrap();
    assert_eq!(st.state, TorrentState::WaitingForDisk);
    assert_eq!(st.total, 2 * GB);
    assert_eq!(fake.added.lock().unwrap().len(), 1);
    assert_eq!(m.will_make_it(HASH, Some(7200.0)), Readiness::Unknown);
    assert!(m.open(HASH).await.is_err());

    // asking again while still full keeps it waiting
    m.start(&magnet(HASH)).await.unwrap();
    assert_eq!(m.status(HASH).unwrap().state, TorrentState::WaitingForDisk);

    // space frees → it starts when asked again
    m.drop_item(a).await;
    m.start(&magnet(HASH)).await.unwrap();
    assert_eq!(m.status(HASH).unwrap().state, TorrentState::Downloading);
    assert_eq!(fake.added.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn drop_item_removes_files() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 100)]));
    let m = manager(fake.clone(), dir.path(), 50);
    m.start(&magnet(HASH)).await.unwrap();
    assert!(dir.path().join(HASH).exists());
    assert!(m.disk_used() > 0);
    m.drop_item(HASH).await;
    assert!(!dir.path().join(HASH).exists());
    assert!(m.status(HASH).is_none());
    assert_eq!(
        *fake.removed.lock().unwrap(),
        vec![(HASH.to_string(), true)]
    );
    assert_eq!(m.disk_used(), 0);
}

#[tokio::test]
async fn leftovers_not_kept_are_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let keep = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    std::fs::create_dir_all(dir.path().join(keep)).unwrap();
    std::fs::create_dir_all(dir.path().join(HASH)).unwrap();
    std::fs::write(dir.path().join("stray.bin"), b"x").unwrap();
    let m = manager(Arc::new(Fake::default()), dir.path(), 50);
    m.cleanup_leftovers(&[keep.to_string()]).await;
    assert!(dir.path().join(keep).exists());
    assert!(!dir.path().join(HASH).exists());
    assert!(!dir.path().join("stray.bin").exists());
}

#[test]
fn upload_limit_is_kilobytes_times_1024() {
    assert_eq!(upload_bps(200), Some(204_800));
    assert_eq!(upload_bps(0), None);
    let fake = Arc::new(Fake::default());
    let m = manager(fake.clone(), Path::new("unused"), 50);
    m.set_upload_kbps(0);
    m.set_upload_kbps(10);
    assert_eq!(
        *fake.upload.lock().unwrap(),
        vec![Some(204_800), None, Some(10_240)]
    );
}

/// The real session starts and stops in a temp folder without network (no DHT, no trackers,
/// no listener) and writes nothing outside it.
#[tokio::test]
async fn real_session_is_created_and_dropped_offline() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("torrents");
    let backend = backend_rqbit::RqbitBackend::new(&root, upload_bps(200), true)
        .await
        .unwrap();
    backend.set_upload_limit(None);
    assert!(backend.progress(HASH).await.is_none());
    drop(backend);
    assert!(root.is_dir());
}
