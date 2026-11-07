//! The PIN of a viewer's nick (spec `2026-11-07-nick-pin-design.md`): setting it (with a
//! one-time recovery code) and adopting the nick on another device or address with the PIN or
//! the code. The PIN and the code are never logged nor returned (except the new code, once).
//! Any mismatch on adopting is the same answer after the same Argon2 work, so the route tells
//! nothing about a nick beyond what a request already does (taken or free).
//!
//! The guessing guard is locked only to start and to settle an attempt (the attempt counts as
//! a failure from its start); Argon2 runs without it, at most [`crate::nick_pin::ARGON2_PARALLEL`] at a time.
//! The store writes only while the secret checked here is still the stored one.

use std::sync::PoisonError;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::{Value, json};

use super::routes_public::{clean_name, nick_error};
use super::{ApiError, ApiResult, AppState, Ctx, JsonBody, json_ok};
use crate::nick_pin::{
    Attempt, constant_time_eq, dummy_phc, hash_pin, hash_recovery_code, new_recovery_code,
    normalize_recovery_code, validate_pin, verify_pin,
};
use crate::nicks::{
    NICK_MAX_CHARS, NickError, NickStore, Verified, badge, hash_device_key, skeleton,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/channels/main/nick/protect", post(protect))
        .route("/api/channels/main/nick/adopt", post(adopt))
}

fn wrong() -> ApiError {
    ApiError::coded(StatusCode::FORBIDDEN, "nick_pin_wrong", &[])
}

fn locked() -> ApiError {
    ApiError::coded(StatusCode::TOO_MANY_REQUESTS, "nick_pin_locked", &[])
}

fn text<'a>(body: &'a Value, field: &str) -> Option<&'a str> {
    body.get(field).and_then(Value::as_str)
}

/// The device key (required) and the nick (required) of the body.
fn device_and_nick(body: &Value) -> Result<(String, String), ApiError> {
    let key = text(body, "deviceKey")
        .and_then(hash_device_key)
        .ok_or_else(|| ApiError::bad_request("invalid_device_key"))?;
    let nick = clean_name(body.get("name"))
        .map(|n| n.chars().take(NICK_MAX_CHARS).collect::<String>())
        .filter(|n| !skeleton(n).is_empty())
        .ok_or_else(|| ApiError::bad_request("nick_pin_no_nick"))?;
    Ok((key, nick))
}

fn new_pin(pin: Option<&str>) -> Result<&str, ApiError> {
    let pin = pin.ok_or_else(|| ApiError::bad_request("nick_pin_invalid"))?;
    validate_pin(pin).map_err(|e| ApiError::bad_request(e.code()))?;
    Ok(pin)
}

fn badge_json(key_hash: &str) -> Value {
    let b = badge(key_hash);
    json!({ "hue": b.hue, "shape": b.shape })
}

