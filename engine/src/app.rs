//! The engine inside the desktop app: settings from `config.json`, the HTTP server with the
//! channel (restarted when the port or another server setting changes), the tools jobs, the
//! built-in tunnel with the automatic pause (`app::tunnel`, `crate::auto_pause`) and the
//! program updates. The `/api/desktop/*` routes (`http::routes_desktop`) call into [`AppCore`].

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::Duration;

use anyhow::anyhow;
use semver::Version;
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::JoinHandle;

use crate::app_settings::{AppSettings, CONFIG_FILE, TunnelModeSetting, app_config};
use crate::auto_pause::{
    AutoPause, AutoPauseSettings, AutoPauseTimings, ChannelTarget, CommandTarget, ViewerActivity,
};
use crate::config::{AppConfig, VideoEncoder};
use crate::engine::Command;
use crate::http::AppState;
use crate::http::auth::{AdminAuth, DesktopKey};
use crate::live::LiveSettings;
use crate::local_files::{AddReport, add_local_files};
use crate::logging::LogSink;
use crate::pipeline::channel::PauseReason;
use crate::pipeline::slates::{SlateSwapper, SwapOutcome};
use crate::self_update::{self, DEFAULT_API_URL, UpdateInfo};
use crate::server::{RunOptions, run};
use crate::tools::{Tool, ToolSources, ToolsManager};
use crate::tunnel::named::{NamedOptions, NamedSetup};
use crate::tunnel::{TunnelConfig, TunnelManager, TunnelState, TunnelTimings};

mod tunnel;

use tunnel::TunnelNotes;

