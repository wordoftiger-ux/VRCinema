//! Torrent sources: an in-process torrent client (librqbit behind [`TorrentBackend`]) that
//! downloads only the selected file of a torrent into `<data dir>/torrents/<info hash>/`, keeps
//! a 30 s download-rate average, answers "will it make it in time" and stops the torrent once
//! the file is complete (no seeding).
//!
//! Nothing here runs until a [`TorrentManager`] is created, which the app does only when the
//! "Torrent sources" setting is on (no session, no ports while it is off).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncSeek};
use tokio::time::Instant;

use crate::i18n::Msg;

pub mod backend_rqbit;
pub mod gate;
pub mod hub;
pub mod settings;

#[cfg(test)]
mod tests;

/// The window of the download-rate average.
pub const RATE_WINDOW: Duration = Duration::from_secs(30);
/// The rate is trusted only once its samples cover this long (the first burst from peers is
/// no measure of the rate that follows).
pub const MIN_RATE_SPAN: Duration = Duration::from_secs(20);
/// At most this many trackers of a magnet are used.
pub const MAX_TRACKERS: usize = 50;
/// How often the sampler reads the progress of active torrents.
pub const SAMPLE_EVERY: Duration = Duration::from_secs(1);
/// Play when `remaining / rate ≤ duration / MARGIN`.
pub const MARGIN: f64 = 1.15;
/// Extensions of the largest-video-file fallback (no `fileIdx`).
pub const VIDEO_EXTENSIONS: [&str; 9] = [
    "mkv", "mp4", "avi", "mov", "webm", "m4v", "ts", "wmv", "flv",
];
pub const DEFAULT_UPLOAD_KBPS: u64 = 200;
pub const DEFAULT_DISK_CAP_GB: u64 = 50;

/// KB/s of the setting → bytes/s of the backend (`0` = unlimited).
pub fn upload_bps(kbps: u64) -> Option<u64> {
    (kbps > 0).then(|| kbps.saturating_mul(1024))
}

/// GB of the setting → bytes.
pub fn disk_cap_bytes(gb: u64) -> u64 {
    gb.saturating_mul(1024 * 1024 * 1024)
}

// ---------- Magnet links ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MagnetRef {
    /// 40 lowercase hex characters (a base32 hash is converted).
    pub info_hash: String,
    pub name: Option<String>,
    pub trackers: Vec<String>,
    /// From `so` (BEP 53 select-only): the first index listed.
    pub file_idx: Option<usize>,
}

impl MagnetRef {
    /// The magnet link of this reference (`xt`, `dn`, `tr`…, `so`).
    pub fn to_link(&self) -> String {
        build_magnet(
            &self.info_hash,
            self.file_idx,
            self.name.as_deref(),
            self.trackers.iter().map(String::as_str),
        )
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn build_magnet<'a>(
    hash: &str,
    file_idx: Option<usize>,
    name: Option<&str>,
    trackers: impl Iterator<Item = &'a str>,
) -> String {
    let mut out = format!("magnet:?xt=urn:btih:{}", hash.to_ascii_lowercase());
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        out.push_str("&dn=");
        out.push_str(&enc(name));
    }
    for tr in trackers {
        out.push_str("&tr=");
        out.push_str(&enc(tr));
    }
    if let Some(idx) = file_idx {
        out.push_str(&format!("&so={idx}"));
    }
    out
}

fn base32_to_hex(s: &str) -> Option<String> {
    if s.len() != 32 {
        return None;
    }
    let mut bits: u64 = 0;
    let mut nbits = 0;
    let mut bytes = Vec::with_capacity(20);
    for c in s.chars() {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u64 - 'A' as u64,
            c @ '2'..='7' => c as u64 - '2' as u64 + 26,
            _ => return None,
        };
        bits = (bits << 5) | v;
        nbits += 5;
        if nbits >= 8 {
            nbits -= 8;
            bytes.push((bits >> nbits) as u8);
            bits &= (1 << nbits) - 1;
        }
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn normalize_hash(s: &str) -> Option<String> {
    if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(s.to_ascii_lowercase());
    }
    base32_to_hex(s)
}

