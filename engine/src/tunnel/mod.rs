//! Built-in Cloudflare tunnel (spec section 7): runs `cloudflared` for the local server, learns
//! the public address and keeps the process alive (restarts with growing waits). While online
//! it keeps asking cloudflared's `/ready` whether the connection to Cloudflare still stands.
//!
//! At most one cloudflared process runs at any time: every operation that replaces or stops the
//! process kills the old one and waits for its exit before anything new is started.

pub mod named;
mod process;
mod state;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, oneshot, watch};
use tokio::task::JoinHandle;

use crate::pipeline::clip_factory::SharedLogFn;
use process::{Cloudflared, Metrics};
use state::Backoff;

pub use state::TunnelState;

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelConfig {
    /// `tools\cloudflared.exe`.
    pub cloudflared: PathBuf,
    /// Port of the local HTTP server (`http://127.0.0.1:<port>`).
    pub port: u16,
    pub mode: TunnelMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelMode {
    /// A random `https://<name>.trycloudflare.com` address, new on every start.
    Quick,
    /// A fixed address on the owner's own domain (spec section 9).
    Named(NamedTunnel),
}

/// A named tunnel created by the "permanent link" wizard. Only the paths are ever logged,
/// never the contents of the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedTunnel {
    /// Public host name, e.g. `kanal.example.com`.
    pub hostname: String,
    /// Tunnel UUID (`cloudflared tunnel create`).
    pub tunnel_id: String,
    /// `tunnel\<id>.json`: what `tunnel run` authenticates with.
    pub credentials_file: PathBuf,
    /// `tunnel\cert.pem`: needed by the management commands (login/create/route/delete),
    /// not by `tunnel run`.
    pub origin_cert: PathBuf,
}

/// Timings of the manager; the defaults are the spec's, tests shorten them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelTimings {
    /// A start that gives no address within this time failed (30 s).
    pub address_timeout: Duration,
    /// How often the metrics server is asked: for the address while connecting, and `/ready`
    /// (is cloudflared still connected to Cloudflare?) once the address is known (500 ms).
    pub poll_interval: Duration,
    /// First wait before a restart (1 s); doubles with every failure.
    pub backoff_initial: Duration,
    /// Longest wait before a restart (30 s).
    pub backoff_max: Duration,
    /// Time online after which the waits start over from `backoff_initial` (60 s).
    pub stable_after: Duration,
    /// Shortest wait after Cloudflare refused a quick tunnel (429, 60 s).
    pub rate_limit_wait: Duration,
    /// While online, `/ready` must fail this long without a break before the state turns
    /// `Reconnecting` (3 s); a single failed poll is not a lost connection.
    pub health_grace: Duration,
    /// cloudflared is running but `/ready` has not answered 200 for this long: it is killed
    /// and started again through the usual waits (5 min).
    pub reconnect_limit: Duration,
}

impl Default for TunnelTimings {
    fn default() -> Self {
        Self {
            address_timeout: Duration::from_secs(30),
            poll_interval: Duration::from_millis(500),
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            stable_after: Duration::from_secs(60),
            rate_limit_wait: Duration::from_secs(60),
            health_grace: Duration::from_secs(3),
            reconnect_limit: Duration::from_secs(5 * 60),
        }
    }
}

/// Starts, restarts and stops cloudflared; publishes [`TunnelState`].
///
/// Operations are serialised and each one runs to completion in its own task, so a caller
/// that gives up waiting (a cancelled HTTP request) never leaves the tunnel half-replaced.
/// Dropping the manager stops cloudflared.
pub struct TunnelManager {
    timings: TunnelTimings,
    log: SharedLogFn,
    state: Arc<watch::Sender<TunnelState>>,
    /// See [`TunnelManager::lost_since`].
    lost: LostSince,
    /// See [`TunnelManager::runs_ended`].
    runs_ended: Arc<AtomicU64>,
    slot: Arc<Mutex<Slot>>,
    /// Set (or dropped) when the manager goes away: every supervisor stops, including one
    /// launched by an operation still in flight at that moment.
    shutdown: watch::Sender<bool>,
}

/// Since when the tunnel has had no working connection (see [`TunnelManager::lost_since`]).
type LostSince = Arc<std::sync::Mutex<Option<Instant>>>;

fn lock_lost(lost: &LostSince) -> std::sync::MutexGuard<'_, Option<Instant>> {
    lost.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The configuration and the supervisor currently running it (at most one).
