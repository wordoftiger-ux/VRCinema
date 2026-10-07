//! HTTP layer (axum): the same routes, status codes, headers and JSON as `src/http/app.ts`.

pub mod auth;
pub mod client_ip;
mod routes_admin;
mod routes_desktop;
mod routes_public;
mod routes_stream;

use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use tower_http::catch_panic::CatchPanicLayer;

use crate::auto_pause::ViewerActivity;
use crate::db::{QueueItem, SettingsRepo};
use crate::engine::{Command, EngineHandle, Snapshot};
use crate::library::MediaLibrary;
use crate::live::{LiveSettings, LiveValue};
use crate::pipeline::channel::ChannelState;
use crate::resolver::MediaResolver;
use crate::rules::{RateLimiter, system_clock};

use auth::{AdminAuth, DesktopKey, SESSION_COOKIE, SessionKind, parse_cookies, session_kind};
use client_ip::{Trust, get_client_ip, is_https};

const LOGIN_FAILURES: u32 = 5;
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
const BODY_LIMIT: usize = 16 * 1024;

pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// The part of the configuration HTTP needs.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub admin_password: String,
    pub session_secret: String,
    pub public_url: String,
    pub trust_cloudflare: bool,
    pub trust_proxy: bool,
}

impl From<&crate::config::AppConfig> for HttpConfig {
    fn from(c: &crate::config::AppConfig) -> Self {
        Self {
            admin_password: c.admin_password.clone(),
            session_secret: c.session_secret.clone(),
            public_url: c.public_url.clone(),
            trust_cloudflare: c.trust_cloudflare,
            trust_proxy: c.trust_proxy,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<HttpConfig>,
    pub channel_id: i64,
    pub db: Arc<Mutex<Connection>>,
    pub resolver: Arc<dyn MediaResolver>,
    pub engine: EngineHandle,
    pub library: Option<Arc<MediaLibrary>>,
    pub request_limiter: Arc<Mutex<RateLimiter>>,
    pub login_limiter: Arc<tokio::sync::Mutex<RateLimiter>>,
    pub log: LogFn,
    /// Changes at runtime (settings of the desktop app), hence behind a lock.
    admin_auth: Arc<RwLock<AdminAuth>>,
    desktop_key: Arc<DesktopKey>,
    /// The desktop app's engine (`/api/desktop/*`); `None` in `.env` mode, where those routes
    /// do not exist. Weak: the engine owns the server that owns this state.
    desktop: Option<Weak<crate::app::AppCore>>,
    /// Read per request (`trust_cloudflare`, `public_url`): HttpConfig keeps the start values.
    live: LiveSettings,
    /// Segment fetches (viewers), for the automatic pause of the app.
    pub(crate) viewers: Arc<ViewerActivity>,
}

impl AppState {
    /// Limiters start from the stored settings (like `buildApp` in TS); no library, silent log.
    pub fn new(
        config: HttpConfig,
        channel_id: i64,
        db: Arc<Mutex<Connection>>,
        resolver: Arc<dyn MediaResolver>,
        engine: EngineHandle,
    ) -> rusqlite::Result<Self> {
        let s0 = {
            let conn = db.lock().unwrap_or_else(PoisonError::into_inner);
            SettingsRepo::new(&conn).get(channel_id)?
        };
        let request_limiter = RateLimiter::new(
            s0.request_limit_count,
            Duration::from_secs(s0.request_limit_window_s),
            system_clock(),
        );
        let login_limiter = RateLimiter::new(LOGIN_FAILURES, LOGIN_WINDOW, system_clock());
        // Headless (`.env`) default; the key made here is never shown to anyone, so
        // `/desktop-login` stays closed until the app shell installs its own with
        // `with_desktop_key`.
        // Fail closed: no configured password means no remote login (app mode starts that way).
        let admin_auth = if config.admin_password.is_empty() {
            AdminAuth::App {
                remote_enabled: false,
                password_hash: None,
                session_epoch: 0,
            }
        } else {
            AdminAuth::PlainPassword(config.admin_password.clone())
        };
        Ok(Self {
            admin_auth: Arc::new(RwLock::new(admin_auth)),
            desktop_key: Arc::new(DesktopKey::generate().0),
            desktop: None,
            live: LiveSettings {
                trust_cloudflare: LiveValue::new(config.trust_cloudflare),
                cloudflare_from_loopback_only: LiveValue::new(false),
                public_url: LiveValue::new(config.public_url.clone()),
                encoder: LiveValue::new(crate::config::VideoEncoder::Libx264),
                ytdlp_cookies: LiveValue::new(None),
                pixeldrain_api_key: LiveValue::new(None),
            },
            config: Arc::new(config),
            channel_id,
            db,
            resolver,
            engine,
            library: None,
            viewers: Arc::new(ViewerActivity::new()),
            request_limiter: Arc::new(Mutex::new(request_limiter)),
            login_limiter: Arc::new(tokio::sync::Mutex::new(login_limiter)),
            log: Arc::new(|_| {}),
        })
    }

    pub fn with_library(mut self, library: Option<Arc<MediaLibrary>>) -> Self {
        self.library = library;
        self
    }

    /// Shares the settings the desktop app changes at runtime (Cloudflare trust, public
    /// address, ...); `.env` mode keeps the values from the start.
    pub fn with_live(mut self, live: LiveSettings) -> Self {
        self.live = live;
        self
    }

    /// Shares the record of segment fetches with the app's automatic pause.
    pub fn with_viewers(mut self, viewers: Arc<ViewerActivity>) -> Self {
        self.viewers = viewers;
        self
    }

    pub fn with_log(mut self, log: LogFn) -> Self {
        self.log = log;
        self
    }

    pub fn with_admin_auth(self, auth: AdminAuth) -> Self {
        self.set_admin_auth(auth);
        self
    }

    /// The app shell keeps a handle to mint a fresh key whenever it opens the window.
    pub fn with_desktop_key(mut self, key: Arc<DesktopKey>) -> Self {
        self.desktop_key = key;
        self
    }

    /// Takes effect for the next request (remote switch / password changed in settings).
    pub fn set_admin_auth(&self, auth: AdminAuth) {
        *self
            .admin_auth
            .write()
            .unwrap_or_else(PoisonError::into_inner) = auth;
    }

    pub fn admin_auth(&self) -> AdminAuth {
        self.admin_auth
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Turns on the `/api/desktop/*` routes, backed by the app engine.
    pub(crate) fn with_desktop(mut self, core: Weak<crate::app::AppCore>) -> Self {
        self.desktop = Some(core);
        self
    }

    /// The app engine behind the desktop routes (503 while it is going away).
    pub(crate) fn desktop(&self) -> Result<Arc<crate::app::AppCore>, ApiError> {
        self.desktop
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "Aplikacja jest zamykana.")
            })
    }