/// Reads a `magnet:?xt=urn:btih:…` link. `None` when it is not one (or has no v1 hash).
pub fn parse_magnet(s: &str) -> Option<MagnetRef> {
    let s = s.trim();
    let query = s
        .get(..8)
        .filter(|p| p.eq_ignore_ascii_case("magnet:?"))
        .map(|_| &s[8..])?;
    let mut hash = None;
    let mut name = None;
    let mut trackers = Vec::new();
    let mut file_idx = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "xt" if hash.is_none() => {
                let lower = value.to_ascii_lowercase();
                if let Some(h) = lower.strip_prefix("urn:btih:") {
                    hash = normalize_hash(&value[value.len() - h.len()..]);
                }
            }
            "dn" if name.is_none() => name = Some(value.into_owned()).filter(|n| !n.is_empty()),
            "tr" => {
                if trackers.len() < MAX_TRACKERS
                    && is_safe_tracker(&value)
                    && !trackers.contains(&value.to_string())
                {
                    trackers.push(value.into_owned());
                }
            }
            "so" if file_idx.is_none() => {
                file_idx = value
                    .split([',', '-'])
                    .next()
                    .and_then(|n| n.trim().parse().ok());
            }
            _ => {}
        }
    }
    Some(MagnetRef {
        info_hash: hash?,
        name,
        trackers,
        file_idx,
    })
}

/// The magnet of an addon stream with `infoHash`: trackers from its `sources` entries that start
/// with `tracker:` (`dht:` and others are left out), the file index as `so`.
pub fn magnet_from_stream(
    info_hash: &str,
    file_idx: Option<usize>,
    name: &str,
    sources: &[String],
) -> String {
    build_magnet(
        info_hash,
        file_idx,
        Some(name),
        sources
            .iter()
            .filter_map(|s| s.strip_prefix("tracker:"))
            .filter(|t| is_safe_tracker(t))
            .take(MAX_TRACKERS),
    )
}

/// A tracker the client may announce to: `http`, `https` or `udp`; a literal IP address must be
/// public (not loopback, private or link-local), a name must not be `localhost`. Addon data is
/// not trusted to point the client at the owner's network.
pub fn is_safe_tracker(tracker: &str) -> bool {
    let Ok(url) = url::Url::parse(tracker.trim()) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https" | "udp") {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => crate::rules::is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => crate::rules::is_public_ip(ip.into()),
        Some(url::Host::Domain(name)) => {
            // `udp:` is not a special scheme: its host stays opaque text, also an IP address.
            if let Ok(ip) = name.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
                return crate::rules::is_public_ip(ip);
            }
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            // A host that looks numeric but is no canonical IP (`2130706433`, `0x7f.1`, `127.1`)
            // may still be read as an address by the system resolver: refused.
            let numeric_looking = name.contains(':')
                || name.split('.').next_back().is_some_and(|last| {
                    !last.is_empty()
                        && (last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x"))
                });
            !numeric_looking && name != "localhost" && !name.ends_with(".localhost")
        }
        None => false,
    }
}

/// Windows device names (with any extension): such a "file" is the device.
fn is_reserved_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        s => {
            (s.starts_with("COM") || s.starts_with("LPT"))
                && s.len() == 4
                && matches!(s.as_bytes()[3], b'1'..=b'9')
        }
    }
}

/// One path component of a torrent file that stays a plain name inside the download folder.
fn is_safe_component(c: &str) -> bool {
    let mut parts = Path::new(c).components();
    let plain =
        matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none();
    plain
        && c != "."
        && c != ".."
        && !c.ends_with('.')
        && !c.ends_with(' ')
        && !c.contains([':', '/', '\\', '\0'])
        && !is_reserved_name(c)
}

/// Every file of the torrent stays inside its download folder: each path (its components
/// joined by `/`) has only plain names, none with `:` (a drive or a stream on Windows), no
/// device name, no trailing dot or space, nothing empty. The torrent library opens every file
/// of the torrent (not only the selected one), so one bad path refuses the whole torrent.
pub fn files_are_safe(files: &TorrentFiles) -> bool {
    !files.files.is_empty()
        && files
            .files
            .iter()
            .all(|(path, _)| path.split(['/', '\\']).all(is_safe_component))
}

