//! What the channel asks about a torrent item (a `magnet:` source): start its download ahead,
//! "can it play now", and drop it once the item has left the channel. The channel is
//! synchronous: every call returns at once and the slow work (starting, probing, deleting) runs
//! in spawned tasks.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::hub::TorrentHub;
use super::{Readiness, TorrentState, TorrentStatus, parse_magnet};
use crate::db::QueueItem;
use crate::i18n::Msg;
use crate::pipeline::channel::BoxFuture;
use crate::resolver::torrent::is_magnet;

/// A film of unknown length is probed (ffprobe on the loopback stream) once this much of its
/// file is in.
pub const PROBE_AFTER_BYTES: u64 = 8 * 1024 * 1024;

/// A probe that could not tell the length is tried again after this long (ffprobe may need
/// parts of the file that had not arrived yet, e.g. a Matroska file's index at its end)...
pub const PROBE_RETRY: std::time::Duration = std::time::Duration::from_secs(20);

/// ...this many times in all; after that the film plays only once its file is complete.
pub const PROBE_TRIES: u32 = 6;

/// A failed start is tried this many times in all before the item fails.
pub const START_ATTEMPTS: u32 = 2;

/// The channel's view of torrent items (real: [`HubGate`]; in tests: a fake).
pub trait TorrentGate: Send + Sync {
    /// Starts the item's download unless it runs (or is starting) already; a download waiting
    /// for disk space, or one whose first start failed, is tried again. `false`: torrents are
    /// off (nothing started).
    fn ensure_started(&self, item: &QueueItem) -> bool;
    /// Can the item start now. `duration_s`: the film's length when known. `Ready` only when
    /// its download will make it (or torrents are off: its start then fails as any); `Failed`
    /// once the download failed twice (see [`TorrentGate::failure`]).
    fn readiness(&self, item: &QueueItem, duration_s: Option<f64>) -> Readiness;
    /// Why the item's download failed for good (`None` while it has not).
    fn failure(&self, item: &QueueItem) -> Option<Msg>;
    /// The item left the channel: its download stops and its files are deleted.
    fn release(&self, item: &QueueItem);
    /// At channel start: deletes every torrent and file not belonging to `keep`.
    fn cleanup(&self, keep: &[QueueItem]);
}

/// ffprobe of a film's length at an address (`None`: could not tell).
pub type ProbeFn = Arc<dyn Fn(String) -> BoxFuture<'static, Option<f64>> + Send + Sync>;

enum Probe {
    /// The `tries`-th probe runs.
    Running {
        tries: u32,
    },
    Done(f64),
    /// The last of `tries` probes could not tell, at `at`.
    Failed {
        tries: u32,
        at: tokio::time::Instant,
    },
}

#[derive(Default)]
struct GateState {
    /// Info hashes whose start is in flight.
    starting: HashSet<String>,
    /// Lengths probed for films of unknown duration.
    probes: HashMap<String, Probe>,
    /// Failed starts of an info hash: how many, the last error.
    failures: HashMap<String, (u32, Msg)>,
}

/// [`TorrentGate`] over the app's [`TorrentHub`].
pub struct HubGate {
    hub: Arc<TorrentHub>,
    probe: ProbeFn,
    state: Arc<Mutex<GateState>>,
}

fn hash_of(item: &QueueItem) -> Option<String> {
    if !is_magnet(&item.source_url) {
        return None;
    }
    parse_magnet(&item.source_url).map(|m| m.info_hash)
}

fn lock(state: &Mutex<GateState>) -> MutexGuard<'_, GateState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `fut` on the current runtime; `false` without one.
fn spawn(fut: impl std::future::Future<Output = ()> + Send + 'static) -> bool {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(fut);
            true
        }
        Err(_) => false,
    }
}

impl HubGate {
    pub fn new(hub: Arc<TorrentHub>, probe: ProbeFn) -> Arc<Self> {
        Arc::new(Self {
            hub,
            probe,
            state: Arc::default(),
        })
    }

    /// The probed length: `None` while not known yet (too little downloaded, or the first probe
    /// runs), `Some(None)` while ffprobe could not tell (a failed probe is tried again every
    /// [`PROBE_RETRY`], [`PROBE_TRIES`] times in all).
    fn probed(&self, hash: &str, status: &TorrentStatus) -> Option<Option<f64>> {
        let mut s = lock(&self.state);
        let tries = match s.probes.get(hash) {
            Some(Probe::Done(d)) => return Some(Some(*d)),
            Some(Probe::Running { tries }) => return (*tries > 1).then_some(None),
            Some(Probe::Failed { tries, at }) => {
                if *tries >= PROBE_TRIES || at.elapsed() < PROBE_RETRY {
                    return Some(None);
                }
                *tries + 1
            }
            None => {
                if status.downloaded < PROBE_AFTER_BYTES.min(status.total) {
                    return None;
                }
                1
            }
        };
        s.probes.insert(hash.to_string(), Probe::Running { tries });
        drop(s);
        let probe = (self.probe)(self.hub.local_url(hash));
        let state = Arc::clone(&self.state);
        let key = hash.to_string();
        let spawned = spawn(async move {
            let d = probe.await.filter(|d| d.is_finite() && *d > 0.0);
            if let Some(p) = lock(&state).probes.get_mut(&key) {
                *p = match d {
                    Some(d) => Probe::Done(d),
                    None => Probe::Failed {
                        tries,
                        at: tokio::time::Instant::now(),
                    },
                };
            }
        });
        if !spawned {
            lock(&self.state).probes.remove(hash);
        }
        (tries > 1).then_some(None)
    }

