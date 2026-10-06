//! The engine inside the desktop app: settings from `config.json`, the HTTP server with the
//! channel (restarted when the port or another server setting changes), the tools jobs and the
//! program updates. The `/api/desktop/*` routes (`http::routes_desktop`) call into [`AppCore`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::Duration;

use anyhow::anyhow;
use semver::Version;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::app_settings::{AppSettings, CONFIG_FILE, app_config};
use crate::config::{AppConfig, VideoEncoder};
use crate::engine::Command;
use crate::http::AppState;
use crate::http::auth::{AdminAuth, DesktopKey};
use crate::live::LiveSettings;
use crate::local_files::{AddReport, add_local_files};
use crate::logging::LogSink;
use crate::self_update::{self, DEFAULT_API_URL, UpdateInfo};
use crate::server::{RunOptions, run};
use crate::tools::{Tool, ToolSources, ToolsManager};

const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 3600);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Downloads may take long, but a stalled connection must not hang a job forever.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const API_TIMEOUT: Duration = Duration::from_secs(15);
/// Lets the answer of `update/apply` reach the window before the shell quits.
const UPDATE_RESTART_DELAY: Duration = Duration::from_millis(500);
/// A server that has not started serving by then is given up on (and stopped).
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a stopped server may take to finish before its task is aborted.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest wait of [`AppEngine::shutdown`] (a restart in progress, a server slow to stop);
/// whatever still runs then ends with the process (the job object kills the children).
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the desktop shell provides to the engine. Calls may block (native dialogs), so the
/// engine runs them off the async workers.
pub trait DesktopHooks: Send + Sync {
    /// Native "open file" dialog for the yt-dlp cookies file; `None` when cancelled.
    fn pick_cookies_file(&self) -> Option<PathBuf>;
    /// Switches start with Windows on or off.
    fn set_autostart(&self, on: bool) -> anyhow::Result<()>;
    /// The running `VRCinema.exe` (target of self-update); `None` where updating is impossible.
    fn exe_path(&self) -> Option<PathBuf>;
    /// The new program is in place: start it with `--after-update` and quit this one. Called
    /// from a background task, shortly after the HTTP answer went out.
    fn restart_after_update(&self);
}

/// Test double for [`DesktopHooks`]: answers from its fields and records the calls.
#[derive(Default)]
pub struct FakeHooks {
    pub cookies_file: Mutex<Option<PathBuf>>,
    pub autostart_calls: Mutex<Vec<bool>>,
    pub exe: Mutex<Option<PathBuf>>,
    pub update_restarts: AtomicUsize,
}

impl DesktopHooks for FakeHooks {
    fn pick_cookies_file(&self) -> Option<PathBuf> {
        lock(&self.cookies_file).clone()
    }
    fn set_autostart(&self, on: bool) -> anyhow::Result<()> {
        lock(&self.autostart_calls).push(on);
        Ok(())
    }
    fn exe_path(&self) -> Option<PathBuf> {
        lock(&self.exe).clone()
    }
    fn restart_after_update(&self) {
        self.update_restarts.fetch_add(1, Ordering::SeqCst);
    }
}

/// Knobs of [`AppEngine::start_with`]; the defaults are the real program's.
pub struct AppOptions {
    /// Version of the program (the desktop crate's), compared with GitHub releases.
    pub version: Version,
    pub update_api_url: String,
    /// Check for a new version at start and every 24 h.
    pub check_updates: bool,
    pub tool_sources: ToolSources,
    /// Tests: a `FakeResolver` file instead of the real resolvers.
    pub fake_resolver: Option<PathBuf>,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            version: Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0)),
            update_api_url: DEFAULT_API_URL.to_string(),
            check_updates: true,
            tool_sources: ToolSources::default(),
            fake_resolver: None,
        }
    }
}

/// How remote login works under these settings.
fn admin_auth(s: &AppSettings) -> AdminAuth {
    AdminAuth::App {
        remote_enabled: s.remote_admin,
        password_hash: s.admin_password_hash.clone(),
        session_epoch: s.remote_session_epoch,
    }
}

