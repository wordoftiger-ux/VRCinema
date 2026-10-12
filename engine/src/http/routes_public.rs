//! Pages, the public API (state, requests) and admin login/logout.

use std::sync::PoisonError;
use std::time::Duration;

use axum::Router;
use axum::extract::{RawQuery, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, REFERRER_POLICY, SET_COOKIE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use super::auth::{
    AdminAuth, SESSION_TTL_MS, app_hint_cookie, check_password, create_desktop_session_token,
    create_session_token, session_cookie, verify_password,
};
use super::{
    ApiError, ApiResult, AppState, Ctx, JsonBody, asset_response, json_ok, json_response, num_json,
    public_item, state_map,
};
use crate::db::{AddedBy, ItemStatus, NewItem, QueueRepo};
use crate::i18n::Msg;
use crate::rules::{is_allowed_domain, parse_source_url, validate_meta};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/admin", get(admin_page))
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .route("/api/channels/main/state", get(channel_state))
        .route("/api/channels/main/requests", post(post_request))
        .route("/desktop-login", get(desktop_login))
        .route("/api/admin/login", post(login))
        .route("/api/admin/logout", post(logout))
}

fn page(file: &str) -> Response {
    asset_response(file).unwrap_or_else(|| {
        ApiError::coded(StatusCode::NOT_FOUND, "page_not_found", &[]).into_response()
    })
}

async fn index() -> Response {
    page("index.html")
}

async fn admin_page() -> Response {
    page("admin.html")
}

async fn channel_state(State(state): State<AppState>, ctx: Ctx) -> ApiResult {
    let settings = state.settings()?;
    let queue = state
        .with_db(|c| QueueRepo::new(c).list_by_status(state.channel_id, &[ItemStatus::Queued]))?;
    let mut body = state_map(&state.snapshot().state);
    body.insert(
        "queue".into(),
        Value::Array(queue.iter().map(public_item).collect()),
    );
    body.insert(
        "channelUrl".into(),
        json!(format!("{}/live/main.m3u8", ctx.base_url)),
    );
    body.insert("requestUrl".into(), json!(format!("{}/", ctx.base_url)));
    body.insert(
        "rules".into(),
        json!({
            "allowedDomains": settings.allowed_domains,
            "maxDurationS": num_json(settings.max_duration_s),
            "moderation": settings.moderation,
        }),
    );
    Ok(json_ok(&Value::Object(body)))
}

/// Control characters removed, trimmed, at most 32 UTF-16 units; empty means no name.
fn clean_name(raw: Option<&Value>) -> Option<String> {
    let raw = raw?.as_str()?;
    let stripped: String = raw
        .chars()
        .filter(|c| !matches!(*c, '\u{0}'..='\u{1f}' | '\u{7f}'))
        .collect();
    let trimmed = stripped.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let mut units = 0;
    let name: String = trimmed
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 32
        })
        .collect();
    (!name.is_empty()).then_some(name)
}

async fn post_request(
    State(state): State<AppState>,
    ctx: Ctx,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let settings = state.settings()?;
    let url = body
        .get("url")
        .and_then(Value::as_str)
        .and_then(parse_source_url)
        .ok_or_else(|| ApiError::bad_request("invalid_link"))?;
    if !is_allowed_domain(&url, &settings.allowed_domains) {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "domain_not_allowed",
            &[("domains", &settings.allowed_domains.join(", "))],
        ));
    }
    let ip = ctx.ip;
    let active = state.with_db(|c| QueueRepo::new(c).count_active_by_ip(state.channel_id, &ip))?;
    if active >= i64::from(settings.max_active_per_requester) {
        return Err(
            ApiError::coded(StatusCode::TOO_MANY_REQUESTS, "too_many_active", &[])
                .with_count(i64::from(settings.max_active_per_requester)),
        );
    }
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
    let meta = state
        .resolver
        .fetch_meta(url.as_str())
        .await
        .map_err(|e| ApiError::bad_request("fetch_failed").with_msg_arg("reason", e.0))?;
    if let Some(reason) = validate_meta(&meta, &settings, false) {
        return Err(ApiError::from_msg(StatusCode::BAD_REQUEST, reason));
    }
    let item = state.with_db(|c| {
        QueueRepo::new(c).add(&NewItem {
            channel_id: state.channel_id,
            source_url: url.to_string(),
            title: meta.title.clone(),
            duration_s: meta.duration_s,
            is_live: meta.is_live,
            status: if settings.moderation {
                ItemStatus::PendingApproval
            } else {
                ItemStatus::Queued
            },
            added_by: AddedBy::Public,
            requester_name: clean_name(body.get("name")),
            requester_ip: Some(ip.clone()),
        })
    })?;
    (state.log)(
        Msg::new("log.request_received")
            .arg("ip", &ip)
            .arg("title", &item.title)
            .arg("status", item.status.as_str()),
    );
    Ok(json_response(
        StatusCode::CREATED,
        &json!({ "status": item.status.as_str(), "item": public_item(&item) }),
    ))
}

