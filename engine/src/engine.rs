//! Channel engine loop and its supervisor (replaces the Node `setInterval(250 ms)` that drove
//! `Broadcaster.tick()` with the Channel as its source).
//!
//! One task owns the `Broadcaster` and the `Channel`. Every 250 ms it drains the commands sent by
//! HTTP, ticks the broadcaster and publishes a fresh [`Snapshot`]. HTTP never calls the Channel
//! directly: it sends [`Command`]s and reads the latest snapshot.

use std::any::Any;
use std::collections::{BTreeSet, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::i18n::{Msg, error_msg};
use crate::pipeline::broadcaster::{Broadcaster, SegmentSource};
use crate::pipeline::channel::{Channel, ChannelState, ChannelStatus, PauseReason};
use crate::pipeline::monitor::MonitorFeed;
use crate::pipeline::slates::Slates;
use crate::stats::ChannelStats;

/// Requests from HTTP, applied by the loop between ticks.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Skip,
    QueueChanged,
    /// Adds a pause reason; the channel pauses while any reason is active. The loop keeps the
    /// active reasons, so a channel rebuilt after a crash starts paused too.
    Pause(PauseReason),
    /// The owner's pause at `at_s` seconds of the film (what the producer's monitor showed): the
    /// film airs up to the end of the segment holding it first (see [`Channel::pause_at`]). The
    /// loop keeps it as the manual reason, so a channel rebuilt after a crash starts paused.
    PauseAt {
        at_s: f64,
    },
    /// Clears a pause reason; the channel resumes once none is left.
    Resume(PauseReason),
    /// Airs another slate set from now on (a new public address is drawn on it). The loop
    /// keeps it, so a channel rebuilt after a crash starts with the newest set.
    ReplaceSlates(Slates),
    /// Seeks the current film to `to_s` seconds (a burst of seeks is joined, see
    /// [`Channel::seek_by`]).
    Seek {
        to_s: f64,
    },
    /// Seeks `delta_s` seconds from the server position (or from the target still waiting).
    SeekBy {
        delta_s: f64,
    },
    /// The current film from its start (a seek to 0).
    Restart,
    /// Airs this queue or history item now; the current film ends as skipped and the manual
    /// pause ends.
    PlayNow {
        item_id: i64,
    },
    /// The ticker's layout changed (switch, mode, interval, speed, code, progress line): the
    /// current film starts again from where it is (a live stream: from the next item).
    ApplyTicker,
    /// The public address changed: like `ApplyTicker` for the ticker's QR code, unless the
    /// film on air already draws the code of `public_url` (see [`Channel::apply_ticker_code`]).
    ApplyTickerCode {
        public_url: String,
    },
}

/// What HTTP reads: the state right after the latest tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub playlist: String,
    /// Every segment file name that may still be requested (window + retired, still on disk).
    pub segments: HashSet<String>,
    pub live_dir: PathBuf,
    pub state: ChannelState,
    pub prefetched_item_id: Option<i64>,
    /// Numbers for the nerd stats.
    pub stats: ChannelStats,
    /// What the producer's monitor lists (the window's `/api/desktop/monitor*`).
    pub monitor: Arc<MonitorFeed>,
}

/// HTTP's side of the engine. Dropping every handle stops the loop.
#[derive(Clone)]
pub struct EngineHandle {
    pub commands: mpsc::Sender<Command>,
    pub snapshot: watch::Receiver<Arc<Snapshot>>,
}

/// What the loop needs from the channel; implemented by [`Channel`] and by test doubles.
pub trait ChannelControl: SegmentSource + Send {
    fn start(&mut self) -> anyhow::Result<()>;
    fn stop(&mut self);
    fn skip(&mut self) -> anyhow::Result<()>;
    fn on_queue_changed(&mut self) -> anyhow::Result<()>;
    fn pause(&mut self, reason: PauseReason);
    /// The owner's pause at `at_s` seconds of the film (test doubles: a plain manual pause).
    fn pause_at(&mut self, at_s: f64) {
        let _ = at_s;
        self.pause(PauseReason::Manual);
    }
    /// The owner's manual pause is active or waiting for its position (`None`: not known).
    fn manual_pause(&self) -> Option<bool> {
        None
    }
    fn resume(&mut self, reason: PauseReason);
    fn replace_slates(&mut self, slates: Slates);
    fn seek_to(&mut self, to_s: f64);
    fn seek_by(&mut self, delta_s: f64);
    fn play_now(&mut self, item_id: i64) -> anyhow::Result<()>;
    fn apply_ticker(&mut self);
    fn apply_ticker_code(&mut self, public_url: &str);
    /// Called every tick before the broadcaster (runs a seek once its quiet has passed).
    fn on_tick(&mut self);
    fn state(&self) -> ChannelState;
    fn prefetched_item_id(&self) -> Option<i64>;
    /// Numbers for the nerd stats (test doubles have none).
    fn stats(&self) -> ChannelStats {
        ChannelStats::default()
    }
    /// What the producer's monitor lists (test doubles: nothing).
    fn monitor_feed(&self) -> MonitorFeed {
        MonitorFeed::default()
    }
    /// The queue item being aired right now (not the one viewers see, which lags behind).
    fn current_item_id(&self) -> Option<i64>;
    /// Marks the item failed with `error` (unless the owner removed it meanwhile).
    fn fail_item(&mut self, id: i64, error: &Msg) -> anyhow::Result<()>;
}

impl ChannelControl for Channel {
    fn start(&mut self) -> anyhow::Result<()> {
        Channel::start(self)
    }
    fn stop(&mut self) {
        Channel::stop(self)
    }
    fn skip(&mut self) -> anyhow::Result<()> {
        Channel::skip(self)
    }
    fn on_queue_changed(&mut self) -> anyhow::Result<()> {
        Channel::on_queue_changed(self)
    }
    fn pause(&mut self, reason: PauseReason) {
        Channel::pause(self, reason)
    }
    fn pause_at(&mut self, at_s: f64) {
        Channel::pause_at(self, at_s)
    }
    fn manual_pause(&self) -> Option<bool> {
        Some(Channel::manual_pause(self))
    }
    fn resume(&mut self, reason: PauseReason) {
        Channel::resume(self, reason)
    }
    fn replace_slates(&mut self, slates: Slates) {
        Channel::replace_slates(self, slates)
    }
    fn seek_to(&mut self, to_s: f64) {
        Channel::seek_to(self, to_s)
    }
    fn seek_by(&mut self, delta_s: f64) {
        Channel::seek_by(self, delta_s)
    }
    fn play_now(&mut self, item_id: i64) -> anyhow::Result<()> {
        Channel::play_now(self, item_id)
    }
    fn apply_ticker(&mut self) {
        Channel::apply_ticker(self)
    }
    fn apply_ticker_code(&mut self, public_url: &str) {
        Channel::apply_ticker_code(self, public_url)
    }
    fn on_tick(&mut self) {
        Channel::on_tick(self)
    }
    fn state(&self) -> ChannelState {
        Channel::state(self)
    }
    fn prefetched_item_id(&self) -> Option<i64> {
        Channel::prefetched_item_id(self)
    }
    fn stats(&self) -> ChannelStats {
        Channel::stats(self)
    }
    fn monitor_feed(&self) -> MonitorFeed {
        Channel::monitor_feed(self)
    }
    fn current_item_id(&self) -> Option<i64> {
        Channel::current_item_id(self)
    }
    fn fail_item(&mut self, id: i64, error: &Msg) -> anyhow::Result<()> {
        Channel::fail_item(self, id, error)
    }
}