/// Settings the running server cannot take over: only the listening address. Everything else
/// is read at use time ([`LiveSettings`]); a new public address waits for the next restart
/// only on the slates ([`AppCore::restart_required`]).
fn needs_restart(old: &AppSettings, new: &AppSettings) -> bool {
    old.port != new.port || old.lan_access != new.lan_access
}

/// A validated change of the settings (`PUT /api/desktop/config`, the cookies picker).
/// `None` leaves a field as it is.
#[derive(Debug, Default, Clone)]
pub(crate) struct ConfigPatch {
    pub port: Option<u16>,
    pub lan_access: Option<bool>,
    pub public_url: Option<String>,
    pub remote_admin: Option<bool>,
    /// A new password for the remote panel (stored as an Argon2id hash).
    pub password: Option<String>,
    pub video_encoder: Option<String>,
    pub ytdlp_cookies: Option<Option<PathBuf>>,
    pub pixeldrain_api_key: Option<Option<String>>,
    pub autostart: Option<bool>,
    pub trust_cloudflare: Option<bool>,
    pub skipped_update_version: Option<Option<String>>,
}

impl ConfigPatch {
    fn apply(self, s: &mut AppSettings, password_hash: Option<String>) {
        let was_remote = s.remote_admin;
        if let Some(v) = self.port {
            s.port = v;
        }
        if let Some(v) = self.lan_access {
            s.lan_access = v;
        }
        if let Some(v) = self.public_url {
            s.public_url = v;
        }
        if let Some(v) = self.remote_admin {
            s.remote_admin = v;
        }
        if let Some(v) = self.video_encoder {
            s.video_encoder = v;
        }
        if let Some(v) = self.ytdlp_cookies {
            s.ytdlp_cookies = v;
        }
        if let Some(v) = self.pixeldrain_api_key {
            s.pixeldrain_api_key = v;
        }
        if let Some(v) = self.autostart {
            s.autostart = v;
        }
        if let Some(v) = self.trust_cloudflare {
            s.trust_cloudflare = v;
        }
        if let Some(v) = self.skipped_update_version {
            s.skipped_update_version = v;
        }
        let new_password = password_hash.is_some();
        if let Some(hash) = password_hash {
            s.admin_password_hash = Some(hash);
        }
        // Remote sessions from before a new password or before the panel was switched on
        // again stop working.
        if new_password || (s.remote_admin && !was_remote) {
            s.remote_session_epoch += 1;
        }
    }
}

