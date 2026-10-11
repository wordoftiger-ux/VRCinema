//! Admin API (behind the session cookie): panel state, queue management, disk files, settings.

use std::sync::PoisonError;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, patch, post};
use serde_json::{Value, json};

use super::{
    ApiError, ApiResult, AppState, Ctx, JsonBody, admin_item, control_json, json_ok, json_response,
    state_map,
};
use crate::db::{
    AddedBy, ItemStatus, NewItem, QueueItem, QueueRepo, SettingsRepo, sanitize_settings_patch,
};
use crate::engine::Command;
use crate::library::LibraryError;
use crate::pipeline::channel::{ControlState, PauseReason};
use crate::rules::parse_source_url;
use crate::stats::process_usage;

const MAX_PLAYLIST_ITEMS: usize = 200;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/channels/main", get(panel))
        .route("/api/admin/channels/main/items", post(add_item))
        .route(
            "/api/admin/items/{id}",
            patch(patch_item).delete(delete_item),
        )
        .route("/api/admin/library", get(library))
        .route("/api/admin/channels/main/local", post(add_local))
        .route("/api/admin/channels/main/skip", post(skip))
        .route("/api/admin/channels/main/pause", post(pause))
        .route("/api/admin/channels/main/resume", post(resume))
        .route("/api/admin/channels/main/seek", post(seek))
        .route("/api/admin/channels/main/restart", post(restart))
        .route("/api/admin/channels/main/stats", get(stats))
        .route("/api/admin/channels/main/play-now", post(play_now))
        .route(
            "/api/admin/channels/main/settings",
            get(get_settings).put(put_settings),
        )
}

/// Runs before body parsing, like Fastify's `onRequest` hook.
pub(super) async fn require_admin(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if !state.is_admin(req.headers()) {
        return axum::response::IntoResponse::into_response(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Zaloguj się.",
        ));
    }
    next.run(req).await
}

fn ok() -> Response {
    json_ok(&json!({ "ok": true }))
}

fn items_json(items: &[QueueItem]) -> Value {
    Value::Array(items.iter().map(admin_item).collect())
}

async fn panel(State(state): State<AppState>, ctx: Ctx) -> ApiResult {
    let id = state.channel_id;
    let (playing, queue, pending, history) = state.with_db(|c| {
        let q = QueueRepo::new(c);
        Ok((
            q.list_by_status(id, &[ItemStatus::Playing])?,
            q.list_by_status(id, &[ItemStatus::Queued])?,
            q.list_by_status(id, &[ItemStatus::PendingApproval])?,
            q.history(id, 50)?,
        ))
    })?;
    let settings = state.settings()?;
    let snapshot = state.snapshot();
    let mut body = state_map(&snapshot.state);
    body.insert(
        "prefetchedItemId".into(),
        json!(snapshot.prefetched_item_id),
    );
    body.insert("playing".into(), items_json(&playing));
    body.insert("queue".into(), items_json(&queue));
    body.insert("pending".into(), items_json(&pending));
    body.insert("history".into(), items_json(&history));
    body.insert("settings".into(), json!(settings));
    body.insert("control".into(), control_json(&snapshot.state));
    body.insert(
        "channelUrl".into(),
        json!(format!("{}/live/main.m3u8", ctx.base_url)),
    );
    body.insert("requestUrl".into(), json!(format!("{}/", ctx.base_url)));
    Ok(json_ok(&Value::Object(body)))
}

async fn add_item(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let url = body
        .get("url")
        .and_then(Value::as_str)
        .and_then(parse_source_url)
        .ok_or_else(|| ApiError::bad_request("To nie wygląda na poprawny link (http/https)."))?;
    let meta = state
        .resolver
        .fetch_meta(url.as_str())
        .await
        .map_err(|e| ApiError::bad_request(format!("Nie udało się odczytać: {e}")))?;
    let new_item =
        |source_url: String, title: String, duration_s: Option<f64>, is_live: bool| NewItem {
            channel_id: state.channel_id,
            source_url,
            title,
            duration_s,
            is_live,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
        };
    let added = if meta.is_playlist {
        // Only http(s) entries: `file:` (and `local:`) items are never created through HTTP,
        // whatever a playlist extractor returns.
        let entries: Vec<_> = meta
            .entries
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|e| parse_source_url(&e.url).is_some())
            .take(MAX_PLAYLIST_ITEMS)
            .collect();
        if entries.is_empty() {
            return Err(ApiError::bad_request("Playlista jest pusta."));
        }
        state.with_db(|c| {
            let q = QueueRepo::new(c);
            for e in &entries {
                q.add(&new_item(
                    e.url.clone(),
                    e.title.clone(),
                    e.duration_s,
                    false,
                ))?;
            }
            Ok(())
        })?;
        entries.len()
    } else {
        state.with_db(|c| {
            QueueRepo::new(c)
                .add(&new_item(
                    url.to_string(),
                    meta.title.clone(),
                    meta.duration_s,
                    meta.is_live,
                ))
                .map(|_| ())
        })?;
        1
    };
    state.send_command(Command::QueueChanged);
    Ok(json_response(
        StatusCode::CREATED,
        &json!({ "added": added, "title": meta.title }),
    ))
}