    pub fn desktop_key(&self) -> &Arc<DesktopKey> {
        &self.desktop_key
    }

    /// Runs `f` with the database locked. The lock is never held across an `.await`.
    fn with_db<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, ApiError> {
        let conn = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        f(&conn).map_err(ApiError::from)
    }

    fn settings(&self) -> Result<crate::db::ChannelSettings, ApiError> {
        self.with_db(|c| SettingsRepo::new(c).get(self.channel_id))
    }

    /// Latest published engine state.
    fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.engine.snapshot.borrow())
    }

    /// Fire-and-forget: HTTP never waits for the engine loop.
    fn send_command(&self, command: Command) {
        if let Err(e) = self.engine.commands.try_send(command.clone()) {
            (self.log)(&format!(
                "Nie można przekazać polecenia {command:?} do kanału: {e}"
            ));
        }
    }

    /// `CF-Connecting-IP` counts for a request from `peer`: trust switched on, and when it comes
    /// from the app's tunnel only for loopback peers (cloudflared), so nobody on the LAN can
    /// forge a viewer address.
    fn trusts_cloudflare(&self, peer: &str) -> bool {
        self.live.trust_cloudflare.get()
            && (!self.live.cloudflare_from_loopback_only.get() || is_loopback(peer))
    }

    fn trusted(&self, peer: &str) -> bool {
        self.trusts_cloudflare(peer) || self.config.trust_proxy
    }

    /// Key remote-session tokens are signed with: the session secret in `.env` mode; in the app
    /// one derived from it, the password hash and the epoch (a new password or switching the
    /// panel on again ends the remote sessions issued before).
    pub(crate) fn remote_session_secret(&self, auth: &AdminAuth) -> String {
        match auth {
            AdminAuth::PlainPassword(_) => self.config.session_secret.clone(),
            AdminAuth::App {
                password_hash,
                session_epoch,
                ..
            } => auth::remote_session_secret(
                &self.config.session_secret,
                password_hash.as_deref(),
                *session_epoch,
            ),
        }
    }

    /// Kind of the valid session on the request, if any. With the remote panel switched off
    /// only window sessions count (a remote cookie issued earlier stops working).
    fn session(&self, headers: &HeaderMap) -> Option<SessionKind> {
        let cookies = parse_cookies(headers.get(COOKIE).and_then(|v| v.to_str().ok()));
        let token = cookies.get(SESSION_COOKIE).map(String::as_str);
        let now = crate::db::now_ms().max(0) as u64;
        let auth = self.admin_auth();
        match &auth {
            AdminAuth::PlainPassword(_) => session_kind(token, &self.config.session_secret, now),
            AdminAuth::App { remote_enabled, .. } => {
                // Window sessions: the plain secret. Remote ones: only the derived key.
                if session_kind(token, &self.config.session_secret, now)
                    == Some(SessionKind::Desktop)
                {
                    return Some(SessionKind::Desktop);
                }
                let key = self.remote_session_secret(&auth);
                (*remote_enabled && session_kind(token, &key, now) == Some(SessionKind::Remote))
                    .then_some(SessionKind::Remote)
            }
        }
    }

    fn is_admin(&self, headers: &HeaderMap) -> bool {
        self.session(headers).is_some()
    }
}

