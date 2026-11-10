//! A viewer sends a small video or audio file (`POST /api/channels/main/upload`, the file as
//! the raw body; sound only airs with a picture made from it). Off unless the owner allows it (Settings → Channel). The same nick, block, limit and
//! approval rules as a link; the bytes are counted as they arrive and the file is checked
//! before it is queued.

use std::sync::PoisonError;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::CONTENT_LENGTH;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use super::routes_public::{check_viewer, claim_viewer};
use super::{ApiError, ApiResult, AppState, Ctx, json_response, public_item};
use crate::db::{AddedBy, ItemStatus, NewItem, QueueRepo};
use crate::i18n::Msg;
use crate::resolver::MediaMeta;
use crate::rules::validate_meta;
use crate::uploads::{HARD_MAX_MB, Refusal, SNIFF_BYTES, probe_file, sniff, title_from_name};

/// Each piece of the body must come within this time.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole body.
const BODY_TIMEOUT: Duration = Duration::from_secs(20 * 60);

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/api/channels/main/upload",
        // The route counts the bytes itself; this only keeps the framework's own limit (if it
        // applies to a streamed body) above the largest allowed file.
        post(upload).layer(DefaultBodyLimit::max(
            ((HARD_MAX_MB as usize) << 20) + 1024 * 1024,
        )),
    )
}

