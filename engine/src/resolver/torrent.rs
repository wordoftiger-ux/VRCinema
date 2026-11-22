//! `magnet:` sources: the in-process torrent client downloads the film's file while it is
//! played, and ffmpeg reads it from the loopback stream route (`http::routes_torrent`).

use std::sync::Arc;

use async_trait::async_trait;

use super::{MediaMeta, MediaResolver, ResolveError, ResolvedStream, StreamInput};
use crate::torrent::hub::TorrentHub;
use crate::torrent::parse_magnet;

pub fn is_magnet(url: &str) -> bool {
    url.get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("magnet:"))
}

pub struct TorrentResolver {
    hub: Arc<TorrentHub>,
}

impl TorrentResolver {
    pub fn new(hub: Arc<TorrentHub>) -> Self {
        Self { hub }
    }
}

#[async_trait]
impl MediaResolver for TorrentResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        let m = parse_magnet(url).ok_or_else(|| ResolveError::coded("torrent_bad_magnet"))?;
        Ok(MediaMeta {
            title: m.name.clone().unwrap_or(m.info_hash),
            duration_s: None,
            is_live: false,
            is_playlist: false,
            entries: None,
        })
    }

    /// Starts the torrent (waits for its file list) and returns the loopback address.
    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        let hash = self.hub.start(url).await?;
        let m = parse_magnet(url).ok_or_else(|| ResolveError::coded("torrent_bad_magnet"))?;
        Ok(ResolvedStream {
            inputs: vec![StreamInput {
                url: self.hub.local_url(&hash),
                headers: Vec::new(),
            }],
            title: m.name.unwrap_or(hash),
            duration_s: None,
            is_live: false,
            ..Default::default()
        })
    }
}
