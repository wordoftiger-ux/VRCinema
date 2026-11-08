//! Media pipeline building blocks (ports of `src/pipeline/*.ts`).
//!
//! The stream's timing in one place: the segment length is [`SEGMENT_S`]; everything counted
//! in segments (the public window, the viewers' lag, the broadcaster's lead, the monitor's
//! lookahead) is derived from a length in seconds, so changing it keeps the delays the same.

/// Length of one HLS segment in seconds: ffmpeg cuts every clip (and forces a key frame) at
/// this interval.
pub const SEGMENT_S: u32 = 1;

/// How far behind the newest published segment the viewers play (seconds). Players start
/// three target durations from the end of the list (hls.js, ExoPlayer, AVPlayer, Media
/// Foundation), so the public list announces a target duration of at least a third of it
/// ([`MIN_TARGET_DURATION_S`]) and the broadcaster publishes that much ahead.
pub const VIEWER_LAG_S: u32 = 12;

/// The smallest `EXT-X-TARGETDURATION` of the public list: three of it are [`VIEWER_LAG_S`]
/// (a target duration above the segments' length is valid HLS: it is an upper bound).
pub const MIN_TARGET_DURATION_S: u32 = VIEWER_LAG_S / 3;

/// Seconds of stream the public list keeps.
pub const WINDOW_S: u32 = 40;

/// Segments of [`SEGMENT_S`] in `seconds` (rounded up, at least 1).
pub const fn segments_for(seconds: u32) -> usize {
    let n = seconds.div_ceil(SEGMENT_S);
    if n == 0 { 1 } else { n as usize }
}

/// [`VIEWER_LAG_S`] in segments.
pub const VIEWER_LAG_SEGMENTS: usize = segments_for(VIEWER_LAG_S);

/// [`WINDOW_S`] in segments.
pub const WINDOW_SEGMENTS: usize = segments_for(WINDOW_S);

pub mod advanced;
pub mod air;
pub mod air_watch;
pub mod audio_visual;
pub mod broadcaster;
pub mod channel;
pub mod clip_factory;
pub mod ffmpeg_args;
pub mod monitor;
pub mod playlist;
pub mod probe;
pub mod segmenter;
pub mod slate_look;
pub mod slates;
pub mod temp_downloads;
pub mod ticker;
pub mod tsrewrite;