const TICK: Duration = Duration::from_millis(250);
/// First restart delay; doubled after every failure up to [`MAX_RESTART_DELAY`].
const RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);
/// A run at least this long counts as healthy: the next restart waits [`RESTART_DELAY`] again.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
/// Consecutive crashes with the same queue item current after which that item is failed.
const CRASHES_BEFORE_FAILING_ITEM: u32 = 3;
/// Error stored on an item that kept crashing the loop.
fn internal_error() -> Msg {
    Msg::new("item_errors.internal")
}
const COMMAND_CAPACITY: usize = 64;

/// The supervisor task, for an explicit shutdown.
pub struct EngineTask {
    stop: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl EngineTask {
    /// Stops the loop, stops the channel and waits until both are done.
    pub async fn shutdown(self) {
        self.stop.send_replace(true);
        let _ = self.join.await;
    }

    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    /// Asks the loop to stop the channel and end, without waiting.
    pub fn request_stop(&self) {
        self.stop.send_replace(true);
    }

    /// Waits until the loop has ended (after [`Self::request_stop`]). Call it at most until it
    /// first completes.
    pub async fn wait(&mut self) {
        let _ = (&mut self.join).await;
    }
}

/// Starts the engine (tick 250 ms, supervised) and logs to stdout. Must be called inside a tokio
/// runtime. The loop runs until every [`EngineHandle`] is dropped.
pub fn spawn_engine(build: impl Fn() -> (Broadcaster, Channel) + Send + 'static) -> EngineHandle {
    spawn_engine_with(build, |msg: Msg| println!("{msg}")).0
}

/// Like [`spawn_engine`], with any channel, an injected log and a handle for explicit shutdown.
/// Dropping the [`EngineTask`] does not stop the engine.
pub fn spawn_engine_with<C, F, L>(build: F, log: L) -> (EngineHandle, EngineTask)
where
    C: ChannelControl + 'static,
    F: Fn() -> (Broadcaster, C) + Send + 'static,
    L: Fn(Msg) + Send + Sync + 'static,
{
    spawn_engine_paused(build, log, [])
}

/// Like [`spawn_engine_with`], with `pause_reasons` active from the start (as if commanded
/// before it): the first channel is paused before it starts. The app passes the automatic
/// pause's active reasons to the loop of a restarted server.
pub fn spawn_engine_paused<C, F, L>(
    build: F,
    log: L,
    pause_reasons: impl IntoIterator<Item = PauseReason>,
) -> (EngineHandle, EngineTask)
where
    C: ChannelControl + 'static,
    F: Fn() -> (Broadcaster, C) + Send + 'static,
    L: Fn(Msg) + Send + Sync + 'static,
{
    let (commands_tx, commands_rx) = mpsc::channel(COMMAND_CAPACITY);
    let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(Snapshot::starting()));
    let (stop_tx, stop_rx) = watch::channel(false);
    let supervisor = Supervisor {
        build,
        commands: commands_rx,
        snapshot: snapshot_tx,
        stop: stop_rx,
        log,
        slates: None,
        pause_reasons: pause_reasons.into_iter().collect(),
    };
    let join = tokio::spawn(supervisor.run());
    let handle = EngineHandle {
        commands: commands_tx,
        snapshot: snapshot_rx,
    };
    (
        handle,
        EngineTask {
            stop: stop_tx,
            join,
        },
    )
}

/// A handle with no loop behind it (desktop app without working tools): the snapshot stays at
/// the empty placeholder and every command is refused (and logged by the sender).
pub fn idle_engine_handle() -> EngineHandle {
    let (commands, _) = mpsc::channel(1);
    let (_, snapshot) = watch::channel(Arc::new(Snapshot::starting()));
    EngineHandle { commands, snapshot }
}

impl Snapshot {
    /// Placeholder until the first channel has published its initial segments.
    fn starting() -> Self {
        Self {
            playlist: String::new(),
            segments: HashSet::new(),
            live_dir: PathBuf::new(),
            state: ChannelState {
                status: ChannelStatus::Idle,
                current: None,
                paused: false,
                pause_reasons: Vec::new(),
                control: Default::default(),
            },
            prefetched_item_id: None,
            stats: ChannelStats::default(),
            monitor: Arc::default(),
        }
    }
}

/// How one run of the loop ended.
enum Exit {
    Shutdown,
    Failed(String),
}

/// Consecutive crashed runs with the same queue item current.
#[derive(Default)]
struct CrashStreak {
    item: Option<i64>,
    count: u32,
}

impl CrashStreak {
    /// Records a crash with `id` current; returns how many crashes in a row it has caused.
    fn record(&mut self, id: i64) -> u32 {
        if self.item == Some(id) {
            self.count += 1;
        } else {
            self.item = Some(id);
            self.count = 1;
        }
        self.count
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Owns everything that must outlive a crashed run: the command receiver, the snapshot sender and
/// the factory of fresh (Broadcaster, Channel) pairs.
struct Supervisor<F, L> {
    build: F,
    commands: mpsc::Receiver<Command>,
    snapshot: watch::Sender<Arc<Snapshot>>,
    stop: watch::Receiver<bool>,
    log: L,
    /// The newest slate set handed over with [`Command::ReplaceSlates`]; every rebuilt channel
    /// gets it before it starts.
    slates: Option<Slates>,
    /// Pause reasons active right now (from [`Command::Pause`] / [`Command::Resume`]); a rebuilt
    /// channel gets them before it starts, so it does not air films while one is active.
    pause_reasons: BTreeSet<PauseReason>,
}

impl<C, F, L> Supervisor<F, L>
where
    C: ChannelControl,
    F: Fn() -> (Broadcaster, C),
    L: Fn(Msg) + Sync,
{
    /// Restarts a failed run with exponential backoff (1 s, 2 s, 4 s … 30 s; back to 1 s after a
    /// run of at least 60 s). A film current in 3 crashed runs in a row is marked failed
    /// ("Internal error"): `Channel::start` would otherwise put it back at the head forever.
    async fn run(mut self) {
        let mut delay = RESTART_DELAY;
        let mut streak = CrashStreak::default();
        loop {
            let started = tokio::time::Instant::now();
            let failure = match guarded(&self.build) {
                Err(panic) => {
                    streak.clear();
                    panic
                }
                Ok((broadcaster, channel)) => {
                    let mut core = Core {
                        broadcaster,
                        channel,
                    };
                    let exit = self.run_core(&mut core).await;
                    if let Exit::Failed(_) = exit {
                        self.judge_current_item(&mut core.channel, &mut streak);
                    }
                    // Also after a panic: dispose the crashed channel's clips (ffmpeg processes)
                    // instead of leaving them to a plain drop.
                    if let Err(panic) = guarded(|| core.channel.stop()) {
                        (self.log)(Msg::new("log.channel_stop_error").arg("error", panic));
                    }
                    match exit {
                        Exit::Shutdown => return,
                        Exit::Failed(msg) => msg,
                    }
                }
            };
            if started.elapsed() >= HEALTHY_RUN {
                delay = RESTART_DELAY;
            }
            (self.log)(Msg::new("log.channel_loop_failed").arg("error", &failure));
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = stop_requested(&mut self.stop) => return,
            }
            delay = (delay * 2).min(MAX_RESTART_DELAY);
            if self.commands.is_closed() {
                return;
            }
        }
    }

    /// Counts the crash against the item current at that moment; on the third crash in a row
    /// with the same item, marks it failed (before the channel is stopped and rebuilt).
    fn judge_current_item(&self, channel: &mut C, streak: &mut CrashStreak) {
        let Ok(Some(id)) = guarded(|| channel.current_item_id()) else {
            streak.clear();
            return;
        };
        if streak.record(id) < CRASHES_BEFORE_FAILING_ITEM {
            return;
        }
        streak.clear();
        (self.log)(
            Msg::new("log.item_keeps_crashing")
                .arg("id", id)
                .arg("count", CRASHES_BEFORE_FAILING_ITEM)
                .arg_msg("reason", internal_error()),
        );
        match guarded(|| channel.fail_item(id, &internal_error())) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => (self.log)(
                Msg::new("log.item_fail_mark_failed")
                    .arg("id", id)
                    .arg_msg("error", error_msg(&e)),
            ),
            Err(panic) => (self.log)(
                Msg::new("log.item_fail_mark_failed")
                    .arg("id", id)
                    .arg("error", panic),
            ),
        }
    }

