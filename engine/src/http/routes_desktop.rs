//! The program's own routes (spec 9): settings, tools, encoder test, cookies picker, log,
//! program updates, the first-run wizard, the tunnel and its permanent-link wizard. The window
//! has them all under `/api/desktop/*` ([`router`]: no session is 401, a remote session 403).
//! The remote panel is the same interface (spec section 2): [`panel_router`] gives it the same
//! handlers under `/api/admin/*` (any admin session) — all but what stays in the window:
//! the Cloudflare login and what needs it (create, delete), the cookies file picker (a dialog
//! on the computer), the first-run wizard, and in the settings everything but
//! [`REMOTE_SETTINGS`] ([`put_panel_config`]): the port, the home network access, the
//! password and the remote panel itself among them.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Map, Value, json};

use super::auth::SessionKind;
use super::{
    ApiError, ApiResult, AppState, Ctx, JsonBody, PanelSession, ReqLang, asset_response, json_ok,
    json_response,
};
use crate::app::{AppCore, ConfigPatch, OpError, ToolsJob};
use crate::app_settings::TunnelModeSetting;
use crate::i18n::{Lang, Msg, error_msg};
use crate::tools::Tool;
use crate::tunnel::TunnelState;
use crate::tunnel::named::{ADD_DOMAIN_HELP, ErrorCode, NamedError, Phase};

const ENCODERS: [&str; 5] = ["auto", "libx264", "h264_nvenc", "h264_qsv", "h264_amf"];
const MIN_PASSWORD_CHARS: usize = 8;

/// The settings a remote session may change (spec section 2). An allowlist: anything else —
/// the port, the home network access, the password, the remote panel and the trust in
/// Cloudflare's headers (they decide who can reach the panel at all and who a request comes
/// from), read-only and unknown fields, and any field added later — stays with the window
/// until it is added here.
const REMOTE_SETTINGS: [&str; 11] = [
    "language",
    "autostart",
    "publicUrl",
    "videoEncoder",
    "ytdlpCookies",
    "pixeldrainApiKey",
    "skippedUpdateVersion",
    "tunnelEnabled",
    "tunnelMode",
    "pauseOnTunnelLoss",
    "pauseWhenNoViewers",
];

/// The window's routes; every one needs the window's session ([`super::require_window`]).
pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/wizard", get(wizard_page))
        .route("/api/desktop/info", get(info))
        .route("/api/desktop/config", get(get_config).put(put_config))
        .route("/api/desktop/tools", get(tools_status))
        .route("/api/desktop/tools/install", post(tools_install))
        .route("/api/desktop/tools/update", post(tools_update))
        .route("/api/desktop/tools/repair", post(tools_repair))
        .route("/api/desktop/tools/progress", get(tools_progress))
        .route("/api/desktop/encoder/detect", post(encoder_detect))
        .route("/api/desktop/pick-cookies", post(pick_cookies))
        .route("/api/desktop/log", get(log))
        .route("/api/desktop/update", get(update_check))
        .route("/api/desktop/update/apply", post(update_apply))
        .route("/api/desktop/wizard/complete", post(wizard_complete))
        .route("/api/desktop/restart", post(restart))
        .route("/api/desktop/tunnel", get(tunnel))
        .route("/api/desktop/tunnel/qr", get(tunnel_qr))
        .route("/api/desktop/tunnel/restart", post(tunnel_restart))
        .route("/api/desktop/tunnel/named/login", post(named_login))
        .route("/api/desktop/tunnel/named/progress", get(named_progress))
        .route("/api/desktop/tunnel/named/create", post(named_create))
        .route("/api/desktop/tunnel/named/quick", post(named_quick))
        .route("/api/desktop/tunnel/named/delete", post(named_delete))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            super::require_window,
        ))
}

/// The same handlers for the panel (`/api/admin/*`, behind the admin session of the admin
/// router): everything but the window-only parts listed above. Only in the app (a server
/// without a window has none of this).
pub(super) fn panel_router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/info", get(info))
        .route(
            "/api/admin/config",
            get(get_panel_config).put(put_panel_config),
        )
        .route("/api/admin/tools", get(tools_status))
        .route("/api/admin/tools/install", post(tools_install))
        .route("/api/admin/tools/update", post(tools_update))
        .route("/api/admin/tools/repair", post(tools_repair))
        .route("/api/admin/tools/progress", get(tools_progress))
        .route("/api/admin/encoder/detect", post(encoder_detect))
        .route("/api/admin/log", get(log))
        .route("/api/admin/update", get(update_check))
        .route("/api/admin/update/apply", post(update_apply))
        .route("/api/admin/restart", post(restart))
        .route("/api/admin/tunnel", get(tunnel))
        .route("/api/admin/tunnel/restart", post(tunnel_restart))
        .route("/api/admin/tunnel/named/quick", post(named_quick))
}

