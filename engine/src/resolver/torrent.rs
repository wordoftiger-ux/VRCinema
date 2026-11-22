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

/// An address of this app's stream route as [`TorrentHub::local_url`] makes it (`http`, an IP
/// address, `/internal/torrent/{token}/{info hash}`, nothing else): the only input a magnet
/// source may have.
pub fn is_stream_route_url(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    let parts: Vec<&str> = u.path().split('/').collect();
    u.scheme() == "http"
        && matches!(u.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)))
        && u.username().is_empty()
        && u.password().is_none()
        && u.query().is_none()
        && u.fragment().is_none()
        && matches!(parts.as_slice(), ["", "internal", "torrent", token, hash]
            if hex(token, 48) && hex(hash, 40))
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

    /// The loopback address of a download the channel's gate let through (complete, or said to
    /// make it). It never starts or restarts one: playing a download that is not ready (or
    /// failed) would air a film that cannot keep up.
    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        let hash = self.hub.playable(url)?;
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