/// `Number(id)` is NaN for anything but a number; ids are integers, so only those can match.
fn parse_id(raw: &str) -> Option<i64> {
    raw.trim().parse().ok()
}

fn no_such_item() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "Nie ma takiej pozycji.")
}

/// A non-negative whole number (JS `Number.isInteger(x) && x >= 0`).
fn position_of(v: Option<&Value>) -> Option<usize> {
    let n = v?.as_f64()?;
    (n.is_finite() && n.fract() == 0.0 && n >= 0.0).then_some(n as usize)
}

async fn patch_item(
    State(state): State<AppState>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let id = parse_id(&id).ok_or_else(no_such_item)?;
    let action = body.get("action").and_then(Value::as_str);
    let position = position_of(body.get("position"));
    // Lookup, checks and update under one lock, so nothing changes in between.
    state.with_db(|c| {
        let q = QueueRepo::new(c);
        let Some(item) = q.get(id)?.filter(|i| i.channel_id == state.channel_id) else {
            return Ok(Err(no_such_item()));
        };
        match (action, position) {
            (Some(action @ ("approve" | "reject")), _) => {
                if item.status != ItemStatus::PendingApproval {
                    return Ok(Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "Ta pozycja nie czeka na akceptację.",
                    )));
                }
                let status = if action == "approve" {
                    ItemStatus::Queued
                } else {
                    ItemStatus::Rejected
                };
                q.set_status(item.id, status, None)?;
            }
            (_, Some(position)) => {
                if item.status != ItemStatus::Queued {
                    return Ok(Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "Przesuwać można tylko pozycje w kolejce.",
                    )));
                }
                q.move_item(item.id, position)?;
            }
            _ => {
                return Ok(Err(ApiError::bad_request(
                    "Podaj action (approve/reject) albo position.",
                )));
            }
        }
        Ok(Ok(()))
    })??;
    state.send_command(Command::QueueChanged);
    Ok(ok())
}

async fn delete_item(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let id = parse_id(&id).ok_or_else(no_such_item)?;
    state.with_db(|c| {
        let q = QueueRepo::new(c);
        let Some(item) = q.get(id)?.filter(|i| i.channel_id == state.channel_id) else {
            return Ok(Err(no_such_item()));
        };
        if !matches!(
            item.status,
            ItemStatus::Queued | ItemStatus::PendingApproval | ItemStatus::Playing
        ) {
            return Ok(Err(ApiError::new(
                StatusCode::CONFLICT,
                "Ta pozycja już się zakończyła.",
            )));
        }
        q.set_status(item.id, ItemStatus::Removed, None)?;
        Ok(Ok(()))
    })??;
    state.send_command(Command::QueueChanged);
    Ok(ok())
}

// ---------- Pliki z dysku ----------

fn library_error(e: LibraryError) -> ApiError {
    ApiError::bad_request(e.0)
}

/// `Number(x)` used as a folder index: `None` when it is not a non-negative whole number.
fn index_of_number(n: f64) -> Option<usize> {
    (n.is_finite() && n.fract() == 0.0 && n >= 0.0).then_some(n as usize)
}

fn index_of_str(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.is_empty() {
        return Some(0);
    }
    s.parse::<f64>().ok().and_then(index_of_number)
}

fn index_of_json(v: Option<&Value>) -> Option<usize> {
    match v? {
        Value::Null => Some(0),
        Value::Bool(b) => Some(usize::from(*b)),
        Value::Number(n) => n.as_f64().and_then(index_of_number),
        Value::String(s) => index_of_str(s),
        _ => None,
    }
}

