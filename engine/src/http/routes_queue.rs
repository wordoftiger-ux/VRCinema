//! The owner's queue tools (behind the session cookie, window and remote panel alike): clear
//! the queue, remove everything of one viewer (optionally blocking their device), undo either
//! for a while, and play a video next. Only the Rust engine has these routes (like the
//! controls), so the API parity scenario does not cover them.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::{Value, json};

use super::{ApiError, ApiResult, AppState, JsonBody, json_ok};
use crate::db::{ItemStatus, NewItem, QueueItem, QueueRepo, Requester};
use crate::engine::Command;
use crate::nicks::NickStore;

/// How long a bulk removal can be undone (the panel offers it for a few seconds; the rest is
/// headroom for a slow connection).
const UNDO_KEEP: Duration = Duration::from_secs(60);
/// Removals kept at most; older ones cannot be undone.
const UNDO_MAX: usize = 16;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/channels/main/clear", post(clear))
        .route(
            "/api/admin/channels/main/remove-requester",
            post(remove_requester),
        )
        .route("/api/admin/channels/main/undo", post(undo))
        .route("/api/admin/channels/main/play-next", post(play_next))
}

/// One bulk removal: the items with the status they had, and the device it blocked (lifted
/// again by the undo).
struct Removal {
    id: u64,
    at: Instant,
    items: Vec<(i64, ItemStatus)>,
    blocked: Option<String>,
}

#[derive(Default)]
pub(crate) struct UndoStore {
    inner: Mutex<(u64, Vec<Removal>)>,
}

impl UndoStore {
    fn push(&self, items: Vec<(i64, ItemStatus)>, blocked: Option<String>, now: Instant) -> u64 {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let (next, removals) = &mut *inner;
        *next += 1;
        removals.retain(|r| now.duration_since(r.at) < UNDO_KEEP);
        if removals.len() >= UNDO_MAX {
            removals.remove(0);
        }
        removals.push(Removal {
            id: *next,
            at: now,
            items,
            blocked,
        });
        *next
    }

    fn take(&self, id: u64, now: Instant) -> Option<Removal> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let removals = &mut inner.1;
        removals.retain(|r| now.duration_since(r.at) < UNDO_KEEP);
        let at = removals.iter().position(|r| r.id == id)?;
        Some(removals.remove(at))
    }
}

/// An optional boolean field: absent or `null` is `false`, anything but a boolean is refused.
fn flag(body: &Value, key: &str) -> ApiResult<bool> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ApiError::bad_request("bad_request")),
    }
}

fn item_id(body: &Value) -> ApiResult<i64> {
    body.get("itemId")
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::bad_request("item_id_required"))
}

fn no_such_item() -> ApiError {
    ApiError::coded(StatusCode::NOT_FOUND, "no_such_item", &[])
}

fn find_item(state: &AppState, id: i64) -> ApiResult<QueueItem> {
    state
        .with_db(|c| QueueRepo::new(c).get(id))?
        .filter(|i| i.channel_id == state.channel_id)
        .ok_or_else(no_such_item)
}

/// What a bulk removal takes: the waiting items (queued and waiting for approval), and the
/// one on air only when the owner ticked it.
fn statuses(include_playing: bool) -> Vec<ItemStatus> {
    let mut statuses = vec![ItemStatus::Queued, ItemStatus::PendingApproval];
    if include_playing {
        statuses.push(ItemStatus::Playing);
    }
    statuses
}

fn removed_answer(
    state: &AppState,
    items: Vec<(i64, ItemStatus)>,
    blocked: Option<String>,
) -> Value {
    let removed = items.len();
    let undo = (removed > 0 || blocked.is_some()).then(|| {
        state
            .queue_undo
            .push(items, blocked.clone(), Instant::now())
    });
    json!({ "removed": removed, "blocked": blocked.is_some(), "undo": undo })
}

/// `{"includePlaying": bool}`: removes every waiting video (and the one on air only with
/// `includePlaying`). Answers `{removed, blocked: false, undo}` (`undo` null when nothing went).
async fn clear(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let include_playing = flag(&body, "includePlaying")?;
    let removed = state.with_db(|c| {
        QueueRepo::new(c).remove_where(state.channel_id, &statuses(include_playing), None)
    })?;
    if !removed.is_empty() {
        state.send_command(Command::QueueChanged);
    }
    Ok(json_ok(&removed_answer(&state, removed, None)))
}

