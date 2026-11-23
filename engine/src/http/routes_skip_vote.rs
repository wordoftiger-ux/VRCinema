//! Skip voting: viewers vote to skip the film on air (`POST /api/channels/main/skip-vote`).
//! The votes live in memory, for the film on air only: a new film starts from none.

use std::collections::HashMap;
use std::sync::PoisonError;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::{Value, json};

use super::routes_public::check_viewer;
use super::{ApiError, ApiResult, AppState, Ctx, JsonBody, json_ok};
use crate::db::{ChannelSettings, SkipVoteMode};
use crate::engine::Command;
use crate::i18n::Msg;
use crate::pipeline::channel::ChannelStatus;
use crate::rules::RateLimiter;

/// Votes (and withdrawals) one address may send per minute.
const VOTE_LIMIT: u32 = 20;
const VOTE_WINDOW: Duration = Duration::from_secs(60);

pub(super) fn new_limiter() -> RateLimiter {
    RateLimiter::new(VOTE_LIMIT, VOTE_WINDOW, crate::rules::system_clock())
}

/// The votes for the film on air: one per client address (a device key is client-made, so
/// many keys from one address must not mean many votes). The value is the key that voted.
#[derive(Debug, Default)]
pub(crate) struct SkipVotes {
    item: Option<i64>,
    voters: HashMap<String, String>,
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

    /// True once (per film) when the votes reach what is needed; marks the film as skipped.
    fn reached(&mut self, settings: &ChannelSettings, watching: usize) -> bool {
        // In percent mode only as many votes count as there are viewers.
        let counted = match settings.skip_vote_mode {
            SkipVoteMode::Percent => self.voters.len().min(watching),
            _ => self.voters.len(),
        };
        let fire =
            !self.fired && needed_votes(settings, watching).is_some_and(|n| counted >= n as usize);
        self.fired |= fire;
        fire
    }
}

/// Votes needed to skip, or `None` when the votes never skip (signal, off, or percent with
/// nobody watching: the estimate is unknown). Percent counts the viewers watching (the
/// estimate of Stats for nerds).
pub(crate) fn needed_votes(settings: &ChannelSettings, watching: usize) -> Option<u32> {
    match settings.skip_vote_mode {
        SkipVoteMode::Off | SkipVoteMode::Signal => None,
        SkipVoteMode::Count => Some(settings.skip_vote_count.max(1)),
        SkipVoteMode::Percent if watching == 0 => None,
        SkipVoteMode::Percent => {
            let percent = u64::from(settings.skip_vote_percent.clamp(1, 100));
            let n = (percent * watching as u64).div_ceil(100).max(1);
            Some(u32::try_from(n).unwrap_or(u32::MAX))
        }
    }
}

impl SkipVotes {
    /// The film on air starts again (a restart, a play-now): its votes start from none.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The film on air (its id and title) and whether it is paused, from one snapshot.
fn on_air(state: &AppState) -> Option<(i64, String, bool)> {
    let snapshot = state.snapshot();
    let s = &snapshot.state;
    let paused = s.paused || s.status == ChannelStatus::Paused;
    s.current
        .as_ref()
        .map(|c| (c.item_id, c.title.clone(), paused))
}

/// Skips film `item_id` once the votes are enough (also when the viewers watching dropped);
/// the channel ignores it if another film airs by then.
fn skip_now(state: &AppState, item_id: i64, title: &str, votes: usize) {
    (state.log)(
        Msg::new("log.skip_vote")
            .arg("title", title)
            .arg("votes", votes),
    );
    state.send_command(Command::SkipItem { item_id });
}

/// `{ mode, votes, needed }` (+ `mine` when the caller's address is given: whether it has
/// voted); `None` when voting is off. Also re-checks the threshold, so a drop of the viewers
/// watching completes a skip.
pub(crate) fn skip_vote_json(
    state: &AppState,
    settings: &ChannelSettings,
    ip: Option<&str>,
) -> Option<Value> {
    if settings.skip_vote_mode == SkipVoteMode::Off {
        return None;
    }
    let watching = state.viewers.stats(std::time::Instant::now()).watching;
    let (out, fire) = {
        let mut store = state
            .skip_votes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Read under the lock, so the count and the film are those of this moment.
        let film = on_air(state);
        store.sync(film.as_ref().map(|f| f.0));
        // A paused film is never skipped by votes (nor marked as skipped).
        let fire = match &film {
            Some((id, title, false)) if store.reached(settings, watching) => {
                Some((*id, title.clone(), store.voters.len()))
            }
            _ => None,
        };
        let mut out = json!({
            "mode": settings.skip_vote_mode.as_str(),
            "votes": store.voters.len(),
            "needed": needed_votes(settings, watching),
        });
        if let Some(ip) = ip {
            out["mine"] = json!(store.voters.contains_key(ip));
        }
        (out, fire)
    };
    if let Some((id, title, votes)) = fire {
        skip_now(state, id, &title, votes);
    }
    Some(out)
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/api/channels/main/skip-vote", post(post_vote))
}

/// `{ deviceKey, vote: bool, itemId?, name? }`. One vote per client address (`mine`: this address voted). 404 when voting is off; 409 `skip_vote_stale`
/// when no film is on air or `itemId` is not the one on air (nothing is recorded). Answers
/// `{ skipVote: { mode, votes, needed, mine } }`.
async fn post_vote(
    State(state): State<AppState>,
    ctx: Ctx,
    JsonBody(mut body): JsonBody,
) -> ApiResult {
    let settings = state.settings()?;
    if settings.skip_vote_mode == SkipVoteMode::Off {
        return Err(ApiError::coded(StatusCode::NOT_FOUND, "not_found", &[]));
    }
    let vote = body
        .get("vote")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request("invalid_vote"))?;
    // Counted before the key is checked: a bad or blocked key costs a request too.
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
    // A vote needs no name: only the key is checked.
    if let Some(fields) = body.as_object_mut() {
        fields.remove("name");
    }
    let viewer = check_viewer(&state, &body)?;
    // One device, one vote: a device key is required.
    let key = viewer
        .key_hash
        .ok_or_else(|| ApiError::bad_request("invalid_device_key"))?;
    let current = on_air(&state).map(|f| f.0);
    let asked = body.get("itemId").and_then(Value::as_i64);
    let Some(item) = current.filter(|item| asked.is_none_or(|a| a == *item)) else {
        return Err(ApiError::coded(
            StatusCode::CONFLICT,
            "skip_vote_stale",
            &[],
        ));
    };
    {
        let mut store = state
            .skip_votes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        store.sync(Some(item));
        if vote {
            // One vote per address: another key from the same address changes nothing.
            store.voters.entry(ctx.ip.clone()).or_insert(key);
        } else {
            store.voters.remove(&ctx.ip);
        }
    }
    let view = skip_vote_json(&state, &settings, Some(&ctx.ip)).unwrap_or(Value::Null);
    Ok(json_ok(&json!({ "skipVote": view })))
}
