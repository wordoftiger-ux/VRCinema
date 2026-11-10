//! `vrcinema ctl <command>`: steers a running engine (the server without a window or the desktop
//! app's) through its admin API on this computer.
//!
//! Every engine writes `<data dir>/ctl.json` (its local address and a random token, new per run,
//! removed when it stops). The token counts only from this computer and never through the
//! tunnel or a proxy (see `http::AppState::ctl_authorized`). Without the file the admin password
//! (`ADMIN_PASSWORD` of the environment or `.env`, or `--password-stdin`) logs in instead; it is
//! never an argument.

use std::collections::HashMap;
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};

use crate::i18n::{Lang, Msg};

/// The control file in the data folder.
pub const CONTROL_FILE: &str = "ctl.json";

/// The settings `ctl settings get/set` may touch (the server checks the values as for the
/// panel). The OSC address and port are left out on purpose.
pub const SETTINGS: [&str; 12] = [
    "moderation",
    "viewerLists",
    "viewerListMax",
    "maxDurationS",
    "maxActivePerRequester",
    "requestLimitCount",
    "requestLimitWindowS",
    "slateLanguage",
    "tickerEnabled",
    "oscEnabled",
    "oscTemplate",
    "oscRepeatS",
];

// ---------- The engine's side: the control file ----------

/// A new random token (32 bytes, base64url).
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("system random number generator");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The address `ctl` reaches a listener bound to `addr` at: loopback for an unspecified one.
pub fn local_url(addr: SocketAddr) -> String {
    let ip = match addr.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::from([127, 0, 0, 1]),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::from(std::net::Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, addr.port()))
}

/// The control file of one run; removed on drop when it still holds this run's token.
#[derive(Debug)]
pub struct ControlFile {
    path: PathBuf,
    token: String,
}

impl ControlFile {
    /// Writes `<data_dir>/ctl.json` (owner-only on unix; replaced atomically).
    pub fn write(data_dir: &Path, url: &str, token: &str) -> std::io::Result<Self> {
        let path = data_dir.join(CONTROL_FILE);
        let tmp = data_dir.join(format!("{CONTROL_FILE}.tmp"));
        let body = serde_json::to_string_pretty(&json!({
            "version": 1,
            "url": url,
            "token": token,
            "pid": std::process::id(),
        }))
        .unwrap_or_default();
        let _ = std::fs::remove_file(&tmp);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)?;
        Ok(Self {
            path,
            token: token.to_string(),
        })
    }
}

