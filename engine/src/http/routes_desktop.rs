//! Routes only the app window may use (`/api/desktop/*`, spec 9): settings, tools, encoder
//! test, cookies picker, log, program updates, the first-run wizard. Every handler takes
//! [`DesktopSession`] first: no session is 401, a remote session 403.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde_json::{Map, Value, json};

use super::{ApiError, ApiResult, AppState, DesktopSession, JsonBody, json_ok, json_response};
use crate::app::{AppCore, ConfigPatch, OpError, ToolsJob};
use crate::tools::Tool;

const ENCODERS: [&str; 5] = ["auto", "libx264", "h264_nvenc", "h264_qsv", "h264_amf"];
const MIN_PASSWORD_CHARS: usize = 8;

pub(super) fn router() -> Router<AppState> {
    Router::new()
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
}

impl From<OpError> for ApiError {
    fn from(e: OpError) -> Self {
        match e {
            OpError::Invalid(m) => ApiError::bad_request(m),
            OpError::Conflict(m) => ApiError::new(StatusCode::CONFLICT, m),
            OpError::Failed(m) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, m),
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
        "pixeldrainApiKey": s.pixeldrain_api_key,
        "autostart": s.autostart,
        "trustCloudflare": s.trust_cloudflare,
        "firstRunCompleted": s.first_run_completed,
        "toolsInstalled": s.tools_installed,
        "skippedUpdateVersion": s.skipped_update_version,
    });
    match view {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

async fn info(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let s = core.settings();
    let update = core
        .offered_update()
        .map(|r| json!({ "version": r.version.to_string(), "notes": r.notes }));
    Ok(json_ok(&json!({
        "version": core.version().to_string(),
        "port": core.port(),
        "publicUrl": s.public_url,
        "lanAccess": s.lan_access,
        "remoteAdmin": s.remote_admin,
        "firstRunCompleted": s.first_run_completed,
        "toolsOk": core.tools.status().missing.is_empty(),
        "channelRunning": core.channel_running(),
        "encoder": s.effective_encoder().as_str(),
        "detectedEncoder": s.detected_encoder,
        "update": update,
        "restartRequired": core.restart_required(),
    })))
}

async fn get_config(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    Ok(json_ok(&Value::Object(config_view(&core))))
}

fn type_error(key: &str) -> ApiError {
    ApiError::bad_request(format!("Pole {key} musi być tekstem."))
}

fn flag(map: &Map<String, Value>, key: &str) -> Result<Option<bool>, ApiError> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(ApiError::bad_request(format!(
            "Pole {key} musi mieć wartość true albo false."
        ))),
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

/// The accepted subset of the settings; read-only and unknown fields are ignored.
fn parse_patch(body: &Value) -> Result<ConfigPatch, ApiError> {
    let Value::Object(map) = body else {
        return Err(ApiError::bad_request("Niepoprawne ustawienia."));
    };
    let port = match map.get("port") {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .filter(|p| (1024..=65535).contains(p))
                .and_then(|p| u16::try_from(p).ok())
                .ok_or_else(|| ApiError::bad_request("Port musi być liczbą od 1024 do 65535."))?,
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
                return Err(ApiError::bad_request(
                    "Adres publiczny musi zaczynać się od http:// lub https://.",
                ));
            }
            Some(s.to_string())
        }
        Some(_) => return Err(type_error("publicUrl")),
    };
    let password = match map.get("password") {
        None | Some(Value::Null) => None,
        Some(Value::String(p)) if p.chars().count() >= MIN_PASSWORD_CHARS => Some(p.clone()),
        Some(Value::String(_)) => {
            return Err(ApiError::bad_request(format!(
                "Hasło musi mieć co najmniej {MIN_PASSWORD_CHARS} znaków."
            )));
        }
        Some(_) => return Err(type_error("password")),
    };
    let video_encoder = match map.get("videoEncoder") {
        None => None,
        Some(Value::String(e)) if ENCODERS.contains(&e.as_str()) => Some(e.clone()),
        Some(Value::String(e)) => {
            return Err(ApiError::bad_request(format!("Nieznany enkoder: {e}.")));
        }
        Some(_) => return Err(type_error("videoEncoder")),
    };
    // Set only through the native picker (`pick-cookies`); here it can only be cleared.
    let ytdlp_cookies = match map.get("ytdlpCookies") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(_) => {
            return Err(ApiError::bad_request(
                "Plik cookies wybierz w oknie wyboru pliku.",
            ));
        }
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
    })
}

/// Partial update: only the fields present change. Only a new port or network access restarts
/// the server (the answer has the new port); everything else applies to the next request,
/// resolve or clip. A new public address reaches the slates after a restart: `restartRequired`.
async fn put_config(
    _: DesktopSession,
    State(state): State<AppState>,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let core = state.desktop()?;
    let patch = parse_patch(&body)?;
    let restarted = core.apply_config(patch).await?;
    let mut view = config_view(&core);
    view.insert("restarted".into(), json!(restarted));
    view.insert("restartRequired".into(), json!(core.restart_required()));
    Ok(json_ok(&Value::Object(view)))
}

async fn tools_status(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
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
    if !core.start_tools_job(job) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Trwa już pobieranie narzędzi.",
        ));
    }
    Ok(json_response(
        StatusCode::ACCEPTED,
        &json!({ "started": true }),
    ))
}

async fn tools_install(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    start_job(&state, ToolsJob::Install)
}

async fn tools_repair(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    start_job(&state, ToolsJob::Repair)
}

/// `{"tool": "yt-dlp" | "ffmpeg" | "deno"}`
async fn tools_update(
    _: DesktopSession,
    State(state): State<AppState>,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let name = body.get("tool").and_then(Value::as_str).unwrap_or("");
    let tool = Tool::ALL
        .into_iter()
        .find(|t| t.name() == name)
        .ok_or_else(|| ApiError::bad_request(format!("Nieznane narzędzie: {name}.")))?;
    start_job(&state, ToolsJob::Update(tool))
}

async fn tools_progress(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let p = core.tools.progress().borrow().clone();
    Ok(json_ok(&json!({
        "phase": p.phase,
        "tool": p.tool,
        "downloaded": p.downloaded,
        "total": p.total,
        "error": p.error,
        "done": p.done,
        "running": core.tools_job_running(),
    })))
}

async fn encoder_detect(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let found = core.detect_encoder().await?;
    Ok(json_ok(&json!({
        "detectedEncoder": found.as_str(),
        "encoder": core.settings().effective_encoder().as_str(),
    })))
}

async fn pick_cookies(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
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

async fn log(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    Ok(json_ok(&json!({ "lines": core.log.tail() })))
}

/// Checks GitHub now. A failure (no network, private repository) only goes to the log.
async fn update_check(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
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
async fn update_apply(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    let version = core.apply_update().await?;
    Ok(json_ok(
        &json!({ "ok": true, "version": version.to_string() }),
    ))
}

async fn wizard_complete(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    core.complete_wizard().await?;
    Ok(json_ok(&json!({ "ok": true })))
}

/// "Uruchom ponownie kanał": the owner decides when the stream is interrupted.
async fn restart(_: DesktopSession, State(state): State<AppState>) -> ApiResult {
    let core = state.desktop()?;
    core.restart_server().await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Nie udało się uruchomić serwera ponownie: {e:#}"),
        )
    })?;
    Ok(json_ok(&json!({ "ok": true, "port": core.port() })))
}
