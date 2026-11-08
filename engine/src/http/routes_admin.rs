//! Admin API (behind the session cookie): panel state, queue management, disk files, settings.

use std::sync::PoisonError;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, patch, post};
use serde_json::{Value, json};

use super::{
    ApiError, ApiResult, AppState, Ctx, JsonBody, ReqLang, admin_item, control_json, json_ok,
    json_response, state_map,
};
use crate::db::journal::{JournalRepo, KEEP_ROWS, journal_csv, journal_json};
use crate::db::{
    AddedBy, ItemStatus, NewItem, QueueItem, QueueRepo, SettingsRepo, sanitize_settings_patch,
};
use crate::engine::Command;
use crate::i18n::Lang;
use crate::library::LibraryError;
use crate::nicks::{NickStore, badge};
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
        .route("/api/admin/nicks", get(list_nicks))
        .route("/api/admin/nicks/{skeleton}/release", post(release_nick))
        .route(
            "/api/admin/nicks/{skeleton}/reset-pin",
            post(reset_nick_pin),
        )
        .route("/api/admin/devices/block", post(block_device))
        .route("/api/admin/devices/unblock", post(unblock_device))
        .route("/api/admin/channels/main/local", post(add_local))
        .route("/api/admin/channels/main/skip", post(skip))
        .route("/api/admin/channels/main/pause", post(pause))
        .route("/api/admin/channels/main/resume", post(resume))
        .route("/api/admin/channels/main/seek", post(seek))
        .route("/api/admin/channels/main/restart", post(restart))
        .route("/api/admin/channels/main/stats", get(stats))
        .route("/api/admin/qr", get(qr))
        .route("/api/admin/environment", get(environment))
        .route("/api/admin/channels/main/play-now", post(play_now))
        .route(
            "/api/admin/channels/main/settings",
            get(get_settings).put(put_settings),
        )
        .route(
            "/api/admin/channels/main/journal",
            get(journal).delete(clear_journal),
        )
        .route(
            "/api/admin/channels/main/journal/export",
            get(journal_export),
        )
}

/// Rows of the broadcast journal shown in Settings → Advanced at most.
const JOURNAL_PAGE_MAX: usize = 500;

/// The newest rows of the broadcast journal (`?limit=`, default 20).
async fn journal(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let limit = query
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, JOURNAL_PAGE_MAX);
    let entries = state.with_db(|c| JournalRepo::new(c).list(state.channel_id, limit))?;
    Ok(json_ok(&json!({ "entries": entries })))
}

async fn clear_journal(State(state): State<AppState>) -> ApiResult {
    let removed = state.with_db(|c| JournalRepo::new(c).clear(state.channel_id))?;
    Ok(json_ok(&json!({ "ok": true, "removed": removed })))
}