impl Drop for ControlFile {
    fn drop(&mut self) {
        let ours = read_control_file(&self.path).is_some_and(|(_, token)| token == self.token);
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// `(url, token)` of a control file.
fn read_control_file(path: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let url = v.get("url")?.as_str()?.to_string();
    let token = v.get("token")?.as_str()?.to_string();
    (!token.is_empty()).then_some((url, token))
}

// ---------- Arguments ----------

/// The exit codes (documented in `ctl.usage` and the README).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok = 0,
    /// The program refused or failed (an HTTP error).
    Refused = 1,
    Usage = 2,
    /// No running program found, or it cannot be reached.
    NotFound = 3,
    /// The token or password was refused, or none is available.
    Unauthorized = 4,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Seek {
    To(f64),
    By(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Help,
    Status,
    QueueList,
    QueueAdd(String),
    QueueRemove(i64),
    QueueClear,
    Skip,
    Pause,
    Resume,
    Seek(Seek),
    PlayNow(i64),
    SettingsGet(Option<String>),
    SettingsSet(String, Value),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    pub json: bool,
    pub data_dir: Option<PathBuf>,
    pub url: Option<String>,
    pub allow_remote: bool,
    pub password_stdin: bool,
}

/// A failure: its exit code, the message and the server's error code (for `--json`).
#[derive(Debug)]
pub struct Failure {
    pub exit: Exit,
    pub msg: Msg,
    pub code: Option<String>,
}

impl Failure {
    fn new(exit: Exit, msg: Msg) -> Self {
        Self {
            exit,
            msg,
            code: None,
        }
    }

    fn usage(msg: Msg) -> Self {
        Self::new(Exit::Usage, msg)
    }
}

/// `90`, `1:30`, `1:02:03` (from the start) or `+10`, `-1:00` (from here); the last part may
/// have a fraction, the others are whole and below 60.
pub fn parse_time(text: &str) -> Option<Seek> {
    let (sign, rest) = match text.as_bytes().first()? {
        b'+' => (Some(1.0), &text[1..]),
        b'-' => (Some(-1.0), &text[1..]),
        _ => (None, text),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let ok = !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_digit() || (last && b == b'.'));
        let value: f64 = part.parse().ok().filter(|_| ok)?;
        if i > 0 && value >= 60.0 {
            return None;
        }
        total = total * 60.0 + value;
    }
    Some(match sign {
        Some(s) => Seek::By(s * total),
        None => Seek::To(total),
    })
}

fn parse_id(text: &str) -> Result<i64, Failure> {
    text.trim_start_matches('#')
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| Failure::usage(Msg::new("ctl.bad_id").arg("value", text)))
}

fn allowed_setting(key: &str) -> Result<String, Failure> {
    if SETTINGS.contains(&key) {
        Ok(key.to_string())
    } else {
        Err(Failure::usage(
            Msg::new("ctl.setting_not_allowed")
                .arg("key", key)
                .arg("keys", SETTINGS.join(", ")),
        ))
    }
}

/// A value of `settings set`: JSON when it parses (`true`, `30`, `"x"`), else the text.
fn setting_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// Options anywhere, then the command words.
pub fn parse_args(args: &[String]) -> Result<(Options, Command), Failure> {
    let mut opts = Options::default();
    let mut words: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let mut value = |name: &str| {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| Failure::usage(Msg::new("ctl.missing_value").arg("what", name)))
        };
        match arg {
            "--json" => opts.json = true,
            "--allow-remote" => opts.allow_remote = true,
            "--password-stdin" => opts.password_stdin = true,
            "--data-dir" => opts.data_dir = Some(PathBuf::from(value(arg)?)),
            "--url" => opts.url = Some(value(arg)?.trim_end_matches('/').to_string()),
            "-h" | "--help" | "help" => return Ok((opts, Command::Help)),
            // A password on the command line would be visible to every process.
            a if a.starts_with("--") => {
                return Err(Failure::usage(
                    Msg::new("ctl.unknown_option").arg("option", a),
                ));
            }
            a => words.push(a),
        }
        i += 1;
    }
    let need = |at: usize, what: &str| {
        words
            .get(at)
            .copied()
            .ok_or_else(|| Failure::usage(Msg::new("ctl.missing_value").arg("what", what)))
    };
    let command = match words.as_slice() {
        [] => Command::Help,
        ["status"] => Command::Status,
        ["queue"] | ["queue", "list"] => Command::QueueList,
        ["queue", "add", ..] => Command::QueueAdd(need(2, "<url>")?.to_string()),
        ["queue", "remove", ..] => Command::QueueRemove(parse_id(need(2, "<id>")?)?),
        ["queue", "clear"] => Command::QueueClear,
        ["skip"] => Command::Skip,
        ["pause"] => Command::Pause,
        ["resume"] => Command::Resume,
        ["seek", ..] => {
            let text = need(1, "<time>")?;
            Command::Seek(
                parse_time(text)
                    .ok_or_else(|| Failure::usage(Msg::new("ctl.bad_time").arg("value", text)))?,
            )
        }
        ["play-now", ..] => Command::PlayNow(parse_id(need(1, "<id>")?)?),
        ["settings"] | ["settings", "get"] => Command::SettingsGet(None),
        ["settings", "get", key] => Command::SettingsGet(Some(allowed_setting(key)?)),
        ["settings", "set", ..] => Command::SettingsSet(
            allowed_setting(need(2, "<key>")?)?,
            setting_value(need(3, "<value>")?),
        ),
        _ => {
            return Err(Failure::usage(
                Msg::new("ctl.unknown_command").arg("command", words.join(" ")),
            ));
        }
    };
    // Extra words after a complete command are a mistake (e.g. `skip 3`).
    let expected = match &command {
        Command::Help => words.len(),
        Command::Status | Command::Skip | Command::Pause | Command::Resume => 1,
        Command::QueueList => words.len().min(2),
        Command::QueueClear => 2,
        Command::QueueAdd(_) | Command::QueueRemove(_) => 3,
        Command::Seek(_) | Command::PlayNow(_) => 2,
        Command::SettingsGet(None) => words.len().min(2),
        Command::SettingsGet(Some(_)) => 3,
        Command::SettingsSet(..) => 4,
    };
    if words.len() > expected {
        return Err(Failure::usage(
            Msg::new("ctl.unknown_command").arg("command", words.join(" ")),
        ));
    }
    Ok((opts, command))
}

