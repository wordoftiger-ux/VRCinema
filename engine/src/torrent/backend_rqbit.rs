//! The real [`TorrentBackend`]: a librqbit session.
//!
//! - DHT on (magnets need it) without persistence, no listening port (outgoing connections
//!   only), no local service discovery, no session persistence: nothing is written outside the
//!   torrents folder.
//! - The upload limit is the session's, changeable while it runs.
//! - Only the selected file is downloaded (`only_files`); reads go through librqbit's file
//!   stream, which fetches the pieces at the read position first.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::Id20;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ManagedTorrent, Session,
    SessionOptions,
};

use crate::i18n::Msg;

use super::{FileStream, MagnetRef, RawProgress, TorrentBackend, TorrentFiles};

/// Getting a magnet's file list from peers gives up after this long.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(120);

fn fail(e: impl std::fmt::Display) -> Msg {
    Msg::chain(vec![Msg::error("torrent_failed"), Msg::text(e)])
}

fn bps_limit(bps: Option<u64>) -> Option<NonZeroU32> {
    bps.and_then(|b| NonZeroU32::new(b.min(u32::MAX as u64) as u32))
}

struct Resolved {
    torrent: Vec<u8>,
    peers: Vec<SocketAddr>,
}

pub struct RqbitBackend {
    session: Arc<Session>,
    resolved: Mutex<HashMap<String, Resolved>>,
}

impl RqbitBackend {
    /// A session with downloads under `root`. `offline` turns off DHT and trackers (tests: no
    /// network at all).
    pub async fn new(root: &Path, upload_bps: Option<u64>, offline: bool) -> Result<Self, Msg> {
        std::fs::create_dir_all(root)
            .map_err(|e| Msg::chain(vec![Msg::error("torrent_session"), Msg::text(e)]))?;
        let mut opts = SessionOptions {
            dht: (!offline).then(|| DhtSessionConfig {
                persistence: None,
                ..Default::default()
            }),
            disable_trackers: offline,
            fastresume: false,
            persistence: None,
            listen: None,
            disable_local_service_discovery: true,
            ..Default::default()
        };
        opts.ratelimits.upload_bps = bps_limit(upload_bps);
        let session = Session::new_with_opts(root.to_path_buf(), opts)
            .await
            .map_err(|e| {
                Msg::chain(vec![
                    Msg::error("torrent_session"),
                    Msg::text(format!("{e:#}")),
                ])
            })?;
        Ok(Self {
            session,
            resolved: Mutex::new(HashMap::new()),
        })
    }

    fn handle(&self, hash: &str) -> Option<Arc<ManagedTorrent>> {
        let id = Id20::from_str(hash).ok()?;
        self.session.get(TorrentIdOrHash::Hash(id))
    }
}

impl Drop for RqbitBackend {
    fn drop(&mut self) {
        // Stops the session's tasks (DHT, peers) without waiting.
        self.session.cancellation_token().cancel();
    }
}

#[async_trait]
impl TorrentBackend for RqbitBackend {
    async fn resolve(&self, m: &MagnetRef) -> Result<TorrentFiles, Msg> {
        let link = MagnetRef {
            file_idx: None,
            ..m.clone()
        }
        .to_link();
        let opts = AddTorrentOptions {
            list_only: true,
            ..Default::default()
        };
        let add = self
            .session
            .add_torrent(AddTorrent::from_url(link), Some(opts));
        let resp = tokio::time::timeout(RESOLVE_TIMEOUT, add)
            .await
            .map_err(|_| Msg::error("torrent_metadata"))?
            .map_err(|e| {
                Msg::chain(vec![
                    Msg::error("torrent_metadata"),
                    Msg::text(format!("{e:#}")),
                ])
            })?;
        let AddTorrentResponse::ListOnly(list) = resp else {
            // Already in the session: read its files.
            let handle = self
                .handle(&m.info_hash)
                .ok_or_else(|| Msg::error("torrent_metadata"))?;
            let files = handle
                .with_metadata(|meta| {
                    meta.file_infos
                        .iter()
                        .map(|f| (f.relative_filename.to_string_lossy().into_owned(), f.len))
                        .collect::<Vec<_>>()
                })
                .map_err(|_| Msg::error("torrent_metadata"))?;
            return Ok(TorrentFiles { files });
        };
        let files = list
            .info
            .iter_file_details()
            .map(|fd| {
                (
                    fd.filename.to_pathbuf().to_string_lossy().into_owned(),
                    fd.len,
                )
            })
            .collect();
        self.resolved.lock().unwrap().insert(
            m.info_hash.clone(),
            Resolved {
                torrent: list.torrent_bytes.to_vec(),
                peers: list.seen_peers,
            },
        );
        Ok(TorrentFiles { files })
    }

