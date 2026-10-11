//! Automatic pause (spec section 8): the channel holds films back while nobody can watch them.
//!
//! - Tunnel loss: the tunnel is enabled and has had no connection for at least 10 s →
//!   `Pause(Tunnel)`, counted from the moment its manager lost the connection (it reports
//!   `Reconnecting` only after a short grace), else from when a state other than `Online` was
//!   first seen; online again → `Resume(Tunnel)`. Only with `pauseOnTunnelLoss`.
//! - No viewers: no segment fetched (`/live/main/<n>.ts`) for at least 30 s →
//!   `Pause(NoViewers)`; the first fetch → `Resume(NoViewers)` at once. Only with
//!   `pauseWhenNoViewers`.
//!
//! The controller sends its reasons to whichever channel loop runs: a new loop (server restarted)
//! starts with the active reasons ([`AutoPause::active`]); the controller then brings it up to date
//! (pauses reasons that came, resumes ones that went while the server restarted). A loop
//! that rebuilds its channel after a crash re-applies the reasons itself (`engine.rs`).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::Instant;

use crate::engine::Command;
use crate::pipeline::channel::PauseReason;
use crate::pipeline::clip_factory::SharedLogFn;
use crate::tunnel::TunnelState;

/// Timings of the controller; the defaults are the spec's, tests shorten them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoPauseTimings {
    /// The tunnel may be down this long before the channel pauses (10 s).
    pub tunnel_loss: Duration,
    /// Without a segment fetch for this long the channel pauses (30 s).
    pub no_viewers: Duration,
    /// Longest time between two checks (a restarted server's channel gets the active reasons
    /// within it; commands the loop could not take yet are sent again).
    pub recheck: Duration,
}

impl Default for AutoPauseTimings {
    fn default() -> Self {
        Self {
            tunnel_loss: Duration::from_secs(10),
            no_viewers: Duration::from_secs(30),
            recheck: Duration::from_secs(1),
        }
    }
}

/// The settings the controller follows (`config.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutoPauseSettings {
    pub tunnel_enabled: bool,
    pub pause_on_tunnel_loss: bool,
    pub pause_when_no_viewers: bool,
}

/// When a viewer last fetched a segment. Recording is a store and a wake-up: cheap enough for
/// every segment request.
#[derive(Debug)]
pub struct ViewerActivity {
    base: Instant,
    /// Milliseconds after `base` of the latest fetch, plus one; 0: none yet.
    last_ms: AtomicU64,
    fetched: Notify,
}

impl Default for ViewerActivity {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewerActivity {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            last_ms: AtomicU64::new(0),
            fetched: Notify::new(),
        }
    }

    /// A viewer (the in-app preview too) fetched a segment.
    pub fn segment_fetched(&self) {
        let ms = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX - 1);
        self.last_ms.store(ms + 1, Ordering::Relaxed);
        self.fetched.notify_one();
    }

    /// The latest fetch, if any.
    pub fn last_fetch(&self) -> Option<Instant> {
        match self.last_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(self.base + Duration::from_millis(ms - 1)),
        }
    }
}

/// The channel loop running right now: its command queue and the pause reasons it was started
/// with (a restarted server's loop starts with the reasons active when it was built).
#[derive(Debug, Clone)]
pub struct ChannelTarget {
    pub commands: mpsc::Sender<Command>,
    pub started_with: BTreeSet<PauseReason>,
}

/// The reasons the automatic pause sets and clears (not the owner's manual pause).
fn is_automatic(reason: PauseReason) -> bool {
    matches!(reason, PauseReason::Tunnel | PauseReason::NoViewers)
}

/// The channel loop running right now (`None`: no channel, e.g. while the server restarts or
/// runs without tools).
pub type CommandTarget = Arc<dyn Fn() -> Option<ChannelTarget> + Send + Sync>;