// ---------- Finding the program ----------

/// What `ctl` reads from the computer.
pub struct CtlEnv {
    /// The application root (`.env`, the default `data` folder).
    pub root: PathBuf,
    /// The environment merged with `<root>/.env`.
    pub vars: HashMap<String, String>,
    /// The desktop app's home folder, if one can be named.
    pub app_home: Option<PathBuf>,
    /// Reads the password for `--password-stdin`.
    pub read_stdin: Box<dyn Fn() -> Option<String> + Send + Sync>,
}

impl CtlEnv {
    fn var(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
    }

    /// The data folders a running program may use, in the order they are tried.
    fn data_dirs(&self, opts: &Options) -> Vec<PathBuf> {
        if let Some(dir) = &opts.data_dir {
            return vec![dir.clone()];
        }
        let mut dirs = vec![self.root.join(self.var("DATA_DIR").unwrap_or("data"))];
        if let Some(home) = &self.app_home {
            dirs.push(home.join("data"));
        }
        dirs
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Credential {
    Token(String),
    Password(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub url: String,
    pub credential: Credential,
}

/// Whether `url` points at this computer (`localhost`, 127.0.0.0/8, ::1).
pub fn is_local_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// The program to talk to and how to sign in.
pub fn discover(opts: &Options, env: &CtlEnv) -> Result<Target, Failure> {
    if let Some(url) = &opts.url {
        let valid = url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        if !valid {
            return Err(Failure::usage(Msg::new("ctl.bad_url").arg("url", url)));
        }
        if !opts.allow_remote && !is_local_url(url) {
            return Err(Failure::usage(
                Msg::new("ctl.remote_refused").arg("url", url),
            ));
        }
    }
    let dirs = env.data_dirs(opts);
    if !opts.password_stdin {
        for dir in &dirs {
            if let Some((url, token)) = read_control_file(&dir.join(CONTROL_FILE))
                && opts.url.as_ref().is_none_or(|u| *u == url)
                && is_local_url(&url)
            {
                return Ok(Target {
                    url,
                    credential: Credential::Token(token),
                });
            }
        }
    }
    let password = if opts.password_stdin {
        let read = (env.read_stdin)()
            .map(|p| p.trim_end_matches(['\r', '\n']).to_string())
            .filter(|p| !p.is_empty());
        Some(read.ok_or_else(|| Failure::new(Exit::Unauthorized, Msg::new("ctl.password_empty")))?)
    } else {
        env.var("ADMIN_PASSWORD").map(str::to_string)
    };
    let Some(password) = password else {
        let looked: Vec<String> = dirs
            .iter()
            .map(|d| d.join(CONTROL_FILE).display().to_string())
            .collect();
        return Err(Failure::new(
            Exit::NotFound,
            Msg::new("ctl.not_found").arg("files", looked.join(", ")),
        ));
    };
    let url = opts
        .url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", env.var("PORT").unwrap_or("8080")));
    Ok(Target {
        url,
        credential: Credential::Password(password),
    })
}

// ---------- Talking to the program ----------

struct Client {
    http: reqwest::Client,
    url: String,
    lang: Lang,
    /// `Authorization` or `Cookie`.
    auth: (reqwest::header::HeaderName, String),
}

/// A request that could not be made, or an error answer.
fn transport_failure(url: &str, e: &reqwest::Error) -> Failure {
    let mut source: &dyn std::error::Error = e;
    while let Some(next) = source.source() {
        source = next;
    }
    Failure::new(
        Exit::NotFound,
        Msg::new("ctl.cannot_connect")
            .arg("url", url)
            .arg("error", source),
    )
}

impl Client {
    async fn connect(target: &Target, lang: Lang) -> Result<Self, Failure> {
        // No proxy: the token must never leave this computer.
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            // Adding a video reads its details first (yt-dlp), which can take a while.
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| transport_failure(&target.url, &e))?;
        let auth = match &target.credential {
            Credential::Token(token) => (reqwest::header::AUTHORIZATION, format!("Bearer {token}")),
            Credential::Password(password) => {
                let res = http
                    .post(format!("{}/api/admin/login", target.url))
                    .header(crate::http::LANG_HEADER, lang.as_str())
                    .json(&json!({ "password": password }))
                    .send()
                    .await
                    .map_err(|e| transport_failure(&target.url, &e))?;
                if !res.status().is_success() {
                    let status = res.status();
                    let body: Value = res.json().await.unwrap_or(Value::Null);
                    return Err(refusal(status, &body, true));
                }
                let cookie = res
                    .headers()
                    .get_all(reqwest::header::SET_COOKIE)
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .filter_map(|v| v.split(';').next())
                    .find(|c| c.starts_with(crate::http::auth::SESSION_COOKIE))
                    .map(str::to_string)
                    .ok_or_else(|| {
                        Failure::new(Exit::Unauthorized, Msg::new("ctl.password_refused"))
                    })?;
                (reqwest::header::COOKIE, cookie)
            }
        };
        Ok(Self {
            http,
            url: target.url.clone(),
            lang,
            auth,
        })
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Failure> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.url))
            .header(self.auth.0.clone(), &self.auth.1)
            .header(crate::http::LANG_HEADER, self.lang.as_str());
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req
            .send()
            .await
            .map_err(|e| transport_failure(&self.url, &e))?;
        let status = res.status();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(body)
        } else {
            Err(refusal(status, &body, false))
        }
    }
}