    async fn add(
        &self,
        m: &MagnetRef,
        file_idx: usize,
        dir: &Path,
        upload_bps: Option<u64>,
    ) -> Result<(), Msg> {
        if let Some(handle) = self.handle(&m.info_hash) {
            let only: HashSet<usize> = [file_idx].into_iter().collect();
            return self
                .session
                .update_only_files(&handle, &only)
                .await
                .map_err(|e| fail(format!("{e:#}")));
        }
        // The session-wide limit (changeable live); a per-torrent one would stay fixed.
        self.set_upload_limit(upload_bps);
        let resolved = self.resolved.lock().unwrap().remove(&m.info_hash);
        let (add, peers) = match resolved {
            Some(r) => (AddTorrent::from_bytes(r.torrent), Some(r.peers)),
            None => (
                AddTorrent::from_url(
                    MagnetRef {
                        file_idx: None,
                        ..m.clone()
                    }
                    .to_link(),
                ),
                None,
            ),
        };
        let opts = AddTorrentOptions {
            only_files: Some(vec![file_idx]),
            output_folder: Some(dir.to_string_lossy().into_owned()),
            overwrite: true,
            initial_peers: peers,
            trackers: (!m.trackers.is_empty()).then(|| m.trackers.clone()),
            ..Default::default()
        };
        self.session
            .add_torrent(add, Some(opts))
            .await
            .map_err(|e| fail(format!("{e:#}")))?;
        Ok(())
    }

    async fn progress(&self, hash: &str) -> Option<RawProgress> {
        let handle = self.handle(hash)?;
        let idx = *handle.only_files()?.first()?;
        let total = handle
            .with_metadata(|meta| meta.file_infos.get(idx).map(|f| f.len))
            .ok()
            .flatten()?;
        let stats = handle.stats();
        let have = stats.file_progress.get(idx).copied().unwrap_or(0);
        let peers = stats
            .live
            .as_ref()
            .map(|l| l.snapshot.peer_stats.live)
            .unwrap_or(0);
        Some(RawProgress {
            file_downloaded: have.min(total),
            file_total: total,
            peers,
            complete: total > 0 && have >= total,
        })
    }

    async fn open_file(&self, hash: &str, idx: usize) -> Result<Box<dyn FileStream>, Msg> {
        let handle = self
            .handle(hash)
            .ok_or_else(|| Msg::error("torrent_not_active"))?;
        let stream = handle
            .stream(idx)
            .await
            .map_err(|e| fail(format!("{e:#}")))?;
        Ok(Box::new(stream))
    }

    async fn stop(&self, hash: &str) {
        if let Some(handle) = self.handle(hash) {
            let _ = self.session.pause(&handle).await;
        }
    }

    async fn remove(&self, hash: &str, delete_files: bool) {
        self.resolved.lock().unwrap().remove(hash);
        if let Ok(id) = Id20::from_str(hash) {
            let _ = self
                .session
                .delete(TorrentIdOrHash::Hash(id), delete_files)
                .await;
        }
    }

    fn set_upload_limit(&self, bps: Option<u64>) {
        self.session.ratelimits.set_upload_bps(bps_limit(bps));
    }
}