/// Deletes `path` (a file or a folder) only when it is really inside `root`: both are made
/// canonical first, so a link or a drive-relative name cannot lead the deletion elsewhere.
pub async fn remove_inside(root: &Path, path: &Path) {
    let (Ok(root), Ok(target)) = (
        tokio::fs::canonicalize(root).await,
        tokio::fs::canonicalize(path).await,
    ) else {
        return;
    };
    if target == root || !target.starts_with(&root) {
        return;
    }
    let is_dir = tokio::fs::symlink_metadata(&target)
        .await
        .is_ok_and(|m| m.is_dir());
    let _ = if is_dir {
        tokio::fs::remove_dir_all(&target).await
    } else {
        tokio::fs::remove_file(&target).await
    };
}

/// Is `path` a video by its extension.
pub fn is_video_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

/// The file to download: `file_idx` when given (and present), else the largest video file.
pub fn pick_file(files: &TorrentFiles, file_idx: Option<usize>) -> Result<usize, Msg> {
    if let Some(idx) = file_idx {
        return if idx < files.files.len() {
            Ok(idx)
        } else {
            Err(Msg::error("torrent_no_video"))
        };
    }
    files
        .files
        .iter()
        .enumerate()
        .filter(|(_, (path, _))| is_video_path(path))
        .max_by_key(|(_, (_, len))| *len)
        .map(|(i, _)| i)
        .ok_or_else(|| Msg::error("torrent_no_video"))
}

// ---------- Backend ----------

/// A readable, seekable stream of the selected file (reads wait for missing pieces, which the
/// backend fetches first).
pub trait FileStream: AsyncRead + AsyncSeek + Send + Unpin {}
impl<T: AsyncRead + AsyncSeek + Send + Unpin> FileStream for T {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TorrentFiles {
    /// `(path inside the torrent, length)` in the torrent's order.
    pub files: Vec<(String, u64)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawProgress {
    pub file_downloaded: u64,
    pub file_total: u64,
    pub peers: u32,
    pub complete: bool,
}

/// The torrent library. `hash` is always the 40-hex info hash.
#[async_trait]
pub trait TorrentBackend: Send + Sync {
    /// Gets the torrent's file list (metadata from peers for a magnet); downloads nothing.
    async fn resolve(&self, m: &MagnetRef) -> Result<TorrentFiles, Msg>;
    /// Starts downloading only file `file_idx` into `dir`.
    async fn add(
        &self,
        m: &MagnetRef,
        file_idx: usize,
        dir: &Path,
        upload_bps: Option<u64>,
    ) -> Result<(), Msg>;
    async fn progress(&self, hash: &str) -> Option<RawProgress>;
    async fn open_file(&self, hash: &str, idx: usize) -> Result<Box<dyn FileStream>, Msg>;
    /// Stops transfers (the torrent stays readable).
    async fn stop(&self, hash: &str);
    async fn remove(&self, hash: &str, delete_files: bool);
    /// Upload limit for all torrents, bytes/s (`None` = unlimited).
    fn set_upload_limit(&self, bps: Option<u64>);
}

// ---------- Manager ----------

#[derive(Debug, Clone, PartialEq)]
pub enum TorrentState {
    Starting,
    Downloading,
    Complete,
    WaitingForDisk,
    Failed(Msg),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TorrentStatus {
    pub state: TorrentState,
    /// Of the selected file.
    pub downloaded: u64,
    pub total: u64,
    /// 30 s moving average, bytes/s.
    pub rate_bps: f64,
    pub peers: u32,
    /// Path of the selected file inside the torrent (empty until known).
    pub file_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    /// Not yet: in about this many seconds at the current rate.
    WaitS(u64),
    /// No estimate (no rate yet, unknown duration, not downloading).
    Unknown,
    /// The download failed for good (its retry too): the item fails instead of airing.
    Failed,
}

/// `Ready` when `remaining / rate ≤ duration / 1.15`, else the wait until it would be.
pub fn estimate(remaining: u64, rate_bps: f64, duration_s: Option<f64>) -> Readiness {
    if remaining == 0 {
        return Readiness::Ready;
    }
    let Some(duration) = duration_s.filter(|d| *d > 0.0) else {
        return Readiness::Unknown;
    };
    if rate_bps <= 0.0 {
        return Readiness::Unknown;
    }
    let needed = remaining as f64 / rate_bps;
    let budget = duration / MARGIN;
    if needed <= budget {
        Readiness::Ready
    } else {
        Readiness::WaitS((needed - budget).ceil() as u64)
    }
}

struct Item {
    magnet: MagnetRef,
    state: TorrentState,
    files: Option<TorrentFiles>,
    file_idx: Option<usize>,
    downloaded: u64,
    total: u64,
    peers: u32,
    samples: VecDeque<(Instant, u64)>,
    /// The estimate once said it will make it: from then on it may be played (also again,
    /// for a retry or a seek, when the rate has dropped since).
    cleared: bool,
}

impl Item {
    fn new(magnet: MagnetRef) -> Self {
        Self {
            magnet,
            state: TorrentState::Starting,
            files: None,
            file_idx: None,
            downloaded: 0,
            total: 0,
            peers: 0,
            samples: VecDeque::new(),
            cleared: false,
        }
    }