async fn library(
    State(state): State<AppState>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> ApiResult {
    let Some(lib) = state.library.clone().filter(|l| l.enabled()) else {
        return Ok(json_ok(&json!({ "enabled": false, "roots": [] })));
    };
    let params: Vec<(String, String)> =
        url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    let param = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let root = param("root");
    let path = param("path").unwrap_or_default();
    let listing = tokio::task::spawn_blocking(move || {
        let Some(root) = root.as_deref().map_or(Some(0), index_of_str) else {
            return Err(LibraryError("Nie ma takiego folderu głównego.".to_string()));
        };
        lib.list(root, &path).map(|listing| (lib.roots(), listing))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let (roots, listing) = listing.map_err(library_error)?;
    let mut body = serde_json::Map::new();
    body.insert("enabled".into(), json!(true));
    body.insert(
        "roots".into(),
        Value::Array(
            roots
                .iter()
                .map(|r| json!({ "index": r.index, "name": r.name }))
                .collect(),
        ),
    );
    if let Value::Object(fields) = json!(listing) {
        body.extend(fields);
    }
    Ok(json_ok(&Value::Object(body)))
}

async fn add_local(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let Some(lib) = state.library.clone().filter(|l| l.enabled()) else {
        return Err(ApiError::bad_request(
            "Pliki z dysku są wyłączone. Ustaw MEDIA_DIRS w pliku .env.",
        ));
    };
    let rel = body
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let root = index_of_json(body.get("root"));
    let (root, files) = {
        let lib = lib.clone();
        let rel = rel.clone();
        tokio::task::spawn_blocking(move || -> ApiResult<(usize, Vec<String>)> {
            let root = root.ok_or_else(|| {
                library_error(LibraryError("Nie ma takiego folderu głównego.".to_string()))
            })?;
            let full = lib.resolve(root, &rel).map_err(library_error)?;
            let is_dir = std::fs::metadata(&full)
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .is_dir();
            let files = if is_dir {
                lib.videos_in(root, &rel)
                    .map_err(library_error)?
                    .into_iter()
                    .map(|e| e.path)
                    .collect()
            } else {
                vec![rel]
            };
            Ok((root, files))
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))??
    };
    if files.is_empty() {
        return Err(ApiError::bad_request("W tym folderze nie ma plików wideo."));
    }
    let mut titles = Vec::new();
    for file in files.iter().take(MAX_PLAYLIST_ITEMS) {
        let source_url = lib.to_source_url(root, file);
        let meta = state
            .resolver
            .fetch_meta(&source_url)
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        state.with_db(|c| {
            QueueRepo::new(c).add(&NewItem {
                channel_id: state.channel_id,
                source_url,
                title: meta.title.clone(),
                duration_s: meta.duration_s,
                is_live: false,
                status: ItemStatus::Queued,
                added_by: AddedBy::Admin,
                requester_name: None,
                requester_ip: None,
            })
        })?;
        titles.push(meta.title);
    }
    state.send_command(Command::QueueChanged);
    let title = if titles.len() == 1 {
        titles.remove(0)
    } else {
        rel.rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("folder")
            .to_string()
    };
    Ok(json_response(
        StatusCode::CREATED,
        &json!({ "added": files.len().min(MAX_PLAYLIST_ITEMS), "title": title }),
    ))
}

async fn skip(State(state): State<AppState>) -> Response {
    state.send_command(Command::Skip);
    ok()
}

// ---------- Sterowanie kanałem ----------

/// How long a control route waits for the engine loop to apply its command.
const CONTROL_WAIT: Duration = Duration::from_secs(1);

fn control_bad_request(message: &str) -> ApiError {
    ApiError::with_code(StatusCode::BAD_REQUEST, "bad_request", message)
}

/// The film the controls act on, when it can be paused and seeked; otherwise 409.
fn controllable_film(state: &AppState) -> ApiResult<ControlState> {
    let control = state.snapshot().state.control.clone();
    if control.item_id.is_none() {
        return Err(ApiError::with_code(
            StatusCode::CONFLICT,
            "nothing_playing",
            "Teraz nic nie leci.",
        ));
    }
    if control.is_live {
        return Err(ApiError::with_code(
            StatusCode::CONFLICT,
            "live_item",
            "Transmisji na żywo nie można wstrzymać ani przewinąć.",
        ));
    }
    Ok(control)
}

/// Sends `command` and answers with the control state once the loop has applied it: it drains
/// commands, ticks and then publishes, so the second snapshot after sending has it. After
/// [`CONTROL_WAIT`] the latest state is returned anyway. A loop that cannot take the command
/// (none running, or its queue full) is 503.
async fn apply_control(state: &AppState, command: Command) -> ApiResult {
    let mut snapshots = state.engine.snapshot.clone();
    snapshots.borrow_and_update();
    if !state.send_command(command) {
        return Err(ApiError::with_code(
            StatusCode::SERVICE_UNAVAILABLE,
            "engine_unavailable",
            "Kanał nie przyjmuje teraz poleceń. Spróbuj za chwilę.",
        ));
    }
    let _ = tokio::time::timeout(CONTROL_WAIT, async {
        for _ in 0..2 {
            if snapshots.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
    Ok(json_ok(
        &json!({ "control": control_json(&state.snapshot().state) }),
    ))
}

async fn pause(State(state): State<AppState>) -> ApiResult {
    controllable_film(&state)?;
    apply_control(&state, Command::Pause(PauseReason::Manual)).await
}

/// Clears only the manual pause; other active reasons stay (and are shown).
async fn resume(State(state): State<AppState>) -> ApiResult {
    apply_control(&state, Command::Resume(PauseReason::Manual)).await
}

/// Largest relative seek (one day).
const MAX_SEEK_BY_S: f64 = 86_400.0;
/// Largest absolute seek target (seven days).
const MAX_SEEK_TO_S: f64 = 7.0 * 86_400.0;

/// `{"toS": seconds}` or `{"byS": seconds}` (exactly one number; |byS| ≤ 1 day, |toS| ≤ 7 days).
async fn seek(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let number = |key: &str, max: f64| {
        body.get(key).map(|v| {
            v.as_f64()
                .filter(|n| n.abs() <= max)
                .ok_or_else(|| control_bad_request("Podaj liczbę sekund."))
        })
    };
    let command = match (number("toS", MAX_SEEK_TO_S), number("byS", MAX_SEEK_BY_S)) {
        (Some(to), None) => Command::Seek { to_s: to? },
        (None, Some(by)) => Command::SeekBy { delta_s: by? },
        _ => return Err(control_bad_request("Podaj toS albo byS (sekundy).")),
    };
    let control = controllable_film(&state)?;
    // Like the engine: without a known length only forward, or back to the start.
    let allowed = match command {
        Command::SeekBy { delta_s } => delta_s >= 0.0,
        Command::Seek { to_s } => to_s <= 0.0,
        _ => false,
    };
    if control.duration_s.is_none() && !allowed {
        return Err(control_bad_request(
            "Długość tego filmu nie jest znana: można go tylko przewinąć do przodu.",
        ));
    }
    apply_control(&state, command).await
}

async fn restart(State(state): State<AppState>) -> ApiResult {
    controllable_film(&state)?;
    apply_control(&state, Command::Restart).await
}

/// `{"itemId": id}` of a queued film or one from the history (played or failed).
async fn play_now(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let id = body
        .get("itemId")
        .and_then(Value::as_i64)
        .ok_or_else(|| control_bad_request("Podaj itemId."))?;
    let item = state
        .with_db(|c| QueueRepo::new(c).get(id))?
        .filter(|i| i.channel_id == state.channel_id)
        .ok_or_else(no_such_item)?;
    if !matches!(
        item.status,
        ItemStatus::Queued | ItemStatus::Played | ItemStatus::Failed
    ) {
        return Err(control_bad_request(
            "Odtworzyć teraz można tylko film z kolejki albo z historii.",
        ));
    }
    apply_control(&state, Command::PlayNow { item_id: item.id }).await
}

/// Nerd stats (spec section 5): computed per request from counters kept in memory. Viewer
/// addresses are counted, never returned. Unavailable values are `null`.
async fn stats(State(state): State<AppState>) -> ApiResult {
    let snapshot = state.snapshot();
    let now = std::time::Instant::now();
    let ffmpeg = snapshot.stats.ffmpeg.clone().map(|mut ffmpeg| {
        let usage = process_usage(ffmpeg.pid);
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        if let Some(sampled) = usage.and_then(|u| state.cpu.sample(ffmpeg.pid, u, now, cores)) {
            ffmpeg.rss_bytes = Some(sampled.rss_bytes);
            ffmpeg.cpu_percent = sampled.cpu_percent;
        }
        ffmpeg
    });
    let tunnel = state.app_core().and_then(|core| core.tunnel_stats());
    let value = |v: Result<Value, serde_json::Error>| v.unwrap_or(Value::Null);
    Ok(json_ok(&json!({
        "onAir": value(serde_json::to_value(&snapshot.stats.on_air)),
        "hls": value(serde_json::to_value(&snapshot.stats.hls)),
        "next": value(serde_json::to_value(&snapshot.stats.next)),
        "ffmpeg": value(serde_json::to_value(&ffmpeg)),
        "viewers": value(serde_json::to_value(state.viewers.stats(now))),
        "tunnel": value(serde_json::to_value(&tunnel)),
    })))
}

async fn get_settings(State(state): State<AppState>) -> ApiResult {
    Ok(json_ok(&json!(state.settings()?)))
}

async fn put_settings(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let body = if body.is_null() { json!({}) } else { body };
    let patch = sanitize_settings_patch(&body).map_err(ApiError::bad_request)?;
    let updated = state.with_db(|c| SettingsRepo::new(c).update(state.channel_id, &patch))?;
    state
        .request_limiter
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set_limits(
            updated.request_limit_count,
            Duration::from_secs(updated.request_limit_window_s),
        );
    Ok(json_ok(&json!(updated)))
}