/// When the tunnel lost its connection, see [`AutoPause::lost_since`].
pub type LossClock = Arc<dyn Fn() -> Option<Instant> + Send + Sync>;

/// The controller; [`AutoPause::run`] drives it until the tunnel or settings sender is gone.
pub struct AutoPause {
    pub timings: AutoPauseTimings,
    pub settings: watch::Receiver<AutoPauseSettings>,
    /// State of the tunnel as the app shows it.
    pub tunnel: watch::Receiver<TunnelState>,
    pub viewers: Arc<ViewerActivity>,
    pub target: CommandTarget,
    /// Asks for a check now (a new server published its channel).
    pub kick: Arc<Notify>,
    /// Since when the tunnel has had no connection, as its manager knows it (the state turns
    /// `Reconnecting` only after a grace period); `None`: unknown, the time the state was seen
    /// counts.
    pub lost_since: LossClock,
    /// The reasons active right now, for the loop of a restarted server to start with.
    pub active: Arc<Mutex<BTreeSet<PauseReason>>>,
    pub log: SharedLogFn,
}

/// The reasons the channel loop `to` has right now (started with or commanded).
#[derive(Default)]
struct Delivered {
    to: Option<mpsc::Sender<Command>>,
    reasons: BTreeSet<PauseReason>,
}

impl AutoPause {
    pub async fn run(mut self) {
        let mut settings = *self.settings.borrow_and_update();
        // Since when the tunnel (enabled) has not been online.
        let mut down_since: Option<Instant> = None;
        // Since when "no viewers" is watched: a fetch before it does not count against it.
        let mut watching_since = Instant::now();
        let mut active = BTreeSet::new();
        let mut delivered = Delivered::default();
        loop {
            let now = Instant::now();
            let fresh = *self.settings.borrow_and_update();
            if fresh.pause_when_no_viewers && !settings.pause_when_no_viewers {
                watching_since = now;
            }
            settings = fresh;
            let online = matches!(*self.tunnel.borrow_and_update(), TunnelState::Online { .. });
            if settings.tunnel_enabled && !online {
                // From the manager's date of the loss when it has one (it reports the loss
                // only after its grace), else from when the state was first seen.
                let seen = *down_since.get_or_insert(now);
                let lost = (self.lost_since)().map_or(seen, |lost| lost.min(seen));
                down_since = Some(lost);
            } else {
                down_since = None;
            }

            let mut wanted = BTreeSet::new();
            let mut next_check = now + self.timings.recheck;
            if settings.tunnel_enabled
                && settings.pause_on_tunnel_loss
                && let Some(since) = down_since
            {
                let at = since + self.timings.tunnel_loss;
                if now >= at {
                    wanted.insert(PauseReason::Tunnel);
                } else {
                    next_check = next_check.min(at);
                }
            }
            if settings.pause_when_no_viewers {
                let seen = self
                    .viewers
                    .last_fetch()
                    .map_or(watching_since, |f| f.max(watching_since));
                let at = seen + self.timings.no_viewers;
                if now >= at {
                    wanted.insert(PauseReason::NoViewers);
                } else {
                    next_check = next_check.min(at);
                }
            }
            self.log_changes(&active, &wanted);
            active = wanted;
            *self.active.lock().unwrap_or_else(PoisonError::into_inner) = active.clone();
            self.deliver(&active, &mut delivered);

            tokio::select! {
                changed = self.tunnel.changed() => if changed.is_err() { return },
                changed = self.settings.changed() => if changed.is_err() { return },
                () = self.viewers.fetched.notified() => {}
                () = self.kick.notified() => {}
                () = tokio::time::sleep_until(next_check) => {}
            }
        }
    }