/// An error answer: 401 is "not authorised", anything else "refused" with the server's text.
fn refusal(status: reqwest::StatusCode, body: &Value, login: bool) -> Failure {
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| status.to_string());
    let code = body.get("code").and_then(Value::as_str).map(str::to_string);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let key = if login {
            "ctl.password_refused"
        } else {
            "ctl.token_refused"
        };
        return Failure {
            exit: Exit::Unauthorized,
            msg: Msg::new(key),
            code,
        };
    }
    Failure {
        exit: Exit::Refused,
        msg: Msg::text(error),
        code,
    }
}

const PANEL: &str = "/api/admin/channels/main";

/// Runs `command` and returns the JSON `--json` prints.
async fn execute(client: &Client, command: &Command) -> Result<Value, Failure> {
    use reqwest::Method;
    let post = |path: &'static str, body: Value| client.call(Method::POST, path, Some(body));
    match command {
        Command::Help => Ok(Value::Null),
        Command::Status => {
            let panel = client.call(Method::GET, PANEL, None).await?;
            let count = |k: &str| panel[k].as_array().map_or(0, Vec::len);
            Ok(json!({
                "status": panel["status"],
                "current": panel["current"],
                "paused": panel["paused"],
                "pauseReasons": panel["pauseReasons"],
                "queued": count("queue"),
                "pending": count("pending"),
                "next": panel["next"],
            }))
        }
        Command::QueueList => {
            let panel = client.call(Method::GET, PANEL, None).await?;
            Ok(json!({
                "playing": panel["playing"],
                "queue": panel["queue"],
                "pending": panel["pending"],
            }))
        }
        Command::QueueAdd(url) => {
            post("/api/admin/channels/main/items", json!({ "url": url })).await
        }
        Command::QueueRemove(id) => {
            client
                .call(Method::DELETE, &format!("/api/admin/items/{id}"), None)
                .await
        }
        Command::QueueClear => post("/api/admin/channels/main/clear", json!({})).await,
        Command::Skip => post("/api/admin/channels/main/skip", json!({})).await,
        Command::Pause => post("/api/admin/channels/main/pause", json!({})).await,
        Command::Resume => post("/api/admin/channels/main/resume", json!({})).await,
        Command::Seek(Seek::To(s)) => {
            post("/api/admin/channels/main/seek", json!({ "toS": s })).await
        }
        Command::Seek(Seek::By(s)) => {
            post("/api/admin/channels/main/seek", json!({ "byS": s })).await
        }
        Command::PlayNow(id) => {
            post("/api/admin/channels/main/play-now", json!({ "itemId": id })).await
        }
        Command::SettingsGet(key) => {
            let all = client
                .call(Method::GET, "/api/admin/channels/main/settings", None)
                .await?;
            Ok(settings_subset(&all, key.as_deref()))
        }
        Command::SettingsSet(key, value) => {
            let mut patch = Map::new();
            patch.insert(key.clone(), value.clone());
            let all = client
                .call(
                    Method::PUT,
                    "/api/admin/channels/main/settings",
                    Some(Value::Object(patch)),
                )
                .await?;
            Ok(settings_subset(&all, Some(key)))
        }
    }
}

