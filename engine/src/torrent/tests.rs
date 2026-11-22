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
                error: None,
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
async fn too_little_free_space_on_the_drive_makes_it_wait_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 18 * GB)]));
    let m = manager(fake.clone(), dir.path(), 50);
    let free = Arc::new(Mutex::new(15 * GB));
    let f = Arc::clone(&free);
    m.set_free_space(Arc::new(move |_: &Path| Some(*f.lock().unwrap())));
    m.start(&magnet(HASH)).await.unwrap();
    let st = m.status(HASH).unwrap();
    assert_eq!(st.state, TorrentState::WaitingForDisk);
    assert_eq!(st.drive_short, Some((15 * GB, 18 * GB + FREE_SPACE_MARGIN)));
    assert!(fake.added.lock().unwrap().is_empty());
    let err = m.playable(HASH).unwrap_err();
    assert_eq!(err.code(), Some("torrent_disk_full"));

    // the rest of the file plus the margin is free now
    *free.lock().unwrap() = 18 * GB + FREE_SPACE_MARGIN;
    m.start(&magnet(HASH)).await.unwrap();
    let st = m.status(HASH).unwrap();
    assert_eq!(st.state, TorrentState::Downloading);
    assert_eq!(st.drive_short, None);
}

#[tokio::test]
async fn a_drive_filling_up_while_downloading_fails_the_download() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 18 * GB)]));
    let m = manager(fake.clone(), dir.path(), 50);
    let free = Arc::new(Mutex::new(30 * GB));
    let f = Arc::clone(&free);
    m.set_free_space(Arc::new(move |_: &Path| Some(*f.lock().unwrap())));
    m.start(&magnet(HASH)).await.unwrap();
    fake.set_progress(HASH, 5 * GB, 18 * GB);
    m.sample().await;
    assert_eq!(m.status(HASH).unwrap().state, TorrentState::Downloading);

    *free.lock().unwrap() = 100 * MB;
    fake.set_progress(HASH, 6 * GB, 18 * GB);
    m.sample().await;
    let TorrentState::Failed(e) = m.status(HASH).unwrap().state else {
        panic!("not failed");
    };
    assert_eq!(e.code(), Some("torrent_disk_full"));
    assert_eq!(*fake.stopped.lock().unwrap(), vec![HASH.to_string()]);
    assert_eq!(
        m.playable(HASH).unwrap_err().code(),
        Some("torrent_disk_full")
    );
}

#[tokio::test]
async fn a_library_error_while_downloading_fails_the_download() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 100)]));
    let m = manager(fake.clone(), dir.path(), 50);
    m.start(&magnet(HASH)).await.unwrap();
    fake.set_progress(HASH, 40, 100);
    fake.progress.lock().unwrap().get_mut(HASH).unwrap().error = Some("write failed".into());
    m.sample().await;
    let TorrentState::Failed(e) = m.status(HASH).unwrap().state else {
        panic!("not failed");
    };
    assert!(e.to_stored().contains("torrent_failed"));
}

