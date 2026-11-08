//! Media metadata shared between the resolver and the request rules.

/// One entry of a playlist returned by the resolver.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    pub url: String,
    pub title: String,
    pub duration_s: Option<f64>,
    /// A live stream (yt-dlp's `live_status` of a flat entry), refused for viewers.
    pub is_live: bool,
}

/// One input handed to ffmpeg/ffprobe: a URL or path plus the HTTP headers to send with it.
/// Headers keep their insertion order (it ends up verbatim in the `-headers` argument).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInput {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// A source resolved into concrete inputs (video, optionally a separate audio input).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedStream {
    pub inputs: Vec<StreamInput>,
    pub title: String,
    pub duration_s: Option<f64>,
    pub is_live: bool,
    /// The site's picture of the media (yt-dlp's `thumbnail`): the cover of an audio-only one.
    pub thumbnail: Option<String>,
    /// Who made it, as the site says (yt-dlp's `artist`, else `uploader`).
    pub artist: Option<String>,
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