impl From<OpError> for ApiError {
    fn from(e: OpError) -> Self {
        match e {
            OpError::Invalid(m) => ApiError::from_msg(StatusCode::BAD_REQUEST, m),
            OpError::Conflict(m) => ApiError::from_msg(StatusCode::CONFLICT, m),
            OpError::Failed(m) => ApiError::from_msg(StatusCode::INTERNAL_SERVER_ERROR, m),
        }
    }
}

/// The settings as the window sees them: never the password hash or the session secret.
fn config_view(core: &AppCore) -> Map<String, Value> {
    let s = core.settings();
    let view = json!({
        "port": core.port(),
        "lanAccess": s.lan_access,
        "publicUrl": s.public_url,
        "remoteAdmin": s.remote_admin,
        "hasPassword": s.admin_password_hash.is_some(),
        "videoEncoder": s.video_encoder,
        "detectedEncoder": s.detected_encoder,
        "ytdlpCookies": s.ytdlp_cookies.as_ref().map(|p| p.to_string_lossy().into_owned()),
        // Write-only: a new key or `null` (clear) can be sent, the key itself never comes back.
        "hasPixeldrainKey": s.pixeldrain_api_key.is_some(),
        "autostart": s.autostart,
        "trustCloudflare": s.trust_cloudflare,
        "firstRunCompleted": s.first_run_completed,
        "toolsInstalled": s.tools_installed,
        "skippedUpdateVersion": s.skipped_update_version,
        "tunnelEnabled": s.tunnel_enabled,
        "tunnelMode": s.tunnel_mode.as_str(),
        "namedTunnelHostname": s.named_tunnel_hostname,
        "pauseOnTunnelLoss": s.pause_on_tunnel_loss,
        "pauseWhenNoViewers": s.pause_when_no_viewers,
        "language": s.language.as_str(),
    });
    match view {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The tunnel for the window: state, address, mode, host name, warning (texts in `lang`).
/// Never the paths or contents of the named tunnel's files.
pub(super) fn tunnel_view(core: &AppCore, lang: Lang) -> Value {
    let info = core.tunnel_info();
    let (state, url, message) = match info.state {
        TunnelState::Off => ("off", None, None),
        TunnelState::Connecting => ("connecting", None, None),
        TunnelState::Online { url } => ("online", Some(url), None),
        TunnelState::Reconnecting => ("reconnecting", None, None),
        TunnelState::Error { message } => ("error", None, Some(message.render(lang))),
    };
    json!({
        "state": state,
        "url": url,
        "message": message,
        "mode": info.mode.as_str(),
        "hostname": info.hostname,
        "warning": info.warning.map(|w| w.render(lang)),
    })
}

async fn info(State(state): State<AppState>, ctx: Ctx, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.desktop()?;
    let s = core.settings();
    let update = core
        .offered_update()
        .map(|r| json!({ "version": r.version.to_string(), "notes": r.notes }));
    let (paused, reasons) = core.pause_state();
    let reasons: Vec<&str> = reasons.iter().map(|r| r.as_str()).collect();
    let missing = core.tools.status().missing;
    Ok(json_ok(&json!({
        "version": core.version().to_string(),
        "port": core.port(),
        "publicUrl": s.public_url,
        "lanAccess": s.lan_access,
        "remoteAdmin": s.remote_admin,
        "firstRunCompleted": s.first_run_completed,
        "toolsOk": missing.is_empty(),
        "missingTools": missing.iter().map(|t| t.name()).collect::<Vec<_>>(),
        "toolsRunning": core.tools_job_running(),
        "channelRunning": core.channel_running(),
        "channelError": core.channel_error().map(|m| m.render(lang)),
        "restarting": core.restarting(),
        "encoder": s.effective_encoder().as_str(),
        "detectedEncoder": s.detected_encoder,
        "update": update,
        "tunnel": tunnel_view(&core, lang),
        "tunnelEnabled": s.tunnel_enabled,
        "paused": paused,
        "pauseReasons": reasons,
        "language": s.language.as_str(),
        "tickerSlow": core.ticker_slow(),
        "sessionViaTunnel": ctx.via_tunnel,
    })))
}

async fn get_config(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    Ok(json_ok(&Value::Object(config_view(&core))))
}

/// [`get_config`] for the panel, see [`for_session`].
async fn get_panel_config(
    PanelSession(kind): PanelSession,
    State(state): State<AppState>,
) -> ApiResult {
    let core = state.desktop()?;
    Ok(json_ok(&Value::Object(for_session(
        config_view(&core),
        kind,
    ))))
}

/// A remote session sees the cookies file by its name only: its folder shows the owner's user
/// name and folders (the window shows the whole path).
fn for_session(mut view: Map<String, Value>, kind: SessionKind) -> Map<String, Value> {
    let name = match view.get("ytdlpCookies") {
        Some(Value::String(path)) if kind == SessionKind::Remote => Some(
            std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        _ => None,
    };
    if let Some(name) = name {
        view.insert("ytdlpCookies".into(), json!(name));
    }
    view
}

fn type_error(key: &str) -> ApiError {
    ApiError::coded(StatusCode::BAD_REQUEST, "field_not_text", &[("field", key)])
}

fn flag(map: &Map<String, Value>, key: &str) -> Result<Option<bool>, ApiError> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "field_not_boolean",
            &[("field", key)],
        )),
    }
}

