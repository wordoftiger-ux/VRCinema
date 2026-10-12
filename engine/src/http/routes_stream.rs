//! The HLS stream: playlist and segments, read from the engine's latest snapshot.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::{ApiError, AppState, Ctx};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/live/main.m3u8", get(playlist))
        .route("/live/main/{name}", get(segment))
}

async fn playlist(State(state): State<AppState>) -> Response {
    let snapshot = state.snapshot();
    (
        [
            (ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (CACHE_CONTROL, "no-cache"),
            (CONTENT_TYPE, "application/vnd.apple.mpegurl"),
        ],
        snapshot.playlist.clone(),
    )
        .into_response()
}

/// `^\d+\.ts$`
fn is_segment_name(name: &str) -> bool {
    name.strip_suffix(".ts")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

async fn segment(State(state): State<AppState>, ctx: Ctx, Path(name): Path<String>) -> Response {
    let missing = || ApiError::coded(StatusCode::NOT_FOUND, "no_segment", &[]).into_response();
    let snapshot = state.snapshot();
    if !is_segment_name(&name) || !snapshot.segments.contains(&name) {
        return missing();
    }
    let Ok(bytes) = tokio::fs::read(snapshot.live_dir.join(&name)).await else {
        return missing();
    };
    // Someone is watching (the app's own preview too): the automatic pause and the stats count this.
    // An address that does not parse (a forged header) is not remembered, the fetch still counts.
    let viewer = ctx.ip.parse().ok();
    state.viewers.record(viewer, bytes.len() as u64);
    let mut res = bytes.into_response();
    let headers = res.headers_mut();
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=60"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("video/mp2t"));
    res
}