    fn log_changes(&self, before: &BTreeSet<PauseReason>, after: &BTreeSet<PauseReason>) {
        for reason in after.difference(before) {
            let message = match reason {
                PauseReason::Tunnel => format!(
                    "Automatyczna pauza: tunel nie działa od {} s — filmy czekają na powrót internetu.",
                    self.timings.tunnel_loss.as_secs()
                ),
                PauseReason::NoViewers => format!(
                    "Automatyczna pauza: nikt nie ogląda od {} s — filmy czekają na widza.",
                    self.timings.no_viewers.as_secs()
                ),
                // Never wanted by the automatic pause (it is the owner's own reason).
                PauseReason::Manual => "Pauza ręczna.".to_string(),
            };
            (self.log)(&message);
        }
        for reason in before.difference(after) {
            (self.log)(match reason {
                PauseReason::Tunnel => "Koniec pauzy z powodu tunelu.",
                PauseReason::NoViewers => "Koniec pauzy z powodu braku widzów.",
                PauseReason::Manual => "Koniec pauzy ręcznej.",
            });
        }
    }

    /// Brings the running channel loop to `wanted`: a new loop starts from the reasons it was
    /// started with; a command the loop cannot take now (full queue) is sent again on a later
    /// check.
    fn deliver(&self, wanted: &BTreeSet<PauseReason>, delivered: &mut Delivered) {
        let target = (self.target)();
        let same = match (&target, &delivered.to) {
            (Some(new), Some(old)) => new.commands.same_channel(old),
            _ => false,
        };
        if !same {
            // A new loop has exactly the reasons it was started with: the ones no longer
            // wanted are resumed, the missing ones paused.
            // Only the reasons this controller owns: the owner's manual pause is never its to
            // resume, whatever the loop was started with.
            delivered.reasons = target
                .as_ref()
                .map(|t| {
                    t.started_with
                        .iter()
                        .copied()
                        .filter(|r| is_automatic(*r))
                        .collect()
                })
                .unwrap_or_default();
            delivered.to = target.as_ref().map(|t| t.commands.clone());
        }
        let Some(tx) = target.map(|t| t.commands) else {
            return;
        };
        let pauses = wanted.difference(&delivered.reasons).copied();
        let resumes = delivered.reasons.difference(wanted).copied();
        let commands: Vec<(PauseReason, Command)> = pauses
            .map(|r| (r, Command::Pause(r)))
            .chain(resumes.map(|r| (r, Command::Resume(r))))
            .collect();
        for (reason, command) in commands {
            let pause = matches!(command, Command::Pause(_));
            match tx.try_send(command) {
                Ok(()) => {
                    if pause {
                        delivered.reasons.insert(reason);
                    } else {
                        delivered.reasons.remove(&reason);
                    }
                }
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Closed(_)) => {
                    *delivered = Delivered::default();
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Env {
        settings: watch::Sender<AutoPauseSettings>,
        tunnel: watch::Sender<TunnelState>,
        viewers: Arc<ViewerActivity>,
        target: Arc<Mutex<Option<ChannelTarget>>>,
        rx: mpsc::Receiver<Command>,
        kick: Arc<Notify>,
        logs: Arc<Mutex<Vec<String>>>,
        /// What the tunnel manager says about the loss.
        lost: Arc<Mutex<Option<Instant>>>,
    }

    const ON_LOSS: AutoPauseSettings = AutoPauseSettings {
        tunnel_enabled: true,
        pause_on_tunnel_loss: true,
        pause_when_no_viewers: false,
    };

    const ON_NO_VIEWERS: AutoPauseSettings = AutoPauseSettings {
        tunnel_enabled: false,
        pause_on_tunnel_loss: true,
        pause_when_no_viewers: true,
    };

    fn online() -> TunnelState {
        TunnelState::Online {
            url: "https://a.trycloudflare.com".into(),
        }
    }

    /// A channel loop behind `tx`, started with `reasons`.
    fn channel(tx: mpsc::Sender<Command>, reasons: &[PauseReason]) -> ChannelTarget {
        ChannelTarget {
            commands: tx,
            started_with: reasons.iter().copied().collect(),
        }
    }

    fn start(settings: AutoPauseSettings, tunnel: TunnelState) -> Env {
        let (settings_tx, settings_rx) = watch::channel(settings);
        let (tunnel_tx, tunnel_rx) = watch::channel(tunnel);
        let (tx, rx) = mpsc::channel(64);
        let target = Arc::new(Mutex::new(Some(channel(tx, &[]))));
        let viewers = Arc::new(ViewerActivity::new());
        let kick = Arc::new(Notify::new());
        let lost: Arc<Mutex<Option<Instant>>> = Arc::default();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let controller = AutoPause {
            timings: AutoPauseTimings::default(),
            settings: settings_rx,
            tunnel: tunnel_rx,
            viewers: Arc::clone(&viewers),
            target: {
                let target = Arc::clone(&target);
                Arc::new(move || target.lock().unwrap().clone())
            },
            kick: Arc::clone(&kick),
            lost_since: {
                let lost = Arc::clone(&lost);
                Arc::new(move || *lost.lock().unwrap())
            },
            active: Arc::default(),
            log: {
                let logs = Arc::clone(&logs);
                Arc::new(move |m: &str| logs.lock().unwrap().push(m.to_string()))
            },
        };
        tokio::spawn(controller.run());
        Env {
            settings: settings_tx,
            tunnel: tunnel_tx,
            viewers,
            target,
            rx,
            kick,
            logs,
            lost,
        }
    }

    impl Env {
        /// Everything sent so far.
        fn sent(&mut self) -> Vec<Command> {
            let mut all = Vec::new();
            while let Ok(c) = self.rx.try_recv() {
                all.push(c);
            }
            all
        }
    }

    /// Lets the (paused) clock run for `ms` milliseconds.
    async fn after_ms(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    /// Review Focus 4: a loss shorter than 10 s does not pause.
    #[tokio::test(start_paused = true)]
    async fn a_tunnel_loss_shorter_than_10_s_does_not_pause() {
        let mut env = start(ON_LOSS, online());
        after_ms(5_000).await;
        env.tunnel.send_replace(TunnelState::Reconnecting);
        after_ms(9_000).await;
        env.tunnel.send_replace(TunnelState::Error {
            message: "x".into(),
        });
        after_ms(900).await;
        env.tunnel.send_replace(online());
        after_ms(60_000).await;
        assert!(env.sent().is_empty());
    }

    /// Review Focus 4: 10 s without the tunnel pause; online again resumes at once.
    #[tokio::test(start_paused = true)]
    async fn a_tunnel_loss_of_10_s_pauses_and_the_return_resumes() {
        let mut env = start(ON_LOSS, online());
        after_ms(1_000).await;
        env.tunnel.send_replace(TunnelState::Reconnecting);
        after_ms(9_900).await;
        assert!(env.sent().is_empty(), "paused before 10 s");
        after_ms(200).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
        // Further states without the tunnel change nothing.
        env.tunnel.send_replace(TunnelState::Connecting);
        after_ms(30_000).await;
        assert!(env.sent().is_empty());
        env.tunnel.send_replace(online());
        after_ms(10).await;
        assert_eq!(env.sent(), [Command::Resume(PauseReason::Tunnel)]);
        assert!(
            env.logs.lock().unwrap().iter().any(|l| l.contains("tunel")),
            "{:?}",
            env.logs.lock().unwrap()
        );
    }

    /// Fix round 1: the 10 s count from the moment the connection was lost (the manager reports
    /// `Reconnecting` only after its grace), so a loss longer than 10 s pauses at 10 s.
    #[tokio::test(start_paused = true)]
    async fn the_loss_counts_from_when_the_manager_lost_the_connection() {
        let mut env = start(ON_LOSS, online());
        after_ms(1_000).await;
        *env.lost.lock().unwrap() = Some(Instant::now());
        // The state follows after the manager's grace.
        after_ms(3_000).await;
        env.tunnel.send_replace(TunnelState::Reconnecting);
        after_ms(6_900).await;
        assert!(env.sent().is_empty(), "paused before 10 s");
        after_ms(200).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
        // A date later than the state change does not delay the count.
        *env.lost.lock().unwrap() = None;
        env.tunnel.send_replace(online());
        after_ms(10).await;
        assert_eq!(env.sent(), [Command::Resume(PauseReason::Tunnel)]);
        env.tunnel.send_replace(TunnelState::Reconnecting);
        after_ms(5_000).await;
        *env.lost.lock().unwrap() = Some(Instant::now());
        after_ms(5_100).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
    }

    /// A tunnel that never came up counts from the start.
    #[tokio::test(start_paused = true)]
    async fn a_tunnel_that_never_comes_up_pauses_after_10_s() {
        let mut env = start(ON_LOSS, TunnelState::Connecting);
        after_ms(9_900).await;
        assert!(env.sent().is_empty());
        after_ms(200).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
    }

    #[tokio::test(start_paused = true)]
    async fn no_tunnel_pause_when_switched_off_or_the_tunnel_is_disabled() {
        let mut env = start(
            AutoPauseSettings {
                pause_on_tunnel_loss: false,
                ..ON_LOSS
            },
            TunnelState::Reconnecting,
        );
        after_ms(60_000).await;
        assert!(env.sent().is_empty());
        // Tunnel switched off: its state is `Off`, which is no loss.
        env.settings.send_replace(AutoPauseSettings {
            tunnel_enabled: false,
            ..ON_LOSS
        });
        env.tunnel.send_replace(TunnelState::Off);
        after_ms(60_000).await;
        assert!(env.sent().is_empty());
    }

    /// Switching the option off while paused for it resumes at once.
    #[tokio::test(start_paused = true)]
    async fn switching_the_option_off_resumes_its_reason() {
        let mut env = start(ON_LOSS, TunnelState::Reconnecting);
        after_ms(10_100).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
        env.settings.send_replace(AutoPauseSettings {
            pause_on_tunnel_loss: false,
            ..ON_LOSS
        });
        after_ms(10).await;
        assert_eq!(env.sent(), [Command::Resume(PauseReason::Tunnel)]);
        // Disabling the tunnel also clears it.
        env.settings.send_replace(ON_LOSS);
        after_ms(10_100).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
        env.settings.send_replace(AutoPauseSettings {
            tunnel_enabled: false,
            ..ON_LOSS
        });
        after_ms(10).await;
        assert_eq!(env.sent(), [Command::Resume(PauseReason::Tunnel)]);
    }

    /// 30 s without a segment fetch pause; the next fetch resumes without waiting for a check.
    #[tokio::test(start_paused = true)]
    async fn thirty_seconds_without_viewers_pause_and_a_fetch_resumes_at_once() {
        let mut env = start(ON_NO_VIEWERS, TunnelState::Off);
        after_ms(20_000).await;
        env.viewers.segment_fetched();
        after_ms(29_900).await;
        assert!(env.sent().is_empty(), "the fetch at 20 s restarts the 30 s");
        after_ms(200).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::NoViewers)]);
        after_ms(500).await;
        env.viewers.segment_fetched();
        // Well under the 1 s recheck: the fetch itself wakes the controller.
        after_ms(1).await;
        assert_eq!(env.sent(), [Command::Resume(PauseReason::NoViewers)]);
    }

    #[tokio::test(start_paused = true)]
    async fn steady_viewers_never_pause_and_the_option_off_never_pauses() {
        let mut env = start(ON_NO_VIEWERS, TunnelState::Off);
        for _ in 0..30 {
            after_ms(4_000).await;
            env.viewers.segment_fetched();
        }
        assert!(env.sent().is_empty());
        env.settings.send_replace(AutoPauseSettings {
            pause_when_no_viewers: false,
            ..ON_NO_VIEWERS
        });
        after_ms(120_000).await;
        assert!(env.sent().is_empty());
    }

    /// Switching "no viewers" on starts its 30 s from that moment, not from the last fetch.
    #[tokio::test(start_paused = true)]
    async fn switching_no_viewers_on_counts_from_that_moment() {
        let mut env = start(
            AutoPauseSettings {
                pause_when_no_viewers: false,
                ..ON_NO_VIEWERS
            },
            TunnelState::Off,
        );
        after_ms(120_000).await;
        env.settings.send_replace(ON_NO_VIEWERS);
        after_ms(29_900).await;
        assert!(env.sent().is_empty());
        after_ms(200).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::NoViewers)]);
    }

    /// Both reasons: each is paused and resumed on its own (the channel resumes when none is
    /// left).
    #[tokio::test(start_paused = true)]
    async fn both_reasons_are_tracked_separately() {
        let mut env = start(
            AutoPauseSettings {
                tunnel_enabled: true,
                pause_on_tunnel_loss: true,
                pause_when_no_viewers: true,
            },
            TunnelState::Reconnecting,
        );
        after_ms(10_100).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
        after_ms(20_000).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::NoViewers)]);
        env.tunnel.send_replace(online());
        after_ms(10).await;
        assert_eq!(env.sent(), [Command::Resume(PauseReason::Tunnel)]);
        env.viewers.segment_fetched();
        after_ms(1).await;
        assert_eq!(env.sent(), [Command::Resume(PauseReason::NoViewers)]);
    }

    /// A new channel loop (the server restarted) starts without reasons: the active ones are
    /// sent to it again; without any loop nothing is sent.
    #[tokio::test(start_paused = true)]
    async fn a_new_channel_gets_the_active_reasons_again() {
        let mut env = start(ON_LOSS, TunnelState::Reconnecting);
        after_ms(10_100).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
        // Server restarting: no channel.
        *env.target.lock().unwrap() = None;
        after_ms(5_000).await;
        let (tx, mut rx) = mpsc::channel(64);
        *env.target.lock().unwrap() = Some(channel(tx, &[]));
        env.kick.notify_one();
        after_ms(1).await;
        assert_eq!(
            rx.try_recv().ok(),
            Some(Command::Pause(PauseReason::Tunnel))
        );
        // Sent once, not on every check.
        after_ms(5_000).await;
        assert!(rx.try_recv().is_err());
        assert!(env.sent().is_empty(), "the old loop got nothing more");
    }

    /// Fix round 2: the new loop started with a reason (the copy taken when its server was
    /// built) that went away while no channel ran: it is resumed, not left paused forever.
    #[tokio::test(start_paused = true)]
    async fn a_reason_cleared_while_no_channel_runs_is_resumed_on_the_new_one() {
        let mut env = start(ON_LOSS, TunnelState::Reconnecting);
        after_ms(10_100).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
        *env.target.lock().unwrap() = None;
        // The tunnel comes back during the server restart.
        env.tunnel.send_replace(online());
        after_ms(1_000).await;
        let (tx, mut rx) = mpsc::channel(64);
        *env.target.lock().unwrap() = Some(channel(tx, &[PauseReason::Tunnel]));
        env.kick.notify_one();
        after_ms(1).await;
        assert_eq!(
            rx.try_recv().ok(),
            Some(Command::Resume(PauseReason::Tunnel))
        );
        after_ms(5_000).await;
        assert!(rx.try_recv().is_err(), "sent once");
    }

    /// Stage 4 fix round 1: the owner's manual pause is not the automatic pause's to clear,
    /// even when a new loop was started with it.
    #[tokio::test(start_paused = true)]
    async fn the_manual_pause_is_never_resumed_by_the_automatic_pause() {
        let mut env = start(ON_LOSS, online());
        after_ms(1_000).await;
        assert!(env.sent().is_empty());
        let (tx, mut rx) = mpsc::channel(64);
        *env.target.lock().unwrap() =
            Some(channel(tx, &[PauseReason::Tunnel, PauseReason::Manual]));
        env.kick.notify_one();
        after_ms(1).await;
        assert_eq!(
            rx.try_recv().ok(),
            Some(Command::Resume(PauseReason::Tunnel))
        );
        env.tunnel.send_replace(TunnelState::Reconnecting);
        after_ms(10_100).await;
        env.tunnel.send_replace(online());
        after_ms(60_000).await;
        let mut rest = Vec::new();
        while let Ok(c) = rx.try_recv() {
            rest.push(c);
        }
        assert_eq!(
            rest,
            [
                Command::Pause(PauseReason::Tunnel),
                Command::Resume(PauseReason::Tunnel)
            ]
        );
    }

    /// Fix round 2: a reason that came while no channel ran pauses the new loop; one it was
    /// already started with is not sent again.
    #[tokio::test(start_paused = true)]
    async fn a_reason_that_appears_while_no_channel_runs_pauses_the_new_one() {
        let mut env = start(ON_LOSS, online());
        *env.target.lock().unwrap() = None;
        env.tunnel.send_replace(TunnelState::Reconnecting);
        after_ms(10_100).await;
        let (tx, mut rx) = mpsc::channel(64);
        *env.target.lock().unwrap() = Some(channel(tx, &[]));
        env.kick.notify_one();
        after_ms(1).await;
        assert_eq!(
            rx.try_recv().ok(),
            Some(Command::Pause(PauseReason::Tunnel))
        );
        let (tx, mut rx) = mpsc::channel(64);
        *env.target.lock().unwrap() = Some(channel(tx, &[PauseReason::Tunnel]));
        env.kick.notify_one();
        after_ms(5_000).await;
        assert!(rx.try_recv().is_err(), "already started paused");
        assert!(env.sent().is_empty());
    }

    /// Without a kick the next periodic check finds the new loop.
    #[tokio::test(start_paused = true)]
    async fn a_new_channel_is_found_by_the_periodic_check() {
        let mut env = start(ON_LOSS, TunnelState::Reconnecting);
        after_ms(10_100).await;
        assert_eq!(env.sent(), [Command::Pause(PauseReason::Tunnel)]);
        let (tx, mut rx) = mpsc::channel(64);
        *env.target.lock().unwrap() = Some(channel(tx, &[]));
        after_ms(1_100).await;
        assert_eq!(
            rx.try_recv().ok(),
            Some(Command::Pause(PauseReason::Tunnel))
        );
    }

    /// A full command queue: the command is sent again on a later check, not lost.
    #[tokio::test(start_paused = true)]
    async fn a_full_queue_is_retried() {
        let mut env = start(ON_LOSS, TunnelState::Reconnecting);
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(Command::Skip).unwrap();
        *env.target.lock().unwrap() = Some(channel(tx, &[]));
        after_ms(10_100).await;
        assert_eq!(rx.try_recv().ok(), Some(Command::Skip));
        after_ms(1_100).await;
        assert_eq!(
            rx.try_recv().ok(),
            Some(Command::Pause(PauseReason::Tunnel))
        );
        assert!(env.sent().is_empty());
    }

    #[test]
    fn viewer_activity_remembers_the_latest_fetch() {
        let activity = ViewerActivity::new();
        assert!(activity.last_fetch().is_none());
        activity.segment_fetched();
        let first = activity.last_fetch().unwrap();
        assert!(first <= Instant::now());
        assert!(Instant::now().duration_since(first) < Duration::from_secs(1));
    }

    #[test]
    fn default_timings_are_the_spec() {
        let t = AutoPauseTimings::default();
        assert_eq!(t.tunnel_loss, Duration::from_secs(10));
        assert_eq!(t.no_viewers, Duration::from_secs(30));
    }
}