fn settings_subset(all: &Value, key: Option<&str>) -> Value {
    let keys: Vec<&str> = key.map_or_else(|| SETTINGS.to_vec(), |k| vec![k]);
    Value::Object(
        keys.into_iter()
            .map(|k| (k.to_string(), all.get(k).cloned().unwrap_or(Value::Null)))
            .collect(),
    )
}

// ---------- Output ----------

/// `m:ss` or `h:mm:ss`.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn opt_clock(v: &Value) -> String {
    v.as_f64().map_or_else(|| "?".to_string(), clock)
}

fn item_line(item: &Value, playing: bool) -> String {
    let by = match &item["addedBy"] {
        Value::String(s) if s == "admin" => "host".to_string(),
        _ => item["requesterName"].as_str().unwrap_or("-").to_string(),
    };
    format!(
        "{} #{:<5} {}  [{}]  {}",
        if playing { "▶" } else { " " },
        item["id"],
        crate::osc::one_line(item["title"].as_str().unwrap_or("")),
        opt_clock(&item["durationS"]),
        crate::osc::one_line(&by),
    )
}

/// The text a person reads for the answer of `command`.
fn human(command: &Command, v: &Value, lang: Lang) -> String {
    let r = |m: Msg| m.render(lang);
    match command {
        Command::Help => r(Msg::new("ctl.usage")),
        Command::Status => {
            let mut lines = Vec::new();
            match v["current"].as_object() {
                Some(c) => lines.push(r(Msg::new("ctl.status_now")
                    .arg(
                        "title",
                        crate::osc::one_line(c["title"].as_str().unwrap_or("")),
                    )
                    .arg("position", clock(c["positionS"].as_f64().unwrap_or(0.0)))
                    .arg("duration", opt_clock(&c["durationS"])))),
                None => lines.push(r(Msg::new("ctl.status_idle"))),
            }
            if v["paused"].as_bool() == Some(true) {
                let reasons: Vec<&str> = v["pauseReasons"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                lines.push(r(
                    Msg::new("ctl.status_paused").arg("reasons", reasons.join(", "))
                ));
            }
            lines.push(r(Msg::new("ctl.status_queue")
                .arg("queued", &v["queued"])
                .arg("pending", &v["pending"])));
            lines.join("\n")
        }
        Command::QueueList => {
            let list = |k: &str| v[k].as_array().cloned().unwrap_or_default();
            let lines: Vec<String> = list("playing")
                .iter()
                .map(|i| item_line(i, true))
                .chain(list("queue").iter().map(|i| item_line(i, false)))
                .collect();
            let mut out = lines;
            if list("queue").is_empty() {
                out.push(r(Msg::new("ctl.queue_empty")));
            }
            let pending = list("pending");
            if !pending.is_empty() {
                out.push(r(Msg::new("ctl.status_queue")
                    .arg("queued", list("queue").len())
                    .arg("pending", pending.len())));
            }
            out.join("\n")
        }
        Command::QueueAdd(_) => r(Msg::new("ctl.added")
            .arg(
                "title",
                crate::osc::one_line(v["title"].as_str().unwrap_or("")),
            )
            .arg("count", &v["added"])),
        Command::QueueClear => r(Msg::new("ctl.removed").arg("count", &v["removed"])),
        Command::SettingsGet(_) | Command::SettingsSet(..) => v
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, val)| format!("{k} = {val}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        _ => r(Msg::new("ctl.done")),
    }
}

fn print(out: &mut dyn Write, text: &str) {
    let _ = writeln!(out, "{text}");
}

/// The whole command: parses `args` (after `ctl`), finds the program, runs the command and
/// prints the answer (`out`) or the error (`err`; with `--json` both go to `out` as JSON).
/// Returns the exit code.
pub async fn run(args: &[String], env: &CtlEnv, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let lang = crate::config::env_language(&env.vars);
    let json_wanted = args.iter().any(|a| a == "--json");
    let result = async {
        let (opts, command) = parse_args(args)?;
        if command == Command::Help {
            return Ok((opts, command, Value::Null));
        }
        let target = discover(&opts, env)?;
        let client = Client::connect(&target, lang).await?;
        let value = execute(&client, &command).await?;
        Ok::<_, Failure>((opts, command, value))
    }
    .await;
    match result {
        Ok((opts, command, value)) => {
            if opts.json && command != Command::Help {
                print(
                    out,
                    &serde_json::to_string_pretty(&value).unwrap_or_default(),
                );
            } else {
                print(out, &human(&command, &value, lang));
            }
            Exit::Ok as u8
        }
        Err(f) => {
            let text = f.msg.render(lang);
            if json_wanted {
                let body = json!({
                    "ok": false,
                    "error": text,
                    "code": f.code,
                    "exitCode": f.exit as u8,
                });
                print(
                    out,
                    &serde_json::to_string_pretty(&body).unwrap_or_default(),
                );
            } else {
                print(err, &text);
            }
            f.exit as u8
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn parse(s: &str) -> Result<Command, Exit> {
        parse_args(&args(s)).map(|(_, c)| c).map_err(|f| f.exit)
    }

    #[test]
    fn times_parse() {
        assert_eq!(parse_time("90"), Some(Seek::To(90.0)));
        assert_eq!(parse_time("1:30"), Some(Seek::To(90.0)));
        assert_eq!(parse_time("1:02:03.5"), Some(Seek::To(3723.5)));
        assert_eq!(parse_time("+10"), Some(Seek::By(10.0)));
        assert_eq!(parse_time("-1:00"), Some(Seek::By(-60.0)));
        for bad in [
            "", "1:60", "a", "1::2", "1.5:00", "1:2:3:4", "+", "--5", "1e3",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn commands_parse() {
        assert_eq!(parse("status"), Ok(Command::Status));
        assert_eq!(parse(""), Ok(Command::Help));
        assert_eq!(parse("--help"), Ok(Command::Help));
        assert_eq!(parse("queue"), Ok(Command::QueueList));
        assert_eq!(
            parse("queue add https://youtu.be/x"),
            Ok(Command::QueueAdd("https://youtu.be/x".into()))
        );
        assert_eq!(parse("queue remove #12"), Ok(Command::QueueRemove(12)));
        assert_eq!(parse("seek -10"), Ok(Command::Seek(Seek::By(-10.0))));
        assert_eq!(parse("play-now 3 --json"), Ok(Command::PlayNow(3)));
        assert_eq!(
            parse("settings set oscEnabled true"),
            Ok(Command::SettingsSet("oscEnabled".into(), json!(true)))
        );
        assert_eq!(
            parse("settings set oscTemplate {title}"),
            Ok(Command::SettingsSet("oscTemplate".into(), json!("{title}")))
        );
        for bad in [
            "frobnicate",
            "queue add",
            "queue remove x",
            "seek soon",
            "skip 3",
            "settings set oscHost 1.2.3.4",
            "settings get adminPassword",
            "--password hunter2 status",
            "status --url",
        ] {
            assert_eq!(parse(bad), Err(Exit::Usage), "{bad}");
        }
        let (opts, _) = parse_args(&args("--json --url http://127.0.0.1:9/ status")).unwrap();
        assert!(opts.json);
        assert_eq!(opts.url.as_deref(), Some("http://127.0.0.1:9"));
    }

    fn env_with(root: &Path, vars: &[(&str, &str)], home: Option<PathBuf>) -> CtlEnv {
        CtlEnv {
            root: root.to_path_buf(),
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            app_home: home,
            read_stdin: Box::new(|| Some("from-stdin\n".into())),
        }
    }

    #[test]
    fn the_control_file_is_found_and_removed_with_its_run() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join("data")).unwrap();
        let env = env_with(tmp.path(), &[], Some(home.clone()));
        // Nothing running, no password: not found (exit 3).
        let f = discover(&Options::default(), &env).unwrap_err();
        assert_eq!(f.exit, Exit::NotFound);
        // The app's engine wrote its file.
        let file = ControlFile::write(&home.join("data"), "http://127.0.0.1:4321", "tok").unwrap();
        let t = discover(&Options::default(), &env).unwrap();
        assert_eq!(t.url, "http://127.0.0.1:4321");
        assert_eq!(t.credential, Credential::Token("tok".into()));
        // A file another run replaced is left alone; its own is removed.
        let newer =
            ControlFile::write(&home.join("data"), "http://127.0.0.1:4322", "tok2").unwrap();
        drop(file);
        assert!(home.join("data").join(CONTROL_FILE).exists());
        drop(newer);
        assert!(!home.join("data").join(CONTROL_FILE).exists());
    }

    #[test]
    fn the_password_comes_from_env_or_stdin_and_remote_needs_consent() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env_with(
            tmp.path(),
            &[("ADMIN_PASSWORD", "secret123"), ("PORT", "8099")],
            None,
        );
        let t = discover(&Options::default(), &env).unwrap();
        assert_eq!(t.url, "http://127.0.0.1:8099");
        assert_eq!(t.credential, Credential::Password("secret123".into()));
        let stdin = Options {
            password_stdin: true,
            ..Default::default()
        };
        assert_eq!(
            discover(&stdin, &env).unwrap().credential,
            Credential::Password("from-stdin".into())
        );
        let remote = Options {
            url: Some("http://192.168.1.5:8080".into()),
            ..Default::default()
        };
        assert_eq!(discover(&remote, &env).unwrap_err().exit, Exit::Usage);
        let allowed = Options {
            allow_remote: true,
            ..remote
        };
        assert_eq!(
            discover(&allowed, &env).unwrap().url,
            "http://192.168.1.5:8080"
        );
        // A control file naming another computer is never trusted with the token.
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let _f = ControlFile::write(&data, "http://10.0.0.9:80", "tok").unwrap();
        assert!(matches!(
            discover(&Options::default(), &env).unwrap().credential,
            Credential::Password(_)
        ));
        assert!(is_local_url("http://localhost:1") && is_local_url("http://[::1]:1"));
        assert!(!is_local_url("http://localhost.evil.example"));
    }

    #[test]
    fn local_urls_of_bound_addresses() {
        assert_eq!(
            local_url("0.0.0.0:8080".parse().unwrap()),
            "http://127.0.0.1:8080"
        );
        assert_eq!(local_url("[::]:8080".parse().unwrap()), "http://[::1]:8080");
        assert_eq!(
            local_url("192.168.1.2:80".parse().unwrap()),
            "http://192.168.1.2:80"
        );
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(clock(65.4), "1:05");
    }
}
