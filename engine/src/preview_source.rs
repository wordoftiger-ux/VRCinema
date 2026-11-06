//! The film the window's Preview plays itself (spec `2026-11-06-preview-player-design.md`,
//! 4.1): a disk file at once, a film from the internet once yt-dlp has downloaded it into
//! `<DATA_DIR>/temp` (the same folder and the same [`TempDownloads`] as the air's own
//! downloads, so a film is downloaded once and the air's next clip of it reads the file).
//! Live streams, very long films and failed downloads have no source here: the window then
//! shows the engine's segments (the monitor) instead.
//!
//! At most [`MAX_ENTRIES`] films are kept for the Preview; the least recently asked one is
//! let go (its downloaded file goes [`crate::pipeline::temp_downloads::GRACE`] later unless a
//! clip still plays it).
//!
//! Only the owner's films are downloaded for the Preview. A viewer's film plays here only from
//! a file the air itself downloaded (after the air's home-network checks of its link and
//! media); otherwise the Preview shows the engine's segments. So nothing a viewer sends makes
//! the program fetch more than the air does, and every file in [`TempDownloads`] went through
//! the air's checks or belongs to the owner. Downloads are limited: [`MAX_DOWNLOADS`] at once,
//! the byte limit [`PREVIEW_MAX_DOWNLOAD_BYTES`] on the files themselves (the whole yt-dlp
//! process tree is stopped beyond it), and free disk space for every download running plus the
//! new one before it starts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde::Serialize;

use crate::db::{AddedBy, QueueItem};
use crate::i18n::Msg;
use crate::pipeline::temp_downloads::{Lease, TempDownloads};
use crate::resolver::MediaResolver;
use crate::resolver::ytdlp::PREVIEW_MAX_DOWNLOAD_BYTES;

/// Longer films are not downloaded for the Preview (time and disk space).
pub const MAX_PREVIEW_DURATION_S: f64 = 3.0 * 3600.0;
/// Films kept for the Preview at once.
pub const MAX_ENTRIES: usize = 3;
/// Downloads for the Preview at once.
pub const MAX_DOWNLOADS: usize = 2;
/// Free space a download needs beyond the byte limits of all running downloads and its own.
pub const FREE_SPACE_MARGIN: u64 = 1024 * 1024 * 1024;

/// Free bytes on the disk of a folder (`None`: not known).
pub type FreeSpaceFn = Box<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// What the Preview can play for a film.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum PreviewStatus {
    /// The file is there: `/api/desktop/preview/<id>/media`.
    Ready,
    /// yt-dlp is downloading it.
    Preparing,
    /// No file for the Preview: `live`, `too_long`, `failed`, `unsupported`, `viewer` (a
    /// viewer's film the air did not download), `busy` (enough downloads run) or `no_space`.
    Unavailable { reason: &'static str },
}

enum Entry {
    Preparing,
    Ready {
        file: PathBuf,
        /// A downloaded film is held while the Preview may play it.
        _lease: Option<Lease>,
    },
    Failed(&'static str),
}

struct Slot {
    entry: Entry,
    touched: Instant,
}

struct Inner {
    resolver: Arc<dyn MediaResolver>,
    downloads: TempDownloads,
    free_space: FreeSpaceFn,
    slots: Mutex<HashMap<i64, Slot>>,
    log: Option<Arc<dyn Fn(Msg) + Send + Sync>>,
}

/// The Preview's films. Cheap to clone.
#[derive(Clone)]
pub struct PreviewSources(Arc<Inner>);

/// A film of the owner's disk (`local:` library file or `file:` path).
pub fn is_disk_source(source: &str) -> bool {
    source.starts_with("local:")
        || source
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case("file:"))
}

impl PreviewSources {
    pub fn new(
        resolver: Arc<dyn MediaResolver>,
        downloads: TempDownloads,
        log: Option<Arc<dyn Fn(Msg) + Send + Sync>>,
    ) -> Self {
        Self::with_free_space(resolver, downloads, log, Box::new(free_space))
    }