#[derive(Default)]
struct Slot {
    config: Option<TunnelConfig>,
    run: Option<Run>,
}

struct Run {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

enum Op {
    Start(TunnelConfig),
    Stop,
    Port(u16),
    Mode(TunnelMode),
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TunnelManager {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

impl TunnelManager {
    /// A stopped manager (state `Off`) with the spec's timings, logging to stderr.
    pub fn new() -> Self {
        Self {
            timings: TunnelTimings::default(),
            log: Arc::new(|m: &str| eprintln!("{m}")),
            state: Arc::new(watch::channel(TunnelState::Off).0),
            lost: LostSince::default(),
            runs_ended: Arc::default(),
            slot: Arc::new(Mutex::new(Slot::default())),
            shutdown: watch::channel(false).0,
        }
    }

    pub fn with_timings(mut self, timings: TunnelTimings) -> Self {
        self.timings = timings;
        self
    }

    /// Sends the manager's notes to `log` instead of stderr.
    pub fn with_log(mut self, log: SharedLogFn) -> Self {
        self.log = log;
        self
    }

    /// The current state and every change of it.
    pub fn state(&self) -> watch::Receiver<TunnelState> {
        self.state.subscribe()
    }

    /// Since when the tunnel has had no working connection: the first failed `/ready` check of
    /// a running cloudflared, the exit of one that was online, or the (re)start when it was not
    /// online before; kept through restarts. `None` while online and while stopped.
    pub fn lost_since(&self) -> Option<Instant> {
        *lock_lost(&self.lost)
    }

    /// How many cloudflared processes have ended without being asked to (exited, or killed
    /// after a failure). A quick tunnel's address dies with its process: one learnt before
    /// the count last grew no longer leads anywhere.
    pub fn runs_ended(&self) -> u64 {
        self.runs_ended.load(AtomicOrdering::SeqCst)
    }

    /// Runs the tunnel for `config`, replacing a running one (which has exited before the new
    /// one starts).
    pub async fn start(&self, config: TunnelConfig) {
        self.apply(Op::Start(config)).await;
    }

    /// Stops cloudflared and waits for its exit; state `Off`. The configuration is kept.
    pub async fn stop(&self) {
        self.apply(Op::Stop).await;
    }

    /// The local server moved to `port`: a running tunnel is replaced by one for the new port
    /// (the old process has exited before the new one starts). A stopped one stays stopped.
    pub async fn restart(&self, port: u16) {
        self.apply(Op::Port(port)).await;
    }

    /// Switches between the quick and the named tunnel; a running tunnel is restarted, a
    /// stopped one stays stopped.
    pub async fn set_mode(&self, mode: TunnelMode) {
        self.apply(Op::Mode(mode)).await;
    }

    async fn apply(&self, op: Op) {
        let supervisor = Supervisor {
            timings: self.timings.clone(),
            log: self.log.clone(),
            state: self.state.clone(),
            lost: self.lost.clone(),
            runs_ended: self.runs_ended.clone(),
        };
        let slot = self.slot.clone();
        let shutdown = self.shutdown.subscribe();
        // Its own task: runs to the end even when the caller stops waiting.
        let _ = tokio::spawn(async move {
            let mut slot = slot.lock().await;
            match op {
                Op::Start(config) => {
                    stop_run(&mut slot).await;
                    slot.config = Some(config.clone());
                    launch(&mut slot, supervisor, config, shutdown);
                }
                Op::Stop => {
                    if slot.run.is_some() {
                        (supervisor.log)("Tunel: zatrzymano cloudflared.");
                    }
                    stop_run(&mut slot).await;
                    *lock_lost(&supervisor.lost) = None;
                    supervisor.state.send_replace(TunnelState::Off);
                }
                Op::Port(port) => {
                    reconfigure(&mut slot, supervisor, shutdown, |c| c.port = port).await;
                }
                Op::Mode(mode) => {
                    reconfigure(&mut slot, supervisor, shutdown, |c| c.mode = mode).await;
                }
            }
        })
        .await;
    }
}

/// Changes the kept configuration; a running tunnel is replaced by one for the new
/// configuration, a stopped (or never started) one stays as it is.
async fn reconfigure(
    slot: &mut Slot,
    supervisor: Supervisor,
    shutdown: watch::Receiver<bool>,
    change: impl FnOnce(&mut TunnelConfig),
) {
    let Some(config) = slot.config.as_mut() else {
        return;
    };
    change(config);
    let config = config.clone();
    if slot.run.is_some() {
        stop_run(slot).await;
        launch(slot, supervisor, config, shutdown);
    }
}

/// Stops the running supervisor and waits until its cloudflared has exited.
async fn stop_run(slot: &mut Slot) {
    if let Some(run) = slot.run.as_mut() {
        if let Some(stop) = run.stop.take() {
            let _ = stop.send(());
        }
        if !run.task.is_finished() {
            let _ = (&mut run.task).await;
        }
    }
    slot.run = None;
}

/// Starts a supervisor for `config` (none when the manager is gone). The slot must be empty.
fn launch(
    slot: &mut Slot,
    supervisor: Supervisor,
    config: TunnelConfig,
    shutdown: watch::Receiver<bool>,
) {
    let (stop_tx, stop_rx) = oneshot::channel();
    let mut stop = StopSignal {
        own: stop_rx,
        shutdown,
        fired: false,
    };
    if stop.is_set() {
        return;
    }
    lock_lost(&supervisor.lost).get_or_insert_with(Instant::now);
    supervisor.state.send_replace(TunnelState::Connecting);
    slot.run = Some(Run {
        stop: Some(stop_tx),
        task: tokio::spawn(supervisor.run(config, stop)),
    });
}

/// Fires when the supervisor's own run is stopped or the whole manager goes away.
struct StopSignal {
    own: oneshot::Receiver<()>,
    shutdown: watch::Receiver<bool>,
    fired: bool,
}

impl StopSignal {
    fn is_set(&mut self) -> bool {
        if !self.fired {
            let own = !matches!(
                self.own.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            );
            let gone = *self.shutdown.borrow() || self.shutdown.has_changed().is_err();
            self.fired = own || gone;
        }
        self.fired
    }

    /// Waits until the signal fires (cancel safe).
    async fn wait(&mut self) {
        if self.fired {
            return;
        }
        tokio::select! {
            _ = &mut self.own => {}
            _ = self.shutdown.wait_for(|down| *down) => {}
        }
        self.fired = true;
    }
}

/// Why a cloudflared run ended without being asked to.
struct Failure {
    /// For the log.
    reason: String,
    /// Shown as `TunnelState::Error` during the wait.
    error: Option<String>,
    rate_limited: bool,
    /// The longest stretch of continuous `Online` in this run; time `Reconnecting` while the
    /// process was alive does not count. At least `stable_after` resets the backoff.
    online_for: Duration,
}

impl Failure {
    fn new(reason: String) -> Self {
        Self {
            reason,
            error: None,
            rate_limited: false,
            online_for: Duration::ZERO,
        }
    }
}

enum Outcome {
    Stopped,
    Failed(Failure),
}

/// Keeps one cloudflared running for a configuration: starts it, finds its address, restarts
/// it with growing waits.
struct Supervisor {
    timings: TunnelTimings,
    log: SharedLogFn,
    state: Arc<watch::Sender<TunnelState>>,
    lost: LostSince,
    runs_ended: Arc<AtomicU64>,
}

impl Supervisor {
    async fn run(self, config: TunnelConfig, mut stop: StopSignal) {
        // Windows takes 1–2 s to refuse a connection to a closed loopback port (the metrics
        // server is not up yet); a short connect timeout keeps the polling at its pace.
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_millis(300))
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_default();
        let mut backoff = Backoff::new(self.timings.backoff_initial, self.timings.backoff_max);
        let mut first = true;
        loop {
            if stop.is_set() {
                return;
            }
            if !first {
                self.state.send_replace(TunnelState::Reconnecting);
            }
            first = false;
            let outcome = self.attempt(&config, &client, &mut stop).await;
            if matches!(outcome, Outcome::Failed(_)) {
                lock_lost(&self.lost).get_or_insert_with(Instant::now);
                // Before the state says so: whoever sees the new state sees the count.
                self.runs_ended.fetch_add(1, AtomicOrdering::SeqCst);
            }
            let failure = match outcome {
                Outcome::Stopped => return,
                Outcome::Failed(failure) => failure,
            };
            if failure.online_for >= self.timings.stable_after {
                backoff.reset();
            }
            let mut wait = backoff.next_delay();
            if failure.rate_limited {
                wait = wait.max(self.timings.rate_limit_wait);
            }
            let seconds = wait.as_secs_f64().ceil() as u64;
            (self.log)(&format!(
                "Tunel: {}; ponowna próba za {seconds} s.",
                failure.reason
            ));
            let shown = if failure.rate_limited {
                TunnelState::Error {
                    message: format!(
                        "Cloudflare chwilowo odmawia nowych szybkich tuneli (zbyt wiele prób). Ponowna próba za {seconds} s."
                    ),
                }
            } else if let Some(message) = failure.error {
                TunnelState::Error { message }
            } else {
                TunnelState::Reconnecting
            };
            self.state.send_replace(shown);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stop.wait() => return,
            }
        }
    }