#[tokio::test]
async fn a_chosen_download_folder_gets_its_own_subfolder() {
    use super::hub::{ManagerFactory, TorrentHub};

    let data = tempfile::tempdir().unwrap();
    let chosen = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 100)]));
    let roots = Arc::new(Mutex::new(Vec::<std::path::PathBuf>::new()));
    let (made, seen) = (Arc::clone(&fake), Arc::clone(&roots));
    let factory: ManagerFactory = Arc::new(move |root, kbps, gb| {
        seen.lock().unwrap().push(root.clone());
        let fake = Arc::clone(&made);
        Box::pin(async move { Ok(TorrentManager::new(fake, root, kbps, gb)) })
    });
    let hub = TorrentHub::with_factory(data.path().join("torrents"), factory);
    let on = settings::TorrentSettings {
        torrents_enabled: true,
        ..Default::default()
    };
    hub.set_settings(&on);
    assert_eq!(hub.root(), data.path().join("torrents"));
    let first = hub.manager().await.unwrap();

    // a stray file of the owner's in the chosen folder is never touched
    let theirs = chosen.path().join("mine.txt");
    std::fs::write(&theirs, b"x").unwrap();
    let dir_text = chosen.path().to_string_lossy().into_owned();
    settings::check_dir(&dir_text).unwrap();
    let ours = chosen.path().join(settings::DIR_NAME);
    assert!(ours.is_dir());
    let moved = settings::TorrentSettings {
        torrent_dir: dir_text,
        ..on.clone()
    };
    hub.apply(&moved).await;
    assert!(
        hub.manager_if_running().is_none(),
        "the old manager is stopped"
    );
    let second = hub.manager().await.unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(second.root(), ours.as_path());
    assert_eq!(
        *roots.lock().unwrap(),
        vec![data.path().join("torrents"), ours.clone()]
    );
    second.start(&magnet(HASH)).await.unwrap();
    assert!(ours.join(HASH).join("part").is_file());
    second.cleanup_leftovers(&[]).await;
    assert!(!ours.join(HASH).exists());
    assert!(theirs.is_file());

    // checks of the folder
    assert_eq!(
        settings::check_dir("relative/dir").unwrap_err().code(),
        Some("torrent_dir_not_absolute")
    );
    let gone = chosen.path().join("gone");
    assert_eq!(
        settings::check_dir(&gone.to_string_lossy())
            .unwrap_err()
            .code(),
        Some("torrent_dir_missing")
    );
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
        vec![(HASH.to_string(), false)],
        "the files are deleted here, inside the root"
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

/// Lets spawned tasks run.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn unknown_duration_plays_only_when_complete_if_probe_fails() {
    use super::gate::{HubGate, ProbeFn, TorrentGate};
    use super::hub::{ManagerFactory, TorrentHub};
    use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};

    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 100 * MB)]));
    let made = Arc::clone(&fake);
    let factory: ManagerFactory = Arc::new(move |root, kbps, gb| {
        let fake = Arc::clone(&made);
        Box::pin(async move { Ok(TorrentManager::new(fake, root, kbps, gb)) })
    });
    let hub = TorrentHub::with_factory(dir.path().join("torrents"), factory);
    hub.set_settings(&settings::TorrentSettings {
        torrents_enabled: true,
        ..Default::default()
    });
    let probes = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&probes);
    let probe: ProbeFn = Arc::new(move |url| {
        seen.lock().unwrap().push(url);
        Box::pin(async { None })
    });
    let gate = HubGate::new(Arc::clone(&hub), probe);
    let conn = open_database(Path::new(":memory:")).unwrap();
    let item = QueueRepo::new(&conn)
        .add(&NewItem {
            channel_id: MAIN_CHANNEL_ID,
            source_url: magnet(HASH).to_link(),
            title: "f".into(),
            duration_s: None,
            is_live: false,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            requester_key_hash: None,
        })
        .unwrap();

    assert!(gate.ensure_started(&item));
    settle().await;
    let m = hub.manager_if_running().unwrap();
    assert_eq!(gate.readiness(&item, None), Readiness::Unknown);
    fake.set_progress(HASH, 10 * MB, 100 * MB);
    m.sample().await;
    assert_eq!(gate.readiness(&item, None), Readiness::Unknown, "probing");
    settle().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    fake.set_progress(HASH, 20 * MB, 100 * MB);
    m.sample().await;
    // ffprobe could not tell: only the complete file plays (80 MB left at 1 MB/s).
    assert_eq!(gate.readiness(&item, None), Readiness::WaitS(80));
    assert_eq!(gate.readiness(&item, None), Readiness::WaitS(80));
    assert_eq!(probes.lock().unwrap().len(), 1, "probed once");
    assert!(probes.lock().unwrap()[0].ends_with(HASH));
    fake.set_progress(HASH, 100 * MB, 100 * MB);
    m.sample().await;
    assert_eq!(gate.readiness(&item, None), Readiness::Ready);

    // Released: the download goes with its files.
    gate.release(&item);
    settle().await;
    assert!(m.status(HASH).is_none());
    let files = dir.path().join("torrents").join(HASH);
    for _ in 0..200 {
        if !files.exists() {
            break;
        }
        // Deleting runs on a blocking thread.
        std::thread::sleep(std::time::Duration::from_millis(5));
        settle().await;
    }
    assert!(!files.exists());
}