    /// How long the rate samples cover.
    fn rate_span(&self) -> Duration {
        match (self.samples.front(), self.samples.back()) {
            (Some(first), Some(last)) => last.0.duration_since(first.0),
            _ => Duration::ZERO,
        }
    }

    fn rate(&self) -> f64 {
        let (Some(first), Some(last)) = (self.samples.front(), self.samples.back()) else {
            return 0.0;
        };
        let secs = last.0.duration_since(first.0).as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        last.1.saturating_sub(first.1) as f64 / secs
    }

    fn file_name(&self) -> String {
        match (&self.files, self.file_idx) {
            (Some(files), Some(idx)) => files.files.get(idx).map(|f| f.0.clone()),
            _ => None,
        }
        .unwrap_or_default()
    }

    /// Takes disk space (started, not waiting or failed).
    fn holds_disk(&self) -> bool {
        matches!(
            self.state,
            TorrentState::Starting | TorrentState::Downloading | TorrentState::Complete
        )
    }
}

pub struct TorrentManager {
    backend: Arc<dyn TorrentBackend>,
    root: PathBuf,
    upload_kbps: AtomicU64,
    disk_cap: AtomicU64,
    items: Mutex<HashMap<String, Item>>,
    /// Set when torrents are switched off: nothing starts any more, a start in flight takes
    /// its torrent away again.
    closed: AtomicBool,
}

impl TorrentManager {
    /// A manager over `backend`, files under `root` (`<data dir>/torrents`). No sampler runs
    /// until [`TorrentManager::spawn_sampler`].
    pub fn new(
        backend: Arc<dyn TorrentBackend>,
        root: PathBuf,
        upload_kbps: u64,
        disk_cap_gb: u64,
    ) -> Arc<Self> {
        backend.set_upload_limit(upload_bps(upload_kbps));
        Arc::new(Self {
            backend,
            root,
            upload_kbps: AtomicU64::new(upload_kbps),
            disk_cap: AtomicU64::new(disk_cap_bytes(disk_cap_gb)),
            items: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        })
    }

    /// Torrents were switched off: no start goes on (see [`TorrentManager::start`]).
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The real manager: a librqbit session rooted in `root`, with the 1 s sampler running.
    pub async fn create(
        root: PathBuf,
        upload_kbps: u64,
        disk_cap_gb: u64,
    ) -> Result<Arc<Self>, Msg> {
        let backend =
            backend_rqbit::RqbitBackend::new(&root, upload_bps(upload_kbps), false).await?;
        let manager = Self::new(Arc::new(backend), root, upload_kbps, disk_cap_gb);
        manager.spawn_sampler();
        Ok(manager)
    }

    /// Samples every second while the manager lives.
    pub fn spawn_sampler(self: &Arc<Self>) {
        let weak: Weak<Self> = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SAMPLE_EVERY);
            loop {
                tick.tick().await;
                let Some(me) = weak.upgrade() else { break };
                me.sample().await;
            }
        });
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir_of(&self, hash: &str) -> PathBuf {
        self.root.join(hash)
    }

    pub fn set_upload_kbps(&self, kbps: u64) {
        self.upload_kbps.store(kbps, Ordering::Relaxed);
        self.backend.set_upload_limit(upload_bps(kbps));
    }

    pub fn set_disk_cap_gb(&self, gb: u64) {
        self.disk_cap.store(disk_cap_bytes(gb), Ordering::Relaxed);
    }

