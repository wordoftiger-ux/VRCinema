//! The pictures of the panel's lists (behind the admin session): `GET
//! /api/admin/items/{id}/thumb` answers a small JPEG frame of a film of the owner's disk
//! (`local:`, `file:`) or of a file a viewer sent (`upload:`), made once by ffmpeg and kept in
//! `<DATA_DIR>/thumbs` (a film that gives no frame is remembered too, and answers 404). Films
//! from the internet have the site's picture instead ([`crate::db::thumbs`]), loaded by the
//! browser itself.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::Value;

use super::{ApiError, ApiResult, AppState};
use crate::db::{QueueItem, QueueRepo};
use crate::preview_source::is_disk_source;
use crate::uploads::is_upload_source;

/// Width of a frame (the lists show 64×36; twice that for sharp screens).
const WIDTH: u32 = 192;
/// Frames made at once (each is one short ffmpeg run).
const PARALLEL: usize = 2;
/// The longest one frame may take.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Where the frames are kept and the tool that makes them.
pub struct ThumbCache {
    dir: PathBuf,
    ffmpeg: PathBuf,
    work: tokio::sync::Semaphore,
}

impl ThumbCache {
    pub fn new(dir: PathBuf, ffmpeg: PathBuf) -> Self {
        Self {
            dir,
            ffmpeg,
            work: tokio::sync::Semaphore::new(PARALLEL),
        }
    }