/// Extractor for routes only the app window may use (`/api/desktop/*`): a remote session is
/// 403, no session 401.
pub(crate) struct DesktopSession;

impl FromRequestParts<AppState> for DesktopSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match state.session(&parts.headers) {
            Some(SessionKind::Desktop) => Ok(Self),
            Some(SessionKind::Remote) => Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "Ta funkcja jest dostępna tylko w oknie aplikacji.",
            )),
            None => Err(ApiError::new(StatusCode::UNAUTHORIZED, "Zaloguj się.")),
        }
    }
}

// ---------- Errors ----------

pub(crate) type ApiResult<T = Response> = Result<T, ApiError>;

/// Logged by [`log_internal_errors`]; carried on the 500 response.
#[derive(Clone)]
struct InternalError(String);

pub(crate) enum ApiError {
    Http(StatusCode, String),
    Internal(String),
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self::Http(status, message.into())
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Http(status, message) => json_response(status, &json!({ "error": message })),
            Self::Internal(message) => {
                let mut res = json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &json!({ "error": "Błąd serwera" }),
                );
                res.extensions_mut().insert(InternalError(message));
                res
            }
        }
    }
}

async fn log_internal_errors(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let mut res = next.run(req).await;
    if let Some(InternalError(message)) = res.extensions_mut().remove::<InternalError>() {
        (state.log)(&format!("Błąd serwera: {message}"));
    }
    res
}

/// The socket peer is this machine (`127.0.0.1`, `::1`, also IPv4-mapped).
fn is_loopback(peer: &str) -> bool {
    peer.strip_prefix("::ffff:")
        .unwrap_or(peer)
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

pub(crate) fn json_response(status: StatusCode, value: &Value) -> Response {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"null".to_vec());
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    res.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    res
}

pub(crate) fn json_ok(value: &Value) -> Response {
    json_response(StatusCode::OK, value)
}

/// Fastify's default 404 (also used for a known path with another method).
async fn not_found(method: Method, uri: Uri) -> Response {
    let url = uri.path_and_query().map_or("/", |p| p.as_str());
    json_response(
        StatusCode::NOT_FOUND,
        &json!({
            "message": format!("Route {method}:{url} not found"),
            "error": "Not Found",
            "statusCode": 404
        }),
    )
}

/// A page or static file from the embedded `public/` (like @fastify/static: charset on text
/// types, `Cache-Control: public, max-age=0`).
pub(crate) fn asset_response(path: &str) -> Option<Response> {
    let (bytes, mime) = crate::assets::asset(path)?;
    Some(
        (
            [(CONTENT_TYPE, mime), (CACHE_CONTROL, "public, max-age=0")],
            bytes.into_owned(),
        )
            .into_response(),
    )
}

async fn static_file(
    axum::extract::Path(path): axum::extract::Path<String>,
    method: Method,
    uri: Uri,
) -> Response {
    match asset_response(&path) {
        Some(res) => res,
        None => not_found(method, uri).await,
    }
}

// ---------- Extractors ----------

/// JSON request body with Fastify's error behaviour; an absent body is `null`.
pub(crate) struct JsonBody(pub Value);

impl<S: Send + Sync> FromRequest<S> for JsonBody {
    type Rejection = ApiError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, ApiError> {
        let (parts, body) = req.into_parts();
        let content_type = parts
            .headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_ascii_lowercase());
        let bytes = to_bytes(body, BODY_LIMIT).await.map_err(|_| {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "Request body is too large")
        })?;
        let is_json = content_type
            .as_deref()
            .is_some_and(|c| c == "application/json" || c.starts_with("application/json;"));
        if bytes.is_empty() {
            return if is_json {
                Err(ApiError::bad_request(
                    "Body cannot be empty when content-type is set to 'application/json'",
                ))
            } else {
                Ok(Self(Value::Null))
            };
        }
        if !is_json {
            // Like Fastify: text/plain reaches the handler as a string; anything else is 415.
            let is_text = content_type
                .as_deref()
                .is_some_and(|c| c == "text/plain" || c.starts_with("text/plain;"));
            return if is_text {
                Ok(Self(Value::String(
                    String::from_utf8_lossy(&bytes).into_owned(),
                )))
            } else {
                Err(ApiError::new(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "Unsupported Media Type",
                ))
            };
        }
        serde_json::from_slice(&bytes).map(Self).map_err(|_| {
            ApiError::bad_request(
                "Body is not valid JSON but content-type is set to 'application/json'",
            )
        })
    }
}