    /// Starts (or retries a waiting/failed) torrent. Waits for the file list; returns `Ok` also
    /// when it is left `WaitingForDisk` (calling again retries). Errors are kept as `Failed`.
    pub async fn start(&self, m: &MagnetRef) -> Result<(), Msg> {
        if self.is_closed() {
            return Err(Msg::error("torrents_disabled"));
        }
        let hash = m.info_hash.clone();
        let files = {
            let mut items = self.items.lock().unwrap();
            match items.get_mut(&hash) {
                Some(item) => match item.state {
                    TorrentState::WaitingForDisk | TorrentState::Failed(_) => {
                        item.state = TorrentState::Starting;
                        item.files.clone()
                    }
                    _ => return Ok(()),
                },
                None => {
                    items.insert(hash.clone(), Item::new(m.clone()));
                    None
                }
            }
        };
        let result = self.start_inner(m, files).await;
        if let Err(e) = &result {
            self.set_state(&hash, TorrentState::Failed(e.clone()));
        }
        result
    }

    async fn start_inner(&self, m: &MagnetRef, files: Option<TorrentFiles>) -> Result<(), Msg> {
        let hash = &m.info_hash;
        let files = match files {
            Some(f) => f,
            None => self.backend.resolve(m).await?,
        };
        // Before the torrent is added: the library creates and opens every file of it.
        if !files_are_safe(&files) {
            return Err(Msg::error("torrent_bad_files"));
        }
        let idx = pick_file(&files, m.file_idx)?;
        let total = files.files[idx].1;
        // Walks the folder: not while holding the lock the channel's checks wait for.
        let on_disk = self.disk_used();
        let needs_wait = {
            let mut items = self.items.lock().unwrap();
            let reserved: u64 = items
                .iter()
                .filter(|(h, i)| *h != hash && i.holds_disk())
                .map(|(_, i)| i.total)
                .sum();
            let used = on_disk.max(reserved);
            let cap = self.disk_cap.load(Ordering::Relaxed);
            let Some(item) = items.get_mut(hash) else {
                // Dropped while resolving.
                return Ok(());
            };
            item.files = Some(files);
            item.file_idx = Some(idx);
            item.total = total;
            let wait = used.saturating_add(total) > cap;
            if wait {
                item.state = TorrentState::WaitingForDisk;
            }
            wait
        };
        if needs_wait {
            return Ok(());
        }
        let dir = self.dir_of(hash);
        if self.is_closed() {
            return Err(Msg::error("torrents_disabled"));
        }
        let bps = upload_bps(self.upload_kbps.load(Ordering::Relaxed));
        self.backend.add(m, idx, &dir, bps).await?;
        // Switched off while it was being added: it must not keep downloading.
        if self.is_closed() {
            self.items.lock().unwrap().remove(hash);
            self.backend.remove(hash, false).await;
            remove_inside(&self.root, &dir).await;
            return Err(Msg::error("torrents_disabled"));
        }
        let dropped = {
            let mut items = self.items.lock().unwrap();
            match items.get_mut(hash) {
                Some(item) => {
                    if item.state == TorrentState::Starting {
                        item.state = TorrentState::Downloading;
                    }
                    false
                }
                None => true,
            }
        };
        if dropped {
            self.backend.remove(hash, false).await;
            remove_inside(&self.root, &dir).await;
        }
        Ok(())
    }

    fn set_state(&self, hash: &str, state: TorrentState) {
        if let Some(item) = self.items.lock().unwrap().get_mut(hash) {
            item.state = state;
        }
    }