/// `null` or text; text is trimmed and an empty one means "none".
fn optional_text(map: &Map<String, Value>, key: &str) -> Result<Option<Option<String>>, ApiError> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) => {
            let s = s.trim();
            Ok(Some((!s.is_empty()).then(|| s.to_string())))
        }
        Some(_) => Err(type_error(key)),
    }
}

/// The accepted subset of the settings; read-only and unknown fields are ignored (among them
/// `namedTunnelHostname`: the permanent-link wizard sets it together with the tunnel's id).
fn parse_patch(body: &Value) -> Result<ConfigPatch, ApiError> {
    let Value::Object(map) = body else {
        return Err(ApiError::bad_request("invalid_settings"));
    };
    let port = match map.get("port") {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .filter(|p| (1024..=65535).contains(p))
                .and_then(|p| u16::try_from(p).ok())
                .ok_or_else(|| ApiError::bad_request("invalid_port"))?,
        ),
    };
    let public_url = match map.get("publicUrl") {
        None => None,
        Some(Value::String(s)) => {
            let s = s.trim().trim_end_matches('/');
            let valid = s.is_empty()
                || url::Url::parse(s)
                    .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some());
            if !valid {
                return Err(ApiError::bad_request("invalid_public_url"));
            }
            Some(s.to_string())
        }
        Some(_) => return Err(type_error("publicUrl")),
    };
    let password = match map.get("password") {
        None | Some(Value::Null) => None,
        Some(Value::String(p)) if p.chars().count() >= MIN_PASSWORD_CHARS => Some(p.clone()),
        Some(Value::String(_)) => {
            return Err(ApiError::coded(
                StatusCode::BAD_REQUEST,
                "password_too_short",
                &[("min", &MIN_PASSWORD_CHARS.to_string())],
            ));
        }
        Some(_) => return Err(type_error("password")),
    };
    let video_encoder = match map.get("videoEncoder") {
        None => None,
        Some(Value::String(e)) if ENCODERS.contains(&e.as_str()) => Some(e.clone()),
        Some(Value::String(e)) => {
            return Err(ApiError::coded(
                StatusCode::BAD_REQUEST,
                "unknown_encoder",
                &[("encoder", e)],
            ));
        }
        Some(_) => return Err(type_error("videoEncoder")),
    };
    // Set only through the native picker (`pick-cookies`); here it can only be cleared.
    let ytdlp_cookies = match map.get("ytdlpCookies") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(_) => {
            return Err(ApiError::bad_request("cookies_by_picker"));
        }
    };
    let tunnel_mode = match map.get("tunnelMode") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .and_then(TunnelModeSetting::parse)
                .ok_or_else(|| ApiError::bad_request("invalid_tunnel_mode"))?,
        ),
    };
    let language = match map.get("language") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .and_then(Lang::parse)
                .ok_or_else(|| ApiError::bad_request("invalid_language"))?,
        ),
    };
    Ok(ConfigPatch {
        port,
        lan_access: flag(map, "lanAccess")?,
        public_url,
        remote_admin: flag(map, "remoteAdmin")?,
        password,
        video_encoder,
        ytdlp_cookies,
        pixeldrain_api_key: optional_text(map, "pixeldrainApiKey")?,
        autostart: flag(map, "autostart")?,
        trust_cloudflare: flag(map, "trustCloudflare")?,
        skipped_update_version: optional_text(map, "skippedUpdateVersion")?,
        tunnel_enabled: flag(map, "tunnelEnabled")?,
        tunnel_mode,
        pause_on_tunnel_loss: flag(map, "pauseOnTunnelLoss")?,
        pause_when_no_viewers: flag(map, "pauseWhenNoViewers")?,
        language,
    })
}