    /// The file of the frame of `source` (and of the mark that it gives none).
    fn files(&self, source: &str) -> (PathBuf, PathBuf) {
        let key = key_of(source);
        (
            self.dir.join(format!("{key}.jpg")),
            self.dir.join(format!("{key}.none")),
        )
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/api/admin/items/{id}/thumb", get(thumb))
}

/// The film a frame can be made of: a file of the owner's disk or a viewer's sent file.
pub(crate) fn has_own_frame(source: &str) -> bool {
    is_disk_source(source) || is_upload_source(source)
}

/// The picture of an item for the lists: the frame route, the site's picture kept for it, or
/// YouTube's picture of its link; `None`: the coloured stand-in.
pub(crate) fn thumb_of(item: &QueueItem, kept: Option<&String>) -> Option<String> {
    if has_own_frame(&item.source_url) {
        return Some(format!("/api/admin/items/{}/thumb", item.id));
    }
    kept.cloned()
        .or_else(|| crate::db::youtube_thumbnail(&item.source_url))
}

/// Adds `thumb` to the JSON of `items` (as listed, same order).
pub(crate) fn add_thumbs(state: &AppState, items: &[QueueItem], json: &mut Value) {
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let kept = state
        .with_db(|c| crate::db::thumbnails(c, &ids))
        .unwrap_or_default();
    let Value::Array(rows) = json else { return };
    for (item, row) in items.iter().zip(rows.iter_mut()) {
        if let Value::Object(map) = row {
            map.insert(
                "thumb".into(),
                serde_json::json!(thumb_of(item, kept.get(&item.id))),
            );
        }
    }
}

fn key_of(source: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(source.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn none() -> ApiError {
    ApiError::coded(StatusCode::NOT_FOUND, "no_such_item", &[])
}

async fn thumb(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult {
    let id: i64 = id.trim().parse().map_err(|_| none())?;
    let cache = state.thumbs.clone().ok_or_else(none)?;
    let item = state
        .with_db(|c| QueueRepo::new(c).get(id))?
        .ok_or_else(none)?;
    if !has_own_frame(&item.source_url) {
        return Err(none());
    }
    let (jpg, mark) = cache.files(&item.source_url);
    if !jpg.is_file() && !mark.is_file() {
        let _one = cache.work.acquire().await.map_err(|_| none())?;
        // Made meanwhile by another request.
        if !jpg.is_file() && !mark.is_file() {
            let made = make(&state, &cache, &item, &jpg).await;
            if !made {
                let _ = std::fs::create_dir_all(&cache.dir);
                let _ = std::fs::write(&mark, b"");
            }
        }
    }
    let bytes = std::fs::read(&jpg).map_err(|_| none())?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        bytes,
    )
        .into_response())
}

/// Makes the frame of `item` into `out`: a few seconds in (the very start is often black),
/// else the first frame (a short film, the cover of an audio file).
async fn make(state: &AppState, cache: &ThumbCache, item: &QueueItem, out: &Path) -> bool {
    // The resolvers of disk and sent files only check the file (no probe): quick.
    let Ok(stream) = state.resolver.resolve(&item.source_url).await else {
        return false;
    };
    let Some(file) = stream.inputs.first().map(|i| PathBuf::from(&i.url)) else {
        return false;
    };
    if !file.is_file() || std::fs::create_dir_all(&cache.dir).is_err() {
        return false;
    }
    let at = item
        .duration_s
        .filter(|d| d.is_finite() && *d > 0.0)
        .map_or(3.0, |d| (d / 4.0).min(5.0));
    for seek in [Some(at), None] {
        if frame(&cache.ffmpeg, &file, seek, out).await {
            return true;
        }
    }
    false
}

async fn frame(ffmpeg: &Path, file: &Path, seek: Option<f64>, out: &Path) -> bool {
    let tmp = out.with_extension("part.jpg");
    let mut args: Vec<OsString> = ["-v", "error", "-y"].iter().map(OsString::from).collect();
    if let Some(s) = seek {
        args.push("-ss".into());
        args.push(format!("{s:.2}").into());
    }
    args.push("-i".into());
    args.push(file.as_os_str().to_owned());
    for a in [
        "-an",
        "-sn",
        "-dn",
        "-frames:v",
        "1",
        "-vf",
        &format!("scale={WIDTH}:-2"),
        "-q:v",
        "5",
        "-f",
        "image2",
    ] {
        args.push(a.into());
    }
    args.push(tmp.as_os_str().to_owned());
    let _ = std::fs::remove_file(&tmp);
    let ran = crate::pipeline::slate_look::run_tool(ffmpeg, args, None, TIMEOUT).await;
    let ok = ran.is_ok() && std::fs::metadata(&tmp).is_ok_and(|m| m.len() > 0);
    if ok {
        return std::fs::rename(&tmp, out).is_ok();
    }
    let _ = std::fs::remove_file(&tmp);
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AddedBy, ItemStatus};

    fn item(id: i64, source: &str) -> QueueItem {
        QueueItem {
            id,
            channel_id: 1,
            position: 0,
            source_url: source.into(),
            title: "t".into(),
            duration_s: Some(60.0),
            is_live: false,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            requester_key_hash: None,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
            in_s: None,
            out_s: None,
            start_at: None,
        }
    }

    #[test]
    fn the_picture_of_an_item_is_its_frame_its_kept_picture_or_youtubes() {
        let kept = "https://cdn.example/p.jpg".to_string();
        assert_eq!(
            thumb_of(&item(3, "local:0/a.mp4"), None).as_deref(),
            Some("/api/admin/items/3/thumb")
        );
        assert_eq!(
            thumb_of(&item(4, "upload:0123.mp4"), Some(&kept)).as_deref(),
            Some("/api/admin/items/4/thumb")
        );
        assert_eq!(
            thumb_of(&item(5, "https://vimeo.com/1"), Some(&kept)),
            Some(kept.clone())
        );
        assert_eq!(
            thumb_of(&item(6, "https://youtu.be/dQw4w9WgXcQ"), None).as_deref(),
            Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg")
        );
        assert_eq!(thumb_of(&item(7, "https://vimeo.com/1"), None), None);
        assert_ne!(key_of("local:0/a.mp4"), key_of("local:0/b.mp4"));
        assert_eq!(key_of("x").len(), 32);
    }

    #[tokio::test]
    async fn a_frame_is_made_once_from_a_film_file() {
        let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bin/ffmpeg.exe");
        if !ffmpeg.is_file() {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let film = dir.path().join("film.mp4");
        let made = std::process::Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=320x240:r=10:d=2",
            ])
            .arg(&film)
            .status()
            .unwrap();
        assert!(made.success());
        let out = dir.path().join("t.jpg");
        // Past the end: no frame; from the start: one.
        assert!(!frame(&ffmpeg, &film, Some(30.0), &out).await);
        assert!(frame(&ffmpeg, &film, None, &out).await);
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..3], b"\xff\xd8\xff", "a JPEG");
    }
}
