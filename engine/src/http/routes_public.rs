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
    AdminAuth, SESSION_TTL_MS, check_password, create_desktop_session_token, create_session_token,
    session_cookie, verify_password,
};
use super::{
    ApiError, ApiResult, AppState, Ctx, JsonBody, asset_response, json_ok, json_response, num_json,
    public_item, state_map,
};
use crate::db::{AddedBy, ItemStatus, LinkMode, NewItem, QueueRepo};
use crate::i18n::Msg;
use crate::nicks::{NICK_MAX_CHARS, NickError, NickStore, hash_device_key, skeleton};
use crate::rules::{
    LinkError, check_host_resolves_public, check_viewer_link_offline, parse_source_url,
    validate_meta,
};
use url::Url;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/admin", get(admin_page))
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .route("/app-icon.png", get(app_icon))
        .route("/api/channels/main/state", get(channel_state))
        .route("/api/channels/main/requests", post(post_request))
        .route("/desktop-login", get(desktop_login))
        .route("/api/admin/login", post(login))
        .route("/api/admin/logout", post(logout))
        .merge(super::viewer_lists::public_router())
}

fn page(file: &str) -> Response {
    asset_response(file).unwrap_or_else(|| {
        ApiError::coded(StatusCode::NOT_FOUND, "page_not_found", &[]).into_response()
    })
}

async fn index() -> Response {
    page("index.html")
}

/// The program's icon, the one file the window shell's taskbar icon comes from too
/// (`desktop/icons/icon.png`): the title bar and the page icon show it.
pub(crate) const APP_ICON: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../desktop/icons/icon.png"
));

async fn app_icon() -> Response {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/png"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        APP_ICON,
    )
        .into_response()
}

/// The player interface: the program's window and the remote panel (which signs in there).
async fn admin_page() -> Response {
    page("app.html")
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
            "linkMode": settings.link_mode.as_str(),
            "allowedDomains": settings.allowed_domains,
            "maxDurationS": num_json(settings.max_duration_s),
            "moderation": settings.moderation,
            "viewerLists": settings.viewer_lists,
            "viewerListMax": settings.viewer_list_max,
        }),
    );
    Ok(json_ok(&Value::Object(body)))
}