/// A DNS host name with at least two labels (`kanal.example.com`): letters, digits and inner
/// hyphens, labels of 1–63 characters, 253 in total, a top-level domain with a letter. No
/// scheme, port, path or IP address.
fn is_hostname(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            (1..=63).contains(&l.len())
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        // The top-level domain has a letter: no IP literal, no all-numeric name.
        && labels
            .last()
            .is_some_and(|tld| tld.bytes().any(|b| b.is_ascii_lowercase()))
}

/// Partial update: only the fields present change. Only a new port or network access restarts
/// the server (the answer has the new port); everything else applies to the next request,
/// resolve or clip; a new public address also redraws the slates in the background.
async fn put_config(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    Ok(json_ok(&Value::Object(save_config(&state, &body).await?)))
}

/// Saves a settings patch; the settings afterwards and whether the server restarted.
async fn save_config(state: &AppState, body: &Value) -> Result<Map<String, Value>, ApiError> {
    let core = state.desktop()?;
    let patch = parse_patch(body)?;
    let restarted = core.apply_config(patch).await?;
    let mut view = config_view(&core);
    view.insert("restarted".into(), json!(restarted));
    Ok(view)
}

/// [`put_config`] for the panel: a remote session changes only [`REMOTE_SETTINGS`] (any other
/// field: 403 `window_only`, nothing saved); the window's session anything.
async fn put_panel_config(
    PanelSession(kind): PanelSession,
    State(state): State<AppState>,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let window_only = body
        .as_object()
        .is_some_and(|map| map.keys().any(|k| !REMOTE_SETTINGS.contains(&k.as_str())));
    if kind == SessionKind::Remote && window_only {
        return Err(ApiError::coded(StatusCode::FORBIDDEN, "window_only", &[]));
    }
    let view = save_config(&state, &body).await?;
    Ok(json_ok(&Value::Object(for_session(view, kind))))
}

async fn tools_status(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let status = core.tools.status();
    let tools: Vec<Value> = status
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.tool.name(),
                "installed": t.installed,
                "version": t.version,
                "installedAt": t.installed_at,
            })
        })
        .collect();
    let missing: Vec<&str> = status.missing.iter().map(|t| t.name()).collect();
    Ok(json_ok(&json!({
        "tools": tools,
        "missing": missing,
        "ok": status.missing.is_empty(),
        "running": core.tools_job_running(),
    })))
}

fn start_job(state: &AppState, job: ToolsJob) -> ApiResult {
    let core = state.desktop()?;
    core.start_tools_job(job)
        .map_err(|why| ApiError::from_msg(StatusCode::CONFLICT, why))?;
    Ok(json_response(
        StatusCode::ACCEPTED,
        &json!({ "started": true }),
    ))
}

async fn tools_install(State(state): State<AppState>) -> ApiResult {
    start_job(&state, ToolsJob::Install)
}

async fn tools_repair(State(state): State<AppState>) -> ApiResult {
    start_job(&state, ToolsJob::Repair)
}

/// `{"tool": "yt-dlp" | "ffmpeg" | "deno"}`
async fn tools_update(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let name = body.get("tool").and_then(Value::as_str).unwrap_or("");
    let tool = Tool::ALL
        .into_iter()
        .find(|t| t.name() == name)
        .ok_or_else(|| {
            ApiError::coded(StatusCode::BAD_REQUEST, "unknown_tool", &[("tool", name)])
        })?;
    start_job(&state, ToolsJob::Update(tool))
}