    /// Tests: the free disk space from `free_space`.
    pub fn with_free_space(
        resolver: Arc<dyn MediaResolver>,
        downloads: TempDownloads,
        log: Option<Arc<dyn Fn(Msg) + Send + Sync>>,
        free_space: FreeSpaceFn,
    ) -> Self {
        Self(Arc::new(Inner {
            resolver,
            downloads,
            free_space,
            slots: Mutex::new(HashMap::new()),
            log,
        }))
    }

    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Slot>> {
        self.0.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the Preview can play for `item`; starts its download when needed.
    pub async fn status(&self, item: &QueueItem) -> PreviewStatus {
        if item.is_live {
            return PreviewStatus::Unavailable { reason: "live" };
        }
        if let Some(known) = self.known(item.id) {
            return known;
        }
        let viewer = item.added_by == AddedBy::Public;
        if is_disk_source(&item.source_url) {
            if viewer {
                // Viewers cannot add disk files; one that says so is not believed.
                return PreviewStatus::Unavailable { reason: "viewer" };
            }
            // The resolvers of disk files only check the file (no probe): quick.
            let entry = match self.0.resolver.resolve(&item.source_url).await {
                Ok(stream) => match stream.inputs.first() {
                    Some(input) => Entry::Ready {
                        file: PathBuf::from(&input.url),
                        _lease: None,
                    },
                    None => Entry::Failed("failed"),
                },
                Err(_) => Entry::Failed("failed"),
            };
            self.insert(item.id, entry);
            return self.known(item.id).unwrap_or(PreviewStatus::Preparing);
        }
        // A film the air downloaded (or an earlier Preview download of the owner's film).
        if let Some(lease) = self.0.downloads.reuse(item.id) {
            let file = lease.path().to_path_buf();
            self.insert(
                item.id,
                Entry::Ready {
                    file,
                    _lease: Some(lease),
                },
            );
            return PreviewStatus::Ready;
        }
        if viewer {
            // Not fetched for the Preview: the engine's segments instead (asked again later,
            // the air may have downloaded it by then).
            return PreviewStatus::Unavailable { reason: "viewer" };
        }
        if item.duration_s.is_some_and(|d| d > MAX_PREVIEW_DURATION_S) {
            return PreviewStatus::Unavailable { reason: "too_long" };
        }
        let preparing = self
            .slots()
            .values()
            .filter(|s| matches!(s.entry, Entry::Preparing))
            .count();
        if preparing >= MAX_DOWNLOADS {
            // Not remembered: asked again later, it may start then.
            return PreviewStatus::Unavailable { reason: "busy" };
        }
        let dir = self.0.downloads.dir();
        let _ = std::fs::create_dir_all(dir);
        // Room for every running download at its limit, this one and a margin.
        let needed = (preparing as u64 + 1) * PREVIEW_MAX_DOWNLOAD_BYTES + FREE_SPACE_MARGIN;
        // Fail closed: free space that cannot be read is no room.
        if (self.0.free_space)(dir).is_none_or(|free| free < needed) {
            return PreviewStatus::Unavailable { reason: "no_space" };
        }
        self.insert(item.id, Entry::Preparing);
        self.download(item.id, item.source_url.clone());
        PreviewStatus::Preparing
    }

    /// The file of a ready film (the media route), while it is on disk.
    pub fn file(&self, item_id: i64) -> Option<PathBuf> {
        let mut slots = self.slots();
        let slot = slots.get_mut(&item_id)?;
        slot.touched = Instant::now();
        match &slot.entry {
            Entry::Ready { file, .. } if file.is_file() => Some(file.clone()),
            _ => None,
        }
    }

    /// The status of a film asked before (and still kept), touched.
    fn known(&self, item_id: i64) -> Option<PreviewStatus> {
        let mut slots = self.slots();
        let slot = slots.get_mut(&item_id)?;
        slot.touched = Instant::now();
        let status = match &slot.entry {
            Entry::Preparing => PreviewStatus::Preparing,
            Entry::Failed(reason) => PreviewStatus::Unavailable { reason },
            Entry::Ready { file, .. } if file.is_file() => PreviewStatus::Ready,
            Entry::Ready { .. } => {
                // Removed from disk meanwhile: asked again from scratch.
                slots.remove(&item_id);
                return None;
            }
        };
        Some(status)
    }

    fn insert(&self, item_id: i64, entry: Entry) {
        let mut slots = self.slots();
        slots.insert(
            item_id,
            Slot {
                entry,
                touched: Instant::now(),
            },
        );
        evict(&mut slots, MAX_ENTRIES);
    }

    /// Downloads the owner's film `source` in the background.
    fn download(&self, item_id: i64, source: String) {
        let this = self.clone();
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            self.finish(item_id, Entry::Failed("failed"));
            return;
        };
        rt.spawn(async move {
            let entry = match this.0.downloads.stem(item_id) {
                Err(_) => Entry::Failed("failed"),
                Ok(stem) => match this.0.resolver.download_preview(&source, &stem).await {
                    None => Entry::Failed("unsupported"),
                    Some(Err(e)) => {
                        if let Some(log) = &this.0.log {
                            log(Msg::new("log.preview_download_failed")
                                .arg("id", item_id)
                                .arg_msg("error", e.0));
                        }
                        Entry::Failed("failed")
                    }
                    Some(Ok(file)) => {
                        let lease = this.0.downloads.adopt(item_id, file);
                        Entry::Ready {
                            file: lease.path().to_path_buf(),
                            _lease: Some(lease),
                        }
                    }
                },
            };
            this.finish(item_id, entry);
        });
    }

