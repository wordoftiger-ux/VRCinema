//! The app's one place for torrents: the settings that matter, the lazily created manager, the
//! per-run token and the port of the loopback stream route (see `http::routes_torrent`).
//!
//! The manager (and with it the torrent session, DHT and sockets) exists only after the first
//! need while `torrentsEnabled` is on; turning the setting off stops every torrent and drops it.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use super::settings::TorrentSettings;
use super::{TorrentManager, parse_magnet};
use crate::i18n::Msg;

/// Makes the manager: `(root, upload_kbps, disk_cap_gb)`.
pub type ManagerFactory = Arc<
    dyn Fn(
            PathBuf,
            u64,
            u64,
        ) -> Pin<Box<dyn Future<Output = Result<Arc<TorrentManager>, Msg>> + Send>>
        + Send
        + Sync,
>;

pub struct TorrentHub {
    /// `<data dir>/torrents`: the folder while `torrentDir` is empty.
    default_root: PathBuf,
    token: String,
    port: AtomicU16,
    settings: Mutex<TorrentSettings>,
    factory: ManagerFactory,
    /// Serialises creation; the sync view below is what readers use.
    create: tokio::sync::Mutex<()>,
    manager: RwLock<Option<Arc<TorrentManager>>>,
    /// The bound address when it is not the unspecified one.
    bound_ip: RwLock<Option<IpAddr>>,
    /// File streams the route has open now.
    open_streams: AtomicUsize,
}

/// At most this many file streams of the route are open at once (each holds one of the torrent
/// library's blocking slots while it waits for pieces).
pub const MAX_STREAMS: usize = 6;

/// One open stream of the route; frees its slot when dropped.
pub struct StreamSlot(Arc<TorrentHub>);

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.0.open_streams.fetch_sub(1, Ordering::SeqCst);
    }
}

impl TorrentHub {
    /// The real hub: torrents live in `<data_dir>/torrents`.
    pub fn new(data_dir: &Path) -> Arc<Self> {
        let factory: ManagerFactory =
            Arc::new(|root, kbps, gb| Box::pin(TorrentManager::create(root, kbps, gb)));
        Self::with_factory(data_dir.join("torrents"), factory)
    }

    /// A hub whose manager comes from `factory` (tests give a fake backend).
    pub fn with_factory(root: PathBuf, factory: ManagerFactory) -> Arc<Self> {
        let mut raw = [0u8; 24];
        getrandom::fill(&mut raw).expect("system random number generator");
        let token = raw.iter().map(|b| format!("{b:02x}")).collect();
        Arc::new(Self {
            default_root: root,
            token,
            port: AtomicU16::new(0),
            settings: Mutex::new(TorrentSettings::default()),
            factory,
            create: tokio::sync::Mutex::new(()),
            manager: RwLock::new(None),
            bound_ip: RwLock::new(None),
            open_streams: AtomicUsize::new(0),
        })
    }

    /// The folder of the torrents' files: `<data dir>/torrents`, or the `VRCinema-torrents`
    /// subfolder of the owner's download folder.
    pub fn root(&self) -> PathBuf {
        super::settings::root_of(&self.settings().torrent_dir, &self.default_root)
    }

    /// The folder used while no download folder is chosen.
    pub fn default_root(&self) -> &Path {
        &self.default_root
    }

    /// Free bytes on the drive of the torrents' folder (`None`: not known).
    pub fn drive_free(&self) -> Option<u64> {
        match self.manager_if_running() {
            Some(m) => m.drive_free(),
            None => super::free_space_at(&self.root()),
        }
    }

    /// The random token of this run: part of every stream address.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The address the server listens on (set once it is bound).
    pub fn set_bound(&self, addr: SocketAddr) {
        self.port.store(addr.port(), Ordering::Relaxed);
        *self
            .bound_ip
            .write()
            .unwrap_or_else(PoisonError::into_inner) =
            Some(addr.ip()).filter(|ip| !ip.is_unspecified());
    }

    /// The address ffmpeg reads a torrent's file from: loopback, or the bound address when the
    /// server listens on one address only (it cannot be reached on loopback then).
    pub fn local_url(&self, hash: &str) -> String {
        let ip = self
            .bound_ip()
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        let addr = SocketAddr::new(ip, self.port.load(Ordering::Relaxed));
        format!("http://{addr}/internal/torrent/{}/{hash}", self.token)
    }