/// `{"itemId": id, "includePlaying"?, "block"?, "dryRun"?}`: everything waiting from the viewer
/// who added `itemId` (their device, or their nick without one). `dryRun` changes nothing and
/// answers what would go: `{name, queued, pending, playing: {itemId, title} | null, canBlock,
/// blocked}`. Otherwise `{removed, blocked, undo}`; `block` also blocks the device.
async fn remove_requester(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let item = find_item(&state, item_id(&body)?)?;
    let include_playing = flag(&body, "includePlaying")?;
    let block = flag(&body, "block")?;
    let dry_run = flag(&body, "dryRun")?;
    let Some(requester) = Requester::of(&item) else {
        return Err(ApiError::coded(StatusCode::CONFLICT, "no_requester", &[]));
    };
    let device = match &requester {
        Requester::Device(key) => Some(key.clone()),
        Requester::Nick(_) => None,
    };
    if block && device.is_none() {
        return Err(ApiError::coded(StatusCode::CONFLICT, "cannot_block", &[]));
    }
    let channel_id = state.channel_id;
    if dry_run {
        let (items, blocked) = state.with_db(|c| {
            let items = QueueRepo::new(c).list_by_status(channel_id, &statuses(true))?;
            let blocked = match &device {
                Some(key) => NickStore::new(c).is_blocked(key)?,
                None => false,
            };
            Ok((items, blocked))
        })?;
        let mine: Vec<&QueueItem> = items.iter().filter(|i| requester.matches(i)).collect();
        let count = |s: ItemStatus| mine.iter().filter(|i| i.status == s).count();
        let playing = mine
            .iter()
            .find(|i| i.status == ItemStatus::Playing)
            .map(|i| json!({ "itemId": i.id, "title": i.title }));
        return Ok(json_ok(&json!({
            "name": item.requester_name,
            "queued": count(ItemStatus::Queued),
            "pending": count(ItemStatus::PendingApproval),
            "playing": playing,
            "canBlock": device.is_some(),
            "blocked": blocked,
        })));
    }
    let (removed, newly_blocked) = state.with_db(|c| {
        let removed = QueueRepo::new(c).remove_where(
            channel_id,
            &statuses(include_playing),
            Some(&requester),
        )?;
        let mut newly_blocked = None;
        if block && let Some(key) = &device {
            let store = NickStore::new(c);
            if !store.is_blocked(key)? {
                store.block_device(key)?;
                newly_blocked = Some(key.clone());
            }
        }
        Ok((removed, newly_blocked))
    })?;
    if !removed.is_empty() {
        state.send_command(Command::QueueChanged);
    }
    let mut answer = removed_answer(&state, removed, newly_blocked);
    // Asked to block a device that already was: it is blocked all the same.
    answer["blocked"] = json!(block);
    Ok(json_ok(&answer))
}

/// `{"undo": id}` of a removal: puts its items back (a film that was on air returns to the
/// front of the queue) and lifts the block it made. 404 `undo_expired` once it is too late.
async fn undo(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let id = body
        .get("undo")
        .and_then(Value::as_u64)
        .ok_or_else(|| ApiError::bad_request("bad_request"))?;
    let removal = state
        .queue_undo
        .take(id, Instant::now())
        .ok_or_else(|| ApiError::coded(StatusCode::NOT_FOUND, "undo_expired", &[]))?;
    let restored = state.with_db(|c| {
        let restored = QueueRepo::new(c).restore_removed(&removal.items)?;
        if let Some(key) = &removal.blocked {
            NickStore::new(c).unblock_key(key)?;
        }
        Ok(restored)
    })?;
    if restored > 0 {
        state.send_command(Command::QueueChanged);
    }
    Ok(json_ok(&json!({ "restored": restored })))
}

/// `{"itemId": id}`: a queued video moves to the front; one from the history (played or
/// failed) is added again at the front, with its link and requester. Answers `{itemId}` of the
/// video that plays next.
async fn play_next(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let item = find_item(&state, item_id(&body)?)?;
    let next = state.with_db(|c| {
        let q = QueueRepo::new(c);
        match item.status {
            ItemStatus::Queued => {
                q.move_item(item.id, 0)?;
                Ok(Some(item.id))
            }
            ItemStatus::Played | ItemStatus::Failed => {
                let again = q.add_first(&NewItem {
                    channel_id: item.channel_id,
                    source_url: item.source_url.clone(),
                    title: item.title.clone(),
                    duration_s: item.duration_s,
                    is_live: item.is_live,
                    status: ItemStatus::Queued,
                    added_by: item.added_by,
                    requester_name: item.requester_name.clone(),
                    // The owner's choice: it does not count towards the viewer's limit.
                    requester_ip: None,
                    requester_key_hash: item.requester_key_hash.clone(),
                })?;
                Ok(Some(again.id))
            }
            _ => Ok(None),
        }
    })?;
    let Some(next) = next else {
        return Err(ApiError::coded(
            StatusCode::CONFLICT,
            "not_playable_next",
            &[],
        ));
    };
    state.send_command(Command::QueueChanged);
    Ok(json_ok(&json!({ "itemId": next })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_removal_is_undone_once_and_only_while_it_is_kept() {
        let store = UndoStore::default();
        let t0 = Instant::now();
        let a = store.push(vec![(1, ItemStatus::Queued)], None, t0);
        let b = store.push(vec![(2, ItemStatus::Queued)], None, t0);
        assert_ne!(a, b);
        assert_eq!(
            store.take(a, t0).map(|r| r.items),
            Some(vec![(1, ItemStatus::Queued)])
        );
        assert!(store.take(a, t0).is_none(), "only once");
        assert!(store.take(b, t0 + UNDO_KEEP).is_none(), "too late");
        for _ in 0..=UNDO_MAX {
            store.push(vec![], None, t0);
        }
        let kept = store.inner.lock().unwrap().1.len();
        assert_eq!(kept, UNDO_MAX);
    }
}
