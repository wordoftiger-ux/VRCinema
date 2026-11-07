//! HTTP layer (axum): the same routes, status codes, headers and JSON as `src/http/app.ts`.

pub mod auth;
pub mod client_ip;
mod routes_admin;
mod routes_desktop;
mod routes_monitor;
mod routes_public;
mod routes_queue;
mod routes_stream;
mod viewer_lists;

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
use crate::i18n::{Lang, Msg};
use crate::library::MediaLibrary;
use crate::live::{LiveSettings, LiveValue};
use crate::pipeline::channel::ChannelState;
use crate::resolver::MediaResolver;
use crate::rules::{HostResolver, RateLimiter, SystemHostResolver, system_clock};

use auth::{AdminAuth, DesktopKey, SESSION_COOKIE, SessionKind, parse_cookies, session_kind};
use client_ip::{Trust, get_client_ip, is_https};

const LOGIN_FAILURES: u32 = 5;
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
const BODY_LIMIT: usize = 16 * 1024;

pub type LogFn = Arc<dyn Fn(Msg) + Send + Sync>;

/// The part of the configuration HTTP needs.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub admin_password: String,
    pub session_secret: String,
    pub public_url: String,
    pub trust_cloudflare: bool,
    pub trust_proxy: bool,
    /// The app's language: of answers to requests that name none they can be read in.
    pub language: Lang,
}