    /// Reads the progress of downloading torrents once: rate samples, completion (→ stop).
    pub async fn sample(&self) {
        let active: Vec<String> = self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, i)| i.state == TorrentState::Downloading)
            .map(|(h, _)| h.clone())
            .collect();
        for hash in active {
            let Some(p) = self.backend.progress(&hash).await else {
                continue;
            };
            let now = Instant::now();
            let completed = {
                let mut items = self.items.lock().unwrap();
                let Some(item) = items.get_mut(&hash) else {
                    continue;
                };
                if item.state != TorrentState::Downloading {
                    continue;
                }
                item.downloaded = p.file_downloaded;
                if p.file_total > 0 {
                    item.total = p.file_total;
                }
                item.peers = p.peers;
                item.samples.push_back((now, p.file_downloaded));
                while item
                    .samples
                    .front()
                    .is_some_and(|(t, _)| now.duration_since(*t) > RATE_WINDOW)
                {
                    item.samples.pop_front();
                }
                let done = p.complete || (item.total > 0 && item.downloaded >= item.total);
                if done {
                    item.state = TorrentState::Complete;
                    item.downloaded = item.total;
                    item.peers = 0;
                }
                done
            };
            if completed {
                self.backend.stop(&hash).await;
            }
        }
    }

    pub fn status(&self, hash: &str) -> Option<TorrentStatus> {
        let items = self.items.lock().unwrap();
        let item = items.get(hash)?;
        Some(TorrentStatus {
            state: item.state.clone(),
            downloaded: item.downloaded,
            total: item.total,
            rate_bps: if item.state == TorrentState::Downloading {
                item.rate()
            } else {
                0.0
            },
            peers: item.peers,
            file_name: item.file_name(),
        })
    }

    /// Can the item start playing now (see [`estimate`]); a complete file is always ready.
    /// While the rate samples cover less than [`MIN_RATE_SPAN`] there is no estimate yet.
    /// A `Ready` answer clears the item for playing (see [`TorrentManager::playable`]).
    pub fn will_make_it(&self, hash: &str, duration_s: Option<f64>) -> Readiness {
        let mut items = self.items.lock().unwrap();
        let Some(item) = items.get_mut(hash) else {
            return Readiness::Unknown;
        };
        let r = match item.state {
            TorrentState::Complete => Readiness::Ready,
            TorrentState::Downloading if item.total > 0 && item.downloaded >= item.total => {
                Readiness::Ready
            }
            TorrentState::Downloading if item.total > 0 && item.rate_span() >= MIN_RATE_SPAN => {
                estimate(
                    item.total.saturating_sub(item.downloaded),
                    item.rate(),
                    duration_s,
                )
            }
            _ => Readiness::Unknown,
        };
        if r == Readiness::Ready {
            item.cleared = true;
        }
        r
    }

    /// May the item be played now: complete, or downloading and once said to make it. Never
    /// starts anything; the error is the stored failure, else `torrent_not_active`.
    pub fn playable(&self, hash: &str) -> Result<(), Msg> {
        let items = self.items.lock().unwrap();
        match items.get(hash) {
            Some(i) if i.state == TorrentState::Complete => Ok(()),
            Some(i) if i.state == TorrentState::Downloading && i.cleared => Ok(()),
            Some(Item {
                state: TorrentState::Failed(e),
                ..
            }) => Err(e.clone()),
            _ => Err(Msg::error("torrent_not_active")),
        }
    }

    /// A stream of the selected file (downloading or complete torrents only).
    pub async fn open(&self, hash: &str) -> Result<Box<dyn FileStream>, Msg> {
        let idx = {
            let items = self.items.lock().unwrap();
            items
                .get(hash)
                .filter(|i| matches!(i.state, TorrentState::Downloading | TorrentState::Complete))
                .and_then(|i| i.file_idx)
        };
        let idx = idx.ok_or_else(|| Msg::error("torrent_not_active"))?;
        self.backend.open_file(hash, idx).await
    }

    /// The magnet the item was started with.
    pub fn magnet(&self, hash: &str) -> Option<MagnetRef> {
        self.items
            .lock()
            .unwrap()
            .get(hash)
            .map(|i| i.magnet.clone())
    }

    /// Forgets the item: removes the torrent and deletes its files.
    pub async fn drop_item(&self, hash: &str) {
        let known = self.items.lock().unwrap().remove(hash).is_some();
        if known {
            // The library would delete the files by the torrent's own paths: this folder is
            // deleted here instead, only inside the root.
            self.backend.remove(hash, false).await;
        }
        if hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            remove_inside(&self.root, &self.dir_of(hash)).await;
        }
    }

    /// Deletes everything under the root that is not an item in `keep` (info hashes).
    pub async fn cleanup_leftovers(&self, keep: &[String]) {
        let forget: Vec<String> = self
            .items
            .lock()
            .unwrap()
            .keys()
            .filter(|h| !keep.iter().any(|k| k.eq_ignore_ascii_case(h)))
            .cloned()
            .collect();
        for hash in forget {
            self.drop_item(&hash).await;
        }
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if keep.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
                continue;
            }
            remove_inside(&self.root, &entry.path()).await;
        }
    }

    /// Bytes of the files under the root.
    pub fn disk_used(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return 0;
            };
            entries
                .flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                    _ => 0,
                })
                .sum()
        }
        walk(&self.root)
    }
}