    /// Its start failed [`START_ATTEMPTS`] times.
    fn gave_up(&self, hash: &str) -> bool {
        lock(&self.state)
            .failures
            .get(hash)
            .is_some_and(|(n, _)| *n >= START_ATTEMPTS)
    }
}

impl TorrentGate for HubGate {
    fn ensure_started(&self, item: &QueueItem) -> bool {
        if !self.hub.enabled() {
            return false;
        }
        let Some(hash) = hash_of(item) else {
            return false;
        };
        if self.gave_up(&hash) {
            return true;
        }
        let state = self
            .hub
            .manager_if_running()
            .and_then(|m| m.status(&hash))
            .map(|s| s.state);
        let needed = matches!(
            state,
            None | Some(TorrentState::WaitingForDisk | TorrentState::Failed(_))
        );
        if !needed || !lock(&self.state).starting.insert(hash.clone()) {
            return true;
        }
        let hub = Arc::clone(&self.hub);
        let shared = Arc::clone(&self.state);
        let link = item.source_url.clone();
        let key = hash.clone();
        let spawned = spawn(async move {
            let result = hub.start(&link).await;
            let released = {
                let mut s = lock(&shared);
                let released = !s.starting.remove(&key);
                if !released {
                    match result {
                        Ok(_) => {
                            s.failures.remove(&key);
                        }
                        Err(e) => {
                            let n = s.failures.get(&key).map_or(0, |f| f.0);
                            s.failures.insert(key.clone(), (n + 1, e));
                        }
                    }
                }
                released
            };
            // Released while its file list was still coming: it must not stay.
            if released && let Some(m) = hub.manager_if_running() {
                m.drop_item(&key).await;
            }
        });
        if !spawned {
            lock(&self.state).starting.remove(&hash);
        }
        true
    }

    fn readiness(&self, item: &QueueItem, duration_s: Option<f64>) -> Readiness {
        let Some(hash) = hash_of(item) else {
            return Readiness::Ready;
        };
        if !self.hub.enabled() {
            return Readiness::Ready;
        }
        if lock(&self.state).starting.contains(&hash) {
            return Readiness::Unknown;
        }
        if self.gave_up(&hash) {
            return Readiness::Failed;
        }
        let manager = self.hub.manager_if_running();
        let Some((m, status)) = manager.and_then(|m| m.status(&hash).map(|s| (m, s))) else {
            // Not started (or its start left nothing): the next check starts it again.
            return Readiness::Unknown;
        };
        match status.state {
            TorrentState::Complete => return m.will_make_it(&hash, None),
            // A failed start is tried again with the next check; never aired from nothing.
            TorrentState::Failed(_) | TorrentState::Starting | TorrentState::WaitingForDisk => {
                return Readiness::Unknown;
            }
            TorrentState::Downloading => {}
        }
        if let Some(d) = duration_s.filter(|d| d.is_finite() && *d > 0.0) {
            return m.will_make_it(&hash, Some(d));
        }
        match self.probed(&hash, &status) {
            Some(Some(d)) => m.will_make_it(&hash, Some(d)),
            // No length at all: only a complete file plays; the wait is until then.
            Some(None) if status.rate_bps > 0.0 => {
                let remaining = status.total.saturating_sub(status.downloaded) as f64;
                Readiness::WaitS(((remaining / status.rate_bps).ceil() as u64).max(1))
            }
            _ => Readiness::Unknown,
        }
    }

    fn failure(&self, item: &QueueItem) -> Option<Msg> {
        let hash = hash_of(item)?;
        lock(&self.state)
            .failures
            .get(&hash)
            .filter(|(n, _)| *n >= START_ATTEMPTS)
            .map(|(_, e)| e.clone())
    }

    fn release(&self, item: &QueueItem) {
        let Some(hash) = hash_of(item) else {
            return;
        };
        {
            let mut s = lock(&self.state);
            s.probes.remove(&hash);
            s.starting.remove(&hash);
            s.failures.remove(&hash);
        }
        let hub = Arc::clone(&self.hub);
        spawn(async move {
            if let Some(m) = hub.manager_if_running() {
                m.drop_item(&hash).await;
            }
        });
    }

    fn cleanup(&self, keep: &[QueueItem]) {
        let keep: Vec<String> = keep.iter().filter_map(hash_of).collect();
        let hub = Arc::clone(&self.hub);
        spawn(async move {
            if let Some(m) = hub.manager_if_running() {
                m.cleanup_leftovers(&keep).await;
                return;
            }
            // No session (yet): files of an earlier run go straight from the disk.
            let Ok(mut entries) = tokio::fs::read_dir(hub.root()).await else {
                return;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if keep.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
                    continue;
                }
                super::remove_inside(hub.root(), &entry.path()).await;
            }
        });
    }
}
