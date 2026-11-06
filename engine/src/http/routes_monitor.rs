//! The producer's monitor for the program's window (spec `2026-10-21-monitor-design.md`): the
//! HLS list of the newest clip segments and the segments themselves, read from the engine's
//! latest snapshot ([`crate::pipeline::monitor`]). Window only: no session is 401, a remote
//! session 403 ([`super::require_window`]); never through the tunnel to viewers.

use std::collections::HashMap;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{ApiError, AppState};
use crate::db::ItemStatus;
use crate::preview_source::{PreviewStatus, Unsatisfiable, byte_range, content_type};

pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/desktop/monitor.m3u8", get(playlist))
        .route("/api/desktop/monitor/{clip}/{name}", get(segment))
        .route("/api/desktop/preview", get(preview_status))
        .route("/api/desktop/preview/{item}/media", get(preview_media))
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

/// What the Preview can play for `?item=<id>` (spec `2026-11-06-preview-player-design.md`):
/// `{itemId, state: ready|preparing|unavailable, reason?, url?}`. A film from the internet is
/// downloaded on the first ask; the window asks again while it is `preparing`.
async fn preview_status(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let no_item = || ApiError::coded(StatusCode::NOT_FOUND, "no_such_item", &[]).into_response();
    let Some(id) = query.get("item").and_then(|s| s.parse::<i64>().ok()) else {
        return no_item();
    };
    let item = match state.with_db(|c| crate::db::QueueRepo::new(c).get(id)) {
        // Only films that will air (not requests waiting for approval, nor the history).
        Ok(Some(item)) if matches!(item.status, ItemStatus::Playing | ItemStatus::Queued) => item,
        Ok(Some(_)) => return no_item(),
        Ok(None) => return no_item(),
        Err(e) => return e.into_response(),
    };
    let status = match &state.preview {
        Some(preview) => preview.status(&item).await,
        None => PreviewStatus::Unavailable {
            reason: "unsupported",
        },
    };
    let mut body = serde_json::to_value(&status).unwrap_or_default();
    if let Some(map) = body.as_object_mut() {
        map.insert("itemId".into(), id.into());
        if status == PreviewStatus::Ready {
            map.insert(
                "url".into(),
                format!("/api/desktop/preview/{id}/media").into(),
            );
        }
    }
    ([(CACHE_CONTROL, "no-store")], axum::Json(body)).into_response()
}

/// The Preview's film itself, with `Range` (the window's `<video>` seeks in it).
async fn preview_media(
    State(state): State<AppState>,
    Path(item): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let missing = || ApiError::coded(StatusCode::NOT_FOUND, "no_such_item", &[]).into_response();
    let Some(path) = state.preview.as_ref().and_then(|p| p.file(item)) else {
        return missing();
    };
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return missing();
    };
    let Ok(len) = file.metadata().await.map(|m| m.len()) else {
        return missing();
    };
    let kind = content_type(&path);
    let range = byte_range(headers.get(RANGE).and_then(|v| v.to_str().ok()), len);
    let (status, first, last) = match range {
        Err(Unsatisfiable) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(CONTENT_RANGE, format!("bytes */{len}"))],
            )
                .into_response();
        }
        Ok(None) if len == 0 => {
            return (
                [(CONTENT_TYPE, kind), (CACHE_CONTROL, "no-store")],
                Vec::<u8>::new(),
            )
                .into_response();
        }
        Ok(None) => (StatusCode::OK, 0, len - 1),
        Ok(Some((first, last))) => (StatusCode::PARTIAL_CONTENT, first, last),
    };
    if first > 0 && file.seek(std::io::SeekFrom::Start(first)).await.is_err() {
        return missing();
    }
    let count = last - first + 1;
    let body = Body::from_stream(chunks(file.take(count)));
    let mut res = (status, body).into_response();
    let h = res.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static(kind));
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(CONTENT_LENGTH, HeaderValue::from(count));
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(v) = HeaderValue::from_str(&format!("bytes {first}-{last}/{len}"))
    {
        h.insert(CONTENT_RANGE, v);
    }
    res
}

/// The bytes of `reader` in pieces of up to 256 KiB.
fn chunks(
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> + Send {
    futures_util::stream::unfold(reader, |mut reader| async move {
        let mut buf = vec![0u8; 256 * 1024];
        match reader.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(buf), reader))
            }
            Err(e) => Some((Err(e), reader)),
        }
    })
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