    /// A download ended: kept only while its film is still asked for (else the lease goes).
    fn finish(&self, item_id: i64, entry: Entry) {
        let mut slots = self.slots();
        if let Some(slot) = slots.get_mut(&item_id)
            && matches!(slot.entry, Entry::Preparing)
        {
            slot.entry = entry;
        }
    }
}

/// Free bytes on the disk holding `dir` (Windows; elsewhere not known).
#[cfg(windows)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path and a valid out pointer; the others may be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

/// Free bytes on the disk holding `dir`, from `df -Pk` (POSIX output: the fourth field of the
/// second line, in KiB).
#[cfg(not(windows))]
pub fn free_space(dir: &Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let kib: u64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(kib * 1024)
}

/// Drops the least recently asked films beyond `max` (never one being downloaded).
fn evict(slots: &mut HashMap<i64, Slot>, max: usize) {
    while slots.len() > max {
        let oldest = slots
            .iter()
            .filter(|(_, s)| !matches!(s.entry, Entry::Preparing))
            .min_by_key(|(_, s)| s.touched)
            .map(|(id, _)| *id);
        match oldest {
            Some(id) => {
                slots.remove(&id);
            }
            None => return,
        }
    }
}

/// The media type of a film file, by its extension.
pub fn content_type(file: &Path) -> &'static str {
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "ts" | "m2ts" => "video/mp2t",
        "ogv" => "video/ogg",
        _ => "application/octet-stream",
    }
}

/// A `Range` that does not fit the file (416).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsatisfiable;