    /// Starts the pair and ticks it every 250 ms until shutdown or a failure.
    async fn run_core(&mut self, core: &mut Core<C>) -> Exit {
        if let Some(slates) = self.slates.clone() {
            core.channel.replace_slates(slates);
        }
        let reasons = self.pause_reasons.clone();
        let started = guarded(|| {
            for reason in reasons {
                core.channel.pause(reason);
            }
            core.start()
        });
        match started {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Exit::Failed(e.to_string()),
            Err(panic) => return Exit::Failed(panic),
        }
        if let Err(panic) = self.publish(core) {
            return Exit::Failed(panic);
        }
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // The first tick completes immediately; the initial segments were just published.
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = stop_requested(&mut self.stop) => return Exit::Shutdown,
            }
            loop {
                let command = match self.commands.try_recv() {
                    Ok(command) => command,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Exit::Shutdown,
                };
                match &command {
                    Command::ReplaceSlates(slates) => self.slates = Some(slates.clone()),
                    Command::Pause(reason) => {
                        self.pause_reasons.insert(*reason);
                    }
                    Command::PauseAt { .. } => {
                        self.pause_reasons.insert(PauseReason::Manual);
                    }
                    Command::Resume(reason) => {
                        self.pause_reasons.remove(reason);
                    }
                    Command::Skip
                    | Command::QueueChanged
                    | Command::Seek { .. }
                    | Command::SeekBy { .. }
                    | Command::Restart
                    | Command::PlayNow { .. }
                    | Command::ApplyTicker
                    | Command::ApplyTickerCode { .. } => {}
                }
                let play_now = matches!(command, Command::PlayNow { .. });
                match guarded(|| core.apply(command)) {
                    Ok(Ok(())) => {
                        // A play-now ends the manual pause (see `Channel::play_now`).
                        if play_now {
                            self.pause_reasons.remove(&PauseReason::Manual);
                        }
                        // A pause waiting for its position also ends with a skip (a seek
                        // keeps it) or when its film ends.
                        match core.channel.manual_pause() {
                            Some(true) => {
                                self.pause_reasons.insert(PauseReason::Manual);
                            }
                            Some(false) => {
                                self.pause_reasons.remove(&PauseReason::Manual);
                            }
                            None => {}
                        }
                    }
                    Ok(Err(e)) => (self.log)(
                        Msg::new("log.channel_command_failed").arg_msg("error", error_msg(&e)),
                    ),
                    Err(panic) => return Exit::Failed(panic),
                }
            }
            if let Err(panic) = guarded(|| core.tick()) {
                return Exit::Failed(panic);
            }
            if let Err(panic) = self.publish(core) {
                return Exit::Failed(panic);
            }
        }
    }

    fn publish(&self, core: &Core<C>) -> Result<(), String> {
        let snapshot = guarded(|| core.snapshot())?;
        self.snapshot.send_replace(Arc::new(snapshot));
        Ok(())
    }
}

/// The pair one run of the loop owns.
struct Core<C> {
    broadcaster: Broadcaster,
    channel: C,
}

impl<C: ChannelControl> Core<C> {
    fn start(&mut self) -> anyhow::Result<()> {
        self.channel.start()?;
        self.broadcaster.start_initial(&mut self.channel);
        Ok(())
    }

    fn apply(&mut self, command: Command) -> anyhow::Result<()> {
        match command {
            Command::Skip => self.channel.skip(),
            Command::QueueChanged => self.channel.on_queue_changed(),
            Command::Pause(reason) => {
                self.channel.pause(reason);
                Ok(())
            }
            Command::PauseAt { at_s } => {
                self.channel.pause_at(at_s);
                Ok(())
            }
            Command::Resume(reason) => {
                self.channel.resume(reason);
                Ok(())
            }
            Command::ReplaceSlates(slates) => {
                self.channel.replace_slates(slates);
                Ok(())
            }
            Command::Seek { to_s } => {
                self.channel.seek_to(to_s);
                Ok(())
            }
            Command::SeekBy { delta_s } => {
                self.channel.seek_by(delta_s);
                Ok(())
            }
            Command::Restart => {
                self.channel.seek_to(0.0);
                Ok(())
            }
            Command::PlayNow { item_id } => self.channel.play_now(item_id),
            Command::ApplyTicker => {
                self.channel.apply_ticker();
                Ok(())
            }
            Command::ApplyTickerCode { public_url } => {
                self.channel.apply_ticker_code(&public_url);
                Ok(())
            }
        }
    }

    fn tick(&mut self) {
        self.channel.on_tick();
        self.broadcaster.tick(&mut self.channel);
    }

    /// Taken between ticks, so the playlist, the known segments and the files on disk agree:
    /// every playlist entry is in the window, hence known, and was written before it was listed.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            playlist: self.broadcaster.playlist(),
            segments: self.broadcaster.known_segments().into_iter().collect(),
            live_dir: self.broadcaster.dir().to_path_buf(),
            state: self.channel.state(),
            prefetched_item_id: self.channel.prefetched_item_id(),
            stats: ChannelStats {
                hls: Some(self.broadcaster.counters()),
                ..self.channel.stats()
            },
            monitor: Arc::new(self.channel.monitor_feed()),
        }
    }
}

/// Runs `f`, turning a panic into its message. After a panic the pair is only stopped and dropped,
/// never ticked again, so observing it in a broken state is harmless (hence `AssertUnwindSafe`).
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|payload| panic_message(&*payload))
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "nieznana panika".to_string()
    }
}

