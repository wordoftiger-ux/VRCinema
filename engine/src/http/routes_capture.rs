//! The owner's live capture of this computer (spec `2026-11-08-capture-design.md`): what can be
//! captured, a preview frame, adding a capture to the queue and Stop. Only the program's own
//! window may use them: its session, from this computer (a loopback peer), not through the
//! tunnel or a proxy. A remote panel, the tunnel and viewers get 403 (`window_only`), no session
//! 401. The remote panel has no such routes at all.

use std::net::SocketAddr;

use axum::Router;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use super::{
    ApiError, ApiResult, AppState, DesktopSession, JsonBody, ReqLang, json_ok, json_response,
};
use crate::capture::{
    CaptureRegistry, CaptureSpec, SOURCE_PREFIX, SpecError, Video, is_capture_source, list,
    parse_spec, redact, sensitive_texts,
};
use crate::db::{AddedBy, ItemStatus, NewItem, QueueRepo};
use crate::engine::Command;
use crate::i18n::{Msg, t};

/// The longest title viewers see for a capture.
const MAX_TITLE_CHARS: usize = 120;

/// Headers a request passed on by a proxy or by cloudflared carries; the window never sends them.
const FORWARDED: [&str; 4] = [
    "cf-connecting-ip",
    "x-forwarded-for",
    "x-real-ip",
    "forwarded",
];

pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/desktop/capture/sources", get(sources))
        .route("/api/desktop/capture/preview", post(preview))
        .route("/api/desktop/capture", post(add))
        .route("/api/desktop/capture/stop", post(stop))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            require_local_window,
        ))
}

/// The request comes from this computer itself and was not passed on by anything.
pub(super) fn from_this_computer(peer: Option<SocketAddr>, headers: &HeaderMap) -> bool {
    peer.is_some_and(|p| p.ip().is_loopback())
        && FORWARDED.iter().all(|name| !headers.contains_key(*name))
}

