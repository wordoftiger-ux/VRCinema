//! Media metadata shared between the resolver and the request rules.

/// One entry of a playlist returned by the resolver.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    pub url: String,
    pub title: String,
    pub duration_s: Option<f64>,
}

/// What the resolver knows about a requested source before it is queued.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaMeta {
    pub title: String,
    pub duration_s: Option<f64>,
    pub is_live: bool,
    pub is_playlist: bool,
    pub entries: Option<Vec<PlaylistEntry>>,
}