impl From<&crate::config::AppConfig> for HttpConfig {
    fn from(c: &crate::config::AppConfig) -> Self {
        Self {
            admin_password: c.admin_password.clone(),
            session_secret: c.session_secret.clone(),
            public_url: c.public_url.clone(),
            trust_cloudflare: c.trust_cloudflare,
            trust_proxy: c.trust_proxy,
            language: c.language,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<HttpConfig>,
    pub channel_id: i64,
    pub db: Arc<Mutex<Connection>>,
    pub resolver: Arc<dyn MediaResolver>,
    /// Looks up the host of a viewer's link (the home-network protection).
    pub host_resolver: Arc<dyn HostResolver>,
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
    /// The slate swapper of the running channel (`None` without a channel). The app swaps
    /// through [`crate::app::AppCore`]; a server without a window swaps from the settings route.
    slates: Option<Arc<crate::pipeline::slates::SlateSwapper>>,
    /// Read per request (`trust_cloudflare`, `public_url`): HttpConfig keeps the start values.
    live: LiveSettings,
    /// Segment fetches (viewers), for the automatic pause of the app.
    pub(crate) viewers: Arc<ViewerActivity>,
    /// Previous sample of the encoder's CPU time, for the stats route.
    pub(crate) cpu: Arc<crate::stats::CpuSampler>,
    /// Random per run: viewer tags in the log cannot be matched across restarts.
    viewer_salt: Arc<[u8; 16]>,
    /// How many times a new public address restarted the film for the ticker's code.
    ticker_redraws: Arc<std::sync::atomic::AtomicUsize>,
    /// Touched by the monitor's routes: the channel feeds the monitor while the window watches.
    pub(crate) monitor_watch: Arc<crate::pipeline::monitor::MonitorWatch>,
    /// The owner's bulk removals that can still be undone.
    pub(crate) queue_undo: Arc<routes_queue::UndoStore>,
    /// The films the window's Preview plays itself (`None`: only the monitor's segments).
    pub(crate) preview: Option<crate::preview_source::PreviewSources>,
    /// Each viewer's latency, from its segment requests (the air clock).
    pub(crate) air_viewers: Arc<crate::pipeline::air::AirViewers>,
    /// The black picture / silence alarms (Settings → Advanced), set by the air observer.
    pub(crate) alarms: Arc<crate::pipeline::air_watch::AlarmBoard>,
    /// The OBS input's hub (its listener's state for the window); `None` without one.
    pub(crate) obs: Option<crate::obs::ObsHub>,
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
            slates: None,
            live: LiveSettings {
                trust_cloudflare: LiveValue::new(config.trust_cloudflare),
                cloudflare_from_loopback_only: LiveValue::new(false),
                public_url: LiveValue::new(config.public_url.clone()),
                encoder: LiveValue::new(crate::config::VideoEncoder::Libx264),
                ytdlp_cookies: LiveValue::new(None),
                pixeldrain_api_key: LiveValue::new(None),
                language: LiveValue::new(config.language),
                obs_key: LiveValue::new(String::new()),
                obs_lan: LiveValue::new(false),
                obs_priority: LiveValue::new(true),
            },
            obs: None,
            config: Arc::new(config),
            channel_id,
            db,
            resolver,
            host_resolver: Arc::new(SystemHostResolver),
            engine,
            library: None,
            viewers: Arc::new(ViewerActivity::new()),
            cpu: Arc::default(),
            viewer_salt: Arc::new(random_salt()),
            ticker_redraws: Arc::default(),
            monitor_watch: Arc::default(),
            queue_undo: Arc::default(),
            preview: None,
            air_viewers: Arc::default(),
            alarms: Arc::default(),
            request_limiter: Arc::new(Mutex::new(request_limiter)),
            login_limiter: Arc::new(tokio::sync::Mutex::new(login_limiter)),
            log: Arc::new(|_| {}),
        })
    }

    /// Replaces the system's name lookup (tests use a table).
    pub fn with_host_resolver(mut self, host_resolver: Arc<dyn HostResolver>) -> Self {
        self.host_resolver = host_resolver;
        self
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

    /// Shares the alarms of the air observer.
    pub fn with_alarms(mut self, alarms: Arc<crate::pipeline::air_watch::AlarmBoard>) -> Self {
        self.alarms = alarms;
        self
    }

    /// Shares the monitor's watch with the channel.
    pub fn with_monitor_watch(
        mut self,
        watch: Arc<crate::pipeline::monitor::MonitorWatch>,
    ) -> Self {
        self.monitor_watch = watch;
        self
    }

    /// The OBS input (the window shows its listener's state).
    pub fn with_obs(mut self, hub: Option<crate::obs::ObsHub>) -> Self {
        self.obs = hub;
        self
    }

    /// The films the window's Preview plays (`/api/desktop/preview*`).
    pub fn with_preview(mut self, preview: crate::preview_source::PreviewSources) -> Self {
        self.preview = Some(preview);
        self
    }

    /// The swapper of the slate set the channel started with.
    pub fn with_slates(
        mut self,
        slates: Option<Arc<crate::pipeline::slates::SlateSwapper>>,
    ) -> Self {
        self.slates = slates;
        self
    }

    pub(crate) fn slates(&self) -> Option<Arc<crate::pipeline::slates::SlateSwapper>> {
        self.slates.clone()
    }

    /// The slate language changed: a server without a window makes the new slate set in the
    /// background and hands it to the channel (the app does the same through `AppCore`).
    /// The address is the one of `.env`; nothing is made when the set on air already fits.
    pub(crate) fn refresh_slates_headless(&self) {
        if self.desktop.is_some() {
            return;
        }
        let Some(swapper) = self.slates.clone() else {
            return;
        };
        let state = self.clone();
        tokio::spawn(async move {
            let address = || crate::pipeline::slates::SlateAddress {
                public_url: state.live.public_url.get(),
                connecting: false,
                lang: state.slate_language(),
            };
            use crate::pipeline::slates::SwapOutcome;
            match swapper.swap_to(address, &state.engine.commands).await {
                SwapOutcome::Unchanged => {}
                SwapOutcome::Replaced {
                    generation,
                    address_changed,
                    reconnecting,
                } => {
                    (state.log)(Msg::new("log.slates_refreshed").arg("generation", generation));
                    for notice in swapper.notices() {
                        (state.log)(notice);
                    }
                    state.after_slates_replaced(address_changed, reconnecting);
                }
                SwapOutcome::Failed {
                    reason,
                    address_changed,
                    reconnecting,
                } => {
                    (state.log)(Msg::new("log.slates_refresh_failed").arg_msg("reason", reason));
                    state.after_slates_replaced(address_changed, reconnecting);
                }
            }
        });
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

    /// The app engine, if this server belongs to one (the stats show its tunnel).
    pub(crate) fn app_core(&self) -> Option<Arc<crate::app::AppCore>> {
        self.desktop.as_ref().and_then(Weak::upgrade)
    }

    /// The app engine behind the desktop routes (503 while it is going away).
    pub(crate) fn desktop(&self) -> Result<Arc<crate::app::AppCore>, ApiError> {
        self.desktop
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| ApiError::coded(StatusCode::SERVICE_UNAVAILABLE, "app_closing", &[]))
    }

    /// The app's language (the desktop app changes it at runtime).
    pub(crate) fn language(&self) -> Lang {
        self.live.language.get()
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

    /// The channel's slate language (English when the settings cannot be read).
    pub(crate) fn slate_language(&self) -> crate::i18n::Lang {
        let conn = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        SettingsRepo::new(&conn)
            .get(self.channel_id)
            .map_or_else(|_| crate::i18n::Lang::default(), |s| s.slate_language)
    }

    /// A new slate set went on air. For another public address the QR code the info ticker
    /// draws is out of date too: the film on air starts again from where it is with the new
    /// one (`Command::ApplyTicker`, joined with other changes within 700 ms).
    ///
    /// `reconnecting`: the tunnel is switched on but has no address (connecting, reconnecting
    /// after a quick tunnel's process died, or in the `Error` state before its next start —
    /// `live::apply_tunnel` treats every enabled tunnel without a URL alike): the film keeps the
    /// old code and waits for the next address (one restart instead of two). Only a tunnel
    /// switched off (the manual address, or none, applies) restarts it at once, so the code of
    /// the tunnel's address does not stay on the stream. Accepted (final wave, fix round 2): in
    /// the `Error` state the old code stays until the tunnel is online again; it shows only with
    /// the pause on tunnel loss off, and an error is followed by a new start.
    pub(crate) fn after_slates_replaced(&self, address_changed: bool, reconnecting: bool) {
        if !address_changed || reconnecting {
            return;
        }
        let draws_code = self.settings().is_ok_and(|s| {
            s.ticker.enabled && s.ticker.qr != crate::pipeline::ticker::TickerQr::Off
        });
        if draws_code
            && self.send_command(Command::ApplyTickerCode {
                public_url: self.live.public_url.get(),
            })
        {
            self.ticker_redraws
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// How many times a new public address restarted the film for the ticker's code (the
    /// tunnel tests, which run on Windows only).
    #[cfg(all(test, windows))]
    pub(crate) fn ticker_redraws(&self) -> usize {
        self.ticker_redraws
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Stores whether the info ticker starts switched on, unless the channel has a value
    /// already (the app's first encoder detection; a failure leaves it to the next start).
    pub(crate) fn seed_ticker_enabled(&self, gpu: bool) {
        let conn = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = SettingsRepo::new(&conn).seed_ticker_enabled(self.channel_id, gpu);
    }

    fn settings(&self) -> Result<crate::db::ChannelSettings, ApiError> {
        self.with_db(|c| SettingsRepo::new(c).get(self.channel_id))
    }

    /// Latest published engine state.
    fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.engine.snapshot.borrow())
    }

    /// Fire-and-forget: never blocks on the engine loop. Returns whether the loop got it.
    fn send_command(&self, command: Command) -> bool {
        match self.engine.commands.try_send(command.clone()) {
            Ok(()) => true,
            Err(e) => {
                (self.log)(
                    Msg::new("log.command_not_sent")
                        .arg("command", format!("{command:?}"))
                        .arg("error", e),
                );
                false
            }
        }
    }

    /// An anonymous tag of a viewer for the log: the first 8 hex digits of SHA-256(salt + ip).
    /// The same viewer has the same tag during one run; the address itself is never logged.
    pub(crate) fn viewer_tag(&self, ip: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.viewer_salt.as_slice());
        hasher.update(ip.as_bytes());
        hasher.finalize()[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// `CF-Connecting-IP` counts for a request from `peer`: trust switched on, and when it comes
    /// from the app's tunnel only for loopback peers (cloudflared), so nobody on the LAN can
    /// forge a viewer address.
    fn trusts_cloudflare(&self, peer: &str) -> bool {
        self.live.trust_cloudflare.get()
            && (!self.live.cloudflare_from_loopback_only.get() || is_loopback(peer))
    }

    /// The request came through the app's own tunnel: cloudflared (a loopback peer) passed it
    /// on with Cloudflare's `CF-Connecting-IP` while the tunnel's trust is on. The program's
    /// window (loopback, no header) and a LAN device forging the header never count.
    fn through_tunnel(&self, peer: &str, headers: &HeaderMap) -> bool {
        self.live.trust_cloudflare.get()
            && self.live.cloudflare_from_loopback_only.get()
            && is_loopback(peer)
            && headers
                .get("cf-connecting-ip")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| !v.trim().is_empty())
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
            Some(SessionKind::Remote) => {
                Err(ApiError::coded(StatusCode::FORBIDDEN, "window_only", &[]))
            }
            None => Err(ApiError::coded(
                StatusCode::UNAUTHORIZED,
                "login_required",
                &[],
            )),
        }
    }
}

/// Middleware of the window's routes (`/api/desktop/*`, `/wizard`): [`DesktopSession`] for
/// every one of them, before the body is read.
pub(crate) async fn require_window(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let (mut parts, body) = req.into_parts();
    if let Err(e) = DesktopSession::from_request_parts(&mut parts, &state).await {
        return e.into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}

/// The kind of the admin session of a panel route (window or remote); no session is 401.
pub(crate) struct PanelSession(pub SessionKind);

impl FromRequestParts<AppState> for PanelSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        state
            .session(&parts.headers)
            .map(Self)
            .ok_or_else(|| ApiError::coded(StatusCode::UNAUTHORIZED, "login_required", &[]))
    }
}