    /// One cloudflared process, from start to its end (failure) or a stop request.
    async fn attempt(
        &self,
        config: &TunnelConfig,
        client: &reqwest::Client,
        stop: &mut StopSignal,
    ) -> Outcome {
        let mut cloudflared = match Cloudflared::spawn(config) {
            Ok(process) => process,
            Err(e) => {
                let message = format!("Nie można uruchomić cloudflared: {e}");
                let mut failure = Failure::new(message.clone());
                failure.error = Some(message);
                return Outcome::Failed(failure);
            }
        };
        (self.log)(&format!(
            "Tunel: uruchomiono cloudflared (pid {}, {}).",
            cloudflared.pid().unwrap_or(0),
            describe(config)
        ));

        let metrics_port = cloudflared.metrics_port;
        let poll = self.timings.poll_interval;
        let deadline = tokio::time::sleep(self.timings.address_timeout);
        tokio::pin!(deadline);
        let mut probe = Box::pin(address_probe(Duration::ZERO, client, metrics_port, config));
        let mut health = Box::pin(health_probe(poll, client, metrics_port));
        // The public address, once known (it stays the same for the life of the process).
        let mut address: Option<String> = None;
        // Since when the state is `Online` (`None` while `Reconnecting` with a live process).
        let mut online_since: Option<Instant> = None;
        let mut longest_online = Duration::ZERO;
        // Since when `/ready` has been failing without a break.
        let mut down_since: Option<Instant> = None;
        // The metrics server answered at least once: only then can `/ready` be watched (an
        // address found on stderr without a metrics server stays online).
        let mut metrics_seen = false;
        let mut last_line = String::new();
        let mut stderr_open = true;
        // Refusals of too many requests concern quick tunnels only (spec section 7).
        let quick = matches!(config.mode, TunnelMode::Quick);

        loop {
            tokio::select! {
                biased;
                _ = stop.wait() => {
                    cloudflared.kill().await;
                    return Outcome::Stopped;
                }
                line = cloudflared.stderr.recv(), if stderr_open => {
                    let Some(line) = line else {
                        stderr_open = false;
                        continue;
                    };
                    if address.is_none() {
                        if quick && process::is_rate_limited(&line) {
                            cloudflared.kill().await;
                            let mut failure = Failure::new(
                                "Cloudflare odmówił szybkiego tunelu (429 Too Many Requests)".into(),
                            );
                            failure.rate_limited = true;
                            return Outcome::Failed(failure);
                        }
                        if let Some(url) = process::address_in_line(&line, &config.mode) {
                            online_since = Some(self.online(&url));
                            address = Some(url);
                        }
                    }
                    if !line.trim().is_empty() {
                        last_line = line;
                    }
                }
                status = cloudflared.child.wait() => {
                    // Lines written just before the exit (a 429 refusal) are already in the pipe;
                    // the end of the pipe itself can come late on Windows, when a process
                    // started at the same moment inherited its handle.
                    let late = cloudflared.remaining_lines(Duration::from_millis(200)).await;
                    let rate_limited = quick
                        && address.is_none()
                        && late.iter().any(|l| process::is_rate_limited(l));
                    if let Some(line) = late.into_iter().rev().find(|l| !l.trim().is_empty()) {
                        last_line = line;
                    }
                    let code = match status {
                        Ok(status) => status
                            .code()
                            .map_or_else(|| "bez kodu".to_string(), |c| format!("kod {c}")),
                        Err(e) => e.to_string(),
                    };
                    let mut reason = format!("cloudflared zakończył działanie ({code})");
                    if !last_line.is_empty() {
                        reason.push_str(&format!(": {last_line}"));
                    }
                    let mut failure = Failure::new(reason);
                    failure.rate_limited = rate_limited;
                    failure.online_for = online_since
                        .map_or(longest_online, |t| longest_online.max(t.elapsed()));
                    return Outcome::Failed(failure);
                }
                answer = &mut probe, if address.is_none() => {
                    if let Metrics::Answered(found) = answer {
                        metrics_seen = true;
                        if let Some(url) = found {
                            online_since = Some(self.online(&url));
                            address = Some(url);
                            health = Box::pin(health_probe(poll, client, metrics_port));
                            continue;
                        }
                    }
                    probe = Box::pin(address_probe(poll, client, metrics_port, config));
                }
                answer = &mut health, if address.is_some() => {
                    let connected = match answer {
                        Metrics::Answered(ready) => {
                            metrics_seen = true;
                            Some(ready)
                        }
                        Metrics::Unreachable => metrics_seen.then_some(false),
                    };
                    match connected {
                        Some(true) => {
                            down_since = None;
                            if online_since.is_none()
                                && let Some(url) = &address
                            {
                                online_since = Some(self.online(url));
                            }
                        }
                        Some(false) => {
                            let since = *down_since.get_or_insert_with(Instant::now);
                            if let Some(online) = online_since
                                && since.elapsed() >= self.timings.health_grace
                            {
                                longest_online = longest_online.max(online.elapsed());
                                online_since = None;
                                lock_lost(&self.lost).get_or_insert(since);
                                (self.log)(
                                    "Tunel: cloudflared stracił połączenie z Cloudflare; czekam, aż je odzyska.",
                                );
                                self.state.send_replace(TunnelState::Reconnecting);
                            }
                            if since.elapsed() >= self.timings.reconnect_limit {
                                cloudflared.kill().await;
                                let mut failure = Failure::new(format!(
                                    "brak połączenia z Cloudflare od {} s mimo działającego cloudflared",
                                    since.elapsed().as_secs()
                                ));
                                failure.online_for = online_since
                                    .map_or(longest_online, |t| longest_online.max(t.elapsed()));
                                return Outcome::Failed(failure);
                            }
                        }
                        None => {}
                    }
                    health = Box::pin(health_probe(poll, client, metrics_port));
                }
                _ = &mut deadline, if address.is_none() => {
                    cloudflared.kill().await;
                    return Outcome::Failed(Failure::new(format!(
                        "brak adresu tunelu po {} s",
                        self.timings.address_timeout.as_secs_f64().ceil() as u64
                    )));
                }
            }
        }
    }