/// A header the page percent-encodes (`encodeURIComponent`); `None` when absent or malformed.
fn text_header(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(name)?.to_str().ok()?;
    let pair = format!("v={}", raw.replace('+', "%2B"));
    url::form_urlencoded::parse(pair.as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
}

fn failed() -> ApiError {
    ApiError::coded(StatusCode::BAD_REQUEST, "upload_failed", &[])
}

fn too_big(max_mb: u32) -> ApiError {
    ApiError::coded(
        StatusCode::PAYLOAD_TOO_LARGE,
        "upload_too_big",
        &[("max", &max_mb.to_string())],
    )
}

fn not_video(msg: Msg) -> ApiError {
    ApiError::from_msg(StatusCode::UNSUPPORTED_MEDIA_TYPE, msg)
}

fn uploads_off() -> ApiError {
    ApiError::coded(StatusCode::FORBIDDEN, "uploads_off", &[])
}

async fn upload(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    let settings = state.settings()?;
    let limits = settings.uploads;
    if !limits.viewer_uploads {
        return Err(uploads_off());
    }
    let store = state.uploads.clone().ok_or_else(uploads_off)?;
    let mut who = serde_json::Map::new();
    if let Some(name) = text_header(&headers, "x-viewer-name") {
        who.insert("name".into(), json!(name));
    }
    if let Some(key) = text_header(&headers, "x-device-key") {
        who.insert("deviceKey".into(), json!(key));
    }
    let viewer = check_viewer(&state, &Value::Object(who))?;
    let max = limits.max_bytes();
    let max_mb = limits.viewer_upload_max_mb.clamp(1, HARD_MAX_MB);
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > max) {
        return Err(too_big(max_mb));
    }
    let ip = ctx.ip;
    let active = state.with_db(|c| QueueRepo::new(c).count_active_by_ip(state.channel_id, &ip))?;
    if active >= i64::from(settings.max_active_per_requester) {
        return Err(
            ApiError::coded(StatusCode::TOO_MANY_REQUESTS, "too_many_active", &[])
                .with_count(i64::from(settings.max_active_per_requester)),
        );
    }
    // The room is taken before the limiter counts the request: a refused start costs the
    // viewer nothing.
    let rows = state.with_db(|c| QueueRepo::new(c).upload_rows())?;
    let reserve = declared.unwrap_or(max).min(max);
    let slot = store
        .begin(
            &ip,
            reserve,
            limits.quota_bytes(),
            &rows,
            crate::db::now_ms(),
        )
        .map_err(|r| match r {
            Refusal::Busy => ApiError::coded(StatusCode::TOO_MANY_REQUESTS, "upload_busy", &[]),
            Refusal::NoRoom => {
                ApiError::coded(StatusCode::INSUFFICIENT_STORAGE, "upload_no_room", &[])
            }
        })?;
    let allowed = {
        let mut limiter = state
            .request_limiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        limiter.set_limits(
            settings.request_limit_count,
            Duration::from_secs(settings.request_limit_window_s),
        );
        limiter.hit(&ip)
    };
    if !allowed {
        return Err(ApiError::coded(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            &[],
        ));
    }

    // The bytes, counted as they come; any failure drops the slot, which deletes the part.
    let mut file = tokio::fs::File::create_new(slot.part())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let deadline = tokio::time::Instant::now() + BODY_TIMEOUT;
    let mut stream = body.into_data_stream();
    let mut received: u64 = 0;
    let mut head: Vec<u8> = Vec::with_capacity(SNIFF_BYTES);
    let mut format = None;
    loop {
        let wait =
            CHUNK_TIMEOUT.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        let chunk = match tokio::time::timeout(wait, stream.next()).await {
            Err(_) | Ok(Some(Err(_))) => return Err(failed()),
            Ok(None) => break,
            Ok(Some(Ok(chunk))) => chunk,
        };
        received += chunk.len() as u64;
        if received > max {
            return Err(too_big(max_mb));
        }
        if format.is_none() && head.len() < SNIFF_BYTES {
            let take = (SNIFF_BYTES - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
            if head.len() >= SNIFF_BYTES {
                // Not a video or audio container: refused before the rest is read.
                format =
                    Some(sniff(&head).ok_or_else(|| not_video(Msg::error("upload_not_video")))?);
            }
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    file.flush()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    drop(file);
    let format = match format {
        Some(f) => f,
        None => sniff(&head).ok_or_else(|| not_video(Msg::error("upload_not_video")))?,
    };
    let probed = probe_file(store.ffprobe(), slot.part(), format)
        .await
        .map_err(not_video)?;

    // The owner may have changed the rules while the file came.
    let settings = state.settings()?;
    if !settings.uploads.viewer_uploads {
        return Err(uploads_off());
    }
    // A sound-only file has its own longest length; the channel's maximum applies to both.
    let mut rules = settings.clone();
    rules.max_duration_s = settings
        .max_duration_s
        .min(f64::from(settings.uploads.max_s(probed.audio)));
    let meta = MediaMeta {
        title: text_header(&headers, "x-upload-name")
            .and_then(|n| title_from_name(&n))
            .unwrap_or_else(|| if probed.audio { "Audio" } else { "Video" }.to_string()),
        duration_s: Some(probed.duration_s),
        is_live: false,
        is_playlist: false,
        entries: None,
    };
    if let Some(reason) = validate_meta(&meta, &rules, false) {
        return Err(ApiError::from_msg(StatusCode::BAD_REQUEST, reason));
    }
    claim_viewer(&state, &viewer)?;
    let source_url = slot
        .finish(format, probed.audio)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let item = state.with_db(|c| {
        QueueRepo::new(c).add(&NewItem {
            channel_id: state.channel_id,
            source_url,
            title: meta.title.clone(),
            duration_s: meta.duration_s,
            is_live: false,
            status: if settings.moderation {
                ItemStatus::PendingApproval
            } else {
                ItemStatus::Queued
            },
            added_by: AddedBy::Public,
            requester_name: viewer.nick.clone(),
            requester_ip: Some(ip.clone()),
            requester_key_hash: viewer.key_hash.clone(),
        })
    })?;
    (state.log)(
        Msg::new("log.upload_received")
            .arg("viewer", state.viewer_tag(&ip))
            .arg("title", &item.title)
            .arg("mb", format!("{:.1}", received as f64 / 1048576.0))
            .arg_msg("status", item.status.msg()),
    );
    Ok(json_response(
        StatusCode::CREATED,
        &json!({ "status": item.status.as_str(), "item": public_item(&item) }),
    ))
}