async fn tools_progress(State(state): State<AppState>, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.desktop()?;
    let p = core.tools.progress().borrow().clone();
    Ok(json_ok(&json!({
        "phase": p.phase,
        "tool": p.tool,
        "downloaded": p.downloaded,
        "total": p.total,
        "error": p.error.map(|e| e.render(lang)),
        "done": p.done,
        "running": core.tools_job_running(),
    })))
}

async fn encoder_detect(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let found = core.detect_encoder().await?;
    Ok(json_ok(&json!({
        "detectedEncoder": found.as_str(),
        "encoder": core.settings().effective_encoder().as_str(),
    })))
}

async fn pick_cookies(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let picked = core.pick_cookies().await?;
    let current = core
        .settings()
        .ytdlp_cookies
        .map(|p| p.to_string_lossy().into_owned());
    Ok(json_ok(&json!({
        "picked": picked.is_some(),
        "ytdlpCookies": current,
    })))
}

async fn log(State(state): State<AppState>, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.desktop()?;
    Ok(json_ok(&json!({ "lines": core.log.tail(lang) })))
}

/// Checks GitHub now. A failure (no network, private repository) only goes to the log.
async fn update_check(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let info = core.check_update().await;
    let available = info
        .available
        .map(|r| json!({ "version": r.version.to_string(), "notes": r.notes }));
    Ok(json_ok(&json!({
        "current": info.current.to_string(),
        "available": available,
    })))
}

/// Installs the release found by the last check; the shell then restarts the program.
async fn update_apply(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let version = core.apply_update().await?;
    Ok(json_ok(
        &json!({ "ok": true, "version": version.to_string() }),
    ))
}

async fn wizard_complete(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    core.complete_wizard().await?;
    Ok(json_ok(&json!({ "ok": true })))
}

/// "Uruchom ponownie kanał": the owner decides when the stream is interrupted.
async fn restart(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    core.restart_server().await.map_err(|e| {
        ApiError::coded(StatusCode::INTERNAL_SERVER_ERROR, "restart_failed", &[])
            .with_msg_arg("detail", error_msg(&e))
    })?;
    Ok(json_ok(&json!({ "ok": true, "port": core.port() })))
}

/// The tunnel's state, address, mode and host name (`warning`: the named mode fell back to
/// quick).
async fn tunnel(State(state): State<AppState>, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.desktop()?;
    Ok(json_ok(&tunnel_view(&core, lang)))
}

/// The QR code (PNG) of the request page at the public address in effect, for the wizard; 404
/// while there is no address.
async fn tunnel_qr(State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    super::request_qr_response(&core.live_settings.public_url.get())
}

/// A new cloudflared (a quick tunnel gets a new address); 409 while the tunnel is off.
async fn tunnel_restart(State(state): State<AppState>, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.desktop()?;
    if !core.restart_tunnel().await {
        return Err(ApiError::coded(StatusCode::CONFLICT, "tunnel_off", &[]));
    }
    Ok(json_ok(&tunnel_view(&core, lang)))
}

// ---------- The permanent link (spec section 9) ----------
//
// Answers of the wizard's routes: a failure is `{"error": <text>, "code": <ErrorCode>}` (plus
// `helpUrl` for `no_domain`), a bad request one with the code `invalid`; the progress is
// `named_progress_view`. Texts are in the request's language. Neither ever holds the
// certificate or the credentials, nor their paths.

fn named_error(e: &NamedError, lang: Lang) -> Response {
    let status = match e.code {
        ErrorCode::Busy
        | ErrorCode::NoCloudflared
        | ErrorCode::NotLoggedIn
        | ErrorCode::DnsTaken
        | ErrorCode::Cancelled => StatusCode::CONFLICT,
        ErrorCode::NoDomain => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorCode::Failed => StatusCode::BAD_GATEWAY,
    };
    json_response(
        status,
        &json!({
            "error": e.message.render(lang),
            "code": e.code.as_str(),
            "helpUrl": help_url(Some(e.code)),
        }),
    )
}

/// A bad request to the wizard: code `invalid` (as the wizard always answered), the text
/// says what is wrong.
fn invalid(e: ApiError) -> ApiError {
    e.with_code("invalid")
}

fn help_url(code: Option<ErrorCode>) -> Option<&'static str> {
    (code == Some(ErrorCode::NoDomain)).then_some(ADD_DOMAIN_HELP)
}