async fn login(State(state): State<AppState>, ctx: Ctx, JsonBody(body): JsonBody) -> ApiResult {
    let password = body.get("password").and_then(Value::as_str);
    let auth = state.admin_auth();
    // App mode with the remote panel off (or without a password set): refused before the
    // limiter, so it does not count as a failed attempt.
    if let AdminAuth::App {
        remote_enabled,
        password_hash,
        ..
    } = &auth
        && (!remote_enabled || password_hash.is_none())
    {
        return Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "remote_panel_off",
            &[],
        ));
    }
    {
        // One lock scope for check, comparison and registration: concurrent wrong passwords
        // cannot all pass the check before any of them is registered. The lock is a tokio one
        // so it can be held across the blocking Argon2 verification without stalling workers.
        let mut limiter = state.login_limiter.lock().await;
        if limiter.peek(&ctx.ip) >= super::LOGIN_FAILURES as usize {
            return Err(ApiError::coded(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_logins",
                &[],
            ));
        }
        let ok = match &auth {
            AdminAuth::PlainPassword(expected) => check_password(password, expected),
            AdminAuth::App { password_hash, .. } => match (password, password_hash.as_deref()) {
                (Some(p), Some(h)) => {
                    let (p, h) = (p.to_string(), h.to_string());
                    tokio::task::spawn_blocking(move || verify_password(&p, &h))
                        .await
                        .unwrap_or(false)
                }
                _ => false,
            },
        };
        if !ok {
            limiter.hit(&ctx.ip);
            return Err(ApiError::coded(
                StatusCode::UNAUTHORIZED,
                "wrong_password",
                &[],
            ));
        }
    }
    let now = crate::db::now_ms().max(0) as u64;
    let token = create_session_token(&state.remote_session_secret(&auth), SESSION_TTL_MS, now);
    let cookie = session_cookie(&token, ctx.https, SESSION_TTL_MS / 1000);
    Ok(with_cookie(&cookie))
}

/// One-time login of the app window. Neither the key nor the query string is logged, and the
/// redirect target does not carry the key.
async fn desktop_login(
    State(state): State<AppState>,
    ctx: Ctx,
    RawQuery(query): RawQuery,
) -> Response {
    let key = query.as_deref().and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "key")
            .map(|(_, v)| v.into_owned())
    });
    if !key.is_some_and(|k| state.desktop_key().redeem(&k)) {
        let mut res =
            ApiError::coded(StatusCode::UNAUTHORIZED, "invalid_window_key", &[]).into_response();
        res.headers_mut()
            .insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        return res;
    }
    let now = crate::db::now_ms().max(0) as u64;
    let token = create_desktop_session_token(&state.config.session_secret, SESSION_TTL_MS, now);
    let cookie = session_cookie(&token, ctx.https, SESSION_TTL_MS / 1000);
    let mut res = StatusCode::FOUND.into_response();
    let headers = res.headers_mut();
    headers.insert(LOCATION, HeaderValue::from_static("/admin"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        headers.insert(SET_COOKIE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&app_hint_cookie(ctx.https, SESSION_TTL_MS / 1000)) {
        headers.append(SET_COOKIE, value);
    }
    res
}

async fn logout(ctx: Ctx) -> Response {
    with_cookie(&session_cookie("", ctx.https, 0))
}

fn with_cookie(cookie: &str) -> Response {
    let mut res = json_ok(&json!({ "ok": true }));
    if let Ok(value) = HeaderValue::from_str(cookie) {
        res.headers_mut().insert(SET_COOKIE, value);
    }
    res
}