/// The whole journal as a file: `?format=csv` (default) or `json`.
async fn journal_export(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let entries =
        state.with_db(|c| JournalRepo::new(c).list(state.channel_id, KEEP_ROWS as usize))?;
    let day = chrono::Local::now().format("%Y%m%d");
    let (body, mime, ext) = match query.get("format").map(String::as_str) {
        Some("json") => (
            serde_json::to_string_pretty(&journal_json(&entries)).unwrap_or_default(),
            "application/json; charset=utf-8",
            "json",
        ),
        // A byte order mark: spreadsheets then read the titles as UTF-8.
        _ => (
            format!("\u{feff}{}", journal_csv(&entries)),
            "text/csv; charset=utf-8",
            "csv",
        ),
    };
    let disposition = format!("attachment; filename=\"vrcinema-journal-{day}.{ext}\"");
    Ok(axum::response::IntoResponse::into_response((
        [
            (axum::http::header::CONTENT_TYPE, mime.to_string()),
            (axum::http::header::CONTENT_DISPOSITION, disposition),
            (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )))
}

/// Runs before body parsing, like Fastify's `onRequest` hook.
pub(super) async fn require_admin(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if !state.is_admin(req.headers()) && !state.ctl_authorized(&req) {
        return axum::response::IntoResponse::into_response(ApiError::coded(
            StatusCode::UNAUTHORIZED,
            "login_required",
            &[],
        ));
    }
    next.run(req).await
}

fn ok() -> Response {
    json_ok(&json!({ "ok": true }))
}

fn items_json(items: &[QueueItem], lang: Lang) -> Value {
    Value::Array(items.iter().map(|i| admin_item(i, lang)).collect())
}

async fn panel(State(state): State<AppState>, ctx: Ctx, ReqLang(lang): ReqLang) -> ApiResult {
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
    // How ready the next film is: resolving, resolved, preparing, ready or failed.
    body.insert(
        "next".into(),
        snapshot.stats.next.as_ref().map_or(
            Value::Null,
            |n| json!({ "itemId": n.item_id, "state": n.state }),
        ),
    );
    body.insert("playing".into(), items_json(&playing, lang));
    body.insert("queue".into(), items_json(&queue, lang));
    body.insert("pending".into(), items_json(&pending, lang));
    body.insert(
        "pendingLists".into(),
        super::viewer_lists::pending_lists_json(&state)?,
    );
    body.insert("history".into(), items_json(&history, lang));
    body.insert("settings".into(), json!(settings));
    body.insert("control".into(), control_json(&snapshot.state));
    body.insert("air".into(), super::air_json(&state, &snapshot));
    // The OBS stream: `off`, `waiting`, `live` or `brb` (never its key or address).
    body.insert(
        "obs".into(),
        json!({ "state": snapshot.state.obs.as_str() }),
    );
    body.insert(
        "link".into(),
        super::link_json(&state, &snapshot, settings.upload_mbps),
    );
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
        .ok_or_else(|| ApiError::bad_request("invalid_link"))?;
    let meta = state
        .resolver
        .fetch_meta(url.as_str())
        .await
        .map_err(|e| ApiError::bad_request("read_failed").with_msg_arg("reason", e.0))?;
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
            requester_key_hash: None,
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
            return Err(ApiError::bad_request("empty_playlist"));
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
    ApiError::coded(StatusCode::NOT_FOUND, "no_such_item", &[])
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
                    return Ok(Err(ApiError::coded(
                        StatusCode::CONFLICT,
                        "not_pending",
                        &[],
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
                    return Ok(Err(ApiError::coded(
                        StatusCode::CONFLICT,
                        "move_only_queued",
                        &[],
                    )));
                }
                q.move_item(item.id, position)?;
            }
            _ => {
                return Ok(Err(ApiError::bad_request("action_or_position")));
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
            return Ok(Err(ApiError::coded(
                StatusCode::CONFLICT,
                "item_finished",
                &[],
            )));
        }
        q.set_status(item.id, ItemStatus::Removed, None)?;
        Ok(Ok(()))
    })??;
    state.send_command(Command::QueueChanged);
    Ok(ok())
}

// ---------- Nicki widzów ----------

fn no_such_nick() -> ApiError {
    ApiError::coded(StatusCode::NOT_FOUND, "nick_not_found", &[])
}

/// Held nicks (with their icon and last use) and blocked devices (with the nick they used
/// last and the block date); never a key or its hash. A block stays listed after its nick is
/// released.
async fn list_nicks(State(state): State<AppState>) -> ApiResult {
    let now = crate::db::now_ms();
    let (entries, blocked) = state.with_db(|c| {
        let store = NickStore::new(c);
        Ok((store.list(now)?, store.blocked()?))
    })?;
    let badge_json = |key_hash: &str| {
        let b = badge(key_hash);
        json!({ "hue": b.hue, "shape": b.shape })
    };
    let nicks: Vec<Value> = entries
        .iter()
        .map(|e| {
            json!({
                "skeleton": e.skeleton,
                "nick": e.display,
                "badge": badge_json(&e.key_hash),
                "lastUsedAt": e.last_used_at,
                "blocked": e.blocked,
                "blockId": e.block_id,
                "protected": e.protected,
                "devices": e.devices,
            })
        })
        .collect();
    let blocked: Vec<Value> = blocked
        .iter()
        .map(|b| {
            json!({
                "id": b.id,
                "nick": b.nick,
                "badge": badge_json(&b.key_hash),
                "blockedAt": b.blocked_at,
            })
        })
        .collect();
    Ok(json_ok(&json!({ "nicks": nicks, "blocked": blocked })))
}

async fn release_nick(State(state): State<AppState>, Path(skeleton): Path<String>) -> ApiResult {
    if state.with_db(|c| NickStore::new(c).release(&skeleton))? {
        Ok(ok())
    } else {
        Err(no_such_nick())
    }
}

/// The nick loses its PIN, recovery code and other devices; the device that used it last keeps
/// it as a nick without a PIN.
async fn reset_nick_pin(State(state): State<AppState>, Path(skeleton): Path<String>) -> ApiResult {
    if state.with_db(|c| NickStore::new(c).reset_pin(&skeleton))? {
        Ok(ok())
    } else {
        Err(ApiError::coded(StatusCode::NOT_FOUND, "nick_no_pin", &[]))
    }
}

/// Blocks the device that holds the nick (every device of a protected nick): its requests are
/// refused from now on.
async fn block_device(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let skeleton = body
        .get("skeleton")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("bad_request"))?;
    if state.with_db(|c| NickStore::new(c).block_nick(skeleton))? {
        Ok(ok())
    } else {
        Err(no_such_nick())
    }
}