/// The byte range a `Range` header asks of a file of `len` bytes: `Ok(None)` for the whole
/// file (no header or one this route ignores), `Ok(Some((first, last)))` inclusive, `Err`
/// when it cannot be satisfied (416). Only the first range of a list is served.
pub fn byte_range(header: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, Unsatisfiable> {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Ok(None);
    };
    let first = spec.split(',').next().unwrap_or("").trim();
    let Some((start, end)) = first.split_once('-') else {
        return Ok(None);
    };
    let (start, end) = (start.trim(), end.trim());
    if len == 0 {
        return Err(Unsatisfiable);
    }
    let range = if start.is_empty() {
        // The last `n` bytes.
        let n: u64 = end.parse().map_err(|_| Unsatisfiable)?;
        if n == 0 {
            return Err(Unsatisfiable);
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = start.parse().map_err(|_| Unsatisfiable)?;
        let e = if end.is_empty() {
            len - 1
        } else {
            end.parse::<u64>().map_err(|_| Unsatisfiable)?.min(len - 1)
        };
        if s >= len || e < s {
            return Err(Unsatisfiable);
        }
        (s, e)
    };
    Ok(Some(range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AddedBy, ItemStatus};
    use crate::resolver::{MediaMeta, ResolveError, ResolvedStream, StreamInput};
    use async_trait::async_trait;
    use std::time::Duration;

    #[test]
    fn ranges() {
        assert_eq!(byte_range(None, 100), Ok(None));
        assert_eq!(byte_range(Some("bytes=0-"), 100), Ok(Some((0, 99))));
        assert_eq!(byte_range(Some("bytes=10-19"), 100), Ok(Some((10, 19))));
        assert_eq!(byte_range(Some("bytes=90-500"), 100), Ok(Some((90, 99))));
        assert_eq!(byte_range(Some("bytes=-10"), 100), Ok(Some((90, 99))));
        assert_eq!(byte_range(Some("bytes=-500"), 100), Ok(Some((0, 99))));
        assert_eq!(byte_range(Some("bytes=5-9, 20-30"), 100), Ok(Some((5, 9))));
        assert_eq!(byte_range(Some("items=0-1"), 100), Ok(None));
        assert_eq!(byte_range(Some("bytes=100-"), 100), Err(Unsatisfiable));
        assert_eq!(byte_range(Some("bytes=20-10"), 100), Err(Unsatisfiable));
        assert_eq!(byte_range(Some("bytes=x-"), 100), Err(Unsatisfiable));
        assert_eq!(byte_range(Some("bytes=0-"), 0), Err(Unsatisfiable));
    }

    #[test]
    fn disk_sources_and_types() {
        assert!(is_disk_source("local:0/a.mp4"));
        assert!(is_disk_source("FILE:///C:/a.mkv"));
        assert!(!is_disk_source("https://youtu.be/x"));
        assert_eq!(content_type(Path::new("a/B.MP4")), "video/mp4");
        assert_eq!(content_type(Path::new("a.webm")), "video/webm");
        assert_eq!(content_type(Path::new("a")), "application/octet-stream");
    }

    /// Resolves `file:` to its path and links to `media`; "downloads" by writing
    /// `<stem>.mp4` after `delay`.
    struct Fake {
        delay: Duration,
        fail: bool,
        media: &'static str,
        downloads: Arc<std::sync::atomic::AtomicUsize>,
    }

    fn fake(delay_ms: u64, fail: bool) -> Fake {
        Fake {
            delay: Duration::from_millis(delay_ms),
            fail,
            media: "https://cdn.example/v.mp4",
            downloads: Arc::default(),
        }
    }

    #[async_trait]
    impl MediaResolver for Fake {
        async fn fetch_meta(&self, _url: &str) -> Result<MediaMeta, ResolveError> {
            unreachable!()
        }
        async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
            let media = match url.strip_prefix("file:") {
                Some(path) => path.to_string(),
                None => self.media.to_string(),
            };
            Ok(ResolvedStream {
                inputs: vec![StreamInput {
                    url: media,
                    headers: vec![],
                }],
                title: String::new(),
                duration_s: None,
                is_live: false,
            })
        }
        async fn download_preview(
            &self,
            _url: &str,
            stem: &Path,
        ) -> Option<Result<PathBuf, ResolveError>> {
            self.downloads
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            if self.fail {
                return Some(Err(ResolveError::new("nope")));
            }
            let file = stem.with_extension("mp4");
            std::fs::write(&file, b"film").unwrap();
            Some(Ok(file))
        }
    }

    fn item(id: i64, source: &str) -> QueueItem {
        QueueItem {
            id,
            channel_id: 1,
            position: 0,
            source_url: source.into(),
            title: "t".into(),
            duration_s: Some(60.0),
            is_live: false,
            status: ItemStatus::Playing,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            requester_key_hash: None,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
            in_s: None,
            out_s: None,
            start_at: None,
        }
    }

    fn with_downloads(downloads: TempDownloads, fake: Fake, free: Option<u64>) -> PreviewSources {
        PreviewSources::with_free_space(Arc::new(fake), downloads, None, Box::new(move |_| free))
    }

    fn sources(dir: &Path, fake: Fake) -> PreviewSources {
        let downloads = TempDownloads::with_grace(dir.join("temp"), Duration::ZERO);
        with_downloads(downloads, fake, Some(u64::MAX))
    }

    async fn until_ready(p: &PreviewSources, it: &QueueItem) -> PreviewStatus {
        for _ in 0..200 {
            let s = p.status(it).await;
            if s != PreviewStatus::Preparing {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        PreviewStatus::Preparing
    }

    #[tokio::test]
    async fn a_disk_file_is_ready_at_once_and_live_or_long_films_have_no_source() {
        let dir = tempfile::tempdir().unwrap();
        let film = dir.path().join("a.mp4");
        std::fs::write(&film, b"x").unwrap();
        let p = sources(dir.path(), fake(0, false));
        let disk = item(1, &format!("file:{}", film.display()));
        assert_eq!(p.status(&disk).await, PreviewStatus::Ready);
        assert_eq!(p.file(1), Some(film));
        let mut live = item(2, "https://x.example/live");
        live.is_live = true;
        assert_eq!(
            p.status(&live).await,
            PreviewStatus::Unavailable { reason: "live" }
        );
        let mut long = item(3, "https://x.example/long");
        long.duration_s = Some(MAX_PREVIEW_DURATION_S + 1.0);
        assert_eq!(
            p.status(&long).await,
            PreviewStatus::Unavailable { reason: "too_long" }
        );
        assert_eq!(p.file(2), None);
    }

    #[tokio::test]
    async fn a_film_from_the_internet_is_prepared_then_ready_and_shared_with_the_air() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = TempDownloads::with_grace(dir.path().join("temp"), Duration::ZERO);
        let p = with_downloads(downloads.clone(), fake(50, false), Some(u64::MAX));
        let web = item(7, "https://x.example/v");
        assert_eq!(p.status(&web).await, PreviewStatus::Preparing);
        assert_eq!(p.file(7), None);
        assert_eq!(until_ready(&p, &web).await, PreviewStatus::Ready);
        let file = p.file(7).expect("downloaded");
        assert!(file.starts_with(dir.path().join("temp")));
        // The air's next clip of the item reads the same file.
        let air = downloads.reuse(7).expect("shared with the clips");
        assert_eq!(air.path(), file);
    }

    #[tokio::test]
    async fn a_failed_download_is_unavailable_and_old_films_are_let_go() {
        let dir = tempfile::tempdir().unwrap();
        let p = sources(dir.path(), fake(0, true));
        let web = item(9, "https://x.example/v");
        p.status(&web).await;
        assert_eq!(
            until_ready(&p, &web).await,
            PreviewStatus::Unavailable { reason: "failed" }
        );
        let films: Vec<PathBuf> = (0..=MAX_ENTRIES)
            .map(|n| {
                let f = dir.path().join(format!("{n}.mp4"));
                std::fs::write(&f, b"x").unwrap();
                f
            })
            .collect();
        for (n, f) in films.iter().enumerate() {
            tokio::time::sleep(Duration::from_millis(2)).await;
            let it = item(100 + n as i64, &format!("file:{}", f.display()));
            assert_eq!(p.status(&it).await, PreviewStatus::Ready);
        }
        assert_eq!(p.slots().len(), MAX_ENTRIES);
        assert!(p.file(100).is_none(), "the least recently asked one went");
        assert!(p.file(100 + MAX_ENTRIES as i64).is_some());
    }

    fn viewers(id: i64, source: &str) -> QueueItem {
        let mut it = item(id, source);
        it.added_by = AddedBy::Public;
        it
    }

    #[tokio::test]
    async fn a_viewers_film_is_never_fetched_for_the_preview_only_the_airs_download_plays() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = TempDownloads::with_grace(dir.path().join("temp"), Duration::ZERO);
        let f = fake(0, false);
        let count = Arc::clone(&f.downloads);
        let p = with_downloads(downloads.clone(), f, Some(u64::MAX));
        for it in [
            viewers(1, "http://home.example/v"),
            viewers(2, "https://x.example/v"),
            viewers(3, "file:C:/secret.mp4"),
        ] {
            assert_eq!(
                p.status(&it).await,
                PreviewStatus::Unavailable { reason: "viewer" }
            );
        }
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "nothing fetched"
        );
        // The air downloaded the viewer's film (after its own checks): the Preview plays it.
        let file = downloads.stem(2).unwrap().with_extension("mp4");
        std::fs::write(&file, b"film").unwrap();
        let _air = downloads.adopt(2, file.clone());
        assert_eq!(
            p.status(&viewers(2, "https://x.example/v")).await,
            PreviewStatus::Ready
        );
        assert_eq!(p.file(2), Some(file));
    }

    #[tokio::test]
    async fn downloads_are_limited_in_number_and_need_room_for_all_of_them() {
        let dir = tempfile::tempdir().unwrap();
        let p = sources(dir.path(), fake(300, false));
        for id in 0..MAX_DOWNLOADS as i64 {
            assert_eq!(
                p.status(&item(id, "https://x.example/v")).await,
                PreviewStatus::Preparing
            );
        }
        assert_eq!(
            p.status(&item(50, "https://x.example/v")).await,
            PreviewStatus::Unavailable { reason: "busy" }
        );
        // One download runs: the next one needs room for both and the margin.
        let room = 2 * PREVIEW_MAX_DOWNLOAD_BYTES + FREE_SPACE_MARGIN;
        for (free, expected) in [
            (room - 1, PreviewStatus::Unavailable { reason: "no_space" }),
            (room, PreviewStatus::Preparing),
        ] {
            let downloads = TempDownloads::with_grace(dir.path().join("t2"), Duration::ZERO);
            let q = with_downloads(downloads, fake(300, false), Some(free));
            assert_eq!(
                q.status(&item(60, "https://x.example/a")).await,
                PreviewStatus::Preparing
            );
            assert_eq!(q.status(&item(61, "https://x.example/b")).await, expected);
        }
        // Free space that cannot be read: no download (fail closed).
        let downloads = TempDownloads::with_grace(dir.path().join("t3"), Duration::ZERO);
        let unknown = with_downloads(downloads, fake(0, false), None);
        assert_eq!(
            unknown.status(&item(70, "https://x.example/c")).await,
            PreviewStatus::Unavailable { reason: "no_space" }
        );
    }
}