/// Resolves once a shutdown was requested; never, if the [`EngineTask`] was dropped without one.
async fn stop_requested(stop: &mut watch::Receiver<bool>) {
    if stop.wait_for(|stop| *stop).await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueItem, QueueRepo};
    use crate::i18n::Lang;
    use crate::pipeline::broadcaster::{BroadcasterOptions, PublishedSegment, SourceSegment};
    use crate::pipeline::channel::{BoxFuture, ChannelOptions, ClipFactory, ClipHandle};
    use crate::pipeline::segmenter::SegmentInfo;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::time::Instant;

    /// A TS segment with a single PES packet carrying the given PTS.
    fn ts_with_pts(pts: u64) -> Vec<u8> {
        let mut buf = vec![0xffu8; 188];
        buf[..4].copy_from_slice(&[0x47, 0x41, 0x00, 0x10]);
        let hi = ((pts >> 30) & 7) as u8;
        let lo = pts & ((1 << 30) - 1);
        let pes: [u8; 14] = [
            0,
            0,
            1,
            0xe0,
            0,
            0,
            0x80,
            0x80,
            5,
            0x21 | (hi << 1),
            ((lo >> 22) & 0xff) as u8,
            ((((lo >> 15) & 0x7f) << 1) | 1) as u8,
            ((lo >> 7) & 0xff) as u8,
            (((lo & 0x7f) << 1) | 1) as u8,
        ];
        buf[4..18].copy_from_slice(&pes);
        buf
    }

    fn tokio_clock() -> Box<dyn Fn() -> u64 + Send> {
        let t0 = Instant::now();
        Box::new(move || u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    /// Shared record of what the fake channels saw (survives restarts).
    #[derive(Default)]
    struct Probe {
        builds: AtomicUsize,
        starts: AtomicUsize,
        stops: AtomicUsize,
        skips: AtomicUsize,
        queue_changes: AtomicUsize,
        nexts: AtomicUsize,
        /// `on_tick()` calls.
        ticks: AtomicUsize,
        panic_build_once: AtomicBool,
        fail_start_once: AtomicBool,
        fail_skip: AtomicBool,
        /// `next()` call number (1-based, counted across restarts) that panics once; 0 = never.
        panic_at_next: AtomicUsize,
        /// `start()` call numbers (1-based) that fail with an error.
        fail_starts: Mutex<HashSet<usize>>,
        /// When each `start()` was called.
        start_times: Mutex<Vec<Instant>>,
        /// Queue item current in the run started by `start()` number n (index n - 1).
        items_per_start: Mutex<Vec<Option<i64>>>,
        /// `start()` panics while an item is current (a film that crashes the loop).
        panic_start_with_item: AtomicBool,
        /// Items marked failed through `fail_item`.
        failed: Mutex<Vec<(i64, String)>>,
        /// Order of `start`, `fail_item` and `stop` calls.
        events: Mutex<Vec<String>>,
    }

    impl Probe {
        fn event(&self, e: String) {
            self.events.lock().unwrap().push(e);
        }
    }

    struct FakeChannel {
        probe: Arc<Probe>,
        src_dir: PathBuf,
        current: Option<i64>,
    }

    impl SegmentSource for FakeChannel {
        fn next(&mut self) -> anyhow::Result<SourceSegment> {
            let n = self.probe.nexts.fetch_add(1, Ordering::SeqCst) + 1;
            if self
                .probe
                .panic_at_next
                .compare_exchange(n, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                panic!("atrapa źródła: awaria");
            }
            let path = self.src_dir.join(format!("{n}.ts"));
            std::fs::write(&path, ts_with_pts(126_000))?;
            Ok(SourceSegment {
                path,
                duration: 4.0,
                source_key: "idle".to_string(),
                index: 0,
            })
        }

        fn on_published(&mut self, _seg: &PublishedSegment) {}
    }

    impl ChannelControl for FakeChannel {
        fn start(&mut self) -> anyhow::Result<()> {
            let n = self.probe.starts.fetch_add(1, Ordering::SeqCst) + 1;
            self.probe.start_times.lock().unwrap().push(Instant::now());
            self.probe.event("start".to_string());
            let item = self
                .probe
                .items_per_start
                .lock()
                .unwrap()
                .get(n - 1)
                .copied()
                .flatten();
            let failed = self.probe.failed.lock().unwrap().clone();
            self.current = item.filter(|id| !failed.iter().any(|(f, _)| f == id));
            if self.probe.fail_start_once.swap(false, Ordering::SeqCst)
                || self.probe.fail_starts.lock().unwrap().contains(&n)
            {
                anyhow::bail!("baza zablokowana");
            }
            if self.current.is_some() && self.probe.panic_start_with_item.load(Ordering::SeqCst) {
                panic!("atrapa filmu: awaria");
            }
            Ok(())
        }
        fn stop(&mut self) {
            self.probe.stops.fetch_add(1, Ordering::SeqCst);
            self.probe.event("stop".to_string());
        }
        fn current_item_id(&self) -> Option<i64> {
            self.current
        }
        fn fail_item(&mut self, id: i64, error: &Msg) -> anyhow::Result<()> {
            let error = error.render(Lang::Pl);
            self.probe.event(format!("fail {id}: {error}"));
            self.probe.failed.lock().unwrap().push((id, error));
            Ok(())
        }
        fn skip(&mut self) -> anyhow::Result<()> {
            self.probe.skips.fetch_add(1, Ordering::SeqCst);
            if self.probe.fail_skip.load(Ordering::SeqCst) {
                anyhow::bail!("nie można pominąć");
            }
            Ok(())
        }
        fn on_queue_changed(&mut self) -> anyhow::Result<()> {
            self.probe.queue_changes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn pause(&mut self, reason: PauseReason) {
            self.probe.event(format!("pause {}", reason.as_str()));
        }
        fn resume(&mut self, reason: PauseReason) {
            self.probe.event(format!("resume {}", reason.as_str()));
        }
        fn replace_slates(&mut self, slates: Slates) {
            self.probe.event(format!("slates g{}", slates.generation));
        }
        fn seek_to(&mut self, to_s: f64) {
            self.probe.event(format!("seek_to {to_s}"));
        }
        fn seek_by(&mut self, delta_s: f64) {
            self.probe.event(format!("seek_by {delta_s}"));
        }
        fn play_now(&mut self, item_id: i64) -> anyhow::Result<()> {
            self.probe.event(format!("play_now {item_id}"));
            Ok(())
        }
        fn apply_ticker(&mut self) {
            self.probe.event("apply_ticker".to_string());
        }
        fn apply_ticker_code(&mut self, public_url: &str) {
            self.probe.event(format!("apply_ticker_code {public_url}"));
        }
        fn on_tick(&mut self) {
            self.probe.ticks.fetch_add(1, Ordering::SeqCst);
        }
        fn state(&self) -> ChannelState {
            ChannelState {
                status: ChannelStatus::Buffering,
                current: None,
                paused: false,
                pause_reasons: Vec::new(),
                control: Default::default(),
            }
        }
        fn prefetched_item_id(&self) -> Option<i64> {
            Some(42)
        }
    }

    struct Env {
        live: tempfile::TempDir,
        src: tempfile::TempDir,
        probe: Arc<Probe>,
        logs: Arc<Mutex<Vec<String>>>,
    }

    impl Env {
        fn new() -> Self {
            Self {
                live: tempfile::tempdir().unwrap(),
                src: tempfile::tempdir().unwrap(),
                probe: Arc::new(Probe::default()),
                logs: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn live_dir(&self) -> PathBuf {
            self.live.path().join("main")
        }

        fn count(&self, field: impl Fn(&Probe) -> &AtomicUsize) -> usize {
            field(&self.probe).load(Ordering::SeqCst)
        }

        fn logs(&self) -> Vec<String> {
            self.logs.lock().unwrap().clone()
        }

        /// Window of 3 segments, retired files deleted on the next tick.
        fn spawn(&self) -> (EngineHandle, EngineTask) {
            self.spawn_paused(&[])
        }

        /// Like `spawn`, with the loop started under `reasons`.
        fn spawn_paused(&self, reasons: &[PauseReason]) -> (EngineHandle, EngineTask) {
            let probe = Arc::clone(&self.probe);
            let live_dir = self.live_dir();
            let src_dir = self.src.path().to_path_buf();
            let build = move || {
                probe.builds.fetch_add(1, Ordering::SeqCst);
                if probe.panic_build_once.swap(false, Ordering::SeqCst) {
                    panic!("atrapa budowy: awaria");
                }
                let options = BroadcasterOptions {
                    dir: live_dir.clone(),
                    window_size: 3,
                    segment_seconds: 4.0,
                    path_prefix: "main/".to_string(),
                    discontinuity_tags: Box::new(|| true),
                    retain_s: Some(0),
                    log: None,
                };
                let channel = FakeChannel {
                    probe: Arc::clone(&probe),
                    src_dir: src_dir.clone(),
                    current: None,
                };
                (Broadcaster::new(options, tokio_clock()), channel)
            };
            let logs = Arc::clone(&self.logs);
            spawn_engine_paused(
                build,
                move |m: Msg| logs.lock().unwrap().push(m.to_string()),
                reasons.iter().copied(),
            )
        }
    }

    /// Segment file names listed in the playlist.
    fn names(s: &Snapshot) -> Vec<String> {
        s.playlist
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.rsplit('/').next().unwrap_or(l).to_string())
            .collect()
    }

    async fn wait_until(
        rx: &mut watch::Receiver<Arc<Snapshot>>,
        pred: impl FnMut(&Arc<Snapshot>) -> bool,
    ) -> Arc<Snapshot> {
        tokio::time::timeout(Duration::from_secs(120), rx.wait_for(pred))
            .await
            .expect("timed out waiting for a snapshot")
            .expect("engine gone")
            .clone()
    }

    async fn next_snapshot(rx: &mut watch::Receiver<Arc<Snapshot>>) -> Arc<Snapshot> {
        tokio::time::timeout(Duration::from_secs(5), rx.changed())
            .await
            .expect("timed out waiting for a snapshot")
            .expect("engine gone");
        rx.borrow_and_update().clone()
    }

    #[tokio::test(start_paused = true)]
    async fn publishes_a_snapshot_after_start_and_after_every_tick() {
        let env = Env::new();
        let (mut handle, _task) = env.spawn();
        let first = wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        assert_eq!(names(&first), vec!["0.ts", "1.ts", "2.ts"]);
        assert_eq!(first.live_dir, env.live_dir());
        assert_eq!(first.state.status, ChannelStatus::Buffering);
        assert_eq!(first.prefetched_item_id, Some(42));

        let mut at = Instant::now();
        for _ in 0..20 {
            next_snapshot(&mut handle.snapshot).await;
            assert_eq!(at.elapsed(), Duration::from_millis(250));
            at = Instant::now();
        }
        // 20 ticks = 5 s: one more 4-second segment went out (window of 3).
        assert_eq!(
            names(&handle.snapshot.borrow()),
            vec!["1.ts", "2.ts", "3.ts"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn skip_and_queue_changed_reach_the_channel_on_the_next_tick() {
        let env = Env::new();
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        handle.commands.send(Command::Skip).await.unwrap();
        handle.commands.send(Command::QueueChanged).await.unwrap();
        assert_eq!(env.count(|p| &p.skips), 0);
        next_snapshot(&mut handle.snapshot).await;
        assert_eq!(env.count(|p| &p.skips), 1);
        assert_eq!(env.count(|p| &p.queue_changes), 1);
        assert_eq!(env.count(|p| &p.builds), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_command_is_logged_and_the_loop_goes_on() {
        let env = Env::new();
        env.probe.fail_skip.store(true, Ordering::SeqCst);
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        handle.commands.send(Command::Skip).await.unwrap();
        next_snapshot(&mut handle.snapshot).await;
        assert!(
            env.logs()
                .contains(&"A channel command failed: nie można pominąć".to_string()),
            "{:?}",
            env.logs()
        );
        wait_until(&mut handle.snapshot, |s| names(s).contains(&"3.ts".into())).await;
        assert_eq!(env.count(|p| &p.builds), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn pause_and_resume_commands_carry_their_reason_to_the_channel() {
        let env = Env::new();
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        handle
            .commands
            .send(Command::Pause(PauseReason::Tunnel))
            .await
            .unwrap();
        handle
            .commands
            .send(Command::Pause(PauseReason::NoViewers))
            .await
            .unwrap();
        handle
            .commands
            .send(Command::Resume(PauseReason::Tunnel))
            .await
            .unwrap();
        next_snapshot(&mut handle.snapshot).await;
        let events = env.probe.events.lock().unwrap().clone();
        assert_eq!(
            events,
            ["start", "pause tunnel", "pause no_viewers", "resume tunnel"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn control_commands_reach_the_channel_and_it_is_ticked_every_250_ms() {
        let env = Env::new();
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        for command in [
            Command::Pause(PauseReason::Manual),
            Command::Seek { to_s: 30.0 },
            Command::SeekBy { delta_s: -10.0 },
            Command::Restart,
            Command::PlayNow { item_id: 5 },
            Command::ApplyTicker,
            Command::ApplyTickerCode {
                public_url: "https://a.example.com".into(),
            },
            Command::Resume(PauseReason::Manual),
        ] {
            handle.commands.send(command).await.unwrap();
        }
        next_snapshot(&mut handle.snapshot).await;
        assert_eq!(
            *env.probe.events.lock().unwrap(),
            [
                "start",
                "pause manual",
                "seek_to 30",
                "seek_by -10",
                "seek_to 0",
                "play_now 5",
                "apply_ticker",
                "apply_ticker_code https://a.example.com",
                "resume manual"
            ]
        );
        let before = env.count(|p| &p.ticks);
        assert!(before >= 1);
        for _ in 0..4 {
            next_snapshot(&mut handle.snapshot).await;
        }
        assert_eq!(env.count(|p| &p.ticks), before + 4);
    }

    /// Controller's ruling: a successful play-now clears the manual pause, also for a channel
    /// rebuilt later.
    #[tokio::test(start_paused = true)]
    async fn play_now_clears_the_manual_pause_for_a_rebuilt_channel_too() {
        let env = Env::new();
        env.probe.panic_at_next.store(5, Ordering::SeqCst);
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        for command in [
            Command::Pause(PauseReason::Manual),
            Command::Pause(PauseReason::Tunnel),
            Command::PlayNow { item_id: 5 },
        ] {
            handle.commands.send(command).await.unwrap();
        }
        next_snapshot(&mut handle.snapshot).await;
        let probe = Arc::clone(&env.probe);
        wait_until(&mut handle.snapshot, |s| {
            probe.builds.load(Ordering::SeqCst) == 2 && !s.segments.is_empty()
        })
        .await;
        assert_eq!(
            *env.probe.events.lock().unwrap(),
            [
                "start",
                "pause manual",
                "pause tunnel",
                "play_now 5",
                "stop",
                "pause tunnel",
                "start"
            ]
        );
    }

    /// A manual pause survives a crashed loop like the automatic reasons.
    #[tokio::test(start_paused = true)]
    async fn a_rebuilt_channel_keeps_the_manual_pause() {
        let env = Env::new();
        env.probe.panic_at_next.store(5, Ordering::SeqCst);
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        handle
            .commands
            .send(Command::Pause(PauseReason::Manual))
            .await
            .unwrap();
        next_snapshot(&mut handle.snapshot).await;
        let probe = Arc::clone(&env.probe);
        wait_until(&mut handle.snapshot, |s| {
            probe.builds.load(Ordering::SeqCst) == 2 && !s.segments.is_empty()
        })
        .await;
        assert_eq!(
            *env.probe.events.lock().unwrap(),
            ["start", "pause manual", "stop", "pause manual", "start"]
        );
    }

    /// Review Focus 2: a snapshot never lists a segment HTTP cannot serve.
    #[tokio::test(start_paused = true)]
    async fn every_playlist_segment_is_known_and_on_disk_in_every_snapshot() {
        let env = Env::new();
        let (mut handle, _task) = env.spawn();
        let mut checked = 0;
        while checked < 200 {
            // On a current-thread runtime the loop is parked while we look, so the disk is
            // exactly as it was when this snapshot was published.
            let s = next_snapshot(&mut handle.snapshot).await;
            if s.segments.is_empty() {
                continue;
            }
            let listed = names(&s);
            assert!(!listed.is_empty());
            for name in &listed {
                assert!(s.segments.contains(name), "{name} not in {:?}", s.segments);
                assert!(s.live_dir.join(name).exists(), "{name} missing on disk");
            }
            for name in &s.segments {
                assert!(s.live_dir.join(name).exists(), "{name} missing on disk");
            }
            checked += 1;
        }
        // 200 ticks = 50 s: the window moved on and old files were really deleted.
        let s = handle.snapshot.borrow().clone();
        assert!(!s.segments.contains("0.ts"));
        assert!(!env.live_dir().join("0.ts").exists());
    }

    /// Review Focus 5: a panicking source does not end the broadcast.
    #[tokio::test(start_paused = true)]
    async fn a_panicking_source_restarts_the_loop_and_the_playlist_keeps_growing() {
        let env = Env::new();
        // Calls 1-3 come from start_initial, 4 at 4 s, 5 at 8 s -> panic inside tick().
        env.probe.panic_at_next.store(5, Ordering::SeqCst);
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| names(s).contains(&"3.ts".into())).await;
        assert_eq!(env.count(|p| &p.builds), 1);

        let probe = Arc::clone(&env.probe);
        let restarted = wait_until(&mut handle.snapshot, |s| {
            probe.builds.load(Ordering::SeqCst) == 2 && !s.segments.is_empty()
        })
        .await;
        assert_eq!(names(&restarted), vec!["0.ts", "1.ts", "2.ts"]);
        assert_eq!(
            env.logs(),
            vec!["The channel loop ended with an error: atrapa źródła: awaria — restarting"]
        );
        // The crashed channel was stopped (its clips disposed) before the rebuild.
        assert_eq!(env.count(|p| &p.stops), 1);
        assert_eq!(env.count(|p| &p.starts), 2);

        wait_until(&mut handle.snapshot, |s| names(s).contains(&"5.ts".into())).await;
        assert_eq!(env.count(|p| &p.builds), 2);
        // Commands still reach the rebuilt channel through the same handle.
        handle.commands.send(Command::Skip).await.unwrap();
        next_snapshot(&mut handle.snapshot).await;
        assert_eq!(env.count(|p| &p.skips), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_build_panic_or_failed_start_is_logged_and_retried_with_backoff() {
        let env = Env::new();
        env.probe.panic_build_once.store(true, Ordering::SeqCst);
        env.probe.fail_start_once.store(true, Ordering::SeqCst);
        let t0 = Instant::now();
        let (mut handle, _task) = env.spawn();
        let s = wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        // 1 s after the first failure, 2 s after the second.
        assert_eq!(t0.elapsed(), Duration::from_secs(3));
        assert_eq!(names(&s), vec!["0.ts", "1.ts", "2.ts"]);
        assert_eq!(
            env.logs(),
            vec![
                "The channel loop ended with an error: atrapa budowy: awaria — restarting",
                "The channel loop ended with an error: baza zablokowana — restarting",
            ]
        );
        assert_eq!(env.count(|p| &p.builds), 3);
        assert_eq!(env.count(|p| &p.starts), 2);
        // The channel whose start failed was stopped too.
        assert_eq!(env.count(|p| &p.stops), 1);
    }

    /// Seconds between consecutive `start()` calls.
    fn start_gaps(probe: &Probe) -> Vec<u64> {
        probe
            .start_times
            .lock()
            .unwrap()
            .windows(2)
            .map(|w| (w[1] - w[0]).as_secs())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn restarts_back_off_exponentially_up_to_30_s() {
        let env = Env::new();
        env.probe.fail_starts.lock().unwrap().extend(1..=9);
        let (_handle, _task) = env.spawn();
        // 1 + 2 + 4 + 8 + 16 + 30 + 30 + 30 = 121 s for 9 starts.
        tokio::time::sleep(Duration::from_millis(121_500)).await;
        assert_eq!(start_gaps(&env.probe), vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_of_60_s_or_more_resets_the_backoff_to_1_s() {
        let env = Env::new();
        env.probe.fail_starts.lock().unwrap().extend([1, 2, 4]);
        // Run 3 starts at 3 s (nexts 1-3); next n airs at 3 + 4 (n - 3) s: #19 at 67 s,
        // after 64 s of running.
        env.probe.panic_at_next.store(19, Ordering::SeqCst);
        let (mut handle, _task) = env.spawn();
        let probe = Arc::clone(&env.probe);
        wait_until(&mut handle.snapshot, |_| {
            probe.starts.load(Ordering::SeqCst) >= 5
        })
        .await;
        let times = env.probe.start_times.lock().unwrap().clone();
        let at: Vec<u64> = times.iter().map(|t| (*t - times[0]).as_secs()).collect();
        // 1 s, 2 s, then the long run, 1 s after its crash, then 2 s again.
        assert_eq!(at, vec![0, 1, 3, 68, 70]);
    }

    #[tokio::test(start_paused = true)]
    async fn three_crashes_in_a_row_with_the_same_film_mark_it_failed() {
        let env = Env::new();
        env.probe
            .panic_start_with_item
            .store(true, Ordering::SeqCst);
        *env.probe.items_per_start.lock().unwrap() = vec![Some(7); 10];
        let (mut handle, _task) = env.spawn();
        let s = wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        assert_eq!(names(&s), vec!["0.ts", "1.ts", "2.ts"]);
        assert_eq!(
            *env.probe.failed.lock().unwrap(),
            vec![(7, "Błąd wewnętrzny".to_string())]
        );
        // Marked failed before the crashed channel was stopped and rebuilt.
        assert_eq!(
            *env.probe.events.lock().unwrap(),
            vec![
                "start",
                "stop",
                "start",
                "stop",
                "start",
                "fail 7: Błąd wewnętrzny",
                "stop",
                "start",
            ]
        );
        let logs = env.logs();
        assert!(
            logs.iter()
                .any(|l| l.contains("Video #7") && l.contains("Internal error")),
            "{logs:?}"
        );
        assert_eq!(start_gaps(&env.probe), vec![1, 2, 4]);
    }

    #[tokio::test(start_paused = true)]
    async fn crashes_with_different_films_are_not_counted_together() {
        let env = Env::new();
        env.probe
            .panic_start_with_item
            .store(true, Ordering::SeqCst);
        *env.probe.items_per_start.lock().unwrap() =
            vec![Some(7), Some(7), Some(8), Some(7), None, None];
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        assert_eq!(env.count(|p| &p.starts), 5);
        assert!(env.probe.failed.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_every_handle_stops_the_loop_and_the_channel() {
        let env = Env::new();
        let (mut handle, task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        let clone = handle.clone();
        drop(handle);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!task.is_finished());
        drop(clone);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(task.is_finished());
        assert_eq!(env.count(|p| &p.stops), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_stops_the_loop_and_the_channel() {
        let env = Env::new();
        let (mut handle, task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        task.shutdown().await;
        assert_eq!(env.count(|p| &p.stops), 1);
        assert!(handle.commands.try_send(Command::Skip).is_err());
    }

    // ---------- Real Channel ----------

    /// Clip starts never finish: the film stays "playing" behind the buffering slate.
    struct PendingFactory;

    impl ClipFactory for PendingFactory {
        fn create(
            &self,
            _item: QueueItem,
            _start_at_s: f64,
            _attempt: u32,
        ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
            Box::pin(std::future::pending())
        }
    }

    /// A film whose content crashes the loop: every poll of its clip panics.
    struct PanickingFactory;

    struct PanickingClip;

    impl ClipHandle for PanickingClip {
        fn state(&self) -> crate::pipeline::segmenter::SegmenterState {
            crate::pipeline::segmenter::SegmenterState::Running
        }
        fn error(&self) -> Option<Msg> {
            None
        }
        fn duration_s(&self) -> Option<f64> {
            None
        }
        fn poll(&mut self) -> Vec<SegmentInfo> {
            panic!("zepsuty fragment filmu");
        }
        fn kill(&mut self) {}
        fn dispose(&mut self) {}
    }

    impl ClipFactory for PanickingFactory {
        fn create(
            &self,
            _item: QueueItem,
            _start_at_s: f64,
            _attempt: u32,
        ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
            Box::pin(async { Ok(Box::new(PanickingClip) as Box<dyn ClipHandle>) })
        }
    }

    fn real_slates(dir: &Path) -> Slates {
        Slates {
            idle: slate_in(dir, "idle.ts"),
            buffering: slate_in(dir, "buffering.ts"),
            pause: slate_in(dir, "pause.ts"),
            generation: 0,
            japanese_font_missing: false,
            font_missing: None,
        }
    }

    /// The crash loop of the final review: `start()` puts the crashing film back at the head,
    /// so without a verdict it would crash the loop forever.
    #[tokio::test(start_paused = true)]
    async fn a_film_that_keeps_crashing_the_loop_is_failed_after_three_crashes() {
        let live = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let db = Arc::new(Mutex::new(
            crate::db::open_database(Path::new(":memory:")).unwrap(),
        ));
        let add = |title: &str| {
            let conn = db.lock().unwrap();
            QueueRepo::new(&conn)
                .add(&NewItem {
                    channel_id: MAIN_CHANNEL_ID,
                    source_url: format!("https://youtu.be/{title}"),
                    title: title.into(),
                    duration_s: Some(100.0),
                    is_live: false,
                    status: ItemStatus::Queued,
                    added_by: AddedBy::Admin,
                    requester_name: None,
                    requester_ip: None,
                    requester_key_hash: None,
                })
                .unwrap()
        };
        let bad = add("zły");
        let slates_dir = src.path().to_path_buf();
        let live_dir = live.path().join("main");
        let shared_db = Arc::clone(&db);
        let builds = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&builds);
        let logs = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&logs);
        let (mut handle, _task) = spawn_engine_with(
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                let channel = Channel::new(ChannelOptions::new(
                    MAIN_CHANNEL_ID,
                    Arc::clone(&shared_db),
                    Arc::new(PanickingFactory),
                    real_slates(&slates_dir),
                ));
                let options = BroadcasterOptions {
                    dir: live_dir.clone(),
                    window_size: 10,
                    segment_seconds: 4.0,
                    path_prefix: "main/".to_string(),
                    discontinuity_tags: Box::new(|| true),
                    retain_s: None,
                    log: None,
                };
                (Broadcaster::new(options, tokio_clock()), channel)
            },
            move |m: Msg| sink.lock().unwrap().push(m.to_string()),
        );
        let status = |db: &Arc<Mutex<rusqlite::Connection>>| {
            let conn = db.lock().unwrap();
            QueueRepo::new(&conn).get(bad.id).unwrap().unwrap()
        };
        let counted = Arc::clone(&builds);
        wait_until(&mut handle.snapshot, |s| {
            counted.load(Ordering::SeqCst) == 4 && !s.segments.is_empty()
        })
        .await;
        let failed = status(&db);
        assert_eq!(failed.status, ItemStatus::Failed);
        assert_eq!(
            failed.error_text(Lang::En).as_deref(),
            Some("Internal error")
        );
        assert_eq!(
            failed.error_text(Lang::Pl).as_deref(),
            Some("Błąd wewnętrzny")
        );
        // The rebuilt channel stays up on the idle slate; no further crashes.
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(builds.load(Ordering::SeqCst), 4);
        let s = handle.snapshot.borrow().clone();
        assert_eq!(s.state.status, ChannelStatus::Idle);
        let logs = logs.lock().unwrap().clone();
        let crashes = logs
            .iter()
            .filter(|l| l.contains("zepsuty fragment filmu"))
            .count();
        assert_eq!(crashes, 3, "{logs:?}");
        assert!(
            logs.iter()
                .any(|l| l.contains(&format!("Video #{}", bad.id)) && l.contains("Internal error")),
            "{logs:?}"
        );
    }

    fn slate_in(dir: &Path, name: &str) -> Vec<SegmentInfo> {
        let path = dir.join(name);
        std::fs::write(&path, ts_with_pts(126_000)).unwrap();
        vec![SegmentInfo {
            path,
            duration: 4.0,
            index: 0,
        }]
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_engine_drives_a_real_channel_and_skip_reaches_it() {
        let live = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let db = Arc::new(Mutex::new(
            crate::db::open_database(Path::new(":memory:")).unwrap(),
        ));
        let item = {
            let conn = db.lock().unwrap();
            QueueRepo::new(&conn)
                .add(&NewItem {
                    channel_id: MAIN_CHANNEL_ID,
                    source_url: "https://youtu.be/a".into(),
                    title: "Film".into(),
                    duration_s: Some(100.0),
                    is_live: false,
                    status: ItemStatus::Queued,
                    added_by: AddedBy::Admin,
                    requester_name: None,
                    requester_ip: None,
                    requester_key_hash: None,
                })
                .unwrap()
        };
        let slates_dir = src.path().to_path_buf();
        let live_dir = live.path().join("main");
        let shared_db = Arc::clone(&db);
        let mut handle = spawn_engine(move || {
            let slates = Slates {
                idle: slate_in(&slates_dir, "idle.ts"),
                buffering: slate_in(&slates_dir, "buffering.ts"),
                pause: slate_in(&slates_dir, "pause.ts"),
                generation: 0,
                japanese_font_missing: false,
                font_missing: None,
            };
            let channel = Channel::new(ChannelOptions::new(
                MAIN_CHANNEL_ID,
                Arc::clone(&shared_db),
                Arc::new(PendingFactory),
                slates,
            ));
            let options = BroadcasterOptions {
                dir: live_dir.clone(),
                window_size: 10,
                segment_seconds: 4.0,
                path_prefix: "main/".to_string(),
                discontinuity_tags: Box::new(|| true),
                retain_s: None,
                log: None,
            };
            (Broadcaster::new(options, tokio_clock()), channel)
        });
        let s = wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        assert_eq!(names(&s), vec!["0.ts", "1.ts", "2.ts"]);
        let status = |db: &Arc<Mutex<rusqlite::Connection>>| {
            let conn = db.lock().unwrap();
            QueueRepo::new(&conn).get(item.id).unwrap().unwrap()
        };
        assert_eq!(status(&db).status, ItemStatus::Playing);

        handle.commands.send(Command::Skip).await.unwrap();
        next_snapshot(&mut handle.snapshot).await;
        let skipped = status(&db);
        assert_eq!(skipped.status, ItemStatus::Played);
        assert_eq!(
            skipped.error_text(Lang::En).as_deref(),
            Some("Skipped by the owner")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn replace_slates_reaches_the_channel_and_a_rebuilt_channel_starts_with_the_newest_set() {
        let env = Env::new();
        // Call 5 of next() panics (around the 8 s mark), long after the command below.
        env.probe.panic_at_next.store(5, Ordering::SeqCst);
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        let slates = Slates {
            generation: 7,
            ..Slates::default()
        };
        handle
            .commands
            .send(Command::ReplaceSlates(slates))
            .await
            .unwrap();
        next_snapshot(&mut handle.snapshot).await;
        assert_eq!(*env.probe.events.lock().unwrap(), ["start", "slates g7"]);

        let probe = Arc::clone(&env.probe);
        wait_until(&mut handle.snapshot, |s| {
            probe.builds.load(Ordering::SeqCst) == 2 && !s.segments.is_empty()
        })
        .await;
        // The rebuilt channel got the set before it started.
        assert_eq!(
            *env.probe.events.lock().unwrap(),
            ["start", "slates g7", "stop", "slates g7", "start"]
        );
    }

    /// Fix round 1: a loop started for a restarted server gets the active reasons at once; its
    /// first channel is paused before it starts (no film aired before the controller's command).
    #[tokio::test(start_paused = true)]
    async fn a_loop_started_with_pause_reasons_starts_its_channel_paused() {
        let env = Env::new();
        let (mut handle, _task) = env.spawn_paused(&[PauseReason::NoViewers]);
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        assert_eq!(
            *env.probe.events.lock().unwrap(),
            ["pause no_viewers", "start"]
        );
        // The loop keeps it like a commanded reason: a Resume clears it.
        handle
            .commands
            .send(Command::Resume(PauseReason::NoViewers))
            .await
            .unwrap();
        next_snapshot(&mut handle.snapshot).await;
        assert_eq!(
            *env.probe.events.lock().unwrap(),
            ["pause no_viewers", "start", "resume no_viewers"]
        );
    }

    /// A channel rebuilt after a crash must not air films while a pause reason is still active:
    /// the loop remembers the reasons and hands them to the new channel before it starts.
    #[tokio::test(start_paused = true)]
    async fn a_rebuilt_channel_starts_with_the_active_pause_reasons() {
        let env = Env::new();
        // Call 5 of next() panics (around the 8 s mark), long after the commands below.
        env.probe.panic_at_next.store(5, Ordering::SeqCst);
        let (mut handle, _task) = env.spawn();
        wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        for command in [
            Command::Pause(PauseReason::Tunnel),
            Command::Pause(PauseReason::NoViewers),
            Command::Resume(PauseReason::NoViewers),
        ] {
            handle.commands.send(command).await.unwrap();
        }
        next_snapshot(&mut handle.snapshot).await;
        let probe = Arc::clone(&env.probe);
        wait_until(&mut handle.snapshot, |s| {
            probe.builds.load(Ordering::SeqCst) == 2 && !s.segments.is_empty()
        })
        .await;
        assert_eq!(
            *env.probe.events.lock().unwrap(),
            [
                "start",
                "pause tunnel",
                "pause no_viewers",
                "resume no_viewers",
                "stop",
                // Only the reason still active, before the start.
                "pause tunnel",
                "start"
            ]
        );
    }

    /// Two segments per kind, so the idle list loops like the real 8 s slates do.
    fn slate_set(dir: &Path, generation: u32) -> Slates {
        let two = |name: &str| -> Vec<SegmentInfo> {
            (0..2)
                .map(|index| {
                    let path = dir.join(format!("g{generation}-{name}-{index}.ts"));
                    std::fs::write(&path, ts_with_pts(126_000)).unwrap();
                    SegmentInfo {
                        path,
                        duration: 4.0,
                        index,
                    }
                })
                .collect()
        };
        Slates {
            idle: two("idle"),
            buffering: two("buffering"),
            pause: two("pause"),
            generation,
            japanese_font_missing: false,
            font_missing: None,
        }
    }

    /// A live swap of the slate set is one discontinuity: the playlist keeps growing without a
    /// gap, and the rewritten timestamps never go backwards across the join.
    #[tokio::test(start_paused = true)]
    async fn a_live_slate_swap_is_a_single_discontinuity_without_a_timestamp_regression() {
        use crate::pipeline::tsrewrite::min_dts;
        let live = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let db = Arc::new(Mutex::new(
            crate::db::open_database(Path::new(":memory:")).unwrap(),
        ));
        let slates_dir = src.path().to_path_buf();
        let live_dir = live.path().join("main");
        let mut handle = spawn_engine(move || {
            let channel = Channel::new(ChannelOptions::new(
                MAIN_CHANNEL_ID,
                Arc::clone(&db),
                Arc::new(PendingFactory),
                slate_set(&slates_dir, 0),
            ));
            let options = BroadcasterOptions {
                dir: live_dir.clone(),
                window_size: 30,
                segment_seconds: 4.0,
                path_prefix: "main/".to_string(),
                discontinuity_tags: Box::new(|| true),
                retain_s: None,
                log: None,
            };
            (Broadcaster::new(options, tokio_clock()), channel)
        });
        let tags = |s: &Snapshot| s.playlist.matches("#EXT-X-DISCONTINUITY\n").count();
        let first = wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        // Let the 2-segment idle slate loop at least once: still no discontinuity.
        let looped = wait_until(&mut handle.snapshot, |s| names(s).len() >= 6).await;
        assert_eq!(tags(&first), 0);
        assert_eq!(tags(&looped), 0, "{}", looped.playlist);

        handle
            .commands
            .send(Command::ReplaceSlates(slate_set(src.path(), 1)))
            .await
            .unwrap();
        let swapped = wait_until(&mut handle.snapshot, |s| tags(s) == 1).await;
        // It goes on from there, still one discontinuity.
        let later = wait_until(&mut handle.snapshot, |s| {
            names(s).len() >= names(&swapped).len() + 3
        })
        .await;
        assert_eq!(tags(&later), 1, "{}", later.playlist);
        let sequence = |s: &Snapshot| {
            s.playlist
                .lines()
                .find_map(|l| l.strip_prefix("#EXT-X-MEDIA-SEQUENCE:"))
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap()
        };
        assert!(sequence(&later) >= sequence(&swapped), "sequence went back");

        let dts: Vec<u64> = names(&later)
            .iter()
            .map(|n| min_dts(&std::fs::read(later.live_dir.join(n)).unwrap()).unwrap())
            .collect();
        assert!(
            dts.windows(2).all(|w| w[0] <= w[1]) && dts.first() < dts.last(),
            "timestamps went backwards (the one-packet test segments of a source share a PTS): {dts:?}"
        );
    }
}
