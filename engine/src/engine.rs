//! Channel engine loop and its supervisor (replaces the Node `setInterval(250 ms)` that drove
//! `Broadcaster.tick()` with the Channel as its source).
//!
//! One task owns the `Broadcaster` and the `Channel`. Every 250 ms it drains the commands sent by
//! HTTP, ticks the broadcaster and publishes a fresh [`Snapshot`]. HTTP never calls the Channel
//! directly: it sends [`Command`]s and reads the latest snapshot.

use std::any::Any;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::pipeline::broadcaster::{Broadcaster, SegmentSource};
use crate::pipeline::channel::{Channel, ChannelState, ChannelStatus};

/// Requests from HTTP, applied by the loop between ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Skip,
    QueueChanged,
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
    fn state(&self) -> ChannelState;
    fn prefetched_item_id(&self) -> Option<i64>;
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
    fn state(&self) -> ChannelState {
        Channel::state(self)
    }
    fn prefetched_item_id(&self) -> Option<i64> {
        Channel::prefetched_item_id(self)
    }
}

const TICK: Duration = Duration::from_millis(250);
const RESTART_DELAY: Duration = Duration::from_secs(1);
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
}

/// Starts the engine (tick 250 ms, supervised) and logs to stdout. Must be called inside a tokio
/// runtime. The loop runs until every [`EngineHandle`] is dropped.
pub fn spawn_engine(build: impl Fn() -> (Broadcaster, Channel) + Send + 'static) -> EngineHandle {
    spawn_engine_with(build, |msg: &str| println!("{msg}")).0
}

/// Like [`spawn_engine`], with any channel, an injected log and a handle for explicit shutdown.
/// Dropping the [`EngineTask`] does not stop the engine.
pub fn spawn_engine_with<C, F, L>(build: F, log: L) -> (EngineHandle, EngineTask)
where
    C: ChannelControl + 'static,
    F: Fn() -> (Broadcaster, C) + Send + 'static,
    L: Fn(&str) + Send + Sync + 'static,
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
            },
            prefetched_item_id: None,
        }
    }
}

/// How one run of the loop ended.
enum Exit {
    Shutdown,
    Failed(String),
}

/// Owns everything that must outlive a crashed run: the command receiver, the snapshot sender and
/// the factory of fresh (Broadcaster, Channel) pairs.
struct Supervisor<F, L> {
    build: F,
    commands: mpsc::Receiver<Command>,
    snapshot: watch::Sender<Arc<Snapshot>>,
    stop: watch::Receiver<bool>,
    log: L,
}

impl<C, F, L> Supervisor<F, L>
where
    C: ChannelControl,
    F: Fn() -> (Broadcaster, C),
    L: Fn(&str) + Sync,
{
    async fn run(mut self) {
        loop {
            let failure = match guarded(&self.build) {
                Err(panic) => panic,
                Ok((broadcaster, channel)) => {
                    let mut core = Core {
                        broadcaster,
                        channel,
                    };
                    let exit = self.run_core(&mut core).await;
                    // Also after a panic: dispose the crashed channel's clips (ffmpeg processes)
                    // instead of leaving them to a plain drop.
                    if let Err(panic) = guarded(|| core.channel.stop()) {
                        (self.log)(&format!("Błąd przy zatrzymywaniu kanału: {panic}"));
                    }
                    match exit {
                        Exit::Shutdown => return,
                        Exit::Failed(msg) => msg,
                    }
                }
            };
            (self.log)(&format!(
                "Pętla kanału zakończyła się błędem: {failure} — uruchamiam ponownie"
            ));
            tokio::select! {
                () = tokio::time::sleep(RESTART_DELAY) => {}
                () = stop_requested(&mut self.stop) => return,
            }
            if self.commands.is_closed() {
                return;
            }
        }
    }

    /// Starts the pair and ticks it every 250 ms until shutdown or a failure.
    async fn run_core(&mut self, core: &mut Core<C>) -> Exit {
        match guarded(|| core.start()) {
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
                match guarded(|| core.apply(command)) {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => (self.log)(&format!("Polecenie kanału nie powiodło się: {e}")),
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
        }
    }

    fn tick(&mut self) {
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
    use crate::pipeline::broadcaster::{BroadcasterOptions, PublishedSegment, SourceSegment};
    use crate::pipeline::channel::{BoxFuture, ChannelOptions, ClipFactory, ClipHandle};
    use crate::pipeline::segmenter::SegmentInfo;
    use crate::pipeline::slates::Slates;
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
        panic_build_once: AtomicBool,
        fail_start_once: AtomicBool,
        fail_skip: AtomicBool,
        /// `next()` call number (1-based, counted across restarts) that panics once; 0 = never.
        panic_at_next: AtomicUsize,
    }

    struct FakeChannel {
        probe: Arc<Probe>,
        src_dir: PathBuf,
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
            self.probe.starts.fetch_add(1, Ordering::SeqCst);
            if self.probe.fail_start_once.swap(false, Ordering::SeqCst) {
                anyhow::bail!("baza zablokowana");
            }
            Ok(())
        }
        fn stop(&mut self) {
            self.probe.stops.fetch_add(1, Ordering::SeqCst);
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
        fn state(&self) -> ChannelState {
            ChannelState {
                status: ChannelStatus::Buffering,
                current: None,
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
                };
                (Broadcaster::new(options, tokio_clock()), channel)
            };
            let logs = Arc::clone(&self.logs);
            spawn_engine_with(build, move |m: &str| {
                logs.lock().unwrap().push(m.to_string())
            })
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
                .contains(&"Polecenie kanału nie powiodło się: nie można pominąć".to_string()),
            "{:?}",
            env.logs()
        );
        wait_until(&mut handle.snapshot, |s| names(s).contains(&"3.ts".into())).await;
        assert_eq!(env.count(|p| &p.builds), 1);
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
            vec!["Pętla kanału zakończyła się błędem: atrapa źródła: awaria — uruchamiam ponownie"]
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
    async fn a_build_panic_or_failed_start_is_logged_and_retried_after_a_second() {
        let env = Env::new();
        env.probe.panic_build_once.store(true, Ordering::SeqCst);
        env.probe.fail_start_once.store(true, Ordering::SeqCst);
        let t0 = Instant::now();
        let (mut handle, _task) = env.spawn();
        let s = wait_until(&mut handle.snapshot, |s| !s.segments.is_empty()).await;
        assert_eq!(t0.elapsed(), Duration::from_secs(2));
        assert_eq!(names(&s), vec!["0.ts", "1.ts", "2.ts"]);
        assert_eq!(
            env.logs(),
            vec![
                "Pętla kanału zakończyła się błędem: atrapa budowy: awaria — uruchamiam ponownie",
                "Pętla kanału zakończyła się błędem: baza zablokowana — uruchamiam ponownie",
            ]
        );
        assert_eq!(env.count(|p| &p.builds), 3);
        assert_eq!(env.count(|p| &p.starts), 2);
        // The channel whose start failed was stopped too.
        assert_eq!(env.count(|p| &p.stops), 1);
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
        ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, String>> {
            Box::pin(std::future::pending())
        }
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
            skipped.error.as_deref(),
            Some("Pominięty przez właściciela")
        );
    }
}
