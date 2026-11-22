//! The app's one place for torrents: the settings that matter, the lazily created manager, the
//! per-run token and the port of the loopback stream route (see `http::routes_torrent`).
//!
//! The manager (and with it the torrent session, DHT and sockets) exists only after the first
//! need while `torrentsEnabled` is on; turning the setting off stops every torrent and drops it.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU16, Ordering};
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
    root: PathBuf,
    token: String,
    port: AtomicU16,
    settings: Mutex<TorrentSettings>,
    factory: ManagerFactory,
    /// Serialises creation; the sync view below is what readers use.
    create: tokio::sync::Mutex<()>,
    manager: RwLock<Option<Arc<TorrentManager>>>,
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
            root,
            token,
            port: AtomicU16::new(0),
            settings: Mutex::new(TorrentSettings::default()),
            factory,
            create: tokio::sync::Mutex::new(()),
            manager: RwLock::new(None),
        })
    }

    /// The folder of the torrents' files.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The random token of this run: part of every stream address.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The port the server listens on (set once it is bound).
    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    /// The loopback address ffmpeg reads a torrent's file from.
    pub fn local_url(&self, hash: &str) -> String {
        format!(
            "http://127.0.0.1:{}/internal/torrent/{}/{hash}",
            self.port.load(Ordering::Relaxed),
            self.token
        )
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
        let m = (self.factory)(
            self.root.clone(),
            s.torrent_upload_kbps,
            s.torrent_disk_cap_gb,
        )
        .await?;
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
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) = new.clone();
        if !new.torrents_enabled {
            let taken = self
                .manager
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(m) = taken {
                m.cleanup_leftovers(&[]).await;
            }
        } else if let Some(m) = self.manager_if_running() {
            m.set_upload_kbps(new.torrent_upload_kbps);
            m.set_disk_cap_gb(new.torrent_disk_cap_gb);
        }
    }

    /// Starts the torrent of `magnet` (waits for its file list: may take minutes).
    pub async fn start(&self, magnet: &str) -> Result<String, Msg> {
        let m = parse_magnet(magnet).ok_or_else(|| Msg::error("torrent_bad_magnet"))?;
        self.manager().await?.start(&m).await?;
        Ok(m.info_hash)
    }
}