    fn online(&self, url: &str) -> Instant {
        *lock_lost(&self.lost) = None;
        (self.log)(&format!("Tunel: w sieci — {url}"));
        self.state
            .send_replace(TunnelState::Online { url: url.into() });
        Instant::now()
    }
}

/// After `delay`, asks the metrics server for the public address: the quick tunnel's host name
/// from `/quicktunnel`, or the named tunnel's address once `/ready` answers 200.
async fn address_probe(
    delay: Duration,
    client: &reqwest::Client,
    metrics_port: u16,
    config: &TunnelConfig,
) -> Metrics<Option<String>> {
    tokio::time::sleep(delay).await;
    match &config.mode {
        TunnelMode::Quick => process::quick_address(client, metrics_port).await,
        TunnelMode::Named(named) => match process::ready(client, metrics_port).await {
            Metrics::Answered(ready) => {
                Metrics::Answered(ready.then(|| process::named_url(&named.hostname)))
            }
            Metrics::Unreachable => Metrics::Unreachable,
        },
    }
}

/// After `delay`, asks `/ready` whether cloudflared is connected to Cloudflare (both modes).
async fn health_probe(
    delay: Duration,
    client: &reqwest::Client,
    metrics_port: u16,
) -> Metrics<bool> {
    tokio::time::sleep(delay).await;
    process::ready(client, metrics_port).await
}

/// The configuration for the log: paths only, never file contents.
fn describe(config: &TunnelConfig) -> String {
    let local = format!("http://127.0.0.1:{}", config.port);
    match &config.mode {
        TunnelMode::Quick => format!("szybki tunel do {local}"),
        TunnelMode::Named(named) => format!(
            "stały tunel {} do {local}, poświadczenia {}",
            named.hostname,
            named.credentials_file.display()
        ),
    }
}
