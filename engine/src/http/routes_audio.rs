//! The window's Preview of an audio-only film (spec `2026-11-08-audio-design.md`, section 4):
//! the title, the artist and the cover the clip factory found for its picture on air, read
//! from the item's newest clip folder ([`crate::pipeline::audio_visual::META_FILE`],
//! [`crate::pipeline::audio_visual::COVER_FILE`]). Window only, like the Preview itself.

use std::path::{Path as FsPath, PathBuf};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::Value;

use super::{ApiError, AppState};
use crate::pipeline::audio_visual::{COVER_FILE, META_FILE};

pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/desktop/audio/{item}", get(meta))
        .route("/api/desktop/audio/{item}/cover", get(cover))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            super::require_window,
        ))
}

/// The clip folders (`<data>/clips`, next to the slates' data).
fn clips_dir(state: &AppState) -> Option<PathBuf> {
    state.slate_media.as_ref().map(|m| m.data_dir.join("clips"))
}

/// The newest clip folder of `item` (`<id>-<attempt>-<n>`) with an audio picture.
pub(super) fn newest_clip(clips: &FsPath, item: i64) -> Option<PathBuf> {
    let prefix = format!("{item}-");
    std::fs::read_dir(clips)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix(&prefix).is_some_and(|rest| {
                rest.split('-').count() == 2
                    && rest.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            }) && e.path().join(META_FILE).is_file()
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

fn no_item() -> Response {
    ApiError::coded(StatusCode::NOT_FOUND, "no_such_item", &[]).into_response()
}

/// `{itemId, title, artist, cover}` (`cover`: the picture's address, or null); 404 while the
/// item has no audio picture (not on air yet, or a video).
async fn meta(State(state): State<AppState>, Path(item): Path<i64>) -> Response {
    let Some(dir) = clips_dir(&state).and_then(|c| newest_clip(&c, item)) else {
        return no_item();
    };
    let Ok(text) = tokio::fs::read_to_string(dir.join(META_FILE)).await else {
        return no_item();
    };
    let stored: Value = serde_json::from_str(&text).unwrap_or_default();
    let text_of = |key: &str| stored.get(key).and_then(Value::as_str).unwrap_or_default();
    let cover = dir
        .join(COVER_FILE)
        .is_file()
        .then(|| format!("/api/desktop/audio/{item}/cover"));
    let body = serde_json::json!({
        "itemId": item,
        "title": text_of("title"),
        "artist": text_of("artist"),
        "cover": cover,
    });
    ([(CACHE_CONTROL, "no-store")], axum::Json(body)).into_response()
}

/// The cover of the item's audio picture (PNG).
async fn cover(State(state): State<AppState>, Path(item): Path<i64>) -> Response {
    let Some(dir) = clips_dir(&state).and_then(|c| newest_clip(&c, item)) else {
        return no_item();
    };
    match tokio::fs::read(dir.join(COVER_FILE)).await {
        Ok(bytes) => (
            [(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")],
            bytes,
        )
            .into_response(),
        Err(_) => no_item(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_audio_clip_of_the_item_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let clip = |name: &str, meta: bool| {
            let p = dir.path().join(name);
            std::fs::create_dir_all(&p).unwrap();
            if meta {
                std::fs::write(p.join(META_FILE), "{}").unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        clip("12-0-1", true);
        clip("12-1-2", true);
        clip("12-1-2-plain", true);
        clip("123-0-3", true);
        clip("12-0-4", false);
        clip("countdown-12-5", true);
        assert_eq!(newest_clip(dir.path(), 12), Some(dir.path().join("12-1-2")));
        assert_eq!(newest_clip(dir.path(), 7), None);
        assert_eq!(newest_clip(&dir.path().join("none"), 12), None);
    }
}