/// Who is asking and how they reached us.
pub(crate) struct Ctx {
    pub ip: String,
    pub base_url: String,
    pub https: bool,
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let remote = parts
            .extensions
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_default();
        let cfg = &state.config;
        let trust = Trust {
            cloudflare: state.trusts_cloudflare(&remote),
            proxy: cfg.trust_proxy,
        };
        let ip = get_client_ip(&parts.headers, &remote, trust);
        let https = is_https(&parts.headers, false, state.trusted(&remote));
        let public_url = state.live.public_url.get();
        let base_url = if public_url.is_empty() {
            let forwarded = state
                .trusted(&remote)
                .then(|| {
                    parts
                        .headers
                        .get("x-forwarded-host")
                        .and_then(|v| v.to_str().ok())
                })
                .flatten()
                .filter(|h| !h.is_empty());
            let host = forwarded
                .map(str::to_string)
                .or_else(|| {
                    parts
                        .headers
                        .get(HOST)
                        .and_then(|v| v.to_str().ok())
                        .filter(|h| !h.is_empty())
                        .map(str::to_string)
                })
                .or_else(|| parts.uri.authority().map(|a| a.to_string()))
                .unwrap_or_else(|| "localhost".to_string());
            format!("{}://{host}", if https { "https" } else { "http" })
        } else {
            public_url
        };
        Ok(Self {
            ip,
            base_url,
            https,
        })
    }
}

// ---------- JSON shapes ----------

/// A number the way JS prints it: whole values as integers.
pub(crate) fn num_json(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_992.0 {
        Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
    }
}

fn opt_num_json(v: Option<f64>) -> Value {
    v.map_or(Value::Null, num_json)
}

pub(crate) fn public_item(i: &QueueItem) -> Value {
    json!({
        "id": i.id,
        "title": i.title,
        "durationS": opt_num_json(i.duration_s),
        "requesterName": i.requester_name,
        "isLive": i.is_live,
    })
}

pub(crate) fn admin_item(i: &QueueItem) -> Value {
    let mut item = public_item(i);
    if let Value::Object(map) = &mut item {
        map.insert("sourceUrl".into(), json!(i.source_url));
        map.insert("status".into(), json!(i.status.as_str()));
        map.insert("addedBy".into(), json!(i.added_by.as_str()));
        map.insert("requesterIp".into(), json!(i.requester_ip));
        map.insert("error".into(), json!(i.error));
        map.insert("createdAt".into(), json!(i.created_at));
        map.insert("finishedAt".into(), json!(i.finished_at));
    }
    item
}

/// `{ status, current }` like the TS channel state plus `paused` / `pauseReasons` (fields only
/// added); the caller adds more keys.
pub(crate) fn state_map(state: &ChannelState) -> Map<String, Value> {
    let current = state.current.as_ref().map_or(Value::Null, |c| {
        json!({
            "itemId": c.item_id,
            "title": c.title,
            "durationS": opt_num_json(c.duration_s),
            "positionS": c.position_s,
            "requesterName": c.requester_name,
            "isLive": c.is_live,
        })
    });
    let mut map = Map::new();
    map.insert("status".into(), json!(state.status.as_str()));
    map.insert("current".into(), current);
    map.insert("paused".into(), json!(state.paused));
    map.insert(
        "pauseReasons".into(),
        json!(
            state
                .pause_reasons
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
        ),
    );
    map
}

// ---------- Router ----------

pub fn build_router(state: AppState) -> Router {
    let log = Arc::clone(&state.log);
    let admin = routes_admin::router().route_layer(middleware::from_fn_with_state(
        state.clone(),
        routes_admin::require_admin,
    ));
    // Only in the desktop app: the headless server keeps answering 404 there, as in stage 1.
    let desktop = if state.desktop.is_some() {
        routes_desktop::router()
    } else {
        Router::new()
    };
    Router::new()
        .merge(routes_public::router())
        .merge(routes_stream::router())
        .merge(admin)
        .merge(desktop)
        .route("/static/{*path}", get(static_file))
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            log_internal_errors,
        ))
        .layer(CatchPanicLayer::custom(
            move |panic: Box<dyn std::any::Any + Send>| {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "nieznana panika".to_string());
                log(&format!("Błąd serwera: {message}"));
                json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &json!({ "error": "Błąd serwera" }),
                )
            },
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests;