/// Lifts a block by its id (from the list of blocked devices).
async fn unblock_device(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let id = body
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::bad_request("bad_request"))?;
    if state.with_db(|c| NickStore::new(c).unblock(id))? {
        Ok(ok())
    } else {
        Err(ApiError::coded(
            StatusCode::NOT_FOUND,
            "block_not_found",
            &[],
        ))
    }
}

// ---------- Pliki z dysku ----------

fn library_error(e: LibraryError) -> ApiError {
    ApiError::from_msg(StatusCode::BAD_REQUEST, e.0)
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
            return Err(LibraryError::no_such_root());
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
        return Err(ApiError::bad_request("library_disabled"));
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
            let root = root.ok_or_else(|| library_error(LibraryError::no_such_root()))?;
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
        return Err(ApiError::bad_request("no_videos_in_folder"));
    }
    let mut titles = Vec::new();
    for file in files.iter().take(MAX_PLAYLIST_ITEMS) {
        let source_url = lib.to_source_url(root, file);
        let meta = state
            .resolver
            .fetch_meta(&source_url)
            .await
            .map_err(|e| ApiError::from_msg(StatusCode::BAD_REQUEST, e.0))?;
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
                requester_key_hash: None,
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

/// A refused control request: 400 with `code` `bad_request` (as the controls always answered)
/// and the text of `errors.<message>`.
fn control_bad_request(message: &'static str) -> ApiError {
    ApiError::bad_request(message).with_code("bad_request")
}

/// The film the controls act on, when it can be paused and seeked; otherwise 409.
fn controllable_film(state: &AppState) -> ApiResult<ControlState> {
    let control = state.snapshot().state.control.clone();
    if control.item_id.is_none() {
        return Err(ApiError::coded(
            StatusCode::CONFLICT,
            "nothing_playing",
            &[],
        ));
    }
    if control.is_live {
        return Err(ApiError::coded(StatusCode::CONFLICT, "live_item", &[]));
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
        return Err(ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "engine_unavailable",
            &[],
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

/// No body, or `{"atS": seconds}`: the position the producer's monitor showed (0 ≤ atS ≤ the
/// film's length when known). With it the film airs up to the end of the segment holding that
/// position first, then pauses there (see `Channel::pause_at`).
async fn pause(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let control = controllable_film(&state)?;
    let at_s = match body.get("atS") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .filter(|n| control.duration_s.is_none_or(|d| *n <= d))
                .ok_or_else(|| control_bad_request("pause_at"))?,
        ),
    };
    let command = match at_s {
        Some(at_s) => Command::PauseAt { at_s },
        None => Command::Pause(PauseReason::Manual),
    };
    apply_control(&state, command).await
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
                .ok_or_else(|| control_bad_request("seek_seconds"))
        })
    };
    let command = match (number("toS", MAX_SEEK_TO_S), number("byS", MAX_SEEK_BY_S)) {
        (Some(to), None) => Command::Seek { to_s: to? },
        (None, Some(by)) => Command::SeekBy { delta_s: by? },
        _ => return Err(control_bad_request("seek_target")),
    };
    let control = controllable_film(&state)?;
    // Like the engine: without a known length only forward, or back to the start.
    let allowed = match command {
        Command::SeekBy { delta_s } => delta_s >= 0.0,
        Command::Seek { to_s } => to_s <= 0.0,
        _ => false,
    };
    if control.duration_s.is_none() && !allowed {
        return Err(control_bad_request("seek_unknown_length"));
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
        .ok_or_else(|| control_bad_request("item_id_required"))?;
    let item = state
        .with_db(|c| QueueRepo::new(c).get(id))?
        .filter(|i| i.channel_id == state.channel_id)
        .ok_or_else(no_such_item)?;
    if !matches!(
        item.status,
        ItemStatus::Queued | ItemStatus::Played | ItemStatus::Failed
    ) {
        return Err(control_bad_request("not_playable_now"));
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

/// What the panel runs in: whether the program has its own window (a server without one has
/// no program language, tools or tunnel to manage), the encoder in use, whether the info
/// ticker makes the computer fall behind, the program's language (the panel's language until
/// the browser picks one), whether this session came through the built-in tunnel, and — in
/// the app — the tunnel and the channel's state (`null` without a window). Open to a remote
/// session too; in the app a remote panel reads the whole info at `/api/admin/info`.
async fn environment(State(state): State<AppState>, ctx: Ctx, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.app_core();
    let settings = core.as_ref().map(|c| c.settings());
    Ok(json_ok(&json!({
        "window": core.is_some(),
        "encoder": state.live.encoder.get().as_str(),
        // A low-power computer converting on its processor (Raspberry Pi 5): the light profile.
        "lightProfile": crate::pipeline::ffmpeg_args::light_profile(state.live.encoder.get()),
        "tickerSlow": state.snapshot().stats.ticker_slow,
        "alarms": state.alarms.get(),
        "language": state.language().as_str(),
        "sessionViaTunnel": ctx.via_tunnel,
        "publicUrl": settings
            .as_ref()
            .map_or_else(|| state.live.public_url.get(), |s| s.public_url.clone()),
        "tunnelEnabled": settings.as_ref().map(|s| s.tunnel_enabled),
        "tunnel": core.as_ref().map(|c| super::routes_desktop::tunnel_view(c, lang)),
        "channelRunning": core.as_ref().map(|c| c.channel_running()),
        "channelError": core.as_ref().and_then(|c| c.channel_error()).map(|m| m.render(lang)),
        "restarting": core.as_ref().map(|c| c.restarting()),
    })))
}

/// The QR code (PNG) of the request page, for the share view (the window has its own route,
/// `/api/desktop/tunnel/qr`; this one also serves a remote session); 404 without an address.
async fn qr(State(state): State<AppState>) -> ApiResult {
    super::request_qr_response(&state.live.public_url.get())
}

async fn get_settings(State(state): State<AppState>) -> ApiResult {
    let mut body = json!(state.settings()?);
    // What the "add popular sites" button of the settings puts in a list.
    body["popularDomains"] = json!(crate::rules::POPULAR_DOMAINS);
    Ok(json_ok(&body))
}

async fn put_settings(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let body = if body.is_null() { json!({}) } else { body };
    let patch = sanitize_settings_patch(&body)
        .map_err(|e| ApiError::from_msg(StatusCode::BAD_REQUEST, e))?;
    let (before, updated) = state.with_db(|c| {
        let repo = SettingsRepo::new(c);
        Ok((
            repo.get(state.channel_id)?,
            repo.update(state.channel_id, &patch)?,
        ))
    })?;
    // A new look of the ticker restarts the film on air from where it is; its texts change
    // through the text file by themselves.
    // Switching to or from Japanese changes the ticker's font (the text alone would come out
    // in boxes): the film starts again with it too. Other languages share the font.
    let font_changed = updated.ticker.enabled
        && (before.slate_language == Lang::Ja) != (updated.slate_language == Lang::Ja);
    if !before.ticker.same_layout(&updated.ticker) || font_changed {
        state.send_command(Command::ApplyTicker);
    }
    state
        .request_limiter
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set_limits(
            updated.request_limit_count,
            Duration::from_secs(updated.request_limit_window_s),
        );
    if patch.slate_language.is_some() || !patch.slate_look.is_empty() {
        // A new slate language or look makes a new slate set at once (the app and the server
        // without a window alike).
        super::routes_slates::refresh_slates(&state);
    }
    Ok(json_ok(&json!(updated)))
}