/// A probe that fails (ffprobe needed parts of the file that had not arrived) is tried again:
/// once it tells the length, the film starts as soon as the estimate allows, not when complete.
#[tokio::test(start_paused = true)]
async fn a_failed_length_probe_is_retried_and_the_film_starts_before_its_file_is_complete() {
    use super::gate::{HubGate, PROBE_RETRY, ProbeFn, TorrentGate};
    use super::hub::{ManagerFactory, TorrentHub};
    use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};

    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 100 * MB)]));
    let made = Arc::clone(&fake);
    let factory: ManagerFactory = Arc::new(move |root, kbps, gb| {
        let fake = Arc::clone(&made);
        Box::pin(async move { Ok(TorrentManager::new(fake, root, kbps, gb)) })
    });
    let hub = TorrentHub::with_factory(dir.path().join("torrents"), factory);
    hub.set_settings(&settings::TorrentSettings {
        torrents_enabled: true,
        ..Default::default()
    });
    let calls = Arc::new(Mutex::new(0_u32));
    let seen = Arc::clone(&calls);
    let probe: ProbeFn = Arc::new(move |_| {
        let mut n = seen.lock().unwrap();
        *n += 1;
        let first = *n == 1;
        Box::pin(async move { if first { None } else { Some(2645.0) } })
    });
    let gate = HubGate::new(Arc::clone(&hub), probe);
    let conn = open_database(Path::new(":memory:")).unwrap();
    let item = QueueRepo::new(&conn)
        .add(&NewItem {
            channel_id: MAIN_CHANNEL_ID,
            source_url: magnet(HASH).to_link(),
            title: "f".into(),
            duration_s: None,
            is_live: false,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            requester_key_hash: None,
        })
        .unwrap();

    assert!(gate.ensure_started(&item));
    settle().await;
    let m = hub.manager_if_running().unwrap();
    fake.set_progress(HASH, 10 * MB, 100 * MB);
    m.sample().await;
    assert_eq!(
        gate.readiness(&item, None),
        Readiness::Unknown,
        "first probe"
    );
    settle().await;
    // 30 s of samples at 1 MB/s: the first probe failed, so the wait is until complete.
    for s in 1..=30 {
        tokio::time::advance(Duration::from_secs(1)).await;
        fake.set_progress(HASH, (10 + s) * MB, 100 * MB);
        m.sample().await;
    }
    assert!(PROBE_RETRY <= Duration::from_secs(30));
    // The retry is due: it runs and tells the length.
    assert!(matches!(gate.readiness(&item, None), Readiness::WaitS(_)));
    settle().await;
    assert_eq!(*calls.lock().unwrap(), 2, "probed again");
    assert_eq!(
        gate.readiness(&item, None),
        Readiness::Ready,
        "60 MB left at 1 MB/s is well within 2645 s"
    );
}