/// What failed in a desktop operation, for the HTTP answer.
#[derive(Debug)]
pub(crate) enum OpError {
    /// 400 with this message.
    Invalid(String),
    /// 409 with this message.
    Conflict(String),
    /// 500 with this message (shown to the owner in the window).
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ToolsJob {
    Install,
    Update(Tool),
    Repair,
}

struct Running {
    stop: oneshot::Sender<()>,
    join: JoinHandle<anyhow::Result<()>>,
}

/// What the running server exposes to the synchronous accessors.
#[derive(Clone)]
struct Live {
    port: u16,
    state: AppState,
    channel: bool,
    /// Public address the server started with (drawn on its slates).
    public_url: String,
}

pub(crate) struct AppCore {
    home: PathBuf,
    hooks: Arc<dyn DesktopHooks>,
    pub(crate) options: AppOptions,
    pub(crate) log: Arc<LogSink>,
    /// Downloads (tools, the program): connect and read timeouts, no total limit.
    http: reqwest::Client,
    /// GitHub API: 15 s in total.
    api_http: reqwest::Client,
    pub(crate) tools: Arc<ToolsManager>,
    desktop_key: Arc<DesktopKey>,
    settings: Mutex<AppSettings>,
    /// Serializes the routes that change settings (and the restart that may follow).
    settings_change: tokio::sync::Mutex<()>,
    /// The running server; holding the lock serializes start, stop and restart.
    server: tokio::sync::Mutex<Option<Running>>,
    live: RwLock<Option<Live>>,
    channel_loops: Arc<AtomicUsize>,
    closing: AtomicBool,
    /// Why the channel did not start although ffmpeg and ffprobe are there (for the window).
    channel_error: Mutex<Option<String>>,
    tools_job: Mutex<Option<JoinHandle<()>>>,
    update: Mutex<Option<UpdateInfo>>,
    applying_update: tokio::sync::Mutex<()>,
    background: Mutex<Vec<JoinHandle<()>>>,
    /// Settings read at use time; shared by every server this engine starts.
    pub(crate) live_settings: LiveSettings,
    /// How many times a server was started (restarts included).
    server_starts: AtomicUsize,
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    start_pause: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl AppCore {
    /// Tests: the next start stops right after the new server is up, before it is published;
    /// the first receiver fires when it gets there, sending on the second lets it go on.
    #[cfg(test)]
    pub(crate) fn pause_next_start(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *lock(&self.start_pause) = Some((reached_tx, release_rx));
        (reached_rx, release_tx)
    }

    pub(crate) fn log(&self, msg: &str) {
        self.log.log(msg);
    }

    pub(crate) fn settings(&self) -> AppSettings {
        lock(&self.settings).clone()
    }

    fn live(&self) -> Option<Live> {
        self.live
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_live(&self, live: Option<Live>) {
        *self.live.write().unwrap_or_else(PoisonError::into_inner) = live;
    }

    /// The port the server listens on (the configured one while it is restarting).
    pub(crate) fn port(&self) -> u16 {
        self.live().map_or_else(|| self.settings().port, |l| l.port)
    }

    pub(crate) fn channel_running(&self) -> bool {
        self.live().is_some_and(|l| l.channel)
    }

    /// The last start of the channel failed although the tools are there: why.
    pub(crate) fn channel_error(&self) -> Option<String> {
        lock(&self.channel_error).clone()
    }

    /// The server is being started, stopped or restarted right now.
    pub(crate) fn restarting(&self) -> bool {
        self.server.try_lock().is_err()
    }

    pub(crate) fn version(&self) -> &Version {
        &self.options.version
    }

    /// Changes the settings and saves them atomically; returns (before, after).
    fn update_settings(
        &self,
        f: impl FnOnce(&mut AppSettings),
    ) -> anyhow::Result<(AppSettings, AppSettings)> {
        let mut guard = lock(&self.settings);
        let old = guard.clone();
        let mut new = old.clone();
        f(&mut new);
        new.save(&self.home).map_err(|e| {
            anyhow!(
                "Nie można zapisać ustawień w {}: {e}",
                self.home.join(CONFIG_FILE).display()
            )
        })?;
        *guard = new.clone();
        Ok((old, new))
    }

    fn tools_present(config: &AppConfig) -> bool {
        config.ffmpeg_path.is_file() && config.ffprobe_path.is_file()
    }

    /// Starts the server for the current settings: with the channel when ffmpeg and ffprobe
    /// are there (and work), otherwise without it, so the window can repair the tools.
    async fn start_server(self: &Arc<Self>, slot: &mut Option<Running>) -> anyhow::Result<()> {
        let settings = self.settings();
        let config = app_config(&settings, &self.home);
        let started = if Self::tools_present(&config) {
            match self.spawn_server(&config, &settings, true).await {
                Ok(started) => {
                    *lock(&self.channel_error) = None;
                    started
                }
                Err(e) => {
                    let why = format!("{e:#}");
                    self.log(&format!(
                        "Kanał nie wystartował: {why}. Serwer działa bez kanału — napraw narzędzia w ustawieniach."
                    ));
                    *lock(&self.channel_error) = Some(why);
                    self.spawn_server(&config, &settings, false).await?
                }
            }
        } else {
            *lock(&self.channel_error) = None;
            self.spawn_server(&config, &settings, false).await?
        };
        let (running, live) = started;
        if live.port != settings.port {
            // The port actually bound (the configured one was taken).
            let port = live.port;
            if let Err(e) = self.update_settings(|s| s.port = port) {
                self.log(&format!("{e:#}"));
            }
        }
        #[cfg(test)]
        {
            let pause = lock(&self.start_pause).take();
            if let Some((reached, release)) = pause {
                let _ = reached.send(());
                let _ = release.await;
            }
        }
        let state = live.state.clone();
        self.set_live(Some(live));
        // Settings saved while this server was starting (while `live` was empty, nobody could
        // hand them to it): take the current ones now that it is published.
        let current = self.settings();
        self.live_settings.apply(&app_config(&current, &self.home));
        state.set_admin_auth(admin_auth(&current));
        *slot = Some(running);
        self.server_starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn spawn_server(
        self: &Arc<Self>,
        config: &AppConfig,
        settings: &AppSettings,
        channel: bool,
    ) -> anyhow::Result<(Running, Live)> {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let captured: Arc<Mutex<Option<AppState>>> = Arc::default();
        let configure = {
            let captured = Arc::clone(&captured);
            let auth = admin_auth(settings);
            let key = Arc::clone(&self.desktop_key);
            let core = Arc::downgrade(self);
            Box::new(move |state: AppState| {
                let state = state
                    .with_admin_auth(auth)
                    .with_desktop_key(key)
                    .with_desktop(core);
                *lock(&captured) = Some(state.clone());
                state
            })
        };
        self.live_settings.apply(config);
        let opts = RunOptions {
            fake_resolver: self.options.fake_resolver.clone(),
            update_ytdlp: channel
                && self.options.fake_resolver.is_none()
                && config.ytdlp_path.is_file(),
            port_fallback: true,
            ready: Some(ready_tx),
            log: Some(self.log.log_fn()),
            start_channel: channel,
            configure_state: Some(configure),
            channel_loops: Some(Arc::clone(&self.channel_loops)),
            live: Some(self.live_settings.clone()),
            ..RunOptions::new(self.home.clone())
        };
        let mut join = tokio::spawn(run(config.clone(), opts, async move {
            let _ = stop_rx.await;
        }));
        match tokio::time::timeout(START_TIMEOUT, ready_rx).await {
            Ok(Ok(addr)) => {
                let Some(state) = lock(&captured).take() else {
                    // Cannot happen (the state is captured before `ready`); never leave a
                    // server running that nobody can stop.
                    let _ = stop_tx.send(());
                    self.wait_or_abort(&mut join).await;
                    anyhow::bail!("Serwer nie przekazał swojego stanu.");
                };
                Ok((
                    Running {
                        stop: stop_tx,
                        join,
                    },
                    Live {
                        port: addr.port(),
                        state,
                        channel,
                        public_url: config.public_url.clone(),
                    },
                ))
            }
            // run() ended before it started serving: its error says why.
            Ok(Err(_)) => Err(match join.await {
                Ok(Err(e)) => e,
                Ok(Ok(())) => anyhow!("Serwer zakończył pracę przed startem."),
                Err(e) => anyhow!("Serwer przerwał pracę: {e}"),
            }),
            Err(_) => {
                let _ = stop_tx.send(());
                self.wait_or_abort(&mut join).await;
                anyhow::bail!(
                    "Serwer nie wystartował w ciągu {} s.",
                    START_TIMEOUT.as_secs()
                )
            }
        }
    }

    /// Waits for a stopped server task; one that does not finish in time is aborted.
    async fn wait_or_abort(&self, join: &mut JoinHandle<anyhow::Result<()>>) {
        if tokio::time::timeout(STOP_TIMEOUT, &mut *join)
            .await
            .is_err()
        {
            self.log("Serwer nie zatrzymał się na czas — przerywam go.");
            join.abort();
            let _ = join.await;
        }
    }

    /// Stops the server and waits until its channel loop, port and data-folder lock are gone
    /// (a server that does not finish in time is aborted).
    async fn stop_server(&self, slot: &mut Option<Running>) {
        self.set_live(None);
        if let Some(running) = slot.take() {
            let _ = running.stop.send(());
            let mut join = running.join;
            match tokio::time::timeout(STOP_TIMEOUT, &mut join).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => self.log(&format!("Serwer zakończył pracę z błędem: {e:#}")),
                Ok(Err(e)) => self.log(&format!("Serwer przerwał pracę: {e}")),
                Err(_) => {
                    self.log("Serwer nie zatrzymał się na czas — przerywam go.");
                    join.abort();
                    let _ = join.await;
                }
            }
        }
    }

    /// Stops the server and starts it again with the current settings. Runs as its own task,
    /// so a dropped HTTP request cannot leave the server half restarted.
    pub(crate) async fn restart_server(self: &Arc<Self>) -> anyhow::Result<()> {
        let core = Arc::clone(self);
        tokio::spawn(async move {
            let mut slot = core.server.lock().await;
            if core.closing.load(Ordering::SeqCst) {
                anyhow::bail!("Program jest zamykany.");
            }
            core.log("Ponowne uruchamianie serwera...");
            core.stop_server(&mut slot).await;
            let started = core.start_server(&mut slot).await;
            // The program began quitting meanwhile (and gave up waiting for this restart):
            // no server may stay behind.
            if core.closing.load(Ordering::SeqCst) {
                core.stop_server(&mut slot).await;
                anyhow::bail!("Program jest zamykany.");
            }
            started
        })
        .await
        .map_err(|e| anyhow!("Ponowne uruchamianie serwera zostało przerwane: {e}"))?
    }

    /// Applies a validated change; returns whether the server was restarted for it.
    pub(crate) async fn apply_config(
        self: &Arc<Self>,
        patch: ConfigPatch,
    ) -> Result<bool, OpError> {
        let _guard = self.settings_change.lock().await;
        let current = self.settings();
        if patch.remote_admin == Some(true)
            && current.admin_password_hash.is_none()
            && patch.password.is_none()
        {
            return Err(OpError::Invalid(
                "Najpierw ustaw hasło panelu zdalnego.".into(),
            ));
        }
        let password_hash = match patch.password.clone() {
            Some(password) => Some(
                tokio::task::spawn_blocking(move || crate::http::auth::hash_password(&password))
                    .await
                    .map_err(|e| OpError::Failed(format!("Nie udało się zapisać hasła: {e}")))?,
            ),
            None => None,
        };
        if let Some(on) = patch.autostart
            && on != current.autostart
        {
            let hooks = Arc::clone(&self.hooks);
            tokio::task::spawn_blocking(move || hooks.set_autostart(on))
                .await
                .map_err(|e| anyhow!("{e}"))
                .and_then(|r| r)
                .map_err(|e| OpError::Failed(format!("Nie udało się zmienić autostartu: {e:#}")))?;
        }
        let (old, new) = self
            .update_settings(|s| patch.apply(s, password_hash))
            .map_err(|e| OpError::Failed(format!("{e:#}")))?;
        self.take_over(&new);
        if !needs_restart(&old, &new) {
            return Ok(false);
        }
        if let Err(e) = self.restart_server().await {
            self.log(&format!(
                "Serwer nie wystartował z nowymi ustawieniami: {e:#}. Przywracam poprzednie."
            ));
            // Back to what worked (autostart was already changed in Windows: keep it).
            let _ = self.update_settings(|s| {
                *s = AppSettings {
                    autostart: s.autostart,
                    ..old.clone()
                }
            });
            return Err(OpError::Failed(match self.restart_server().await {
                Ok(()) => format!(
                    "Nie udało się uruchomić serwera z nowymi ustawieniami ({e:#}). Przywrócono poprzednie."
                ),
                Err(again) => format!("Nie udało się uruchomić serwera: {again:#}"),
            }));
        }
        Ok(true)
    }

    /// Hands saved settings to the running server: remote-panel access and everything read at
    /// use time. During a restart (`live` empty) the new server picks them up in
    /// `start_server`.
    fn take_over(&self, settings: &AppSettings) {
        self.live_settings.apply(&app_config(settings, &self.home));
        if let Some(live) = self.live() {
            live.state.set_admin_auth(admin_auth(settings));
        }
    }

    /// The running server's slates show another public address than the saved one.
    pub(crate) fn restart_required(&self) -> bool {
        let saved = app_config(&self.settings(), &self.home).public_url;
        self.live().is_some_and(|l| l.public_url != saved)
    }

    /// Native picker for the cookies file; the choice is saved and used by the next yt-dlp call.
    pub(crate) async fn pick_cookies(self: &Arc<Self>) -> Result<Option<PathBuf>, OpError> {
        let hooks = Arc::clone(&self.hooks);
        let picked = tokio::task::spawn_blocking(move || hooks.pick_cookies_file())
            .await
            .unwrap_or(None);
        let Some(path) = picked else {
            return Ok(None);
        };
        self.apply_config(ConfigPatch {
            ytdlp_cookies: Some(Some(path.clone())),
            ..ConfigPatch::default()
        })
        .await?;
        Ok(Some(path))
    }

    pub(crate) async fn complete_wizard(&self) -> Result<(), OpError> {
        let _guard = self.settings_change.lock().await;
        self.update_settings(|s| s.first_run_completed = true)
            .map(|_| ())
            .map_err(|e| OpError::Failed(format!("{e:#}")))
    }

    /// Tests the hardware encoders with the installed ffmpeg and saves the result; the next
    /// clip uses it (no restart).
    pub(crate) async fn detect_encoder(self: &Arc<Self>) -> Result<VideoEncoder, OpError> {
        let ffmpeg = app_config(&self.settings(), &self.home).ffmpeg_path;
        if !ffmpeg.is_file() {
            return Err(OpError::Conflict(
                "Brak ffmpeg — najpierw pobierz narzędzia.".into(),
            ));
        }
        let found = crate::tools::detect_encoder(&ffmpeg).await;
        self.log(&format!("Test karty graficznej: {}", found.as_str()));
        let _guard = self.settings_change.lock().await;
        let (_, new) = self
            .update_settings(|s| s.detected_encoder = Some(found.as_str().to_string()))
            .map_err(|e| OpError::Failed(format!("{e:#}")))?;
        // The next clip uses it; the stream is never interrupted for this.
        self.take_over(&new);
        Ok(found)
    }

    pub(crate) fn tools_job_running(&self) -> bool {
        lock(&self.tools_job)
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    /// Starts a tools job in the background; `false` when one is already running.
    pub(crate) fn start_tools_job(self: &Arc<Self>, job: ToolsJob) -> bool {
        let mut slot = lock(&self.tools_job);
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return false;
        }
        let core = Arc::clone(self);
        *slot = Some(tokio::spawn(async move { core.run_tools_job(job).await }));
        true
    }

    async fn run_tools_job(self: Arc<Self>, job: ToolsJob) {
        let (what, result) = match job {
            ToolsJob::Install => ("pobieranie", self.tools.install_missing().await),
            ToolsJob::Update(tool) => ("aktualizacja", self.tools.update(tool).await),
            ToolsJob::Repair => ("naprawa", self.tools.repair().await),
        };
        if let Err(e) = result {
            self.log(&format!("Narzędzia ({what}): błąd — {e:#}"));
            return;
        }
        self.log(&format!("Narzędzia ({what}): gotowe."));
        let ok = self.tools.status().missing.is_empty();
        if let Err(e) = self.update_settings(|s| s.tools_installed = ok) {
            self.log(&format!("{e:#}"));
        }
        // Now the channel can start.
        let config = app_config(&self.settings(), &self.home);
        if !self.channel_running()
            && Self::tools_present(&config)
            && let Err(e) = self.restart_server().await
        {
            self.log(&format!("{e:#}"));
        }
    }

    /// Asks GitHub for a newer program. Problems (no network, private repository) are only
    /// logged; the answer then offers nothing.
    pub(crate) async fn check_update(&self) -> UpdateInfo {
        let skipped = self.settings().skipped_update_version;
        let mut info = self_update::check_latest(
            &self.api_http,
            &self.options.update_api_url,
            &self.options.version,
            skipped.as_deref(),
        )
        .await;
        if let Some(e) = info.error.take() {
            self.log(&format!("Sprawdzanie aktualizacji nie powiodło się: {e}"));
        }
        if let Some(release) = &info.available {
            self.log(&format!(
                "Dostępna nowa wersja programu: {}",
                release.version
            ));
        }
        *lock(&self.update) = Some(info.clone());
        info
    }

    /// The release found by the latest check, unless the owner skipped that version since.
    pub(crate) fn offered_update(&self) -> Option<self_update::Release> {
        let skipped = self.settings().skipped_update_version;
        lock(&self.update)
            .as_ref()
            .and_then(|i| i.available.clone())
            .filter(|r| skipped.as_deref() != Some(r.version.to_string().as_str()))
    }

    /// Downloads and installs the offered release, then asks the shell to restart.
    pub(crate) async fn apply_update(&self) -> Result<Version, OpError> {
        let Ok(_guard) = self.applying_update.try_lock() else {
            return Err(OpError::Conflict("Aktualizacja już trwa.".into()));
        };
        let release = self
            .offered_update()
            .ok_or_else(|| OpError::Conflict("Brak dostępnej aktualizacji.".into()))?;
        let exe = self.hooks.exe_path().ok_or_else(|| {
            OpError::Conflict("Aktualizacja nie jest dostępna w tej wersji programu.".into())
        })?;
        self.log(&format!("Instalowanie wersji {}...", release.version));
        if let Err(e) = self_update::apply(&release, &exe, &self.http).await {
            self.log(&format!("Aktualizacja nie powiodła się: {e:#}"));
            return Err(OpError::Failed(format!(
                "Aktualizacja nie powiodła się: {e:#}"
            )));
        }
        self.log(&format!(
            "Zainstalowano wersję {}; ponowne uruchamianie programu.",
            release.version
        ));
        let hooks = Arc::clone(&self.hooks);
        tokio::spawn(async move {
            tokio::time::sleep(UPDATE_RESTART_DELAY).await;
            hooks.restart_after_update();
        });
        Ok(release.version)
    }
}

/// The running engine of the desktop app.
pub struct AppEngine {
    core: Arc<AppCore>,
}

impl AppEngine {
    /// Loads `config.json` from `home` (saving it at once: the session secret and the bound
    /// port persist), starts the server (with the channel when the tools are there) and the
    /// update checks.
    pub async fn start(home: PathBuf, hooks: Arc<dyn DesktopHooks>) -> anyhow::Result<Self> {
        Self::start_with(home, hooks, AppOptions::default()).await
    }

    pub async fn start_with(
        home: PathBuf,
        hooks: Arc<dyn DesktopHooks>,
        options: AppOptions,
    ) -> anyhow::Result<Self> {
        crate::home::ensure_home(&home)?;
        let log = Arc::new(LogSink::open(&home.join("logs")));
        log.log(&format!("VRCinema {}: start", options.version));
        let (mut settings, warning) = AppSettings::load(&home);
        if let Some(warning) = warning {
            log.log(&warning);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()?;
        let api_http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(API_TIMEOUT)
            .build()?;
        let tools = Arc::new(
            ToolsManager::with_sources(
                home.join("tools"),
                http.clone(),
                options.tool_sources.clone(),
            )
            .with_log(log.log_fn()),
        );
        // Backups of programs that were still running at the last swap (nothing uses them now).
        crate::tools::install::clean_old_backups(tools.tools_dir());
        settings.tools_installed = tools.status().missing.is_empty();
        settings.save(&home).map_err(|e| {
            anyhow!(
                "Nie można zapisać ustawień w {}: {e}",
                home.join(CONFIG_FILE).display()
            )
        })?;
        let live_settings = LiveSettings::from_config(&app_config(&settings, &home));
        let core = Arc::new(AppCore {
            home,
            hooks,
            options,
            log,
            http,
            api_http,
            tools,
            desktop_key: Arc::new(DesktopKey::generate().0),
            settings: Mutex::new(settings),
            settings_change: tokio::sync::Mutex::new(()),
            server: tokio::sync::Mutex::new(None),
            live: RwLock::new(None),
            channel_loops: Arc::new(AtomicUsize::new(0)),
            closing: AtomicBool::new(false),
            channel_error: Mutex::new(None),
            tools_job: Mutex::new(None),
            update: Mutex::new(None),
            applying_update: tokio::sync::Mutex::new(()),
            background: Mutex::new(Vec::new()),
            live_settings,
            server_starts: AtomicUsize::new(0),
            #[cfg(test)]
            start_pause: Mutex::new(None),
        });
        {
            let mut slot = core.server.lock().await;
            if let Err(e) = core.start_server(&mut slot).await {
                core.log(&format!("Serwer nie wystartował: {e:#}"));
                return Err(e);
            }
        }
        if core.options.check_updates {
            let weak = Arc::downgrade(&core);
            let checks = tokio::spawn(async move { update_checks(weak).await });
            lock(&core.background).push(checks);
        }
        Ok(Self { core })
    }

    /// The port the server listens on.
    pub fn port(&self) -> u16 {
        self.core.port()
    }

    /// A fresh one-time key for `/desktop-login`; the previous one stops working.
    pub fn new_desktop_key(&self) -> String {
        self.core.desktop_key.rotate()
    }

    /// Queues files and folders dropped on the window (`file:` items).
    pub async fn add_files(&self, paths: Vec<PathBuf>) -> anyhow::Result<AddReport> {
        let live = self
            .core
            .live()
            .ok_or_else(|| anyhow!("Serwer jest ponownie uruchamiany — spróbuj za chwilę."))?;
        let report = add_local_files(&live.state.db, live.state.resolver.as_ref(), paths).await?;
        self.core.log(&format!(
            "Dodano z dysku: {} (pominięto: {})",
            report.added.len(),
            report.skipped
        ));
        if !report.added.is_empty() {
            self.send(&live, Command::QueueChanged);
        }
        Ok(report)
    }

    /// Skips the current film (tray menu).
    pub fn skip(&self) {
        match self.core.live() {
            Some(live) => self.send(&live, Command::Skip),
            None => self.core.log("Nie można pominąć filmu: serwer nie działa."),
        }
    }

    fn send(&self, live: &Live, command: Command) {
        if live.state.engine.commands.try_send(command).is_err() {
            self.core.log(&format!(
                "Kanał nie działa — polecenie {command:?} pominięte."
            ));
        }
    }

    /// Title of the film on air (tray tooltip).
    pub fn now_playing(&self) -> Option<String> {
        let live = self.core.live()?;
        let snapshot = live.state.engine.snapshot.borrow();
        snapshot.state.current.as_ref().map(|c| c.title.clone())
    }

    pub fn settings(&self) -> AppSettings {
        self.core.settings()
    }

    /// The channel loop runs (false: tools missing or broken, the server runs without it).
    pub fn channel_running(&self) -> bool {
        self.core.channel_running()
    }

    /// A line in the app's log (never pass passwords or the window key).
    pub fn log(&self, msg: &str) {
        self.core.log(msg);
    }

    #[cfg(test)]
    pub(crate) fn server_starts(&self) -> usize {
        self.core.server_starts.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn channel_loop_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.core.channel_loops)
    }

    #[cfg(test)]
    pub(crate) fn log_tail(&self) -> Vec<String> {
        self.core.log.tail()
    }

    /// Stops the server and starts it again with the current settings.
    pub async fn restart(&self) -> anyhow::Result<()> {
        self.core.restart_server().await
    }

    /// Stops everything: background jobs, the server, the channel and its processes; gives up
    /// after [`SHUTDOWN_TIMEOUT`].
    pub async fn shutdown(self) {
        self.shutdown_within(SHUTDOWN_TIMEOUT).await;
    }

    /// Like [`Self::shutdown`], waiting at most `limit` (for a restart in progress and for the
    /// server to stop). Returns false when it gave up; the caller ends the process anyway.
    pub async fn shutdown_within(self, limit: Duration) -> bool {
        let core = self.core;
        core.closing.store(true, Ordering::SeqCst);
        for task in lock(&core.background).drain(..) {
            task.abort();
        }
        if let Some(job) = lock(&core.tools_job).take() {
            job.abort();
        }
        let stopped = tokio::time::timeout(limit, async {
            let mut slot = core.server.lock().await;
            core.stop_server(&mut slot).await;
        })
        .await
        .is_ok();
        if stopped {
            core.log("Program zatrzymany.");
        } else {
            core.log(&format!(
                "Zatrzymywanie trwa za długo (ponad {:.1} s) — kończę bez czekania.",
                limit.as_secs_f32()
            ));
        }
        stopped
    }
}

/// At start and every 24 h, while the engine lives.
async fn update_checks(core: Weak<AppCore>) {
    loop {
        let Some(c) = core.upgrade() else {
            return;
        };
        c.check_update().await;
        drop(c);
        tokio::time::sleep(UPDATE_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests;