/// The window's session ([`DesktopSession`]) on a request from this computer (see
/// [`from_this_computer`]); anything else is refused before the body is read.
async fn require_local_window(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    if let Err(e) = DesktopSession::from_request_parts(&mut parts, &state).await {
        return e.into_response();
    }
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    if !from_this_computer(peer, &parts.headers) {
        return ApiError::coded(StatusCode::FORBIDDEN, "window_only", &[]).into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}

fn registry(state: &AppState) -> Result<&CaptureRegistry, ApiError> {
    state
        .capture
        .as_deref()
        .ok_or_else(|| ApiError::coded(StatusCode::NOT_FOUND, "capture_unavailable", &[]))
}

/// What can be captured now (kept as the list a new capture is checked against): `{platform,
/// desktop, region, screens: [{index, x, y, width, height, primary}], windows: [title],
/// cameras: [{id, name}], audio: [{id, name}], fast, notes: [code]}`.
async fn sources(State(state): State<AppState>) -> ApiResult {
    let registry = registry(&state)?;
    let catalog = list::catalog(registry.ffmpeg()).await;
    registry.set_catalog(catalog.clone());
    let mut res = json_ok(&serde_json::to_value(&catalog).unwrap_or(Value::Null));
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

fn spec_error(e: SpecError) -> ApiError {
    let status = match e {
        SpecError::Invalid => StatusCode::BAD_REQUEST,
        SpecError::UnknownSource => StatusCode::CONFLICT,
    };
    ApiError::coded(status, e.code(), &[])
}

/// The capture the body asks for, checked against the last listing.
fn checked_spec(registry: &CaptureRegistry, body: &Value) -> Result<CaptureSpec, ApiError> {
    let catalog = registry
        .catalog()
        .ok_or_else(|| spec_error(SpecError::UnknownSource))?;
    parse_spec(body, &catalog).map_err(spec_error)
}

/// One frame of the picture the body asks for (as for adding), a JPEG never stored.
async fn preview(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let registry = registry(&state)?;
    let spec = checked_spec(registry, &body)?;
    if spec.video == Video::None {
        return Err(ApiError::bad_request("capture_invalid"));
    }
    match list::grab_preview(registry.ffmpeg(), &spec).await {
        Ok(jpeg) => Ok((
            [
                (CONTENT_TYPE, HeaderValue::from_static("image/jpeg")),
                (CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
            jpeg,
        )
            .into_response()),
        Err(detail) => Err(ApiError::coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "capture_preview_failed",
            &[("detail", &redact(&detail, &sensitive_texts(&spec)))],
        )),
    }
}

/// The capture the body asks for (see [`crate::capture::parse_spec`]) and `title`, the text
/// viewers see (the default "Live" in the program's language), goes to the end of the queue as
/// a live item. Answers 201 `{itemId, title}`.
async fn add(
    State(state): State<AppState>,
    ReqLang(lang): ReqLang,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let registry = registry(&state)?;
    let spec = checked_spec(registry, &body)?;
    let title = match body.get("title") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.split_whitespace().collect::<Vec<_>>().join(" "),
        Some(_) => return Err(ApiError::bad_request("capture_invalid")),
    };
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(ApiError::bad_request("capture_invalid"));
    }
    let title = if title.is_empty() {
        t(lang, "capture.default_title", &[])
    } else {
        title
    };
    let kind = spec.video.kind_key();
    let token = registry.register(spec);
    let added = state.with_db(|c| {
        QueueRepo::new(c).add(&NewItem {
            channel_id: state.channel_id,
            source_url: format!("{SOURCE_PREFIX}{token}"),
            title: title.clone(),
            duration_s: None,
            is_live: true,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            requester_key_hash: None,
        })
    });
    let item = match added {
        Ok(item) => item,
        Err(e) => {
            registry.forget(&token);
            return Err(e);
        }
    };
    registry.bind(&token, item.id);
    (state.log)(
        Msg::new("log.capture_added")
            .arg("id", item.id)
            .arg_msg("kind", Msg::new(kind)),
    );
    state.send_command(Command::QueueChanged);
    Ok(json_response(
        StatusCode::CREATED,
        &json!({ "itemId": item.id, "title": item.title }),
    ))
}

/// Stop: every capture ends (a running one within a quarter of a second, ending as played; what
/// it made still airs) and the captures waiting in the queue are removed. Answers `{stopped,
/// removed}`.
async fn stop(State(state): State<AppState>) -> ApiResult {
    let registry = registry(&state)?;
    let stopped = registry.stop_all();
    let channel_id = state.channel_id;
    let removed = state.with_db(|c| {
        let q = QueueRepo::new(c);
        let waiting: Vec<i64> = q
            .list_by_status(channel_id, &[ItemStatus::Queued])?
            .into_iter()
            .filter(|i| is_capture_source(&i.source_url))
            .map(|i| i.id)
            .collect();
        for id in &waiting {
            q.set_status(*id, ItemStatus::Removed, None)?;
        }
        Ok(waiting.len())
    })?;
    (state.log)(Msg::new("log.capture_stopped").arg("count", stopped));
    if removed > 0 {
        state.send_command(Command::QueueChanged);
    }
    Ok(json_ok(&json!({ "stopped": stopped, "removed": removed })))
}

/// The window's info about captures: `{running, queued}` (items whose capture runs now, and
/// captures waiting); `None` on a server without captures.
pub(super) fn capture_info(state: &AppState) -> Option<Value> {
    let registry = state.capture.as_deref()?;
    let running = registry.running();
    let queued = state
        .with_db(|c| QueueRepo::new(c).list_by_status(state.channel_id, &[ItemStatus::Queued]))
        .map_or(0, |items| {
            items
                .iter()
                .filter(|i| is_capture_source(&i.source_url) && !running.contains(&i.id))
                .count()
        });
    Some(json!({ "running": running, "queued": queued }))
}