#[tokio::test]
async fn a_torrent_with_an_unsafe_file_path_is_refused_before_it_is_added() {
    let dir = tempfile::tempdir().unwrap();
    // Paths as the backend gives them: the torrent's components joined by `/` (`["C:",
    // "x.mkv"]` is `C:/x.mkv`).
    for bad in [
        "C:/x.mkv",
        "C:evil.mkv",
        "..",
        "../x.mkv",
        "a/../../x.mkv",
        "NUL.txt",
        "dir/com1.mkv",
        "lpt9",
        "film.mkv.",
        "film.mkv ",
        "a//b.mkv",
        "",
        "a:b/c.mkv",
        "/abs.mkv",
        r"x\..\..\y.mkv",
        r"C:\Users\x\thesis.docx",
    ] {
        let fake = Arc::new(Fake::default().with_files(HASH, &[("film.mkv", GB), (bad, 10)]));
        let m = manager(fake.clone(), dir.path(), 50);
        let err = m.start(&magnet(HASH)).await.unwrap_err();
        assert_eq!(err.code(), Some("torrent_bad_files"), "{bad:?}");
        assert!(
            fake.added.lock().unwrap().is_empty(),
            "{bad:?}: never added"
        );
        assert!(matches!(
            m.status(HASH).unwrap().state,
            TorrentState::Failed(_)
        ));
    }
    // Plain names (with folders, dots inside, a name that only starts like a device) are fine.
    let fake = Arc::new(Fake::default().with_files(
        HASH,
        &[
            ("Film (2001)/Film.2001.mkv", GB),
            ("Film (2001)/console.txt", 1),
            ("COM10.nfo", 1),
        ],
    ));
    let m = manager(fake.clone(), dir.path(), 50);
    m.start(&magnet(HASH)).await.unwrap();
    assert_eq!(fake.added.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn removing_never_deletes_outside_the_root() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("torrents");
    let outside = base.path().join("outside.txt");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&outside, b"keep").unwrap();
    remove_inside(&root, &outside).await;
    remove_inside(&root, &root.join("..").join("outside.txt")).await;
    remove_inside(&root, &root).await;
    assert!(outside.exists());
    assert!(root.exists());
    let inside = root.join(HASH);
    std::fs::create_dir_all(inside.join("sub")).unwrap();
    remove_inside(&root, &inside).await;
    assert!(!inside.exists());
}

#[tokio::test(start_paused = true)]
async fn a_first_burst_gives_no_estimate_until_20_s_of_samples() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 1000 * MB)]));
    let m = manager(fake.clone(), dir.path(), 50);
    m.start(&magnet(HASH)).await.unwrap();
    fake.set_progress(HASH, 0, 1000 * MB);
    m.sample().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    // 40 MB in one second would say "ready" for a two-hour film.
    fake.set_progress(HASH, 40 * MB, 1000 * MB);
    m.sample().await;
    assert_eq!(m.will_make_it(HASH, Some(7200.0)), Readiness::Unknown);
    assert!(m.playable(HASH).is_err(), "not cleared");
    for s in 2..=20 {
        tokio::time::advance(Duration::from_secs(1)).await;
        fake.set_progress(HASH, 40 * MB + s * MB, 1000 * MB);
        m.sample().await;
    }
    assert_eq!(m.will_make_it(HASH, Some(7200.0)), Readiness::Ready);
    assert!(m.playable(HASH).is_ok(), "cleared once ready");
}

#[test]
fn only_public_http_and_udp_trackers_are_kept() {
    let m = parse_magnet(&format!(
        "magnet:?xt=urn:btih:{HASH}&tr=udp://tracker.example.org:80&tr=http://127.0.0.1:8080/a\
         &tr=http://192.168.1.1/a&tr=udp://[::1]:80&tr=http://169.254.169.254/latest\
         &tr=http://localhost/a&tr=ws://t.example.org/a&tr=file:///etc&tr=https://t.example.org/a\
         &tr=http://8.8.8.8/a"
    ))
    .unwrap();
    assert_eq!(
        m.trackers,
        vec![
            "udp://tracker.example.org:80",
            "https://t.example.org/a",
            "http://8.8.8.8/a"
        ]
    );
    // `udp:` hosts are opaque text to the URL parser: an IP address there is checked too.
    for dropped in [
        "udp://127.0.0.1:80/announce",
        "udp://[::1]:80",
        "udp://192.168.1.1:6969",
        "udp://169.254.1.1:1",
        "udp://2130706433:80",
        "udp://0x7f.1:80",
        "udp://127.1:80",
        "http://2130706433/announce",
    ] {
        assert!(!is_safe_tracker(dropped), "{dropped}");
    }
    assert!(is_safe_tracker("udp://tracker.example:1337/announce"));
    assert!(is_safe_tracker("udp://8.8.8.8:1337/announce"));
    let many: String = (0..80)
        .map(|i| format!("&tr=udp://t{i}.example.org:80"))
        .collect();
    let m = parse_magnet(&format!("magnet:?xt=urn:btih:{HASH}{many}")).unwrap();
    assert_eq!(m.trackers.len(), MAX_TRACKERS);
    let link = magnet_from_stream(
        HASH,
        None,
        "x",
        &[
            "tracker:http://10.0.0.1/a".into(),
            "tracker:udp://t.example.org:1".into(),
        ],
    );
    assert_eq!(
        parse_magnet(&link).unwrap().trackers,
        vec!["udp://t.example.org:1"]
    );
}

