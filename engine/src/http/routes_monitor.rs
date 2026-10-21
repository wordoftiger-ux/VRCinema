//! The producer's monitor for the program's window (spec `2026-10-21-monitor-design.md`): the
//! HLS list of the newest clip segments and the segments themselves, read from the engine's
//! latest snapshot ([`crate::pipeline::monitor`]). Window only: no session is 401, a remote
//! session 403 ([`super::require_window`]); never through the tunnel to viewers.

use std::collections::HashMap;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::{ApiError, AppState};

pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/desktop/monitor.m3u8", get(playlist))
        .route("/api/desktop/monitor/{clip}/{name}", get(segment))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            super::require_window,
        ))
}

/// The list (`?from=<row>`: the player starts there). Without a film it has no segments.
async fn playlist(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    state.monitor_watch.touch();
    let from = query.get("from").and_then(|s| s.parse::<u64>().ok());
    let feed = std::sync::Arc::clone(&state.snapshot().monitor);
    (
        [
            (CONTENT_TYPE, "application/vnd.apple.mpegurl"),
            (CACHE_CONTROL, "no-store"),
        ],
        feed.playlist(from),
    )
        .into_response()
}

/// A clip's source key (`clip-12-0-r1`).
fn is_clip_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A segment file of ffmpeg's segment muxer (`seg_00003.ts`): no path, nothing but `.ts`.
fn is_segment_name(name: &str) -> bool {
    name.len() <= 64
        && name.strip_suffix(".ts").is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

/// A segment the list has now (checked by name first, read from where the list keeps it).
async fn segment(
    State(state): State<AppState>,
    Path((clip, name)): Path<(String, String)>,
) -> Response {
    state.monitor_watch.touch();
    let missing = || ApiError::coded(StatusCode::NOT_FOUND, "no_segment", &[]).into_response();
    if !is_clip_name(&clip) || !is_segment_name(&name) {
        return missing();
    }
    // The snapshot is only looked at here; nothing is held across the read.
    let path = state
        .snapshot()
        .monitor
        .row(&clip, &name)
        .map(|r| r.path.clone());
    let Some(path) = path else {
        return missing();
    };
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return missing();
    };
    (
        [(CONTENT_TYPE, "video/mp2t"), (CACHE_CONTROL, "no-store")],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_file_names() {
        assert!(is_clip_name("clip-12-0-r3"));
        for bad in [
            "",
            "..",
            "clip/1",
            "Clip-1",
            "clip_1",
            "a\\b",
            &"a".repeat(65),
        ] {
            assert!(!is_clip_name(bad), "{bad:?}");
        }
        assert!(is_segment_name("seg_00003.ts"));
        assert!(is_segment_name("7-0-3.ts"));
        for bad in [
            "", ".ts", "seg.mp4", "../a.ts", "a/b.ts", "a b.ts", "a.ts\0", "..ts",
        ] {
            assert!(!is_segment_name(bad), "{bad:?}");
        }
    }
}