// ---------- Language of the request ----------

/// Header the panel and the request page send with the language shown (spec section 6).
pub(crate) const LANG_HEADER: &str = "x-vrcinema-lang";

/// The language to answer in: [`LANG_HEADER`], else `Accept-Language`, else the app's.
pub(crate) fn request_lang(headers: &HeaderMap, app: Lang) -> Lang {
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    header(LANG_HEADER)
        .and_then(Lang::parse)
        .or_else(|| header("accept-language").and_then(Lang::from_accept_language))
        .unwrap_or(app)
}

/// Extractor of [`request_lang`].
pub(crate) struct ReqLang(pub Lang);

impl FromRequestParts<AppState> for ReqLang {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(request_lang(&parts.headers, state.language())))
    }
}

// ---------- Errors ----------

pub(crate) type ApiResult<T = Response> = Result<T, ApiError>;

/// Logged by [`log_internal_errors`]; carried on the 500 response.
#[derive(Clone)]
struct InternalError(String);

/// The code and message of an error answer, put into the request's language by
/// [`localize_errors`].
#[derive(Clone)]
struct ErrorMessage(Option<&'static str>, Msg);

/// An error answer: JSON `{"error": <text in the request's language>, "code": <code>}`. The
/// text is the key `errors.<code>` of the dictionaries, unless [`ApiError::with_code`] gave the
/// answer a broader code (whose key `errors.<code>` then holds a generic text).
pub(crate) enum ApiError {
    Coded {
        status: StatusCode,
        /// `None`: the message's own code.
        code: Option<&'static str>,
        msg: Msg,
    },
    /// 500 `internal`; the cause is only logged.
    Internal(String),
}

impl ApiError {
    pub(crate) fn coded(
        status: StatusCode,
        code: &'static str,
        args: &[(&'static str, &str)],
    ) -> Self {
        let msg = args
            .iter()
            .fold(Msg::error(code), |msg, (name, value)| msg.arg(name, value));
        Self::from_msg(status, msg)
    }

    pub(crate) fn bad_request(code: &'static str) -> Self {
        Self::coded(StatusCode::BAD_REQUEST, code, &[])
    }

    /// An error message (`Msg::error`) of another part of the program as the answer.
    pub(crate) fn from_msg(status: StatusCode, msg: Msg) -> Self {
        debug_assert!(
            msg.code().is_some(),
            "{} is not an error message",
            msg.key()
        );
        Self::Coded {
            status,
            code: None,
            msg,
        }
    }

    /// The answer's `code` is `code` — one a route promised before its messages got codes of
    /// their own (`bad_request` of the controls, `invalid` of the permanent-link wizard); the
    /// text stays the message's.
    pub(crate) fn with_code(self, code: &'static str) -> Self {
        match self {
            Self::Coded { status, msg, .. } => Self::Coded {
                status,
                code: Some(code),
                msg,
            },
            other => other,
        }
    }

    /// A field filled with a message in the same language.
    pub(crate) fn with_msg_arg(self, name: &'static str, value: Msg) -> Self {
        self.map_msg(|msg| msg.arg_msg(name, value))
    }

    /// The number that picks the plural form of the message.
    pub(crate) fn with_count(self, n: i64) -> Self {
        self.map_msg(|msg| msg.count(n))
    }

    fn map_msg(self, f: impl FnOnce(Msg) -> Msg) -> Self {
        match self {
            Self::Coded { status, code, msg } => Self::Coded {
                status,
                code,
                msg: f(msg),
            },
            other => other,
        }
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

fn error_body(code: Option<&'static str>, msg: &Msg, lang: Lang) -> Value {
    json!({ "error": msg.render(lang), "code": code.or(msg.code()) })
}

/// The answer with the message in English; [`localize_errors`] puts it into the request's
/// language.
fn error_response(status: StatusCode, code: Option<&'static str>, msg: Msg) -> Response {
    let mut res = json_response(status, &error_body(code, &msg, Lang::En));
    res.extensions_mut().insert(ErrorMessage(code, msg));
    res
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Coded { status, code, msg } => error_response(status, code, msg),
            Self::Internal(message) => {
                let mut res = error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    None,
                    Msg::error("internal"),
                );
                res.extensions_mut().insert(InternalError(message));
                res
            }
        }
    }
}

/// Error answers in the language of the request (see [`request_lang`]).
async fn localize_errors(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let lang = request_lang(req.headers(), state.language());
    let mut res = next.run(req).await;
    if let Some(ErrorMessage(code, msg)) = res.extensions_mut().remove::<ErrorMessage>() {
        let bytes =
            serde_json::to_vec(&error_body(code, &msg, lang)).unwrap_or_else(|_| b"null".to_vec());
        res.headers_mut().remove(axum::http::header::CONTENT_LENGTH);
        *res.body_mut() = Body::from(bytes);
    }
    res
}

async fn log_internal_errors(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let mut res = next.run(req).await;
    if let Some(InternalError(message)) = res.extensions_mut().remove::<InternalError>() {
        (state.log)(Msg::new("log.server_error").arg("error", message));
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

/// The QR code (PNG, never cached) of the request page at the public address `address`; 404
/// while there is none. Shared by the window's route and the panel's.
pub(crate) fn request_qr_response(address: &str) -> ApiResult {
    if address.is_empty() {
        return Err(ApiError::coded(
            StatusCode::NOT_FOUND,
            "no_public_address",
            &[],
        ));
    }
    let png = crate::pipeline::slates::qr_png(&crate::pipeline::slates::request_url(address))
        .map_err(|e| {
            ApiError::coded(StatusCode::INTERNAL_SERVER_ERROR, "qr_failed", &[])
                .with_msg_arg("detail", e.message())
        })?;
    Ok((
        [(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")],
        png,
    )
        .into_response())
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
        let bytes = to_bytes(body, BODY_LIMIT)
            .await
            .map_err(|_| ApiError::coded(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", &[]))?;
        let is_json = content_type
            .as_deref()
            .is_some_and(|c| c == "application/json" || c.starts_with("application/json;"));
        if bytes.is_empty() {
            return if is_json {
                Err(ApiError::bad_request("empty_json_body"))
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
                Err(ApiError::coded(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "unsupported_media_type",
                    &[],
                ))
            };
        }
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|_| ApiError::bad_request("invalid_json"))
    }
}

/// Who is asking and how they reached us.
pub(crate) struct Ctx {
    pub ip: String,
    pub base_url: String,
    pub https: bool,
    /// Through the app's own tunnel (see [`AppState::through_tunnel`]).
    pub via_tunnel: bool,
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
            via_tunnel: state.through_tunnel(&remote, &parts.headers),
        })
    }
}

fn random_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("system random number generator");
    salt
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
        "requesterBadge": requester_badge(i.added_by, i.requester_key_hash.as_deref()),
        "isLive": i.is_live,
    })
}