#[tokio::test]
async fn a_closed_manager_starts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default().with_files(HASH, &[("f.mkv", 100)]));
    let m = manager(fake.clone(), dir.path(), 50);
    m.close();
    let err = m.start(&magnet(HASH)).await.unwrap_err();
    assert_eq!(err.code(), Some("torrents_disabled"));
    assert!(fake.added.lock().unwrap().is_empty());
}

/// Switched off while `add` runs: the torrent is taken away again.
#[tokio::test]
async fn switching_off_during_a_start_removes_the_added_torrent() {
    struct Closing {
        inner: Fake,
        manager: Mutex<Option<Weak<TorrentManager>>>,
    }
    #[async_trait]
    impl TorrentBackend for Closing {
        async fn resolve(&self, m: &MagnetRef) -> Result<TorrentFiles, Msg> {
            self.inner.resolve(m).await
        }
        async fn add(
            &self,
            m: &MagnetRef,
            idx: usize,
            dir: &Path,
            bps: Option<u64>,
        ) -> Result<(), Msg> {
            let r = self.inner.add(m, idx, dir, bps).await;
            let weak = self.manager.lock().unwrap().clone();
            if let Some(mgr) = weak.and_then(|w| w.upgrade()) {
                mgr.close();
            }
            r
        }
        async fn progress(&self, hash: &str) -> Option<RawProgress> {
            self.inner.progress(hash).await
        }
        async fn open_file(&self, hash: &str, idx: usize) -> Result<Box<dyn FileStream>, Msg> {
            self.inner.open_file(hash, idx).await
        }
        async fn stop(&self, hash: &str) {
            self.inner.stop(hash).await
        }
        async fn remove(&self, hash: &str, delete_files: bool) {
            self.inner.remove(hash, delete_files).await
        }
        fn set_upload_limit(&self, bps: Option<u64>) {
            self.inner.set_upload_limit(bps)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(Closing {
        inner: Fake::default().with_files(HASH, &[("f.mkv", 100)]),
        manager: Mutex::new(None),
    });
    let m = TorrentManager::new(backend.clone(), dir.path().to_path_buf(), 200, 50);
    *backend.manager.lock().unwrap() = Some(Arc::downgrade(&m));
    let err = m.start(&magnet(HASH)).await.unwrap_err();
    assert_eq!(err.code(), Some("torrents_disabled"));
    assert_eq!(
        *backend.inner.removed.lock().unwrap(),
        vec![(HASH.to_string(), false)]
    );
    assert!(!dir.path().join(HASH).exists());
    assert!(m.status(HASH).is_none());
}

/// A start that fails is tried once more; after the second failure the gate says `Failed`
/// (never `Ready`), with the reason.
#[tokio::test]
async fn a_failed_start_is_retried_once_then_the_gate_reports_failed() {
    use super::gate::{HubGate, ProbeFn, TorrentGate};
    use super::hub::{ManagerFactory, TorrentHub};
    use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};

    let dir = tempfile::tempdir().unwrap();
    // No file list for it: every start fails with `torrent_metadata`.
    let fake = Arc::new(Fake::default());
    let made = Arc::clone(&fake);
    let factory: ManagerFactory = Arc::new(move |root, kbps, gb| {
        let fake = Arc::clone(&made);
        Box::pin(async move { Ok(TorrentManager::new(fake, root, kbps, gb)) })
    });
    let hub = TorrentHub::with_factory(dir.path().join("torrents"), factory);
    hub.set_settings(&settings::TorrentSettings {
        torrents_enabled: true,
        ..Default::default()
    });
    let probe: ProbeFn = Arc::new(|_| Box::pin(async { None }));
    let gate = HubGate::new(Arc::clone(&hub), probe);
    let conn = open_database(Path::new(":memory:")).unwrap();
    let item = QueueRepo::new(&conn)
        .add(&NewItem {
            channel_id: MAIN_CHANNEL_ID,
            source_url: magnet(HASH).to_link(),
            title: "f".into(),
            duration_s: Some(100.0),
            is_live: false,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            requester_key_hash: None,
        })
        .unwrap();

    // Never started: wait, not ready.
    assert_eq!(gate.readiness(&item, Some(100.0)), Readiness::Unknown);
    assert!(gate.ensure_started(&item));
    settle().await;
    // The first failure: still waiting (the next check retries).
    assert_eq!(gate.readiness(&item, Some(100.0)), Readiness::Unknown);
    assert!(gate.failure(&item).is_none());
    assert!(gate.ensure_started(&item));
    settle().await;
    assert_eq!(gate.readiness(&item, Some(100.0)), Readiness::Failed);
    assert_eq!(
        gate.failure(&item).unwrap().code(),
        Some("torrent_metadata")
    );
    // No third try, and playback does not start it either.
    assert!(gate.ensure_started(&item));
    settle().await;
    assert_eq!(gate.readiness(&item, Some(100.0)), Readiness::Failed);
    let err = hub.playable(&item.source_url).unwrap_err();
    assert_eq!(err.code(), Some("torrent_metadata"));
    assert!(fake.added.lock().unwrap().is_empty());
}

/// Live experiment (network): a stream opened while the torrent downloads keeps reading to the
/// end after the download completes and the torrent is stopped.
/// `cargo test --lib live_stream_survives_completion -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_stream_survives_completion() {
    use tokio::io::AsyncReadExt;
    let dir = tempfile::tempdir().unwrap();
    let m = TorrentManager::create(dir.path().join("t"), 0, 50)
        .await
        .unwrap();
    // Big Buck Bunny (CC BY), the WebTorrent test torrent.
    let link = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big+Buck+Bunny&tr=udp%3A%2F%2Fexplodie.org%3A6969&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=wss%3A%2F%2Ftracker.webtorrent.dev";
    let mr = parse_magnet(link).unwrap();
    m.start(&mr).await.unwrap();
    let hash = mr.info_hash.clone();
    let mut s = m.open(&hash).await.unwrap();
    let total = m.status(&hash).unwrap().total;
    eprintln!("file total {total}");
    let mut read: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];
    let mut saw_complete_at = None;
    let start = std::time::Instant::now();
    loop {
        let n = match tokio::time::timeout(std::time::Duration::from_secs(120), s.read(&mut buf))
            .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => panic!("read error at {read}: {e}"),
            Err(_) => panic!("read timed out at {read}"),
        };
        if n == 0 {
            break;
        }
        read += n as u64;
        let st = m.status(&hash).unwrap();
        if saw_complete_at.is_none() && st.state == TorrentState::Complete {
            saw_complete_at = Some(read);
            eprintln!(
                "complete (stopped) while the stream was at {read} of {total}, {:?}",
                start.elapsed()
            );
        }
        // Read slower than the download so completion happens mid-stream.
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    eprintln!(
        "stream ended at {read} of {total}, complete seen at {saw_complete_at:?}, {:?}",
        start.elapsed()
    );
    assert!(
        saw_complete_at.is_some_and(|at| at < total),
        "completion must happen mid-stream for the experiment"
    );
    assert_eq!(read, total, "the stream ended early");
}