const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// A missing cloudflared after an upgrade: the next download attempt after a failure.
const CLOUDFLARED_RETRY: Duration = Duration::from_secs(5 * 60);
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
    /// Timings of the tunnel manager (tests shorten them).
    pub tunnel_timings: TunnelTimings,
    /// Timings of the automatic pause (tests shorten them).
    pub auto_pause_timings: AutoPauseTimings,
    /// The permanent-link wizard's timeouts and cloudflared's own folder (tests: a temporary one).
    pub named_tunnel: NamedOptions,
    /// How often a missing cloudflared (an upgrade from a version without the tunnel) is
    /// downloaded again after a failure (5 min; tests shorten it).
    pub cloudflared_retry: Duration,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            version: Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0)),
            update_api_url: DEFAULT_API_URL.to_string(),
            check_updates: true,
            tool_sources: ToolSources::default(),
            fake_resolver: None,
            tunnel_timings: TunnelTimings::default(),
            auto_pause_timings: AutoPauseTimings::default(),
            named_tunnel: NamedOptions::default(),
            cloudflared_retry: CLOUDFLARED_RETRY,
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
/// is read at use time ([`LiveSettings`]); a new public address also reaches the slates live
/// ([`AppCore::refresh_slates`]).
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
    pub tunnel_enabled: Option<bool>,
    pub tunnel_mode: Option<TunnelModeSetting>,
    pub pause_on_tunnel_loss: Option<bool>,
    pub pause_when_no_viewers: Option<bool>,
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
        if let Some(v) = self.tunnel_enabled {
            s.tunnel_enabled = v;
        }
        if let Some(v) = self.tunnel_mode {
            s.tunnel_mode = v;
        }
        if let Some(v) = self.pause_on_tunnel_loss {
            s.pause_on_tunnel_loss = v;
        }
        if let Some(v) = self.pause_when_no_viewers {
            s.pause_when_no_viewers = v;
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
    /// Replaces the slate set when the public address changes (`None` without a channel).
    slates: Option<Arc<SlateSwapper>>,
    /// The pause reasons its channel loop was started with.
    started_paused: BTreeSet<PauseReason>,
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
    /// Serializes the computation and hand-over of the live settings.
    live_apply: Mutex<()>,
    /// cloudflared (spec section 7); stopped when the app quits.
    tunnel: TunnelManager,
    /// What the tunnel was last asked to run (`None`: stopped); the lock serializes changes.
    tunnel_running: tokio::sync::Mutex<Option<TunnelConfig>>,
    tunnel_notes: Mutex<TunnelNotes>,
    /// The tunnel's state as the window sees it: `Off` while switched off, an error while it
    /// cannot run, else the manager's state.
    tunnel_view: watch::Sender<TunnelState>,
    /// The settings the automatic pause follows.
    auto_pause: watch::Sender<AutoPauseSettings>,
    /// Segment fetches of every server this engine starts.
    viewers: Arc<ViewerActivity>,
    /// A new channel is on: the automatic pause hands it the active reasons now.
    pause_kick: Arc<Notify>,
    /// The automatic pause's active reasons: a new server's channel loop starts with them.
    pause_active: Arc<Mutex<BTreeSet<PauseReason>>>,
    /// The tunnel's public address in effect (kept through a short `Reconnecting`, see
    /// [`crate::live::tunnel_address`]); updated with `tunnel_view`.
    tunnel_address: Mutex<Option<tunnel::KeptAddress>>,
    /// A tools job replaces cloudflared: the tunnel stays stopped until it is done.
    tunnel_held: AtomicBool,
    /// A tools job is downloading a missing cloudflared: the tunnel says so (not "missing").
    cloudflared_fetching: AtomicBool,
    /// The permanent-link wizard: one operation at a time and its progress.
    named: NamedSetup,
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
    /// The port of the published server (`None` while it restarts or is down).
    fn live_port(&self) -> Option<u16> {
        self.live().map(|l| l.port)
    }

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

    /// Whether the channel holds films back right now, and why.
    pub(crate) fn pause_state(&self) -> (bool, Vec<PauseReason>) {
        self.live().map_or((false, Vec::new()), |l| {
            let snapshot = l.state.engine.snapshot.borrow();
            (snapshot.state.paused, snapshot.state.pause_reasons.clone())
        })
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
        // The address in effect (the tunnel's while it is enabled) is drawn on the slates.
        let (config, connecting) = self.effective_config(&settings);
        let started = if Self::tools_present(&config) {
            match self
                .spawn_server(&config, &settings, true, connecting)
                .await
            {
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
                    self.spawn_server(&config, &settings, false, connecting)
                        .await?
                }
            }
        } else {
            *lock(&self.channel_error) = None;
            self.spawn_server(&config, &settings, false, connecting)
                .await?
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
        self.set_live(Some(live));
        // Settings saved (or a tunnel address found) while this server was starting, while
        // `live` was empty and nobody could hand them to it: take the current ones now.
        self.apply_live();
        *slot = Some(running);
        self.server_starts.fetch_add(1, Ordering::SeqCst);
        // The new channel starts with the pause reasons active when it was built (the
        // automatic pause brings them up to date now) and with the start-up slates.
        self.pause_kick.notify_one();
        self.refresh_slates();
        Ok(())
    }

    /// `connecting`: the tunnel is enabled but has no address yet (for the idle slate).
    async fn spawn_server(
        self: &Arc<Self>,
        config: &AppConfig,
        settings: &AppSettings,
        channel: bool,
        connecting: bool,
    ) -> anyhow::Result<(Running, Live)> {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let captured: Arc<Mutex<Option<AppState>>> = Arc::default();
        let configure = {
            let captured = Arc::clone(&captured);
            let auth = admin_auth(settings);
            let key = Arc::clone(&self.desktop_key);
            let core = Arc::downgrade(self);
            let viewers = Arc::clone(&self.viewers);
            Box::new(move |state: AppState| {
                let state = state
                    .with_admin_auth(auth)
                    .with_desktop_key(key)
                    .with_desktop(core)
                    .with_viewers(viewers);
                *lock(&captured) = Some(state.clone());
                state
            })
        };
        self.live_settings.apply(config);
        let started_paused = lock(&self.pause_active).clone();
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
            slates_connecting: connecting,
            builtin_tunnel: settings.tunnel_enabled,
            pause_reasons: started_paused.iter().copied().collect(),
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
                        started_paused,
                        slates: channel
                            .then(|| Arc::new(SlateSwapper::for_config(config, connecting))),
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
            drop(slot);
            // A new port (or the next free one): the tunnel follows the server.
            core.sync_tunnel().await;
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
        // The named mode needs the permanent link its wizard set up (a start-up without its
        // files still falls back to the quick tunnel with a warning).
        if patch.tunnel_mode == Some(TunnelModeSetting::Named)
            && tunnel::find_named_tunnel(
                &self.home,
                current.named_tunnel_hostname.as_deref(),
                current.named_tunnel_id.as_deref(),
            )
            .is_none()
        {
            return Err(OpError::Conflict(
                "Najpierw ustaw stały link (Ustawienia → Internet → Ustaw stały link).".into(),
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
        self.apply_live();
        if !needs_restart(&old, &new) {
            // Switched on or off, another mode: the tunnel follows (an unchanged one is left
            // alone). The address in effect may have changed: the slates follow it.
            self.sync_tunnel().await;
            self.refresh_slates();
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

    /// Hands the saved settings to the running server: remote-panel access and everything read
    /// at use time, with the public address and Cloudflare trust in effect (the tunnel's while
    /// it is enabled), and to the automatic pause. During a restart (`live` empty) the new
    /// server picks them up in `start_server`.
    fn apply_live(&self) {
        // Computed and handed over under one lock: a stale computation never wins.
        let _guard = lock(&self.live_apply);
        let settings = self.settings();
        let (config, _) = self.effective_config(&settings);
        self.live_settings.apply(&config);
        // The tunnel's trust holds only for cloudflared's own (loopback) connections; the
        // manual `trustCloudflare` keeps the stage-2 behaviour.
        self.live_settings
            .cloudflare_from_loopback_only
            .set(settings.tunnel_enabled);
        if let Some(live) = self.live() {
            live.state.set_admin_auth(admin_auth(&settings));
        }
        let pause = AutoPauseSettings {
            tunnel_enabled: settings.tunnel_enabled,
            pause_on_tunnel_loss: settings.pause_on_tunnel_loss,
            pause_when_no_viewers: settings.pause_when_no_viewers,
        };
        self.auto_pause.send_if_modified(|current| {
            let changed = *current != pause;
            *current = pause;
            changed
        });
    }

    /// The public address in effect may have changed: makes the slates for it in the
    /// background and hands them to the running channel (no restart, no break in the stream);
    /// nothing is made when the set on air already shows it. A failure leaves the previous
    /// slates on air and is logged. During a restart nothing is done: the new server starts
    /// with the current address and refreshes once it is published.
    fn refresh_slates(self: &Arc<Self>) {
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        let core = Arc::clone(self);
        let task = tokio::spawn(async move { core.swap_slates().await });
        let mut background = lock(&self.background);
        background.retain(|t| !t.is_finished());
        background.push(task);
    }

    async fn swap_slates(&self) {
        let Some(live) = self.live() else { return };
        let Some(swapper) = live.slates.clone() else {
            return;
        };
        // The address in effect when the swap runs (read under its lock): quick changes collapse.
        let current = || self.slate_address();
        match swapper.swap_to(current, &live.state.engine.commands).await {
            SwapOutcome::Unchanged => {}
            SwapOutcome::Replaced { generation } => self.log(&format!(
                "Plansze odświeżone dla nowego adresu publicznego (zestaw {generation})."
            )),
            SwapOutcome::Failed(why) => self.log(&format!(
                "Nie udało się odświeżyć plansz: {why}. Zostają poprzednie plansze."
            )),
        }
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
        self.update_settings(|s| s.detected_encoder = Some(found.as_str().to_string()))
            .map_err(|e| OpError::Failed(format!("{e:#}")))?;
        // The next clip uses it; the stream is never interrupted for this.
        self.apply_live();
        Ok(found)
    }

    /// The first-run wizard is done and every tool is there but cloudflared: an upgrade from
    /// a version without the tunnel (the app downloads it by itself).
    fn cloudflared_only_missing(&self) -> bool {
        self.settings().first_run_completed
            && self.tools.status().missing == vec![Tool::Cloudflared]
    }

    pub(crate) fn tools_job_running(&self) -> bool {
        lock(&self.tools_job)
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    /// Starts a tools job in the background; refused (why, in Polish) while one is already
    /// running, or — for a job that replaces cloudflared — while the permanent-link wizard runs
    /// cloudflared (which in turn refuses its operations until the job is done).
    pub(crate) fn start_tools_job(self: &Arc<Self>, job: ToolsJob) -> Result<(), &'static str> {
        let mut slot = lock(&self.tools_job);
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return Err("Trwa już pobieranie narzędzi.");
        }
        let wizard_hold = if Self::replaces_cloudflared(job) {
            Some(self.named.hold_for_tools().ok_or(
                "Trwa operacja stałego linku (logowanie do Cloudflare lub ustawianie adresu) — spróbuj ponownie, gdy się skończy.",
            )?)
        } else {
            None
        };
        let core = Arc::clone(self);
        *slot = Some(tokio::spawn(async move {
            let _wizard_hold = wizard_hold;
            core.run_tools_job(job).await;
        }));
        Ok(())
    }

    /// The job swaps `cloudflared.exe`.
    fn replaces_cloudflared(job: ToolsJob) -> bool {
        matches!(job, ToolsJob::Update(Tool::Cloudflared) | ToolsJob::Repair)
    }

    /// The job installs cloudflared when it is missing (an update of another tool does not).
    fn fetches_cloudflared(job: ToolsJob) -> bool {
        !matches!(job, ToolsJob::Update(tool) if tool != Tool::Cloudflared)
    }

    async fn run_tools_job(self: Arc<Self>, job: ToolsJob) {
        // A running cloudflared.exe would be moved aside by the swap and keep running the old
        // program (the configuration looks unchanged): stop it first, start it after.
        let replaces_cloudflared = Self::replaces_cloudflared(job);
        if replaces_cloudflared {
            self.tunnel_held.store(true, Ordering::SeqCst);
        }
        // A missing cloudflared is on its way: the tunnel says so instead of "missing".
        let fetching = Self::fetches_cloudflared(job)
            && !self
                .tools
                .tools_dir()
                .join(Tool::Cloudflared.exe())
                .is_file();
        if fetching {
            self.cloudflared_fetching.store(true, Ordering::SeqCst);
        }
        if replaces_cloudflared || fetching {
            self.sync_tunnel().await;
        }
        let (what, result) = match job {
            ToolsJob::Install => ("pobieranie", self.tools.install_missing().await),
            ToolsJob::Update(tool) => ("aktualizacja", self.tools.update(tool).await),
            ToolsJob::Repair => ("naprawa", self.tools.repair().await),
        };
        self.tunnel_held.store(false, Ordering::SeqCst);
        self.cloudflared_fetching.store(false, Ordering::SeqCst);
        if let Err(e) = result {
            self.log(&format!("Narzędzia ({what}): błąd — {e:#}"));
            // cloudflared may have come before the failure, or was stopped for it.
            self.sync_tunnel().await;
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
        // Now cloudflared is there (a running tunnel is left alone).
        self.sync_tunnel().await;
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
        let tunnel = TunnelManager::new()
            .with_timings(options.tunnel_timings.clone())
            .with_log(log.log_fn());
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
            live_apply: Mutex::new(()),
            tunnel,
            tunnel_running: tokio::sync::Mutex::new(None),
            tunnel_notes: Mutex::new(TunnelNotes::default()),
            tunnel_view: watch::channel(TunnelState::Off).0,
            auto_pause: watch::channel(AutoPauseSettings::default()).0,
            viewers: Arc::new(ViewerActivity::new()),
            pause_kick: Arc::new(Notify::new()),
            pause_active: Arc::default(),
            tunnel_address: Mutex::new(None),
            tunnel_held: AtomicBool::new(false),
            cloudflared_fetching: AtomicBool::new(false),
            named: NamedSetup::default(),
            #[cfg(test)]
            start_pause: Mutex::new(None),
        });
        // The tunnel's state as the window sees it, and the address in effect from the start.
        core.update_tunnel_view();
        core.apply_live();
        {
            let mut slot = core.server.lock().await;
            if let Err(e) = core.start_server(&mut slot).await {
                core.log(&format!("Serwer nie wystartował: {e:#}"));
                return Err(e);
            }
        }
        let watcher = tokio::spawn(follow_tunnel(Arc::downgrade(&core), core.tunnel.state()));
        lock(&core.background).push(watcher);
        let auto_pause = tokio::spawn(auto_pause(&core).run());
        lock(&core.background).push(auto_pause);
        core.sync_tunnel().await;
        // An upgrade from a version without the tunnel: everything is there but cloudflared.
        if core.cloudflared_only_missing() {
            core.log(
                "Narzędzia: brak programu cloudflared (nowy w tej wersji) — pobieram go w tle.",
            );
            if let Err(e) = core.start_tools_job(ToolsJob::Install) {
                core.log(&format!("Narzędzia: {e}"));
            }
        }
        let retry = tokio::spawn(cloudflared_retries(
            Arc::downgrade(&core),
            core.options.cloudflared_retry,
        ));
        lock(&core.background).push(retry);
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
        if live
            .state
            .engine
            .commands
            .try_send(command.clone())
            .is_err()
        {
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

    /// The public address in effect right now (no trailing slash, empty: none): the tunnel's
    /// while it is enabled (empty while it has none), else the manual one. The tray's links.
    pub fn public_url(&self) -> String {
        self.core.live_settings.public_url.get()
    }

    /// The tunnel's state (tray tooltip).
    pub fn tunnel_state(&self) -> crate::tunnel::TunnelState {
        self.core.tunnel_state()
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
        // cloudflared is stopped alongside the server (not after it: a stuck restart must not
        // keep it running); a restart that finishes later finds `closing` and stops it too.
        let stopped = tokio::time::timeout(limit, async {
            let server = async {
                let mut slot = core.server.lock().await;
                core.stop_server(&mut slot).await;
            };
            tokio::join!(server, core.sync_tunnel());
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

/// After an upgrade whose download of cloudflared failed (no internet at the time): tries
/// again every `interval` while the engine lives, until cloudflared is there.
async fn cloudflared_retries(core: Weak<AppCore>, interval: Duration) {
    loop {
        tokio::time::sleep(interval).await;
        let Some(c) = core.upgrade() else {
            return;
        };
        if c.cloudflared_only_missing() && !c.tools_job_running() {
            c.log("Narzędzia: ponownie pobieram brakujący program cloudflared.");
            if let Err(e) = c.start_tools_job(ToolsJob::Install) {
                c.log(&format!("Narzędzia: {e}"));
            }
        }
    }
}

/// Every change of the tunnel manager's state reaches the window, the address in effect and
/// the slates, while the engine lives.
async fn follow_tunnel(core: Weak<AppCore>, mut states: watch::Receiver<TunnelState>) {
    while states.changed().await.is_ok() {
        let Some(core) = core.upgrade() else {
            return;
        };
        core.update_tunnel_view();
    }
}

/// The automatic pause for this engine: it follows the settings, the tunnel as the window sees
/// it and the segment fetches, and talks to whichever channel runs.
fn auto_pause(core: &Arc<AppCore>) -> AutoPause {
    let weak = Arc::downgrade(core);
    let target: CommandTarget = Arc::new(move || {
        let live = weak.upgrade()?.live()?;
        live.channel.then(|| ChannelTarget {
            commands: live.state.engine.commands.clone(),
            started_with: live.started_paused.clone(),
        })
    });
    AutoPause {
        timings: core.options.auto_pause_timings.clone(),
        settings: core.auto_pause.subscribe(),
        tunnel: core.tunnel_view.subscribe(),
        viewers: Arc::clone(&core.viewers),
        target,
        kick: Arc::clone(&core.pause_kick),
        log: core.log.log_fn(),
        lost_since: {
            let weak = Arc::downgrade(core);
            Arc::new(move || {
                let lost = weak.upgrade()?.tunnel.lost_since()?;
                Some(tokio::time::Instant::from_std(lost))
            })
        },
        active: Arc::clone(&core.pause_active),
    }
}

#[cfg(test)]
mod tests;