/// The icon of whoever added an item: `"host"` for the owner, `{hue, shape}` for a viewer with
/// a device key, `null` for an anonymous one. Never the key or its hash.
fn requester_badge(added_by: crate::db::AddedBy, key_hash: Option<&str>) -> Value {
    if added_by == crate::db::AddedBy::Admin {
        return json!("host");
    }
    key_hash.map_or(Value::Null, |hash| {
        let b = crate::nicks::badge(hash);
        json!({ "hue": b.hue, "shape": b.shape })
    })
}

pub(crate) fn admin_item(i: &QueueItem, lang: Lang) -> Value {
    let mut item = public_item(i);
    if let Value::Object(map) = &mut item {
        map.insert("sourceUrl".into(), json!(i.source_url));
        map.insert("status".into(), json!(i.status.as_str()));
        map.insert("addedBy".into(), json!(i.added_by.as_str()));
        map.insert("error".into(), json!(i.error_text(lang)));
        map.insert("createdAt".into(), json!(i.created_at));
        map.insert("finishedAt".into(), json!(i.finished_at));
        map.insert("inS".into(), opt_num_json(i.in_s));
        map.insert("outS".into(), opt_num_json(i.out_s));
        map.insert("startAt".into(), json!(i.start_at));
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
            "requesterBadge": requester_badge(c.added_by, c.requester_key_hash.as_deref()),
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

/// The owner's controls for the panel: the film they act on (`itemId`, `isLive`), the server
/// position (or the target of a waiting seek) and what is possible now, plus the pause state.
pub(crate) fn control_json(state: &ChannelState) -> Value {
    let c = &state.control;
    json!({
        "itemId": c.item_id,
        "isLive": c.is_live,
        "positionS": num_json(c.position_s),
        "durationS": opt_num_json(c.duration_s),
        "canPause": c.can_pause,
        "canSeek": c.can_seek,
        "seekPending": c.seek_pending,
        "positionApprox": c.position_approx,
        "pausePending": c.pause_pending,
        "inS": opt_num_json(c.in_s),
        "outS": opt_num_json(c.out_s),
        "paused": state.paused,
        "pauseReasons": state.pause_reasons.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
    })
}

/// The air clock for the panel: where a typical viewer is (`latencyS` behind the stream's
/// edge, `source` `viewers` or `nominal`) and the film position on its screen.
pub(crate) fn air_json(state: &AppState, snapshot: &Snapshot) -> Value {
    let now = std::time::Instant::now();
    let latencies = state.air_viewers.latencies(now);
    let e = crate::pipeline::air::estimate(&snapshot.air, &latencies, now);
    json!({
        "latencyS": num_json(e.latency_s),
        "source": e.source,
        "viewers": e.viewers,
        "itemId": e.item_id,
        "positionS": e.position_s.map(num_json),
        "segmentS": num_json(snapshot.air.segment_s),
    })
}

/// Segments of the playlist the stream's bitrate is measured from (the newest 20 s).
const BITRATE_SEGMENTS: usize = crate::pipeline::segments_for(20);
/// Above this share of the upload the panel warns.
const LINK_WARN_SHARE: f64 = 0.8;

/// The stream's bitrate in bits per second, measured from the newest `n` segments of the
/// playlist (their size on disk over their length); `None` while there are none.
pub(crate) fn stream_bitrate(playlist: &str, dir: &std::path::Path, n: usize) -> Option<f64> {
    let mut segments = Vec::new();
    let mut duration = None;
    for line in playlist.lines() {
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            duration = rest
                .split(',')
                .next()
                .and_then(|d| d.trim().parse::<f64>().ok());
        } else if !line.starts_with('#')
            && !line.trim().is_empty()
            && let Some(d) = duration.take()
        {
            segments.push((line.trim().to_string(), d));
        }
    }
    let (mut bytes, mut seconds) = (0u64, 0.0);
    for (name, d) in segments.iter().rev().take(n) {
        // Only a plain file name of the playlist's folder.
        let name = name.rsplit('/').next().unwrap_or(name);
        if let Ok(meta) = std::fs::metadata(dir.join(name)) {
            bytes += meta.len();
            seconds += d;
        }
    }
    (seconds > 0.0).then(|| bytes as f64 * 8.0 / seconds)
}

/// `ok`, `warn` (above 80 % of the upload), `over` (above the upload) or `unknown` (no upload
/// speed given).
pub(crate) fn link_level(needed_mbps: f64, upload_mbps: f64) -> &'static str {
    if upload_mbps <= 0.0 {
        "unknown"
    } else if needed_mbps > upload_mbps {
        "over"
    } else if needed_mbps > upload_mbps * LINK_WARN_SHARE {
        "warn"
    } else {
        "ok"
    }
}

