//! Skip voting: viewers vote to skip the film on air (`POST /api/channels/main/skip-vote`).
//! The votes live in memory, for the film on air only: a new film starts from none.

use std::collections::HashSet;
use std::sync::PoisonError;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use serde_json::{Value, json};

use super::routes_public::check_viewer;
use super::{ApiError, ApiResult, AppState, Ctx, JsonBody, json_ok};
use crate::db::{ChannelSettings, SkipVoteMode};
use crate::engine::Command;
use crate::i18n::Msg;
use crate::nicks::hash_device_key;
use crate::rules::RateLimiter;

/// Votes (and withdrawals) one address may send per minute.
const VOTE_LIMIT: u32 = 20;
const VOTE_WINDOW: Duration = Duration::from_secs(60);

pub(super) fn new_limiter() -> RateLimiter {
    RateLimiter::new(VOTE_LIMIT, VOTE_WINDOW, crate::rules::system_clock())
}

/// The votes for the film on air: the device keys (hashed) that want it skipped.
#[derive(Debug, Default)]
pub(crate) struct SkipVotes {
    item: Option<i64>,
    voters: HashSet<String>,
    /// The skip was already sent for this film: later votes change nothing.
    fired: bool,
}

impl SkipVotes {
    /// Forgets the votes when the film on air is not the one they were for.
    fn sync(&mut self, on_air: Option<i64>) {
        if self.item != on_air {
            self.item = on_air;
            self.voters.clear();
            self.fired = false;
        }
    }
}

/// Votes needed to skip, or `None` when the votes never skip (signal, off). Percent counts the
/// viewers watching (the estimate of Stats for nerds); never less than 1, also with nobody
/// watching.
pub(crate) fn needed_votes(settings: &ChannelSettings, watching: usize) -> Option<u32> {
    match settings.skip_vote_mode {
        SkipVoteMode::Off | SkipVoteMode::Signal => None,
        SkipVoteMode::Count => Some(settings.skip_vote_count.max(1)),
        SkipVoteMode::Percent => {
            let percent = u64::from(settings.skip_vote_percent.clamp(1, 100));
            let n = (percent * watching as u64).div_ceil(100).max(1);
            Some(u32::try_from(n).unwrap_or(u32::MAX))
        }
    }
}

fn on_air(state: &AppState) -> Option<i64> {
    state.snapshot().state.current.as_ref().map(|c| c.item_id)
}

/// `{ mode, votes, needed }` (+ `mine` when the device key is known); `None` when voting is off.
pub(crate) fn skip_vote_json(
    state: &AppState,
    settings: &ChannelSettings,
    key_hash: Option<&str>,
) -> Option<Value> {
    if settings.skip_vote_mode == SkipVoteMode::Off {
        return None;
    }
    let watching = state.viewers.stats(std::time::Instant::now()).watching;
    let mut votes = state
        .skip_votes
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    votes.sync(on_air(state));
    let mut out = json!({
        "mode": settings.skip_vote_mode.as_str(),
        "votes": votes.voters.len(),
        "needed": needed_votes(settings, watching),
    });
    if let Some(key) = key_hash {
        out["mine"] = json!(votes.voters.contains(key));
    }
    Some(out)
}

/// The caller's device key from the `X-Device-Key` header (the state route is a GET), hashed.
pub(crate) fn header_key_hash(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-device-key")
        .and_then(|v| v.to_str().ok())
        .and_then(hash_device_key)
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/api/channels/main/skip-vote", post(post_vote))
}

/// `{ deviceKey, vote: bool, itemId?, name? }`. 404 when voting is off; 409 `skip_vote_stale`
/// when no film is on air or `itemId` is not the one on air (nothing is recorded). Answers
/// `{ skipVote: { mode, votes, needed, mine } }`.
async fn post_vote(State(state): State<AppState>, ctx: Ctx, JsonBody(body): JsonBody) -> ApiResult {
    let settings = state.settings()?;
    if settings.skip_vote_mode == SkipVoteMode::Off {
        return Err(ApiError::coded(StatusCode::NOT_FOUND, "not_found", &[]));
    }
    let vote = body
        .get("vote")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request("invalid_vote"))?;
    let viewer = check_viewer(&state, &body)?;
    // One device, one vote: a device key is required.
    let key = viewer
        .key_hash
        .ok_or_else(|| ApiError::bad_request("invalid_device_key"))?;
    let allowed = state
        .skip_limiter
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .hit(&ctx.ip);
    if !allowed {
        return Err(ApiError::coded(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            &[],
        ));
    }
    let current = on_air(&state);
    let asked = body.get("itemId").and_then(Value::as_i64);
    let Some(item) = current.filter(|item| asked.is_none_or(|a| a == *item)) else {
        return Err(ApiError::coded(
            StatusCode::CONFLICT,
            "skip_vote_stale",
            &[],
        ));
    };
    let watching = state.viewers.stats(std::time::Instant::now()).watching;
    let (votes, fire) = {
        let mut store = state
            .skip_votes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        store.sync(Some(item));
        if vote {
            store.voters.insert(key.clone());
        } else {
            store.voters.remove(&key);
        }
        let votes = store.voters.len();
        let fire = vote
            && !store.fired
            && needed_votes(&settings, watching).is_some_and(|n| votes >= n as usize);
        store.fired |= fire;
        (votes, fire)
    };
    if fire {
        let title = state
            .snapshot()
            .state
            .current
            .as_ref()
            .map(|c| c.title.clone());
        (state.log)(
            Msg::new("log.skip_vote")
                .arg("title", title.unwrap_or_default())
                .arg("votes", votes),
        );
        state.send_command(Command::Skip);
    }
    let view = skip_vote_json(&state, &settings, Some(&key)).unwrap_or(Value::Null);
    Ok(json_ok(&json!({ "skipVote": view })))
}