/// Argon2 off the async workers, at most [`crate::nick_pin::ARGON2_PARALLEL`] at a time.
async fn argon2<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce() -> T + Send + 'static,
) -> ApiResult<T> {
    let _permit = state
        .pin_work
        .acquire()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// Starts an attempt in the guard (a brief lock); `nick_pin_locked` when it may not start.
fn begin(state: &AppState, ip: &str, skel: Option<&str>) -> Result<Attempt, ApiError> {
    state
        .pin_guard
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .begin(ip, skel)
        .ok_or_else(locked)
}

/// Settles an attempt: a success gives its counted failure back, a miss keeps it.
fn settle(state: &AppState, attempt: Attempt, ok: bool) {
    let mut guard = state
        .pin_guard
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if ok {
        guard.succeed(attempt);
    } else {
        guard.fail(attempt);
    }
}

/// A new PIN and recovery code: (PHC, code hash, code shown once).
async fn new_secret(state: &AppState, pin: &str) -> ApiResult<(String, String, String)> {
    let pin = pin.to_string();
    let phc = argon2(state, move || hash_pin(&pin)).await?;
    let code = new_recovery_code();
    let normalized = normalize_recovery_code(&code).unwrap_or_default();
    Ok((phc, hash_recovery_code(&normalized), code))
}

/// `{deviceKey, name, pin, currentPin?}`: protects the nick for this device (claiming it
/// when it is free). A nick that already has a PIN needs the current one.
async fn protect(State(state): State<AppState>, ctx: Ctx, JsonBody(body): JsonBody) -> ApiResult {
    let (key_hash, nick) = device_and_nick(&body)?;
    let pin = new_pin(text(&body, "pin"))?;
    let skel = skeleton(&nick);
    let now = crate::db::now_ms();
    // Blocked, reserved or another device's nick: the codes a request gets.
    state
        .with_db(|c| Ok(NickStore::new(c).check(&nick, Some(&key_hash), now)))?
        .map_err(nick_error)?;
    let current_phc = state
        .with_db(|c| NickStore::new(c).held(&skel))?
        .filter(|h| !h.expired(now))
        .and_then(|h| h.pin_hash);
    if let Some(phc) = &current_phc {
        let Some(current) = text(&body, "currentPin").map(str::to_string) else {
            return Err(ApiError::coded(
                StatusCode::CONFLICT,
                "nick_pin_required",
                &[],
            ));
        };
        let attempt = begin(&state, &ctx.ip, Some(&skel))?;
        let phc = phc.clone();
        let ok = argon2(&state, move || verify_pin(&current, &phc)).await;
        let ok = ok.unwrap_or(false);
        settle(&state, attempt, ok);
        if !ok {
            return Err(wrong());
        }
    }
    let may_protect = state
        .pin_guard
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .may_protect(&ctx.ip);
    if !may_protect {
        return Err(locked());
    }
    let (phc, recovery_hash, code) = new_secret(&state, pin).await?;
    state
        .with_db(|c| {
            Ok(NickStore::new(c).protect(
                &nick,
                &key_hash,
                &phc,
                &recovery_hash,
                current_phc.as_deref(),
                now,
            ))
        })?
        .map_err(nick_error)?;
    Ok(json_ok(&json!({
        "nick": nick,
        "recoveryCode": code,
        "requesterBadge": badge_json(&key_hash),
    })))
}

/// `{deviceKey, name, pin, signOutOthers?}` or `{deviceKey, name, recoveryCode, newPin}`:
/// binds this device to the protected nick. The recovery code signs every other device out
/// and gives a new code; it works once, also against concurrent requests.
async fn adopt(State(state): State<AppState>, ctx: Ctx, JsonBody(body): JsonBody) -> ApiResult {
    let (key_hash, nick) = device_and_nick(&body)?;
    let skel = skeleton(&nick);
    let code = text(&body, "recoveryCode");
    // Checked before any attempt: it says nothing about the nick.
    let replacement = match code {
        Some(_) => Some(new_pin(text(&body, "newPin"))?.to_string()),
        None => None,
    };
    let pin = match code {
        Some(_) => None,
        None => Some(
            text(&body, "pin")
                .filter(|p| !p.is_empty())
                .ok_or_else(|| ApiError::bad_request("nick_pin_invalid"))?
                .to_string(),
        ),
    };
    if state.with_db(|c| NickStore::new(c).is_blocked(&key_hash))? {
        return Err(nick_error(NickError::Blocked));
    }
    let now = crate::db::now_ms();
    // A PIN counts against the nick too; a recovery code (80 bits) only against the address,
    // so a stranger locking the nick does not lock its owner out.
    let attempt = begin(&state, &ctx.ip, pin.is_some().then_some(skel.as_str()))?;
    let held = match state.with_db(|c| NickStore::new(c).held(&skel)) {
        Ok(held) => held.filter(|h| h.protected() && !h.expired(now)),
        Err(e) => {
            settle(&state, attempt, false);
            return Err(e);
        }
    };
    // The stored secret the input matched, if it did.
    let matched: Option<String> = match (&pin, code) {
        (Some(pin), _) => {
            let stored = held.as_ref().and_then(|h| h.pin_hash.clone());
            let phc = stored.clone().unwrap_or_else(|| dummy_phc().to_string());
            let pin = pin.clone();
            let ok = argon2(&state, move || verify_pin(&pin, &phc))
                .await
                .unwrap_or(false);
            stored.filter(|_| ok)
        }
        (None, Some(code)) => {
            let presented = normalize_recovery_code(code).map(|n| hash_recovery_code(&n));
            let stored = held.as_ref().and_then(|h| h.recovery_hash.clone());
            match (presented, stored) {
                (Some(p), Some(s)) if constant_time_eq(&p, &s) => Some(s),
                _ => None,
            }
        }
        (None, None) => None,
    };
    settle(&state, attempt, matched.is_some());
    let Some(matched) = matched else {
        return Err(wrong());
    };
    let verified = if pin.is_some() {
        Verified::Pin(&matched)
    } else {
        Verified::RecoveryCode(&matched)
    };
    let fresh = match &replacement {
        Some(pin) => Some(new_secret(&state, pin).await?),
        None => None,
    };
    let sign_out_others =
        fresh.is_some() || body.get("signOutOthers").and_then(Value::as_bool) == Some(true);
    let display = state
        .with_db(|c| {
            Ok(NickStore::new(c).adopt(
                &skel,
                &key_hash,
                verified,
                sign_out_others,
                fresh.as_ref().map(|(p, r, _)| (p.as_str(), r.as_str())),
                now,
            ))
        })?
        .map_err(|e| match e {
            // Released or reset between the check and here.
            NickError::Taken => wrong(),
            other => nick_error(other),
        })?;
    let mut answer = json!({ "nick": display, "requesterBadge": badge_json(&key_hash) });
    if let Some((_, _, code)) = fresh {
        answer["recoveryCode"] = json!(code);
    }
    Ok(json_ok(&answer))
}