/// `state`: idle | running | done | failed | cancelled; `step` while running; `code` after a
/// failure (see [`ErrorCode`]); `loginUrl` while the login waits for the owner, `browserOpened`
/// whether cloudflared opened it in the browser itself (`false`: the window opens it, `null`:
/// not known yet); `loggedIn`: the
/// certificate is there (the next step can start).
fn named_progress_view(core: &AppCore, lang: Lang) -> Value {
    let (p, logged_in) = core.named_progress();
    json!({
        "operation": p.operation.map(|o| o.as_str()),
        "state": p.phase.as_str(),
        "running": p.phase == Phase::Running,
        "step": p.step.map(|s| s.as_str()),
        "message": p.message.map(|m| m.render(lang)),
        "code": p.code.map(|c| c.as_str()),
        "helpUrl": help_url(p.code),
        "loginUrl": p.login_url,
        "browserOpened": p.browser_opened,
        "loggedIn": logged_in,
    })
}

/// Step 1, "Połącz z Cloudflare": starts `cloudflared tunnel login` (202 with the progress; the
/// window then polls `progress` for the login page to open). `{"cancel": true}` cancels a
/// running login instead (200 with the progress, `state` "cancelled"); "Spróbuj ponownie" is
/// another plain POST once it ended.
async fn named_login(
    State(state): State<AppState>,
    ReqLang(lang): ReqLang,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let core = state.desktop()?;
    let cancel = match &body {
        Value::Null => false,
        Value::Object(map) => flag(map, "cancel").map_err(invalid)?.unwrap_or(false),
        _ => return Err(ApiError::bad_request("invalid")),
    };
    if cancel {
        core.cancel_named_login();
        return Ok(json_ok(&named_progress_view(&core, lang)));
    }
    match core.start_named_login() {
        Ok(()) => Ok(json_response(
            StatusCode::ACCEPTED,
            &named_progress_view(&core, lang),
        )),
        Err(e) => Ok(named_error(&e, lang)),
    }
}

async fn named_progress(State(state): State<AppState>, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.desktop()?;
    Ok(json_ok(&named_progress_view(&core, lang)))
}

/// What a finished operation answers: its message and the tunnel as `GET tunnel` shows it.
fn named_done(core: &AppCore, result: Result<Msg, NamedError>, lang: Lang) -> ApiResult {
    Ok(match result {
        Ok(message) => json_ok(&json!({
            "ok": true,
            "message": message.render(lang),
            "tunnel": tunnel_view(core, lang),
        })),
        Err(e) => named_error(&e, lang),
    })
}

/// Step 2, "Wybierz adres": `{"hostname": "kanal.example.com", "overwrite": false}`. Creates (or
/// reuses) the tunnel `vrcinema`, routes the host name to it (`overwrite`: replace another DNS
/// record of that name, after a `dns_taken`) and switches to the permanent link.
async fn named_create(
    State(state): State<AppState>,
    ReqLang(lang): ReqLang,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let core = state.desktop()?;
    let required = || invalid(ApiError::bad_request("hostname_required"));
    let Value::Object(map) = &body else {
        return Err(required());
    };
    let hostname = match map.get("hostname") {
        Some(Value::String(h)) => h.trim().to_ascii_lowercase(),
        _ => return Err(required()),
    };
    if !is_hostname(&hostname) {
        return Err(invalid(ApiError::bad_request("hostname_invalid")));
    }
    let overwrite = flag(map, "overwrite").map_err(invalid)?;
    let result = core
        .create_named(hostname, overwrite.unwrap_or(false))
        .await;
    named_done(&core, result, lang)
}

/// "Wróć do szybkiego tunelu": the permanent link's files stay.
async fn named_quick(State(state): State<AppState>, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.desktop()?;
    let result = core.named_to_quick().await;
    named_done(&core, result, lang)
}

/// "Usuń stały link": the tunnel in Cloudflare, then its files here; the quick tunnel runs.
async fn named_delete(State(state): State<AppState>, ReqLang(lang): ReqLang) -> ApiResult {
    let core = state.desktop()?;
    let result = core.delete_named().await;
    named_done(&core, result, lang)
}

/// The first-run wizard page (`public/wizard.html`, its modules in `public/app/wizard`); like the API it needs the window session.
async fn wizard_page() -> Response {
    asset_response("wizard.html").unwrap_or_else(|| {
        ApiError::coded(StatusCode::NOT_FOUND, "page_not_found", &[]).into_response()
    })
}