/// Control and invisible format characters removed, trimmed, at most 32 UTF-16 units; empty
/// means no name.
pub(super) fn clean_name(raw: Option<&Value>) -> Option<String> {
    let raw = raw?.as_str()?;
    let stripped: String = raw
        .chars()
        .filter(|c| !c.is_control() && !crate::nicks::is_invisible(*c))
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

/// The nick and device key of a request.
pub(super) struct Viewer {
    pub(super) nick: Option<String>,
    pub(super) key_hash: Option<String>,
}

pub(super) fn nick_error(e: NickError) -> ApiError {
    match e {
        NickError::Taken => ApiError::coded(StatusCode::CONFLICT, "nick_taken", &[]),
        NickError::Reserved => ApiError::coded(StatusCode::CONFLICT, "nick_reserved", &[]),
        NickError::Blocked => ApiError::coded(StatusCode::FORBIDDEN, "device_blocked", &[]),
        // The PIN or code checked a moment ago is not the stored one any more.
        NickError::Changed => ApiError::coded(StatusCode::FORBIDDEN, "nick_pin_wrong", &[]),
        NickError::Db(e) => ApiError::Internal(e),
    }
}

/// Reads the viewer's nick and device key and checks them without reserving anything: a
/// malformed key is 400, a blocked device 403, a nick reserved for the owner or held by another
/// device 409 (in the viewer's language).
pub(super) fn check_viewer(state: &AppState, body: &Value) -> Result<Viewer, ApiError> {
    let key_hash = match body.get("deviceKey") {
        None | Some(Value::Null) => None,
        Some(Value::String(key)) => {
            Some(hash_device_key(key).ok_or_else(|| ApiError::bad_request("invalid_device_key"))?)
        }
        Some(_) => return Err(ApiError::bad_request("invalid_device_key")),
    };
    let nick = clean_name(body.get("name"))
        .map(|n| n.chars().take(NICK_MAX_CHARS).collect::<String>())
        .filter(|n| !skeleton(n).is_empty());
    // A nick is only kept for a device: the page always sends its key with one.
    if nick.is_some() && key_hash.is_none() {
        return Err(ApiError::bad_request("invalid_device_key"));
    }
    let now = crate::db::now_ms();
    state
        .with_db(|c| {
            let store = NickStore::new(c);
            Ok(match (&nick, &key_hash) {
                (Some(nick), key) => store.check(nick, key.as_deref(), now),
                (None, Some(key)) => store.is_blocked(key).map_or_else(
                    |e| Err(NickError::from(e)),
                    |blocked| {
                        if blocked {
                            Err(NickError::Blocked)
                        } else {
                            Ok(())
                        }
                    },
                ),
                (None, None) => Ok(()),
            })
        })?
        .map_err(nick_error)?;
    Ok(Viewer { nick, key_hash })
}

/// Reserves the nick for the device (or renews it). Only for a request that is going to be
/// queued: one the limiter or the link validation refused must not reserve or refresh a nick.
pub(super) fn claim_viewer(state: &AppState, viewer: &Viewer) -> Result<(), ApiError> {
    let (Some(nick), Some(key)) = (&viewer.nick, &viewer.key_hash) else {
        return Ok(());
    };
    let now = crate::db::now_ms();
    state
        .with_db(|c| Ok(NickStore::new(c).claim(nick, key, now)))?
        .map_err(nick_error)
}

/// The address a viewer sent. What is no address at all (or the internal `local:` kind) is an
/// invalid link; another scheme (`file:`, `ftp:`, ...) is refused as such.
fn viewer_link(raw: Option<&str>) -> Result<Url, ApiError> {
    let invalid = || ApiError::bad_request("invalid_link");
    let text = raw.ok_or_else(invalid)?;
    if let Some(url) = parse_source_url(text) {
        return Ok(url);
    }
    match Url::parse(text.trim()) {
        Ok(url) if url.scheme() != "local" => {
            Err(ApiError::bad_request(LinkError::BadScheme.code()))
        }
        _ => Err(invalid()),
    }
}

fn link_error(e: LinkError, settings: &crate::db::ChannelSettings) -> ApiError {
    let bad_request = |code| ApiError::coded(StatusCode::BAD_REQUEST, code, &[]);
    match e {
        LinkError::DomainNotAllowed if settings.link_mode == LinkMode::Blocklist => {
            bad_request("domain_blocked").with_code("domain_not_allowed")
        }
        LinkError::DomainNotAllowed => ApiError::coded(
            StatusCode::BAD_REQUEST,
            "domain_not_allowed",
            &[("domains", &settings.allowed_domains.join(", "))],
        ),
        other => bad_request(other.code()),
    }
}

async fn post_request(
    State(state): State<AppState>,
    ctx: Ctx,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let settings = state.settings()?;
    let url = viewer_link(body.get("url").and_then(Value::as_str))?;
    check_viewer_link_offline(&url, &settings).map_err(|e| link_error(e, &settings))?;
    let viewer = check_viewer(&state, &body)?;
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
    // The name lookup comes after the limiter and before the resolver touches the address.
    check_host_resolves_public(&url, state.host_resolver.as_ref())
        .await
        .map_err(|e| link_error(e, &settings))?;
    let meta = state
        .resolver
        .fetch_meta(url.as_str())
        .await
        .map_err(|e| ApiError::bad_request("fetch_failed").with_msg_arg("reason", e.0))?;
    if let Some(reason) = validate_meta(&meta, &settings, false) {
        return Err(ApiError::from_msg(StatusCode::BAD_REQUEST, reason));
    }
    claim_viewer(&state, &viewer)?;
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
            requester_name: viewer.nick.clone(),
            requester_ip: Some(ip.clone()),
            requester_key_hash: viewer.key_hash.clone(),
        })
    })?;
    (state.log)(
        Msg::new("log.request_received")
            .arg("viewer", state.viewer_tag(&ip))
            .arg("title", &item.title)
            .arg_msg("status", item.status.msg()),
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
/// redirect target does not carry the key. A wrong, used or missing key gets no session but
/// the same redirect: the window then shows the interface's "session ended" card (with its
/// title bar — the window has no frame) instead of a bare error.
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
    let mut res = StatusCode::FOUND.into_response();
    let headers = res.headers_mut();
    headers.insert(LOCATION, HeaderValue::from_static("/admin"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if !key.is_some_and(|k| state.desktop_key().redeem(&k)) {
        return res;
    }
    let now = crate::db::now_ms().max(0) as u64;
    let token = create_desktop_session_token(&state.config.session_secret, SESSION_TTL_MS, now);
    let cookie = session_cookie(&token, ctx.https, SESSION_TTL_MS / 1000);
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        res.headers_mut().insert(SET_COOKIE, value);
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