/// What the viewers take from the owner's link: viewers fetching segments, the stream's
/// measured bitrate, what they need together, what is being sent and the upload given in
/// the settings (Mbit/s). Never used to refuse anything.
pub(crate) fn link_json(state: &AppState, snapshot: &Snapshot, upload_mbps: f64) -> Value {
    let viewers = state.viewers.stats(std::time::Instant::now());
    let stream_mbps =
        stream_bitrate(&snapshot.playlist, &snapshot.live_dir, BITRATE_SEGMENTS).map(|b| b / 1e6);
    let needed = stream_mbps.map(|s| s * viewers.watching as f64);
    let round = |v: f64| (v * 10.0).round() / 10.0;
    json!({
        "viewers": viewers.watching,
        "streamMbps": stream_mbps.map(|v| num_json(round(v))),
        "neededMbps": needed.map(|v| num_json(round(v))),
        "sendingMbps": num_json(round(viewers.bytes_per_s as f64 * 8.0 / 1e6)),
        "uploadMbps": num_json(upload_mbps),
        "level": link_level(needed.unwrap_or(0.0), upload_mbps),
    })
}

// ---------- Router ----------

pub fn build_router(state: AppState) -> Router {
    let log = Arc::clone(&state.log);
    // Only in the desktop app: the headless server keeps answering 404 there, as in stage 1
    // (the program's routes for the window, and the same ones for the remote panel).
    let app = state.desktop.is_some();
    let admin = if app {
        routes_admin::router().merge(routes_desktop::panel_router())
    } else {
        routes_admin::router()
    }
    .merge(routes_queue::router())
    .merge(viewer_lists::admin_router())
    .route_layer(middleware::from_fn_with_state(
        state.clone(),
        routes_admin::require_admin,
    ));
    let desktop = if app {
        routes_desktop::router(state.clone()).merge(routes_monitor::router(state.clone()))
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
                    .unwrap_or_else(|| "unknown panic".to_string());
                log(Msg::new("log.server_error").arg("error", message));
                ApiError::coded(StatusCode::INTERNAL_SERVER_ERROR, "internal", &[]).into_response()
            },
        ))
        // Outermost: also the answer of a panic is put into the request's language.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            localize_errors,
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests;
