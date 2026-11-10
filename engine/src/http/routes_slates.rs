//! The owner's slate background (Settings → Channel): what is set, the upload (only from the
//! window or the home network), its removal and a preview of the idle slate over it.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::json;

use super::{ApiError, ApiResult, AppState, Ctx, json_ok};
use crate::pipeline::slate_look::{
    BACKGROUND_DIR, Background, MAX_MOTION_S, MAX_SIDE, MAX_UPLOAD_BYTES, SlateMedia,
    remove_background, store_background,
};

const ROUTE: &str = "/api/admin/channels/main/slate-background";

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            ROUTE,
            get(info)
                .put(upload)
                .delete(remove)
                // The upload's own limit (the default is far smaller); larger bodies are cut
                // off before they are read whole.
                .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 64 * 1024)),
        )
        .route(&format!("{ROUTE}/preview"), get(preview))
}

/// One upload at a time (they share the upload file).
static UPLOADS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn media(state: &AppState) -> Result<&SlateMedia, ApiError> {
    state
        .slate_media
        .as_deref()
        .ok_or_else(|| ApiError::coded(StatusCode::NOT_FOUND, "slate_background_none", &[]))
}

fn info_json(background: Option<&Background>) -> serde_json::Value {
    json!({
        "background": background,
        "maxBytes": MAX_UPLOAD_BYTES,
        "maxSeconds": MAX_MOTION_S,
        "maxSide": MAX_SIDE,
    })
}

async fn info(State(state): State<AppState>) -> ApiResult {
    let background = state
        .slate_media
        .as_ref()
        .and_then(|m| Background::load(&m.data_dir));
    Ok(json_ok(&info_json(background.as_ref())))
}

/// The slates are made again with the new look (the app and the server without a window).
pub(super) fn refresh_slates(state: &AppState) {
    if let Some(core) = state.app_core() {
        core.refresh_slates();
    } else {
        state.refresh_slates_headless();
    }
}

async fn upload(State(state): State<AppState>, ctx: Ctx, body: Bytes) -> ApiResult {
    // A file only comes from the window or the home network, never through the tunnel.
    if ctx.via_tunnel {
        return Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "slate_background_remote",
            &[],
        ));
    }
    store(&state, &body).await
}

/// Stores `bytes` as the background and makes the slates again; the new state.
async fn store(state: &AppState, bytes: &[u8]) -> ApiResult {
    let media = media(state)?.clone();
    let stored = {
        let _one = UPLOADS.lock().await;
        store_background(&media.tools(), &media.data_dir, bytes).await
    };
    let background = stored.map_err(|e| match e.code() {
        Some(_) => ApiError::from_msg(StatusCode::UNPROCESSABLE_ENTITY, e),
        None => ApiError::Internal(e.to_string()),
    })?;
    refresh_slates(state);
    Ok(json_ok(&info_json(Some(&background))))
}

/// The window's native picker (`POST /api/desktop/slate-background/pick`): the file chosen
/// is stored like an upload; `{"picked": false}` when the owner cancelled.
pub(super) async fn pick(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    media(&state)?;
    let Some(path) = core.pick_slate_background().await else {
        return Ok(json_ok(&json!({ "picked": false })));
    };
    let len = std::fs::metadata(&path)
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .len();
    // Not read at all when larger than an upload may be.
    if len > MAX_UPLOAD_BYTES as u64 {
        return Err(ApiError::from_msg(
            StatusCode::UNPROCESSABLE_ENTITY,
            crate::i18n::Msg::error("slate_background_too_big").arg("max", MAX_UPLOAD_BYTES >> 20),
        ));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    store(&state, &bytes).await
}

async fn remove(State(state): State<AppState>, ctx: Ctx) -> ApiResult {
    if ctx.via_tunnel {
        return Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "slate_background_remote",
            &[],
        ));
    }
    let media = media(&state)?;
    {
        let _one = UPLOADS.lock().await;
        remove_background(&media.data_dir).map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    refresh_slates(&state);
    Ok(json_ok(&info_json(None)))
}

/// A PNG of the idle slate over the background, in the slate language (made on request).
async fn preview(State(state): State<AppState>) -> ApiResult {
    static PREVIEWS: AtomicU64 = AtomicU64::new(0);
    let media = media(&state)?.clone();
    let none = || ApiError::coded(StatusCode::NOT_FOUND, "slate_background_none", &[]);
    let background = Background::load(&media.data_dir).ok_or_else(none)?;
    let n = PREVIEWS.fetch_add(1, Ordering::Relaxed);
    let work = media
        .data_dir
        .join(BACKGROUND_DIR)
        .join(format!("preview-{n}"));
    let png = crate::pipeline::slates::render_preview(
        &media.ffmpeg,
        &media.font_file,
        state.slate_language(),
        &background,
        &work,
    )
    .await;
    let _ = std::fs::remove_dir_all(&work);
    let png = png.map_err(|e| {
        ApiError::from_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            crate::i18n::Msg::error("slate_background_failed").arg_msg("error", e),
        )
    })?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        png,
    )
        .into_response())
}