    /// The one address the server listens on (`None`: all of them).
    pub fn bound_ip(&self) -> Option<IpAddr> {
        *self.bound_ip.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Is a request from `peer` one of this computer: loopback, or the bound address itself.
    pub fn is_this_computer(&self, peer: IpAddr) -> bool {
        let peer = peer.to_canonical();
        peer.is_loopback() || self.bound_ip().is_some_and(|ip| ip.to_canonical() == peer)
    }

    /// A slot for one open file stream of the route; `None` while [`MAX_STREAMS`] are open.
    pub fn stream_slot(self: &Arc<Self>) -> Option<StreamSlot> {
        let taken = self
            .open_streams
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_STREAMS).then_some(n + 1)
            });
        taken.ok().map(|_| StreamSlot(Arc::clone(self)))
    }

    /// Takes the stored settings at start (nothing is created or stopped).
    pub fn set_settings(&self, new: &TorrentSettings) {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) = new.clone();
    }

    pub fn enabled(&self) -> bool {
        self.settings().torrents_enabled
    }

    pub fn settings(&self) -> TorrentSettings {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The manager when one exists (never creates it).
    pub fn manager_if_running(&self) -> Option<Arc<TorrentManager>> {
        self.manager
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The manager, created now when needed; refused while the setting is off.
    pub async fn manager(&self) -> Result<Arc<TorrentManager>, Msg> {
        if !self.enabled() {
            return Err(Msg::error("torrents_disabled"));
        }
        if let Some(m) = self.manager_if_running() {
            return Ok(m);
        }
        let _one_at_a_time = self.create.lock().await;
        if let Some(m) = self.manager_if_running() {
            return Ok(m);
        }
        let s = self.settings();
        let root = self.root();
        if !s.torrent_dir.is_empty() {
            // Only our own subfolder is made, never the chosen folder itself (a drive that
            // is gone stays an error).
            if !Path::new(&s.torrent_dir).is_dir() {
                return Err(Msg::error("torrent_dir_missing").arg("path", &s.torrent_dir));
            }
            if tokio::fs::create_dir_all(&root).await.is_err() {
                return Err(Msg::error("torrent_dir_not_writable").arg("path", &s.torrent_dir));
            }
        }
        let m = (self.factory)(root, s.torrent_upload_kbps, s.torrent_disk_cap_gb).await?;
        // Switched off while it was being made: do not keep it.
        if !self.enabled() {
            return Err(Msg::error("torrents_disabled"));
        }
        *self.manager.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&m));
        Ok(m)
    }

    /// New settings: the limits apply to a running manager at once; switching off stops every
    /// torrent and drops the manager (and with it the session).
    pub async fn apply(&self, new: &TorrentSettings) {
        let old = std::mem::replace(
            &mut *self.settings.lock().unwrap_or_else(PoisonError::into_inner),
            new.clone(),
        );
        // A new download folder: the running torrents stop and their files (in our folder)
        // go; the next need makes the manager in the new folder.
        let moved = old.torrent_dir != new.torrent_dir;
        if !new.torrents_enabled || moved {
            let taken = self
                .manager
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(m) = taken {
                // A start still in flight holds the manager: it must not go on downloading.
                m.close();
                m.cleanup_leftovers(&[]).await;
            }
        } else if let Some(m) = self.manager_if_running() {
            m.set_upload_kbps(new.torrent_upload_kbps);
            m.set_disk_cap_gb(new.torrent_disk_cap_gb);
        }
    }

    /// The info hash of `magnet` when its download may be played now (see
    /// [`TorrentManager::playable`]). Never starts or restarts a download.
    pub fn playable(&self, magnet: &str) -> Result<String, Msg> {
        let m = parse_magnet(magnet).ok_or_else(|| Msg::error("torrent_bad_magnet"))?;
        if !self.enabled() {
            return Err(Msg::error("torrents_disabled"));
        }
        let manager = self
            .manager_if_running()
            .ok_or_else(|| Msg::error("torrent_not_active"))?;
        manager.playable(&m.info_hash)?;
        Ok(m.info_hash)
    }

    /// Starts the torrent of `magnet` (waits for its file list: may take minutes).
    pub async fn start(&self, magnet: &str) -> Result<String, Msg> {
        let m = parse_magnet(magnet).ok_or_else(|| Msg::error("torrent_bad_magnet"))?;
        self.manager().await?.start(&m).await?;
        Ok(m.info_hash)
    }
}
