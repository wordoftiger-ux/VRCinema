//! Channel orchestration: decides what airs and feeds the broadcaster. Port of
//! `src/pipeline/channel.ts`.
//!
//! Rule: `next()` always returns something — a clip segment or a slate — so the channel never
//! goes silent.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Connection;
use tokio::sync::mpsc;

use crate::db::{ItemStatus, NewItem, QueueItem, QueueRepo};
use crate::i18n::{Msg, error_msg, msg_error};
use crate::pipeline::broadcaster::{LogFn, PublishedSegment, SegmentSource, SourceSegment};
use crate::pipeline::monitor::{
    LOOKAHEAD, MonitorFeed, MonitorOptions, MonitorTimeline, MonitorWatch,
};
use crate::pipeline::segmenter::{SegmentInfo, SegmenterState};
use crate::pipeline::slates::{SlateKind, Slates, request_url};
use crate::stats::{ChannelStats, ClipStats, FfmpegInfo, NextInfo, NextState, OnAirInfo};

/// A boxed, sendable future (like `futures::future::BoxFuture`).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// Clock in milliseconds.
pub type ClockFn = Box<dyn Fn() -> u64 + Send>;
/// Removes one aired segment file.
pub type RemoveFileFn = Box<dyn Fn(&Path) + Send>;

/// A running cut of one film (real: `ClipSegmenter`; in tests: a fake).
pub trait ClipHandle: Send {
    fn state(&self) -> SegmenterState;
    fn error(&self) -> Option<Msg>;
    /// Source duration from ffprobe (may fill in a duration missing from the queue).
    fn duration_s(&self) -> Option<f64>;
    fn poll(&mut self) -> Vec<SegmentInfo>;
    /// The video is copied, not converted: a start midway lands on the keyframe before it.
    fn copies_video(&self) -> bool {
        false
    }
    fn kill(&mut self);
    /// The conversion the info ticker needs has been too slow for 30 s.
    fn ticker_slow(&self) -> bool {
        false
    }
    /// The request address whose QR code this clip's ticker draws (`Some("")`: a ticker
    /// without a code); `None`: no ticker, or not known.
    fn ticker_code(&self) -> Option<String> {
        None
    }
    /// What the stats show about this clip (analysis, mode, process).
    fn stats(&self) -> ClipStats {
        ClipStats::default()
    }
    /// Kills the process and removes the clip files.
    fn dispose(&mut self);
}

pub trait ClipFactory: Send + Sync {
    /// Resolves the source, analyses it and starts cutting from `start_at_s`. Fails with a
    /// user-facing message.
    fn create(
        &self,
        item: QueueItem,
        start_at_s: f64,
        attempt: u32,
    ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>>;

    /// Like [`Self::create`] from `start_at_s` (the film's in point), for the next film
    /// prepared ahead of its turn (the current one still airs for up to a minute).
    fn create_ahead(
        &self,
        item: QueueItem,
        start_at_s: f64,
    ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
        self.create(item, start_at_s, 0)
    }

    /// Resolves and analyses `item` ahead of its turn (the film on air still plays for a
    /// while), so its start later skips that step. Default: nothing to do.
    fn prepare(&self, item: QueueItem) -> BoxFuture<'static, Result<(), Msg>> {
        let _ = item;
        Box::pin(async { Ok(()) })
    }

    /// A countdown of `seconds` before `item` (which starts at a set time): "Starting in 2:30"
    /// and its title, read at real time. `Err`: no countdown (the idle slate airs instead).
    fn create_countdown(
        &self,
        item: QueueItem,
        seconds: f64,
    ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
        let _ = (item, seconds);
        Box::pin(async { Err(Msg::new(COUNTDOWN_UNAVAILABLE)) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    Idle,
    Starting,
    Playing,
    Buffering,
    /// Viewers see the pause slate. The queue item stays `playing` in the database.
    Paused,
    /// A countdown airs before a film that starts at a set time.
    Countdown,
}

impl ChannelStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Playing => "playing",
            Self::Buffering => "buffering",
            Self::Paused => "paused",
            Self::Countdown => "countdown",
        }
    }
}

/// Why the channel is paused. The channel airs films again once no reason is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PauseReason {
    /// The tunnel is being set up or restarted.
    Tunnel,
    /// Nobody is watching.
    NoViewers,
    /// The owner paused the channel.
    Manual,
}

impl PauseReason {
    /// The reason as a word for a person (a log message field).
    pub fn msg(self) -> Msg {
        Msg::new(match self {
            Self::Tunnel => "log.pause_reason_tunnel",
            Self::NoViewers => "log.pause_reason_no_viewers",
            Self::Manual => "log.pause_reason_manual",
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tunnel => "tunnel",
            Self::NoViewers => "no_viewers",
            Self::Manual => "manual",
        }
    }
}

/// The film viewers currently see.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub item_id: i64,
    pub title: String,
    pub duration_s: Option<f64>,
    pub position_s: i64,
    pub requester_name: Option<String>,
    /// Who added it: the owner shows as "host", a viewer with a device key by an icon.
    pub added_by: crate::db::AddedBy,
    pub requester_key_hash: Option<String>,
    pub is_live: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelState {
    /// What viewers see (lags `viewer_lag_segments` behind). `Paused` shows the paused film in
    /// `current` with the position it will resume from.
    pub status: ChannelStatus,
    pub current: Option<NowPlaying>,
    /// A pause reason is active and the channel is holding films back right now (not lagged).
    /// False while a live stream keeps airing through a pause request.
    pub paused: bool,
    /// Active pause reasons, in a stable order.
    pub pause_reasons: Vec<PauseReason>,
    /// What the owner's controls act on right now.
    pub control: ControlState,
}

/// The film the owner's controls act on: the one being aired (or started) or paused right
/// now. Unlike [`ChannelState::current`] it does not lag behind the viewers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControlState {
    pub item_id: Option<i64>,
    pub is_live: bool,
    /// Server position in the film (where the channel is cutting it, or resumes it from;
    /// viewers are a few segments behind). While a seek waits, its target; after a seek during
    /// a pending pause, where the film will resume.
    pub position_s: f64,
    pub duration_s: Option<f64>,
    pub can_pause: bool,
    pub can_seek: bool,
    /// A seek waits for 700 ms of quiet before the film is started again from its target.
    pub seek_pending: bool,
    /// The video is copied: a seek lands on the keyframe before its target.
    pub position_approx: bool,
    /// The owner paused at a position the channel has not aired yet ([`Channel::pause_at`]):
    /// the film keeps airing up to the end of the segment holding it, then pauses there.
    pub pause_pending: bool,
    /// The film's in and out points (film seconds), when set.
    pub in_s: Option<f64>,
    pub out_s: Option<f64>,
}

pub struct ChannelOptions {
    pub channel_id: i64,
    pub db: Arc<Mutex<Connection>>,
    pub factory: Arc<dyn ClipFactory>,
    pub slates: Slates,
    /// Clock in milliseconds (default: monotonic time since construction).
    pub now: Option<ClockFn>,
    pub log: Option<LogFn>,
    /// Removal of aired clip segment files (injected in tests).
    pub remove_file: Option<RemoveFileFn>,
    pub stall_timeout_s: Option<u64>,
    pub start_timeout_s: Option<u64>,
    pub prefetch_s: Option<f64>,
    /// How many segments viewers lag behind the newest published one (for "now playing").
    pub viewer_lag_segments: Option<usize>,
    /// The producer's monitor ([`crate::pipeline::monitor`]): its links' folder and who watches
    /// it (the program's window has one); `None`: no monitor, nothing listed.
    pub monitor: Option<MonitorOptions>,
    /// Wall clock in Unix milliseconds (start times of films); default: the system clock.
    pub wall_now: Option<ClockFn>,
}

impl ChannelOptions {
    /// Options with every optional field at its default.
    pub fn new(
        channel_id: i64,
        db: Arc<Mutex<Connection>>,
        factory: Arc<dyn ClipFactory>,
        slates: Slates,
    ) -> Self {
        Self {
            channel_id,
            db,
            factory,
            slates,
            now: None,
            log: None,
            remove_file: None,
            stall_timeout_s: None,
            start_timeout_s: None,
            prefetch_s: None,
            viewer_lag_segments: None,
            monitor: None,
            wall_now: None,
        }
    }
}

const DEFAULT_STALL_TIMEOUT_S: u64 = 30;
const DEFAULT_START_TIMEOUT_S: u64 = 120;
const DEFAULT_PREFETCH_S: f64 = 60.0;
const DEFAULT_VIEWER_LAG_SEGMENTS: usize = 3;
/// How many aired segments are remembered for `state()`.
const AIRED_KEEP: usize = 20;
/// Upper bound on state transitions within one `next()` call.
const NEXT_GUARD: usize = 50;
/// A seek runs after this much quiet; every new seek command moves the deadline (and target).
const SEEK_QUIET_MS: u64 = 700;
/// A seek target this close to the end ends the film (like a skip).
const SEEK_END_MARGIN_S: f64 = 1.0;
/// A film whose position is this close to its out point has reached it (segment lengths are
/// not exact to the millisecond).
const OUT_POINT_SLACK_S: f64 = 0.05;
/// A film with a start time starts when it is this close (the countdown's last segment may
/// end a little early).
const START_SLACK_MS: i64 = 1000;
/// A start time closer than this gets no countdown: the film just waits.
const MIN_COUNTDOWN_S: f64 = 3.0;
const COUNTDOWN_UNAVAILABLE: &str = "log.countdown_unavailable";

// Why a film ended without playing to the end (kept with it in the history as a message, shown
// in the reader's language).
const STALL_ERROR: &str = "item_errors.stalled";
const START_TIMEOUT_ERROR: &str = "item_errors.start_timeout";
const SKIPPED: &str = "item_errors.skipped";
const FFMPEG_STOPPED: &str = "item_errors.ffmpeg_stopped";
const UNKNOWN_ERROR: &str = "item_errors.unknown";

/// A film being started or aired.
struct ActiveClip {
    /// Matches the clip with the result of its asynchronous start.
    id: u64,
    item: QueueItem,
    attempt: u32,
    /// How many times a resume or a seek relaunched this film; keeps source keys unique per
    /// encoder process.
    relaunches: u32,
    /// Film second the current process started from.
    base_s: f64,
    handle: Option<Box<dyn ClipHandle>>,
    start_error: Option<Msg>,
    started_at: u64,
    ready: VecDeque<SegmentInfo>,
    /// Seconds aired from the current process.
    emitted_s: f64,
    stalled_since: Option<u64>,
    /// What an earlier process of this film found out (kept while the new one starts).
    earlier: Probed,
    /// The background start, so a seek can cancel it.
    start: Option<tokio::task::AbortHandle>,
    /// The "too slow for the ticker" warning was logged while this film was on air.
    slow_logged: bool,
    /// Rows of the monitor for the first segments of `ready` (listed, not aired yet).
    monitor_listed: VecDeque<u64>,
    /// The retry of a film that failed while on air: viewers see the fallback slate until it
    /// produces again.
    recovering: bool,
    /// The out point its process was started with (ffmpeg stops there).
    started_out_s: Option<f64>,
}

/// What the clip's analysis found out about a film.
#[derive(Debug, Clone, Copy, Default)]
struct Probed {
    duration_s: Option<f64>,
    copies_video: Option<bool>,
}

impl ActiveClip {
    fn position_s(&self) -> f64 {
        self.base_s + self.emitted_s
    }

    /// Unique per encoder process: a retry (attempt) and every relaunch after a pause or a seek
    /// change it, so the broadcaster marks a discontinuity even when nothing else aired in
    /// between.
    fn source_key(&self) -> String {
        match self.relaunches {
            0 => format!("clip-{}-{}", self.item.id, self.attempt),
            n => format!("clip-{}-{}-r{n}", self.item.id, self.attempt),
        }
    }

    /// `None` for live streams and unknown durations.
    fn duration_s(&self) -> Option<f64> {
        if self.item.is_live {
            return None;
        }
        self.item
            .duration_s
            .or_else(|| self.handle.as_ref().and_then(|h| h.duration_s()))
            .or(self.earlier.duration_s)
    }

    fn probed(&self) -> Probed {
        Probed {
            duration_s: self.duration_s(),
            copies_video: self
                .handle
                .as_ref()
                .map(|h| h.copies_video())
                .or(self.earlier.copies_video),
        }
    }

    /// Where the film ends: its out point, else its length (`None`: live or unknown).
    fn end_s(&self) -> Option<f64> {
        self.item.end_s(self.duration_s())
    }

    /// Seconds left until the end of the film (its out point); `None` for live streams and
    /// unknown durations.
    fn remaining_s(&self) -> Option<f64> {
        Some(self.end_s()? - self.position_s())
    }

    /// The film has aired up to its out point (a point set or moved while it plays; a process
    /// started with it stops there by itself).
    fn past_out_point(&self) -> bool {
        !self.item.is_live
            && self
                .item
                .out_s
                .is_some_and(|out| self.position_s() >= out - OUT_POINT_SLACK_S)
    }

    /// Accounts one aired segment of this clip.
    fn emit(&mut self, seg: &SegmentInfo) -> (SourceSegment, SegmentMeta) {
        let position_s = self.position_s();
        self.emitted_s += seg.duration;
        self.stalled_since = None;
        self.recovering = false;
        let out = SourceSegment {
            path: seg.path.clone(),
            duration: seg.duration,
            source_key: self.source_key(),
            index: seg.index as u64,
        };
        let meta = SegmentMeta {
            kind: MetaKind::Clip,
            item: Some(self.item.clone()),
            position_s,
            monitor_seq: None,
        };
        (out, meta)
    }

    fn dispose(&mut self) {
        if let Some(handle) = self.handle.as_mut() {
            handle.dispose();
        }
    }

    /// Its start is still in flight (neither a clip nor an error yet).
    fn is_starting(&self) -> bool {
        self.handle.is_none() && self.start_error.is_none()
    }

    /// The start or the process failed (and `next()` has not dealt with it yet).
    fn failure(&self) -> Option<Msg> {
        if let Some(error) = &self.start_error {
            return Some(error.clone());
        }
        let handle = self.handle.as_ref()?;
        matches!(
            handle.state(),
            SegmenterState::Failed | SegmenterState::Killed
        )
        .then(|| handle.error().unwrap_or_else(|| Msg::new(FFMPEG_STOPPED)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetaKind {
    Clip,
    Slate(SlateKind),
    /// The countdown before a film with a start time.
    Countdown,
}

#[derive(Debug, Clone)]
struct SegmentMeta {
    kind: MetaKind,
    item: Option<QueueItem>,
    position_s: f64,
    /// The monitor's row of this clip segment.
    monitor_seq: Option<u64>,
}

/// Result of an asynchronous clip start.
struct Launch {
    clip_id: u64,
    result: Result<Box<dyn ClipHandle>, Msg>,
}

/// A non-live film taken off the air (its clip disposed): by a pause until the resume, or by a
/// seek for the moment of its relaunch. It goes on from `position_s`.
struct Parked {
    item: QueueItem,
    position_s: f64,
    attempt: u32,
    relaunches: u32,
    probed: Probed,
}

impl Parked {
    /// Takes the clip off the air (disposes it) at its current position.
    fn off_air(mut c: ActiveClip) -> Self {
        c.dispose();
        Self {
            position_s: c.position_s(),
            attempt: c.attempt,
            relaunches: c.relaunches,
            probed: c.probed(),
            item: c.item,
        }
    }
}

/// A seek waiting for quiet (see [`SEEK_QUIET_MS`]).
struct PendingSeek {
    item_id: i64,
    target_s: f64,
    due_at: u64,
    /// A new ticker look asked for it: no target of its own, the film starts again from where
    /// the channel is when it runs (and the log says why).
    ticker: bool,
    /// A restart for a later out point ([`Channel::retrim_current`], already logged): the
    /// ticker's line is not written.
    quiet: bool,
}

/// A manual pause waiting for the film to reach `at_s` (see [`Channel::pause_at`]).
struct ArmedPause {
    item_id: i64,
    at_s: f64,
    /// The owner sought meanwhile: the film resumes from here (not from the pause boundary).
    resume_at: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
enum SeekRequest {
    To(f64),
    By(f64),
}

impl SeekRequest {
    /// The target from `from_s`, never before the film's start (its in point); `None` when the
    /// request is not finite or the film's end is unknown and the request is not a forward
    /// relative seek (or a seek back to the start).
    fn target(self, from_s: f64, start_s: f64, end_s: Option<f64>) -> Option<f64> {
        let target = match self {
            Self::To(to) => to,
            Self::By(delta) => from_s + delta,
        };
        if !target.is_finite() {
            return None;
        }
        let allowed = end_s.is_some()
            || match self {
                Self::To(to) => to <= start_s,
                Self::By(delta) => delta >= 0.0,
            };
        allowed.then_some(target.max(start_s))
    }
}

/// The target ends the film (it is in its last second).
fn ends_film(target_s: f64, duration_s: Option<f64>) -> bool {
    duration_s.is_some_and(|d| target_s >= d - SEEK_END_MARGIN_S)
}

fn slate_slot(kind: SlateKind) -> usize {
    match kind {
        SlateKind::Idle => 0,
        SlateKind::Buffering => 1,
        SlateKind::Pause => 2,
        SlateKind::Fallback => 3,
    }
}

/// JavaScript `Math.round` for non-negative positions.
fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

fn default_clock() -> ClockFn {
    let t0 = std::time::Instant::now();
    Box::new(move || u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX))
}

/// Channel orchestration: picks what airs and hands the broadcaster its next segments.
pub struct Channel {
    o: ChannelOptions,
    now: ClockFn,
    current: Option<ActiveClip>,
    prefetched: Option<ActiveClip>,
    /// The film a pause took off the air (its clip is disposed; the queue item stays playing).
    parked: Option<Parked>,
    pause_reasons: BTreeSet<PauseReason>,
    pending_seek: Option<PendingSeek>,
    /// The owner's pause at a position not aired yet; the film airs up to it first.
    armed_pause: Option<ArmedPause>,
    slate_index: [usize; 4],
    last_slate_kind: Option<SlateKind>,
    previous_clip_segment: Option<PathBuf>,
    pending_meta: HashMap<PathBuf, SegmentMeta>,
    aired: VecDeque<SegmentMeta>,
    /// Every published segment of the last minutes (the air clock, [`crate::pipeline::air`]).
    air_log: crate::pipeline::air::AirLog,
    next_clip_id: u64,
    launch_tx: mpsc::UnboundedSender<Launch>,
    launch_rx: mpsc::UnboundedReceiver<Launch>,
    /// The producer's monitor (with [`ChannelOptions::monitor`]).
    monitor: Option<MonitorTimeline>,
    /// Whether the window watches the monitor now (it is then fed every tick).
    monitor_watch: Option<Arc<MonitorWatch>>,
    /// The monitor's newest row handed to the broadcaster.
    monitor_aired: Option<u64>,
    wall_now: ClockFn,
    /// The countdown airing (or starting) before the queue head's start time.
    countdown: Option<Countdown>,
    /// Countdowns made so far (their source keys differ).
    countdowns: u64,
    /// The queue head resolved ahead of its turn ([`ClipFactory::prepare`]).
    preparing: Option<Preparing>,
    prepare_tx: mpsc::UnboundedSender<(i64, Result<(), Msg>)>,
    prepare_rx: mpsc::UnboundedReceiver<(i64, Result<(), Msg>)>,
    /// The film whose prepared clip failed and was started once more.
    prefetch_retried: Option<i64>,
}

/// The film resolved ahead of its turn, and how far it got.
struct Preparing {
    item_id: i64,
    source_url: String,
    title: String,
    state: NextState,
}

/// The film on air has aired this long before the next one is resolved ahead.
const PREPARE_AFTER_S: f64 = 10.0;
/// A prepared clip that failed is started once more after this long.
const PREFETCH_RETRY_MS: u64 = 10_000;
/// Only a head starting within this time is resolved ahead (the address may expire).
const PREPARE_AHEAD_MAX_MS: i64 = 15 * 60 * 1000;

/// The countdown before a film with a start time (its clip's `item` is that film).
struct Countdown {
    clip: ActiveClip,
    start_at: i64,
    n: u64,
    /// Its failure was logged.
    failure_logged: bool,
}

fn system_wall_clock() -> ClockFn {
    Box::new(|| u64::try_from(crate::db::now_ms()).unwrap_or_default())
}

impl Channel {
    pub fn new(mut o: ChannelOptions) -> Self {
        let now = o.now.take().unwrap_or_else(default_clock);
        let wall_now = o.wall_now.take().unwrap_or_else(system_wall_clock);
        let (prepare_tx, prepare_rx) = mpsc::unbounded_channel();
        let (launch_tx, launch_rx) = mpsc::unbounded_channel();
        let monitor = o
            .monitor
            .as_ref()
            .map(|m| MonitorTimeline::new(&m.dir, m.first_seq));
        let monitor_watch = o.monitor.as_ref().map(|m| Arc::clone(&m.watch));
        Self {
            monitor,
            monitor_watch,
            monitor_aired: None,
            o,
            now,
            current: None,
            prefetched: None,
            parked: None,
            pause_reasons: BTreeSet::new(),
            pending_seek: None,
            armed_pause: None,
            slate_index: [0; 4],
            last_slate_kind: None,
            previous_clip_segment: None,
            pending_meta: HashMap::new(),
            aired: VecDeque::new(),
            air_log: Default::default(),
            next_clip_id: 0,
            launch_tx,
            launch_rx,
            wall_now,
            countdown: None,
            countdowns: 0,
            preparing: None,
            prepare_tx,
            prepare_rx,
            prefetch_retried: None,
        }
    }

    /// A film left "playing" by a restart goes back to the front of the queue.
    pub fn start(&mut self) -> anyhow::Result<()> {
        let channel_id = self.o.channel_id;
        self.queue(|q| q.reset_playing(channel_id))
    }

    pub fn stop(&mut self) {
        self.parked = None;
        self.pending_seek = None;
        self.armed_pause = None;
        self.drop_countdown();
        for mut clip in [self.current.take(), self.prefetched.take()]
            .into_iter()
            .flatten()
        {
            clip.dispose();
        }
    }

    pub fn skip(&mut self) -> anyhow::Result<()> {
        // A pause waiting for this film's position goes with it.
        self.armed_pause = None;
        let skipped = Msg::new(SKIPPED);
        if let Some(p) = self.parked.take() {
            // Paused film: its clip is already gone. The channel stays paused for the next one.
            return self
                .queue(|q| q.finish(p.item.id, ItemStatus::Played, Some(&skipped.to_stored())))
                .map(|_| ());
        }
        self.finish_current(ItemStatus::Played, Some(&skipped))
    }

    /// Airs `slates` from now on (a new address was drawn on them). Films are not touched; the
    /// next slate segment comes from the new set, from its first segment, under a new source key.
    pub fn replace_slates(&mut self, slates: Slates) {
        self.o.slates = slates;
        self.slate_index = [0; 4];
        self.last_slate_kind = None;
    }

    /// Called after queue changes (removal, reorder, approval).
    pub fn on_queue_changed(&mut self) -> anyhow::Result<()> {
        if let Some(id) = self.current.as_ref().map(|c| c.item.id) {
            match self
                .queue(|q| q.get(id))?
                .filter(|f| f.status == ItemStatus::Playing)
            {
                None => self.drop_current(),
                Some(fresh) => self.retrim_current(&fresh),
            }
        }
        if let Some(id) = self.parked.as_ref().map(|p| p.item.id) {
            match self
                .queue(|q| q.get(id))?
                .filter(|f| f.status == ItemStatus::Playing)
            {
                None => self.parked = None,
                Some(fresh) => {
                    if let Some(p) = self.parked.as_mut() {
                        p.item.in_s = fresh.in_s;
                        p.item.out_s = fresh.out_s;
                    }
                }
            }
        }
        if let Some(prepared) = self.prefetched.as_ref().map(|c| c.item.clone()) {
            let channel_id = self.o.channel_id;
            let head = self.queue(|q| q.next_queued(channel_id))?;
            // Another head, or new in/out points or start time: prepared again.
            if head.is_none_or(|h| {
                h.id != prepared.id
                    || h.in_s != prepared.in_s
                    || h.out_s != prepared.out_s
                    || h.start_at != prepared.start_at
            }) {
                self.drop_prefetched();
            }
        }
        Ok(())
    }

    /// New in/out points of the film on air: a new out point counts at once (the channel ends
    /// the film there); a later one than its process was started with (or none) starts the
    /// film again from where it is, so ffmpeg does not stop at the old one. A new in point
    /// counts from the film's next start.
    fn retrim_current(&mut self, fresh: &QueueItem) {
        let Some(c) = self.current.as_mut() else {
            return;
        };
        if c.item.is_live || (c.item.in_s == fresh.in_s && c.item.out_s == fresh.out_s) {
            return;
        }
        c.item.in_s = fresh.in_s;
        c.item.out_s = fresh.out_s;
        let extended = c
            .started_out_s
            .is_some_and(|old| fresh.out_s.is_none_or(|new| new > old + OUT_POINT_SLACK_S));
        if extended && !c.is_starting() {
            let (id, title) = (c.item.id, c.item.title.clone());
            self.log(
                Msg::new("log.out_point_moved")
                    .arg("id", id)
                    .arg("title", title),
            );
            self.request(SeekRequest::By(0.0), true);
            if let Some(seek) = self.pending_seek.as_mut().filter(|s| s.ticker) {
                seek.quiet = true;
            }
        }
    }

    /// What viewers see, `viewer_lag_segments` behind the newest published segment.
    pub fn state(&self) -> ChannelState {
        let lag = self
            .o
            .viewer_lag_segments
            .unwrap_or(DEFAULT_VIEWER_LAG_SEGMENTS);
        let (status, current) = match self.aired.get(self.aired.len().saturating_sub(lag)) {
            None => (ChannelStatus::Idle, None),
            Some(meta) => {
                let now_playing = meta.item.as_ref().map(|item| NowPlaying {
                    item_id: item.id,
                    title: item.title.clone(),
                    duration_s: item.duration_s,
                    position_s: js_round(meta.position_s),
                    requester_name: item.requester_name.clone(),
                    added_by: item.added_by,
                    requester_key_hash: item.requester_key_hash.clone(),
                    is_live: item.is_live,
                });
                match (meta.kind, now_playing) {
                    (MetaKind::Slate(SlateKind::Idle), _) => (ChannelStatus::Idle, None),
                    (MetaKind::Countdown, _) => (ChannelStatus::Countdown, None),
                    (MetaKind::Slate(SlateKind::Pause), current) => {
                        (ChannelStatus::Paused, current)
                    }
                    (_, None) => (ChannelStatus::Buffering, None),
                    (MetaKind::Clip, current) => (ChannelStatus::Playing, current),
                    (MetaKind::Slate(SlateKind::Buffering | SlateKind::Fallback), current) => {
                        (ChannelStatus::Buffering, current)
                    }
                }
            }
        };
        ChannelState {
            status,
            current,
            paused: self.is_holding_back(),
            pause_reasons: self.pause_reasons.iter().copied().collect(),
            control: self.control_state(),
        }
    }

    fn control_state(&self) -> ControlState {
        if let Some(p) = &self.parked {
            return ControlState {
                item_id: Some(p.item.id),
                is_live: false,
                position_s: p.position_s,
                duration_s: p.probed.duration_s,
                can_pause: true,
                can_seek: true,
                seek_pending: false,
                position_approx: p.probed.copies_video.unwrap_or(false),
                pause_pending: false,
                in_s: p.item.in_s,
                out_s: p.item.out_s,
            };
        }
        let Some(c) = &self.current else {
            return ControlState::default();
        };
        let live = c.item.is_live;
        let probed = c.probed();
        let pending = self.pending_seek_of(c.item.id);
        let armed = self.armed_pause.as_ref().filter(|a| a.item_id == c.item.id);
        // A seek during a pending pause: where the film resumes, as for a parked film.
        let position_s = match pending.or(armed.and_then(|a| a.resume_at)) {
            Some(target) => c.end_s().map_or(target, |d| target.min(d)),
            None => c.position_s(),
        };
        ControlState {
            item_id: Some(c.item.id),
            is_live: live,
            position_s,
            duration_s: probed.duration_s,
            can_pause: !live,
            can_seek: !live,
            seek_pending: pending.is_some(),
            position_approx: !live && probed.copies_video.unwrap_or(false),
            pause_pending: armed.is_some(),
            in_s: c.item.in_s.filter(|_| !live),
            out_s: c.item.out_s.filter(|_| !live),
        }
    }

    /// Target of the seek waiting for film `item_id`.
    fn pending_seek_of(&self, item_id: i64) -> Option<f64> {
        self.pending_seek
            .as_ref()
            .filter(|s| s.item_id == item_id && !s.ticker)
            .map(|s| s.target_s)
    }

    /// Seeks to `to_s` seconds of the current film (see [`Self::seek_by`]).
    pub fn seek_to(&mut self, to_s: f64) {
        self.request_seek(SeekRequest::To(to_s));
    }

    /// Seeks `delta_s` seconds from the server position, or from the target of a seek still
    /// waiting. A paused film only gets a new remembered position. Otherwise the seek runs after
    /// 700 ms of quiet: the clip stops and the film starts again from the target (a new source
    /// key, so a discontinuity; the buffering slate until its first segment). A target in the
    /// last second ends the film like a skip. Live streams are not seeked; a film of unknown
    /// length only forward (or back to the start).
    pub fn seek_by(&mut self, delta_s: f64) {
        self.request_seek(SeekRequest::By(delta_s));
    }

    fn request_seek(&mut self, request: SeekRequest) {
        self.request(request, false);
    }

    fn request(&mut self, request: SeekRequest, ticker: bool) {
        // The owner's seek (also a restart) while a pause waits for its position only moves where
        // the film resumes, as a seek during a pause does: the film airs on up to the pause
        // boundary (viewers see what the Preview showed), then pauses there. A ticker restart
        // keeps the pause as it is (the film's positions stay the same).
        if !ticker && self.armed_pause.is_some() {
            let film = self
                .current
                .as_ref()
                .filter(|c| !c.item.is_live)
                .map(|c| (c.item.id, c.item.start_s(), c.end_s()));
            if let (Some(armed), Some((id, start_s, end_s))) = (self.armed_pause.as_mut(), film)
                && armed.item_id == id
            {
                let from = armed.resume_at.unwrap_or(armed.at_s);
                match request.target(from, start_s, end_s) {
                    Some(target) => armed.resume_at = Some(target),
                    None => self.log(
                        Msg::new("log.seek_not_allowed").arg("request", format!("{request:?}")),
                    ),
                }
                return;
            }
            self.armed_pause = None;
        }
        if let Some(p) = &self.parked {
            let end_s = p.item.end_s(p.probed.duration_s);
            let Some(target) = request.target(p.position_s, p.item.start_s(), end_s) else {
                self.log(Msg::new("log.seek_not_allowed").arg("request", format!("{request:?}")));
                return;
            };
            if ends_film(target, end_s) {
                self.log(
                    Msg::new("log.seek_to_end_paused")
                        .arg("id", p.item.id)
                        .arg("title", &p.item.title),
                );
                if let Err(e) = self.skip() {
                    self.log_error("log.channel_error_seek", &e);
                }
            } else if let Some(p) = self.parked.as_mut() {
                p.position_s = target;
            }
            return;
        }
        let Some(c) = self.current.as_ref().filter(|c| !c.item.is_live) else {
            self.log(Msg::new("log.seek_nothing_to_seek").arg("request", format!("{request:?}")));
            return;
        };
        let from = self
            .pending_seek_of(c.item.id)
            .unwrap_or_else(|| c.position_s());
        let Some(target_s) = request.target(from, c.item.start_s(), c.end_s()) else {
            self.log(Msg::new("log.seek_not_allowed").arg("request", format!("{request:?}")));
            return;
        };
        // A real seek joined with a ticker restart is logged as the seek.
        let ticker = ticker
            && self
                .pending_seek
                .as_ref()
                .is_none_or(|s| s.item_id != c.item.id || s.ticker);
        self.pending_seek = Some(PendingSeek {
            item_id: c.item.id,
            target_s,
            due_at: (self.now)().saturating_add(SEEK_QUIET_MS),
            ticker,
            quiet: false,
        });
    }

    /// The ticker's layout changed: the current film starts again from where the channel is
    /// (like a seek to the server position: after 700 ms of quiet, under a new source key, so
    /// one discontinuity; the clip factory reads the new layout). A live stream keeps airing
    /// and the next item gets it; a paused film gets it when it resumes.
    pub fn apply_ticker(&mut self) {
        if self.parked.is_some() {
            return;
        }
        match &self.current {
            None => {}
            Some(c) if c.item.is_live => self.log(Msg::new("log.ticker_live_next_item")),
            Some(_) => self.request(SeekRequest::By(0.0), true),
        }
    }

    /// The public address became `public_url`: like [`Self::apply_ticker`] for the ticker's QR
    /// code, unless the process on air already draws the code of that address (it was started
    /// after the change, e.g. by a resume when the tunnel came back).
    pub fn apply_ticker_code(&mut self, public_url: &str) {
        let wanted = request_url(public_url);
        let drawn = self
            .current
            .as_ref()
            .and_then(|c| c.handle.as_ref())
            .and_then(|h| h.ticker_code());
        if drawn.is_some_and(|code| code == wanted) {
            return;
        }
        self.apply_ticker();
    }

    /// Called by the engine loop every tick: runs a seek whose quiet has passed and warns
    /// when the film on air cannot keep up with its ticker.
    pub fn on_tick(&mut self) {
        if let Err(e) = self.run_due_seek() {
            self.log_error("log.channel_error_seek", &e);
        }
        self.warn_if_too_slow();
        self.refresh_monitor();
    }

    /// The monitor follows the clip being cut: while the window watches it, the clip's fresh
    /// segments are taken from ffmpeg every tick (not only when the broadcaster needs one) and
    /// up to [`LOOKAHEAD`] of them that have not aired yet are listed; a paused film keeps its
    /// still picture; rows past their time go. Unwatched (and without a monitor) the clip is
    /// polled when a segment is due, as before.
    fn refresh_monitor(&mut self) {
        let Some(watched) = self.monitor_watch.as_ref().map(|w| w.watched()) else {
            return;
        };
        // A clip that started since the last segment is taken now, not at the next one.
        if watched {
            self.poll_launches();
        }
        let Some(m) = self.monitor.as_mut() else {
            return;
        };
        let now = (self.now)();
        // A paused film: the segment it resumes from stays listed however long the pause.
        let still = match (&self.current, &self.parked) {
            (None, Some(p)) => m.find(p.item.id, p.position_s),
            _ => None,
        };
        m.pin(still);
        match self.current.as_mut() {
            Some(c) => {
                let key = c.source_key();
                m.set_current(Some(&key), now);
                if watched {
                    if c.start_error.is_none()
                        && let Some(handle) = c.handle.as_mut()
                    {
                        let fresh = handle.poll();
                        c.ready.extend(fresh);
                    }
                    let mut position_s = c.position_s();
                    let listed = c.monitor_listed.len();
                    for (i, seg) in c.ready.iter().enumerate().take(LOOKAHEAD) {
                        if i >= listed {
                            let seq = m.push(&key, c.item.id, seg, position_s);
                            c.monitor_listed.push_back(seq);
                        }
                        position_s += seg.duration;
                    }
                }
            }
            None => m.set_current(None, now),
        }
        m.sweep(now);
    }

    /// The monitor's row of `seg`, the next segment of `c` to air (listed now if it was not).
    fn monitor_emit(
        monitor: Option<&mut MonitorTimeline>,
        c: &mut ActiveClip,
        seg: &SegmentInfo,
        now: u64,
    ) -> Option<u64> {
        let m = monitor?;
        let seq = c
            .monitor_listed
            .pop_front()
            .unwrap_or_else(|| m.push(&c.source_key(), c.item.id, seg, c.position_s()));
        m.aired(seq, now);
        Some(seq)
    }

    /// What the producer's monitor lists: the rows, the one the viewers see now (lagging like
    /// [`Self::state`]) and the newest aired one; `active` while a film is on air or paused.
    pub fn monitor_feed(&self) -> MonitorFeed {
        let Some(m) = &self.monitor else {
            return MonitorFeed::default();
        };
        let lag = self
            .o
            .viewer_lag_segments
            .unwrap_or(DEFAULT_VIEWER_LAG_SEGMENTS);
        let antenna = self
            .aired
            .get(self.aired.len().saturating_sub(lag))
            .filter(|meta| meta.kind == MetaKind::Clip)
            .and_then(|meta| meta.monitor_seq);
        let active = self.control_state().item_id.is_some();
        m.feed(active, antenna, self.monitor_aired)
    }

    /// Logs once per film on air (not for the next one being prepared) that the conversion
    /// its ticker needs is too slow.
    fn warn_if_too_slow(&mut self) {
        let Some(c) = self.current.as_mut() else {
            return;
        };
        let Some(handle) = c.handle.as_ref() else {
            return;
        };
        if c.slow_logged || !handle.ticker_slow() {
            return;
        }
        c.slow_logged = true;
        let speed = handle.stats().speed.unwrap_or_default();
        let msg = Msg::new("log.ticker_slow")
            .arg("id", c.item.id)
            .arg("title", &c.item.title)
            .arg("speed", format!("{speed:.2}"));
        self.log(msg);
    }

    fn run_due_seek(&mut self) -> anyhow::Result<()> {
        let Some(item_id) = self
            .pending_seek
            .as_ref()
            .filter(|s| (self.now)() >= s.due_at)
            .map(|s| s.item_id)
        else {
            return Ok(());
        };
        // Never two processes of the same film: a start still in flight is cancelled, and the
        // seek runs only once it is gone (whatever it produced in the meantime is taken by
        // `poll_launches` below and disposed with the clip).
        if let Some(start) = self
            .current
            .as_ref()
            .filter(|c| c.item.id == item_id && c.is_starting())
            .and_then(|c| c.start.as_ref())
            .filter(|start| !start.is_finished())
        {
            start.abort();
            return Ok(());
        }
        self.poll_launches();
        let Some(seek) = self.pending_seek.take() else {
            return Ok(());
        };
        let Some(mut c) = self
            .current
            .take_if(|c| c.item.id == seek.item_id && !c.item.is_live)
        else {
            return Ok(());
        };
        let target_s = if seek.ticker {
            c.position_s()
        } else {
            seek.target_s
        };
        if seek.ticker && ends_film(target_s, c.end_s()) {
            // Not worth a restart: the film plays out with the old look. The next film,
            // prepared with the old look, is prepared again with the new one.
            self.current = Some(c);
            self.drop_prefetched();
            return Ok(());
        }
        if ends_film(target_s, c.end_s()) {
            self.log(
                Msg::new("log.seek_to_end")
                    .arg("id", c.item.id)
                    .arg("title", &c.item.title),
            );
            let skipped = Msg::new(SKIPPED);
            return self.finish(c, ItemStatus::Played, Some(&skipped));
        }
        // The prefetched film may now be far from its turn; it is prefetched again near the end.
        self.drop_prefetched();
        if let Some(error) = c.failure() {
            // The process had already failed: the usual retry accounting, from the target.
            c.base_s = target_s;
            c.emitted_s = 0.0;
            c.ready.clear();
            self.current = Some(c);
            return self.handle_failure(error);
        }
        let mut off = Parked::off_air(c);
        if !seek.quiet {
            self.log(if seek.ticker {
                Msg::new("log.ticker_applied")
                    .arg("id", off.item.id)
                    .arg("title", &off.item.title)
                    .arg("seconds", format!("{target_s:.0}"))
            } else {
                Msg::new("log.seek")
                    .arg("id", off.item.id)
                    .arg("title", &off.item.title)
                    .arg("from", format!("{:.0}", off.position_s))
                    .arg("to", format!("{target_s:.0}"))
            });
        }
        off.position_s = target_s;
        self.current = Some(self.relaunch(off));
        Ok(())
    }

    /// Starts a film taken off the air again from its position, under a new source key.
    fn relaunch(&mut self, p: Parked) -> ActiveClip {
        let mut clip = self.launch(p.item, p.position_s, p.attempt);
        clip.relaunches = p.relaunches + 1;
        clip.earlier = p.probed;
        clip
    }

    /// Airs `item_id` next: a queued film moves to the front, a film from the history (played
    /// or failed) is added again (a new entry with the same link and nick) at the front. The
    /// current film ends as skipped and the manual pause ends (automatic reasons stay).
    /// Pending approval, removed, rejected or playing films are refused.
    pub fn play_now(&mut self, item_id: i64) -> anyhow::Result<()> {
        let channel_id = self.o.channel_id;
        let item = self
            .queue(|q| q.get(item_id))?
            .filter(|i| i.channel_id == channel_id)
            .ok_or_else(|| msg_error(Msg::error("no_such_item")))?;
        let chosen = match item.status {
            ItemStatus::Queued => {
                // "Now" wins over a start time set before.
                self.queue(|q| {
                    q.set_start_at(item.id, None)?;
                    q.move_item(item.id, 0)
                })?;
                item.id
            }
            ItemStatus::Played | ItemStatus::Failed => {
                let again = self.queue(|q| {
                    q.add_first(&NewItem {
                        channel_id,
                        source_url: item.source_url.clone(),
                        title: item.title.clone(),
                        duration_s: item.duration_s,
                        is_live: item.is_live,
                        status: ItemStatus::Queued,
                        added_by: item.added_by,
                        requester_name: item.requester_name.clone(),
                        // The owner's choice: it does not count towards the viewer's limit.
                        requester_ip: None,
                        requester_key_hash: item.requester_key_hash.clone(),
                    })
                })?;
                again.id
            }
            status => {
                return Err(msg_error(
                    Msg::error("item_not_playable")
                        .arg("id", item_id)
                        .arg_msg("status", status.msg()),
                ));
            }
        };
        self.log(
            Msg::new("log.play_now")
                .arg("title", &item.title)
                .arg("id", chosen),
        );
        self.skip()?;
        // The owner's explicit "play this now" ends the manual pause (the paused film was just
        // skipped, so nothing is relaunched); automatic reasons keep holding the film.
        self.resume(PauseReason::Manual);
        self.on_queue_changed()
    }

    /// Takes the current film off the air and shows the pause slate until every reason is
    /// resumed. A live stream is not paused (it keeps airing; the next film waits for the
    /// resume). The queue item stays `playing`; the clip is disposed, the position remembered.
    ///
    /// While a manual pause waits for its position ([`Self::pause_at`]) another manual pause
    /// changes nothing; an automatic one pauses at once and the manual reason with it.
    pub fn pause(&mut self, reason: PauseReason) {
        if self.armed_pause.is_some() && reason == PauseReason::Manual {
            return;
        }
        let armed = self.armed_pause.take();
        let first = self.pause_reasons.is_empty();
        self.pause_reasons.insert(reason);
        if armed.is_some() {
            self.pause_reasons.insert(PauseReason::Manual);
        }
        if !first {
            return;
        }
        // The prefetched film would only sit there; it is prefetched again after the resume.
        self.drop_prefetched();
        if self.current.as_ref().is_none_or(|c| c.item.is_live) {
            return;
        }
        // A seek still waiting decides where the film is parked.
        let seek = self.pending_seek.take();
        self.settle_before_parking();
        let Some(c) = self.current.take() else {
            return;
        };
        // So does a seek made while the manual pause waited for its position.
        let target = seek
            .filter(|s| s.item_id == c.item.id && !s.ticker)
            .map(|s| s.target_s)
            .or(armed
                .filter(|a| a.item_id == c.item.id)
                .and_then(|a| a.resume_at));
        if let Some(target) = target
            && ends_film(target, c.end_s())
        {
            let skipped = Msg::new(SKIPPED);
            if let Err(e) = self.finish(c, ItemStatus::Played, Some(&skipped)) {
                self.log_error("log.channel_error_pause", &e);
            }
            return;
        }
        let mut p = Parked::off_air(c);
        if let Some(target) = target {
            p.position_s = target;
        }
        self.log(
            Msg::new("log.pause")
                .arg_msg("reason", reason.msg())
                .arg("id", p.item.id)
                .arg("title", &p.item.title)
                .arg("seconds", format!("{:.0}", p.position_s)),
        );
        self.parked = Some(p);
    }

    /// The owner's pause at `at_s` seconds of the film (where the producer's monitor stood).
    /// Viewers cannot be shown less than what was already handed to the broadcaster: when the
    /// channel has aired up to `at_s` or beyond, the film pauses at once (like
    /// [`Self::pause`]). Otherwise it keeps airing until the segment holding `at_s` has aired
    /// and pauses at that segment's end (the first boundary at or after `at_s`, at most one
    /// segment past it), so the resume neither skips nor repeats a frame. Meanwhile
    /// [`ControlState::pause_pending`] is set; a resume, a skip or "play now" cancels it, a seek
    /// only moves where the film resumes (as during a pause), another manual pause changes
    /// nothing. A live stream, a seek still waiting or an active automatic reason make it a
    /// plain pause. A film that ends before `at_s` (a shorter file than probed, a pause in its
    /// last segment) pauses at its end: the channel stays paused before the next film.
    /// Boundaries are those of the segments the clip really cut, so nothing here depends on
    /// the segment length.
    pub fn pause_at(&mut self, at_s: f64) {
        if self.armed_pause.is_some() || self.pause_reasons.contains(&PauseReason::Manual) {
            return;
        }
        let ahead = self
            .current
            .as_ref()
            .filter(|_| self.pause_reasons.is_empty() && self.pending_seek.is_none())
            .filter(|c| !c.item.is_live && at_s.is_finite() && at_s > c.position_s());
        let Some(c) = ahead else {
            self.pause(PauseReason::Manual);
            return;
        };
        self.log(
            Msg::new("log.pause_armed")
                .arg("id", c.item.id)
                .arg("title", &c.item.title)
                .arg("seconds", format!("{at_s:.0}"))
                .arg("from", format!("{:.0}", c.position_s())),
        );
        self.armed_pause = Some(ArmedPause {
            item_id: c.item.id,
            at_s,
            resume_at: None,
        });
    }

    /// The owner's manual pause is active or waits for its position.
    pub fn manual_pause(&self) -> bool {
        self.armed_pause.is_some() || self.pause_reasons.contains(&PauseReason::Manual)
    }

    /// Runs the pause of [`Self::pause_at`] once the film has aired up to its position; a film
    /// that ended (or gave way) before that leaves the channel paused for the next one.
    fn fire_armed_pause(&mut self) {
        let Some(armed) = &self.armed_pause else {
            return;
        };
        let waiting = self
            .current
            .as_ref()
            .is_some_and(|c| c.item.id == armed.item_id && c.position_s() < armed.at_s);
        if waiting {
            return;
        }
        let Some(armed) = self.armed_pause.take() else {
            return;
        };
        self.pause(PauseReason::Manual);
        // A seek made meanwhile moves the paused position (a target in the last second ends
        // the film, the channel staying paused for the next one, as for any paused film).
        if let Some(target) = armed.resume_at
            && self
                .parked
                .as_ref()
                .is_some_and(|p| p.item.id == armed.item_id)
        {
            self.request(SeekRequest::To(target), false);
        }
    }

    /// A clip that already ended or failed must not be parked: a finished film is finished
    /// (played), a failure goes through the normal retry accounting.
    fn settle_before_parking(&mut self) {
        self.poll_launches();
        let stopped = Msg::new(FFMPEG_STOPPED);
        let Some(c) = self.current.as_mut() else {
            return;
        };
        let mut failure = c.start_error.clone();
        let mut done = false;
        if failure.is_none()
            && let Some(handle) = c.handle.as_mut()
        {
            let fresh = handle.poll();
            c.ready.extend(fresh);
            if c.ready.is_empty() {
                match (handle.state(), handle.error()) {
                    (SegmenterState::Done, _) => done = true,
                    (SegmenterState::Failed | SegmenterState::Killed, error) => {
                        failure = Some(error.unwrap_or_else(|| stopped.clone()));
                    }
                    _ => {}
                }
            }
        }
        let result = if done {
            self.finish_current(ItemStatus::Played, None)
        } else if let Some(error) = failure {
            self.handle_failure(error)
        } else {
            Ok(())
        };
        if let Err(e) = result {
            self.log_error("log.channel_error_pause", &e);
        }
    }

    /// Clears `reason`; once no reason is left the paused film restarts from where it stopped
    /// (behind the buffering slate) and queued films start again.
    pub fn resume(&mut self, reason: PauseReason) {
        if reason == PauseReason::Manual
            && let Some(armed) = self.armed_pause.take()
        {
            // A pause still waiting for its position never happens; a seek made meanwhile
            // runs now (the film goes on from its target, not from where it is).
            if let Some(target) = armed.resume_at {
                self.request(SeekRequest::To(target), false);
            }
        }
        if !self.pause_reasons.remove(&reason) || !self.pause_reasons.is_empty() {
            return;
        }
        if let Some(p) = self.parked.take() {
            self.log(
                Msg::new("log.resume")
                    .arg("id", p.item.id)
                    .arg("title", &p.item.title)
                    .arg("seconds", format!("{:.0}", p.position_s)),
            );
            self.current = Some(self.relaunch(p));
        }
    }

    /// A pause reason is active and no live stream keeps airing through it.
    fn is_holding_back(&self) -> bool {
        !self.pause_reasons.is_empty() && !self.current.as_ref().is_some_and(|c| c.item.is_live)
    }

    /// What the stats show about the film being cut, the one prepared after it and the encoder
    /// process (CPU and memory are added per request). The playlist counters come from the
    /// broadcaster.
    pub fn stats(&self) -> ChannelStats {
        let on_air = if let Some(c) = &self.current {
            let clip = c.handle.as_ref().map(|h| h.stats()).unwrap_or_default();
            Some((
                OnAirInfo {
                    title: c.item.title.clone(),
                    source: c.item.source_url.clone(),
                    position_s: c.position_s(),
                    attempt: c.attempt,
                    probe: clip.probe.clone(),
                    mode: clip.mode,
                    encoder: clip.encoder.clone(),
                    ready_segments: c.ready.len(),
                },
                clip,
            ))
        } else {
            // Paused or seeking: no process, the film is only parked.
            self.parked.as_ref().map(|p| {
                (
                    OnAirInfo {
                        title: p.item.title.clone(),
                        source: p.item.source_url.clone(),
                        position_s: p.position_s,
                        attempt: p.attempt,
                        probe: None,
                        mode: None,
                        encoder: None,
                        ready_segments: 0,
                    },
                    ClipStats::default(),
                )
            })
        };
        let next = match (&self.prefetched, &self.preparing) {
            (Some(c), _) => Some(NextInfo {
                item_id: c.item.id,
                title: c.item.title.clone(),
                state: if c.failure().is_some() {
                    NextState::Failed
                } else if c.is_starting() {
                    NextState::Preparing
                } else {
                    NextState::Ready
                },
                ready_segments: c.ready.len(),
            }),
            (None, Some(p)) => Some(NextInfo {
                item_id: p.item_id,
                title: p.title.clone(),
                state: p.state,
                ready_segments: 0,
            }),
            (None, None) => None,
        };
        let ffmpeg = on_air
            .as_ref()
            .and_then(|(_, clip)| Some((clip.pid?, clip.speed)))
            .map(|(pid, speed)| FfmpegInfo {
                pid,
                speed,
                cpu_percent: None,
                rss_bytes: None,
            });
        ChannelStats {
            ticker_slow: on_air.as_ref().is_some_and(|(_, clip)| clip.ticker_slow),
            on_air: on_air.map(|(info, _)| info),
            hls: None,
            next,
            ffmpeg,
        }
    }

    /// The published segments of the last minutes, oldest first (the air clock).
    pub fn air_segments(&self) -> Vec<crate::pipeline::air::AirSegment> {
        self.air_log.segments()
    }

    /// How many segments viewers lag behind the newest published one.
    pub fn viewer_lag_segments(&self) -> usize {
        self.o
            .viewer_lag_segments
            .unwrap_or(DEFAULT_VIEWER_LAG_SEGMENTS)
    }

    /// The item being started in the background ahead of time (shown in the admin panel).
    pub fn prefetched_item_id(&self) -> Option<i64> {
        self.prefetched.as_ref().map(|c| c.item.id)
    }

    /// The queue item being aired (or started) right now; `state()` lags behind it.
    pub fn current_item_id(&self) -> Option<i64> {
        self.current.as_ref().map(|c| c.item.id)
    }

    /// Marks `id` failed with `error` (the supervisor's verdict on a film that keeps crashing
    /// the loop). An owner's removal is not overwritten. The clips are disposed by `stop()`.
    pub fn fail_item(&mut self, id: i64, error: &Msg) -> anyhow::Result<()> {
        let error = error.to_stored();
        self.queue(|q| q.finish(id, ItemStatus::Failed, Some(&error)))
            .map(|_| ())
    }

    /// Hands finished clip starts to their clips; a start whose clip was dropped meanwhile is
    /// disposed.
    pub fn poll_launches(&mut self) {
        self.poll_prepared();
        while let Ok(launch) = self.launch_rx.try_recv() {
            let target = [
                self.current.as_mut(),
                self.prefetched.as_mut(),
                self.countdown.as_mut().map(|c| &mut c.clip),
            ]
            .into_iter()
            .flatten()
            .find(|c| c.id == launch.clip_id);
            match (target, launch.result) {
                (Some(clip), Ok(handle)) => clip.handle = Some(handle),
                (Some(clip), Err(e)) => {
                    clip.start_error = Some(if e.to_string().is_empty() {
                        Msg::new(UNKNOWN_ERROR)
                    } else {
                        e
                    });
                }
                (None, Ok(mut handle)) => handle.dispose(),
                (None, Err(_)) => {}
            }
        }
    }

    // ---------- Internals ----------

    fn queue<T>(&self, f: impl FnOnce(&QueueRepo<'_>) -> rusqlite::Result<T>) -> anyhow::Result<T> {
        let conn = self.o.db.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(f(&QueueRepo::new(&conn))?)
    }

    fn log(&self, msg: Msg) {
        if let Some(log) = &self.o.log {
            log(msg);
        }
    }

    /// A log entry `key` with the error and its causes (`{error}`).
    fn log_error(&self, key: &'static str, e: &anyhow::Error) {
        self.log(Msg::new(key).arg_msg("error", error_msg(e)));
    }

    fn next_inner(&mut self) -> anyhow::Result<SourceSegment> {
        let start_timeout_ms = self
            .o
            .start_timeout_s
            .unwrap_or(DEFAULT_START_TIMEOUT_S)
            .saturating_mul(1000);
        let stall_timeout_ms = self
            .o
            .stall_timeout_s
            .unwrap_or(DEFAULT_STALL_TIMEOUT_S)
            .saturating_mul(1000);
        for _ in 0..NEXT_GUARD {
            self.fire_armed_pause();
            if self.current.is_none() {
                if !self.pause_reasons.is_empty() {
                    // A countdown made before the pause would be out of date after it.
                    self.drop_countdown();
                    return self.paused_slate();
                }
                self.promote()?;
            }
            let Some(c) = self.current.as_mut() else {
                return self.countdown_or_idle();
            };

            if let Some(error) = c.start_error.clone() {
                self.handle_failure(error)?;
                continue;
            }
            if let Some(handle) = c.handle.as_mut() {
                let fresh = handle.poll();
                c.ready.extend(fresh);
            }
            if c.past_out_point() {
                // Aired up to its out point (set or moved while it plays).
                self.finish_current(ItemStatus::Played, None)?;
                continue;
            }

            if let Some(seg) = c.ready.pop_front() {
                let now = (self.now)();
                let monitor_seq = Self::monitor_emit(self.monitor.as_mut(), c, &seg, now);
                let (out, mut meta) = c.emit(&seg);
                meta.monitor_seq = monitor_seq;
                if monitor_seq.is_some() {
                    self.monitor_aired = monitor_seq;
                }
                let remaining = c.remaining_s();
                self.last_slate_kind = None;
                self.previous_clip_segment = Some(seg.path.clone());
                self.pending_meta.insert(seg.path, meta);
                self.retry_failed_prefetch(remaining);
                self.maybe_prefetch(remaining);
                self.maybe_prepare();
                return Ok(out);
            }

            let now = (self.now)();
            match c.handle.as_ref().map(|h| (h.state(), h.error())) {
                Some((SegmenterState::Done, _)) => {
                    self.finish_current(ItemStatus::Played, None)?;
                    continue;
                }
                Some((SegmenterState::Failed | SegmenterState::Killed, error)) => {
                    let stopped = Msg::new(FFMPEG_STOPPED);
                    self.handle_failure(error.unwrap_or(stopped))?;
                    continue;
                }
                Some(_) => {}
                None => {
                    if now.saturating_sub(c.started_at) > start_timeout_ms {
                        self.handle_failure(Msg::new(START_TIMEOUT_ERROR))?;
                        continue;
                    }
                    return self.waiting_slate();
                }
            }
            let stalled_since = *c.stalled_since.get_or_insert(now);
            if now.saturating_sub(stalled_since) > stall_timeout_ms {
                if let Some(handle) = c.handle.as_mut() {
                    handle.kill();
                }
                let stalled = Msg::new(STALL_ERROR);
                self.finish_current(ItemStatus::Failed, Some(&stalled))?;
                continue;
            }
            return self.waiting_slate();
        }
        self.slate(SlateKind::Idle)
    }

    /// Nothing airs while a pause reason is active. The pause slate explains why a film is held
    /// back; with nothing waiting in the queue the usual idle slate stays.
    fn paused_slate(&mut self) -> anyhow::Result<SourceSegment> {
        let channel_id = self.o.channel_id;
        let waiting = self.parked.is_some() || self.queue(|q| q.next_queued(channel_id))?.is_some();
        self.slate(if waiting {
            SlateKind::Pause
        } else {
            SlateKind::Idle
        })
    }

    /// What viewers see while the film is not producing: "Technical problem" while a film that
    /// failed on air is started again, otherwise "Loading".
    fn waiting_slate(&mut self) -> anyhow::Result<SourceSegment> {
        let recovering = self.current.as_ref().is_some_and(|c| c.recovering);
        self.slate(if recovering {
            SlateKind::Fallback
        } else {
            SlateKind::Buffering
        })
    }

    fn slate(&mut self, kind: SlateKind) -> anyhow::Result<SourceSegment> {
        // A set without the fallback slate shows the buffering one.
        let kind = if kind == SlateKind::Fallback && self.o.slates.fallback.is_empty() {
            SlateKind::Buffering
        } else {
            kind
        };
        let list = match kind {
            SlateKind::Idle => &self.o.slates.idle,
            SlateKind::Buffering => &self.o.slates.buffering,
            SlateKind::Pause => &self.o.slates.pause,
            SlateKind::Fallback => &self.o.slates.fallback,
        };
        if list.is_empty() {
            return Err(msg_error(
                Msg::new("log.slate_empty").arg("kind", kind.as_str()),
            ));
        }
        let slot = slate_slot(kind);
        if self.last_slate_kind != Some(kind) {
            self.slate_index[slot] = 0;
        }
        self.last_slate_kind = Some(kind);
        let index = self.slate_index[slot] % list.len();
        self.slate_index[slot] = index + 1;
        let seg = &list[index];
        let item = match kind {
            SlateKind::Buffering | SlateKind::Fallback => {
                self.current.as_ref().map(|c| c.item.clone())
            }
            SlateKind::Pause => self.parked.as_ref().map(|p| p.item.clone()),
            SlateKind::Idle => None,
        };
        let position_s = match (&self.current, &self.parked) {
            (Some(c), _) => c.position_s(),
            (None, Some(p)) if kind == SlateKind::Pause => p.position_s,
            _ => 0.0,
        };
        self.pending_meta.insert(
            seg.path.clone(),
            SegmentMeta {
                kind: MetaKind::Slate(kind),
                item,
                position_s,
                monitor_seq: None,
            },
        );
        Ok(SourceSegment {
            path: seg.path.clone(),
            duration: seg.duration,
            source_key: self.o.slates.source_key(kind),
            index: index as u64,
        })
    }

    /// The current film ended or there is none: take the prefetched one or the queue head.
    ///
    /// The head is only taken while it is still queued: an owner's removal that lands between
    /// reading the head and starting it wins, the started clip is dropped and the next head is
    /// picked.
    fn promote(&mut self) -> anyhow::Result<()> {
        let channel_id = self.o.channel_id;
        for _ in 0..NEXT_GUARD {
            let head = self.queue(|q| q.next_queued(channel_id))?;
            let prefetched_stale = self
                .prefetched
                .as_ref()
                .is_some_and(|p| head.as_ref().is_none_or(|h| h.id != p.item.id));
            if prefetched_stale {
                self.drop_prefetched();
            }
            let Some(head) = head else {
                return Ok(());
            };
            // A film with a start time waits for it (the countdown airs meanwhile).
            if self.waits_for_start(&head) {
                return Ok(());
            }
            self.drop_countdown();
            let mut clip = match self.prefetched.take() {
                Some(clip) => clip,
                None => {
                    let start_at_s = head.start_s();
                    self.launch(head.clone(), start_at_s, 0)
                }
            };
            let id = head.id;
            let fresh = self.queue(|q| {
                if q.start_playing(id)? {
                    q.get(id)
                } else {
                    Ok(None)
                }
            });
            match fresh {
                Ok(Some(fresh)) => {
                    if self
                        .preparing
                        .as_ref()
                        .is_some_and(|p| p.item_id == fresh.id)
                    {
                        self.preparing = None;
                    }
                    clip.item = fresh;
                    self.current = Some(clip);
                    return Ok(());
                }
                // No longer queued (removed meanwhile): drop the clip and look again.
                Ok(None) => clip.dispose(),
                Err(e) => {
                    // As before the conditional update: keep airing it, the error is logged.
                    clip.item = head;
                    self.current = Some(clip);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Milliseconds until `item`'s start time (negative: past); `None`: no start time.
    fn ms_to_start(&self, item: &QueueItem) -> Option<i64> {
        let now = i64::try_from((self.wall_now)()).unwrap_or(i64::MAX);
        item.start_at.map(|at| at.saturating_sub(now))
    }

    /// `head` has a start time still ahead: not yet, unless its countdown has run out (its last
    /// segment may end a little before the time).
    fn waits_for_start(&self, head: &QueueItem) -> bool {
        let Some(ms) = self.ms_to_start(head) else {
            return false;
        };
        if ms <= START_SLACK_MS {
            return false;
        }
        let countdown_over = self.countdown.as_ref().is_some_and(|cd| {
            cd.clip.item.id == head.id
                && cd.start_at == head.start_at.unwrap_or_default()
                && cd.clip.ready.is_empty()
                && cd
                    .clip
                    .handle
                    .as_ref()
                    .is_some_and(|h| h.state() == SegmenterState::Done)
        });
        !countdown_over
    }

    /// Nothing airs: the countdown before the queue head's start time (made when needed, the
    /// head prepared during its last minute), else the idle slate.
    fn countdown_or_idle(&mut self) -> anyhow::Result<SourceSegment> {
        let channel_id = self.o.channel_id;
        let head = self
            .queue(|q| q.next_queued(channel_id))?
            .filter(|h| self.ms_to_start(h).is_some_and(|ms| ms > START_SLACK_MS));
        let Some(head) = head else {
            self.drop_countdown();
            return self.slate(SlateKind::Idle);
        };
        let start_at = head.start_at.unwrap_or_default();
        let ms = self.ms_to_start(&head).unwrap_or_default();
        let stale = self
            .countdown
            .as_ref()
            .is_some_and(|cd| cd.clip.item.id != head.id || cd.start_at != start_at);
        if stale {
            self.drop_countdown();
        }
        let seconds = ms as f64 / 1000.0;
        if self.countdown.is_none() && seconds >= MIN_COUNTDOWN_S {
            self.countdowns += 1;
            let n = self.countdowns;
            self.log(
                Msg::new("log.countdown")
                    .arg("id", head.id)
                    .arg("title", &head.title)
                    .arg("seconds", format!("{seconds:.0}")),
            );
            let start = self.o.factory.create_countdown(head.clone(), seconds);
            let clip = self.launch_with(head.clone(), 0.0, 0, start);
            self.countdown = Some(Countdown {
                clip,
                start_at,
                n,
                failure_logged: false,
            });
        }
        // The film is prepared during the countdown's last minute.
        if self.prefetched.is_none()
            && !head.is_live
            && seconds <= self.o.prefetch_s.unwrap_or(DEFAULT_PREFETCH_S)
        {
            self.prefetched = Some(self.launch_ahead(head.clone()));
        }
        let failure = {
            let Some(cd) = self.countdown.as_mut() else {
                return self.slate(SlateKind::Idle);
            };
            if let Some(handle) = cd.clip.handle.as_mut() {
                let fresh = handle.poll();
                cd.clip.ready.extend(fresh);
            }
            if let Some(seg) = cd.clip.ready.pop_front() {
                let out = SourceSegment {
                    path: seg.path.clone(),
                    duration: seg.duration,
                    source_key: format!("countdown-{}-{}", head.id, cd.n),
                    index: seg.index as u64,
                };
                self.last_slate_kind = None;
                self.previous_clip_segment = Some(seg.path.clone());
                self.pending_meta.insert(
                    seg.path,
                    SegmentMeta {
                        kind: MetaKind::Countdown,
                        item: Some(head),
                        position_s: 0.0,
                        monitor_seq: None,
                    },
                );
                return Ok(out);
            }
            match (cd.clip.failure(), cd.failure_logged) {
                (Some(error), false) => {
                    cd.failure_logged = true;
                    Some(error)
                }
                _ => None,
            }
        };
        if let Some(error) = failure {
            self.log(Msg::new("log.countdown_failed").arg_msg("error", error));
        }
        self.slate(SlateKind::Idle)
    }

    /// Once the film on air has aired a little, the queue head (not live, starting within
    /// [`PREPARE_AHEAD_MAX_MS`]) is resolved and analysed ahead: one at a time, once per film
    /// and link.
    fn maybe_prepare(&mut self) {
        let aired = self
            .current
            .as_ref()
            .is_some_and(|c| c.emitted_s >= PREPARE_AFTER_S || c.relaunches > 0);
        if !aired || self.prefetched.is_some() {
            return;
        }
        if self
            .preparing
            .as_ref()
            .is_some_and(|p| p.state == NextState::Resolving)
        {
            return;
        }
        let channel_id = self.o.channel_id;
        let Ok(Some(head)) = self.queue(|q| q.next_queued(channel_id)) else {
            return;
        };
        let done = self
            .preparing
            .as_ref()
            .is_some_and(|p| p.item_id == head.id && p.source_url == head.source_url);
        let far = self
            .ms_to_start(&head)
            .is_some_and(|ms| ms > PREPARE_AHEAD_MAX_MS);
        if done || far || head.is_live {
            return;
        }
        self.preparing = Some(Preparing {
            item_id: head.id,
            source_url: head.source_url.clone(),
            title: head.title.clone(),
            state: NextState::Resolving,
        });
        let start = self.o.factory.prepare(head.clone());
        let tx = self.prepare_tx.clone();
        let id = head.id;
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = tx.send((id, start.await));
            });
        }
    }

    /// Hands the results of resolving ahead to [`Self::preparing`].
    fn poll_prepared(&mut self) {
        while let Ok((id, result)) = self.prepare_rx.try_recv() {
            let Some(p) = self.preparing.as_mut().filter(|p| p.item_id == id) else {
                continue;
            };
            match result {
                Ok(()) => p.state = NextState::Resolved,
                Err(error) => {
                    let title = p.title.clone();
                    self.preparing_failed(id, &title, error);
                }
            }
        }
    }

    fn preparing_failed(&mut self, id: i64, title: &str, error: Msg) {
        if let Some(p) = self.preparing.as_mut() {
            p.state = NextState::Failed;
        }
        self.log(
            Msg::new("log.prepare_failed")
                .arg("id", id)
                .arg("title", title)
                .arg_msg("error", error),
        );
    }

    /// A clip prepared ahead whose start failed is started once more after a while, while
    /// the film on air still has time left (no gap between the two).
    fn retry_failed_prefetch(&mut self, remaining_s: Option<f64>) {
        let now = (self.now)();
        let due = self.prefetched.as_ref().is_some_and(|p| {
            p.failure().is_some()
                && self.prefetch_retried != Some(p.item.id)
                && now.saturating_sub(p.started_at) >= PREFETCH_RETRY_MS
        });
        if !due || remaining_s.is_some_and(|r| r < PREPARE_AFTER_S) {
            return;
        }
        if let Some(mut failed) = self.prefetched.take() {
            failed.dispose();
            self.prefetch_retried = Some(failed.item.id);
            self.log(
                Msg::new("log.prefetch_retry")
                    .arg("id", failed.item.id)
                    .arg("title", &failed.item.title),
            );
            self.prefetched = Some(self.launch_ahead(failed.item));
        }
    }

    fn drop_countdown(&mut self) {
        if let Some(mut cd) = self.countdown.take() {
            cd.clip.dispose();
        }
    }

    fn maybe_prefetch(&mut self, remaining_s: Option<f64>) {
        if self.prefetched.is_some() {
            return;
        }
        let Some(remaining_s) = remaining_s else {
            return;
        };
        let prefetch_s = self.o.prefetch_s.unwrap_or(DEFAULT_PREFETCH_S);
        if remaining_s > prefetch_s {
            return;
        }
        let channel_id = self.o.channel_id;
        // A head whose start time is further off than the end of this film plus the prefetch
        // window is prepared during its countdown instead.
        let head = self.queue(|q| q.next_queued(channel_id)).map(|head| {
            head.filter(|h| {
                self.ms_to_start(h)
                    .is_none_or(|ms| ms as f64 / 1000.0 <= remaining_s + prefetch_s)
            })
        });
        match head {
            Ok(Some(head)) => self.prefetched = Some(self.launch_ahead(head)),
            Ok(None) => {}
            Err(e) => self.log_error("log.queue_error_prefetch", &e),
        }
    }

    /// Starts the next film ahead of its turn (see [`ClipFactory::create_ahead`]).
    fn launch_ahead(&mut self, item: QueueItem) -> ActiveClip {
        let start_at_s = item.start_s();
        let start = self.o.factory.create_ahead(item.clone(), start_at_s);
        self.launch_with(item, start_at_s, 0, start)
    }

    /// Starts the clip in a background task; the result arrives through `poll_launches`.
    fn launch(&mut self, item: QueueItem, start_at_s: f64, attempt: u32) -> ActiveClip {
        let start = self.o.factory.create(item.clone(), start_at_s, attempt);
        self.launch_with(item, start_at_s, attempt, start)
    }

    fn launch_with(
        &mut self,
        item: QueueItem,
        start_at_s: f64,
        attempt: u32,
        start: BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>>,
    ) -> ActiveClip {
        self.next_clip_id += 1;
        let id = self.next_clip_id;
        let mut clip = ActiveClip {
            id,
            item: item.clone(),
            attempt,
            relaunches: 0,
            base_s: start_at_s,
            handle: None,
            start_error: None,
            started_at: (self.now)(),
            ready: VecDeque::new(),
            emitted_s: 0.0,
            stalled_since: None,
            earlier: Probed::default(),
            start: None,
            slow_logged: false,
            monitor_listed: VecDeque::new(),
            recovering: false,
            started_out_s: item.out_s.filter(|_| !item.is_live),
        };
        let tx = self.launch_tx.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                let task = rt.spawn(async move {
                    let result = start.await;
                    if let Err(mpsc::error::SendError(launch)) = tx.send(Launch {
                        clip_id: id,
                        result,
                    }) && let Ok(mut handle) = launch.result
                    {
                        // The channel is gone: nobody will ever dispose this clip.
                        handle.dispose();
                    }
                });
                clip.start = Some(task.abort_handle());
            }
            Err(e) => clip.start_error = Some(Msg::text(e)),
        }
        clip
    }

    fn handle_failure(&mut self, error: Msg) -> anyhow::Result<()> {
        let Some(mut c) = self.current.take() else {
            return Ok(());
        };
        self.log(
            Msg::new("log.clip_failed")
                .arg("id", c.item.id)
                .arg("title", &c.item.title)
                .arg_msg("error", error.clone()),
        );
        let probed = c.probed();
        if let Some(mut handle) = c.handle.take() {
            handle.dispose();
        }
        // A refused address (the home network, not http) would be refused again.
        let refused = matches!(
            error.code(),
            Some("local_address" | "bad_scheme" | "bad_address")
        );
        if c.attempt == 0 && !refused {
            // One retry from where it stopped (resolved anew, the old address may have expired).
            let resume_at = if c.item.is_live { 0.0 } else { c.position_s() };
            let on_air = c.emitted_s > 0.0 || c.recovering;
            let mut retry = self.launch(c.item, resume_at, 1);
            retry.earlier = probed;
            retry.recovering = on_air;
            self.current = Some(retry);
            return Ok(());
        }
        self.finish(c, ItemStatus::Failed, Some(&error))
    }

    fn finish_current(&mut self, status: ItemStatus, error: Option<&Msg>) -> anyhow::Result<()> {
        match self.current.take() {
            Some(c) => self.finish(c, status, error),
            None => Ok(()),
        }
    }

    fn finish(
        &mut self,
        mut c: ActiveClip,
        status: ItemStatus,
        error: Option<&Msg>,
    ) -> anyhow::Result<()> {
        // Conditional: an owner's removal (status `removed`) is never overwritten.
        let stored = error.map(Msg::to_stored);
        let result = self.queue(|q| q.finish(c.item.id, status, stored.as_deref()));
        c.dispose();
        self.forget_seek_of(c.item.id);
        result.map(|_| ())
    }

    fn drop_current(&mut self) {
        if let Some(mut c) = self.current.take() {
            c.dispose();
            self.forget_seek_of(c.item.id);
            // A removed film takes the pause waiting for its position with it.
            if self
                .armed_pause
                .as_ref()
                .is_some_and(|a| a.item_id == c.item.id)
            {
                self.armed_pause = None;
            }
        }
    }

    /// A seek waiting for a film that is gone never runs.
    fn forget_seek_of(&mut self, item_id: i64) {
        if self
            .pending_seek
            .as_ref()
            .is_some_and(|s| s.item_id == item_id)
        {
            self.pending_seek = None;
        }
    }

    fn drop_prefetched(&mut self) {
        if let Some(mut c) = self.prefetched.take() {
            c.dispose();
        }
    }

    /// The broadcaster reads a segment right after `next()`, so it can go on the next call.
    fn release_previous_segment(&mut self) {
        let Some(path) = self.previous_clip_segment.take() else {
            return;
        };
        match &self.o.remove_file {
            Some(remove) => remove(&path),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

impl SegmentSource for Channel {
    /// Always yields a segment: a clip segment or a slate. Queue errors fall back to a slate.
    fn next(&mut self) -> anyhow::Result<SourceSegment> {
        self.poll_launches();
        self.on_tick();
        self.release_previous_segment();
        match self.next_inner() {
            Ok(seg) => Ok(seg),
            Err(e) => {
                self.log_error("log.channel_error", &e);
                let kind = if self.current.is_some() {
                    SlateKind::Buffering
                } else {
                    SlateKind::Idle
                };
                self.slate(kind)
            }
        }
    }

    fn on_published(&mut self, seg: &PublishedSegment) {
        let film = self
            .pending_meta
            .get(&seg.path)
            .filter(|m| m.kind == MetaKind::Clip)
            .and_then(|m| Some((m.item.as_ref()?.id, m.position_s)));
        self.air_log.push(crate::pipeline::air::AirSegment {
            seq: seg.seq,
            channel_time: seg.channel_time,
            duration: seg.duration,
            item_id: film.map(|f| f.0),
            position_s: film.map(|f| f.1),
        });
        let meta = match self.pending_meta.get(&seg.path) {
            None => return,
            // Slate files air again and again; clip and countdown files only once.
            Some(meta) if !matches!(meta.kind, MetaKind::Slate(_)) => {
                self.pending_meta.remove(&seg.path)
            }
            Some(meta) => Some(meta.clone()),
        };
        if let Some(meta) = meta {
            self.aired.push_back(meta);
            if self.aired.len() > AIRED_KEEP {
                self.aired.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AddedBy, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};
    use crate::i18n::Lang;
    use crate::pipeline::monitor::{MonitorOptions, MonitorWatch, RETAIN_MS};
    use crate::stats::StreamMode;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::oneshot;

    /// The tests' wall clock at their start (Unix ms).
    const WALL0: u64 = 1_800_000_000_000;

    fn info(path: &str, index: usize) -> SegmentInfo {
        SegmentInfo {
            path: PathBuf::from(path),
            duration: 4.0,
            index,
        }
    }

    fn slates() -> Slates {
        Slates {
            idle: vec![info("/slates/idle/0.ts", 0), info("/slates/idle/1.ts", 1)],
            buffering: vec![info("/slates/buf/0.ts", 0)],
            pause: vec![info("/slates/pause/0.ts", 0), info("/slates/pause/1.ts", 1)],
            generation: 0,
            fallback: vec![info("/slates/fallback/0.ts", 0)],
            japanese_font_missing: false,
            font_missing: None,
        }
    }

    struct FakeState {
        state: SegmenterState,
        error: Option<Msg>,
        killed: bool,
        disposed: bool,
        pending: Vec<SegmentInfo>,
        n: usize,
        item_id: i64,
        start_at_s: f64,
        ticker_slow: bool,
        ticker_code: Option<String>,
    }

    /// Test-side view of a fake handle (shared with the handle the channel owns).
    #[derive(Clone)]
    struct Fake(Arc<Mutex<FakeState>>);

    impl Fake {
        fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.0.lock().unwrap()
        }
        fn produce(&self, count: usize) {
            let mut s = self.lock();
            for _ in 0..count {
                let path = format!("/clips/{}/{}-{}.ts", s.item_id, s.start_at_s, s.n);
                let index = s.n;
                s.pending.push(info(&path, index));
                s.n += 1;
            }
        }
        fn set_state(&self, state: SegmenterState) {
            self.lock().state = state;
        }
        fn set_error(&self, error: &str) {
            self.lock().error = Some(Msg::text(error));
        }
        fn killed(&self) -> bool {
            self.lock().killed
        }
        fn disposed(&self) -> bool {
            self.lock().disposed
        }
        fn item_id(&self) -> i64 {
            self.lock().item_id
        }
    }

    struct FakeHandle {
        fake: Fake,
        duration_s: Option<f64>,
        copies_video: bool,
    }

    impl ClipHandle for FakeHandle {
        fn state(&self) -> SegmenterState {
            self.fake.lock().state
        }
        fn error(&self) -> Option<Msg> {
            self.fake.lock().error.clone()
        }
        fn duration_s(&self) -> Option<f64> {
            self.duration_s
        }
        fn copies_video(&self) -> bool {
            self.copies_video
        }
        fn ticker_slow(&self) -> bool {
            self.fake.lock().ticker_slow
        }
        fn ticker_code(&self) -> Option<String> {
            self.fake.lock().ticker_code.clone()
        }
        fn poll(&mut self) -> Vec<SegmentInfo> {
            std::mem::take(&mut self.fake.lock().pending)
        }
        fn kill(&mut self) {
            let mut s = self.fake.lock();
            s.killed = true;
            if s.state == SegmenterState::Running {
                s.state = SegmenterState::Killed;
            }
        }
        fn stats(&self) -> ClipStats {
            ClipStats {
                mode: Some(if self.copies_video {
                    StreamMode::Copy
                } else {
                    StreamMode::Transcode
                }),
                pid: Some(4321),
                speed: Some(1.5),
                ticker_slow: self.fake.lock().ticker_slow,
                ..ClipStats::default()
            }
        }
        fn dispose(&mut self) {
            self.kill();
            self.fake.lock().disposed = true;
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Call {
        item_id: i64,
        start_at_s: f64,
        attempt: u32,
    }

    type CreateHook = Box<dyn Fn(&QueueItem) + Send>;

    #[derive(Default)]
    struct FactoryState {
        handles: Vec<Fake>,
        calls: Vec<Call>,
        fail_next: u32,
        /// The error of those failing starts (a timeout when unset).
        fail_with: Option<Msg>,
        /// When set, every create() waits until its gate is released.
        manual: bool,
        gates: Vec<oneshot::Sender<()>>,
        /// Runs inside create(), i.e. between the channel reading the queue head and starting it.
        on_create: Option<CreateHook>,
        /// Clips started from now on copy the video (instead of converting it).
        copies_video: bool,
        /// Items started ahead of their turn (`create_ahead`).
        ahead: Vec<i64>,
        /// Countdowns made: the film and the seconds.
        countdowns: Vec<(i64, f64)>,
        /// Films resolved ahead (`prepare`).
        prepared: Vec<i64>,
    }

    #[derive(Clone, Default)]
    struct FakeFactory(Arc<Mutex<FactoryState>>);

    impl FakeFactory {
        fn lock(&self) -> std::sync::MutexGuard<'_, FactoryState> {
            self.0.lock().unwrap()
        }
        fn handle(&self, i: usize) -> Fake {
            self.lock().handles[i].clone()
        }
        fn calls(&self) -> Vec<Call> {
            self.lock().calls.clone()
        }
        fn last_call_item(&self) -> i64 {
            self.lock().calls.last().unwrap().item_id
        }
    }

    impl ClipFactory for FakeFactory {
        fn create_ahead(
            &self,
            item: QueueItem,
            start_at_s: f64,
        ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
            self.lock().ahead.push(item.id);
            self.create(item, start_at_s, 0)
        }

        fn prepare(&self, item: QueueItem) -> BoxFuture<'static, Result<(), Msg>> {
            self.lock().prepared.push(item.id);
            Box::pin(async { Ok(()) })
        }

        /// A fake clip at "position -1" (its segment paths differ from the film's).
        fn create_countdown(
            &self,
            item: QueueItem,
            seconds: f64,
        ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
            self.lock().countdowns.push((item.id, seconds));
            let start = self.create(item, -1.0, 0);
            self.lock().calls.pop();
            start
        }

        fn create(
            &self,
            item: QueueItem,
            start_at_s: f64,
            attempt: u32,
        ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
            let mut s = self.lock();
            s.calls.push(Call {
                item_id: item.id,
                start_at_s,
                attempt,
            });
            if let Some(hook) = &s.on_create {
                hook(&item);
            }
            if s.fail_next > 0 {
                s.fail_next -= 1;
                let error = s.fail_with.clone().unwrap_or_else(|| {
                    Msg::error("process_timeout")
                        .arg("seconds", 30)
                        .arg("command", "yt-dlp")
                });
                return Box::pin(async { Err(error) });
            }
            let gate = s.manual.then(|| {
                let (tx, rx) = oneshot::channel();
                s.gates.push(tx);
                rx
            });
            let copies_video = s.copies_video;
            let shared = Arc::clone(&self.0);
            Box::pin(async move {
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                let fake = Fake(Arc::new(Mutex::new(FakeState {
                    state: SegmenterState::Running,
                    error: None,
                    killed: false,
                    disposed: false,
                    pending: Vec::new(),
                    n: 0,
                    item_id: item.id,
                    start_at_s,
                    ticker_slow: false,
                    ticker_code: None,
                })));
                shared.lock().unwrap().handles.push(fake.clone());
                Ok(Box::new(FakeHandle {
                    fake,
                    duration_s: item.duration_s,
                    copies_video,
                }) as Box<dyn ClipHandle>)
            })
        }
    }

    struct Env {
        db: Arc<Mutex<Connection>>,
        factory: FakeFactory,
        now: Arc<AtomicU64>,
        removed: Arc<Mutex<Vec<PathBuf>>>,
        logs: Arc<Mutex<Vec<String>>>,
        channel: Channel,
    }

    fn channel_for(
        db: &Arc<Mutex<Connection>>,
        factory: &FakeFactory,
        now: &Arc<AtomicU64>,
        removed: &Arc<Mutex<Vec<PathBuf>>>,
        logs: &Arc<Mutex<Vec<String>>>,
    ) -> Channel {
        let mut o = ChannelOptions::new(
            MAIN_CHANNEL_ID,
            Arc::clone(db),
            Arc::new(factory.clone()),
            slates(),
        );
        let clock = Arc::clone(now);
        o.now = Some(Box::new(move || clock.load(Ordering::SeqCst)));
        let wall = Arc::clone(now);
        o.wall_now = Some(Box::new(move || WALL0 + wall.load(Ordering::SeqCst)));
        let removed = Arc::clone(removed);
        o.remove_file = Some(Box::new(move |p: &Path| {
            removed.lock().unwrap().push(p.to_path_buf())
        }));
        let logs = Arc::clone(logs);
        o.log = Some(Box::new(move |m: Msg| {
            logs.lock().unwrap().push(m.to_string())
        }));
        Channel::new(o)
    }

    fn setup() -> Env {
        let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
        let factory = FakeFactory::default();
        let now = Arc::new(AtomicU64::new(0));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let logs = Arc::new(Mutex::new(Vec::new()));
        let mut channel = channel_for(&db, &factory, &now, &removed, &logs);
        channel.start().unwrap();
        Env {
            db,
            factory,
            now,
            removed,
            logs,
            channel,
        }
    }

    impl Env {
        fn add(&self, title: &str, duration_s: Option<f64>, is_live: bool) -> QueueItem {
            let conn = self.db.lock().unwrap();
            QueueRepo::new(&conn)
                .add(&NewItem {
                    channel_id: MAIN_CHANNEL_ID,
                    source_url: format!("https://youtu.be/{title}"),
                    title: title.into(),
                    duration_s,
                    is_live,
                    status: ItemStatus::Queued,
                    added_by: AddedBy::Admin,
                    requester_name: Some("Ola".into()),
                    requester_ip: None,
                    requester_key_hash: Some("ab".repeat(32)),
                })
                .unwrap()
        }
        fn add_item(&self, title: &str) -> QueueItem {
            self.add(title, Some(100.0), false)
        }
        fn get(&self, id: i64) -> QueueItem {
            let conn = self.db.lock().unwrap();
            QueueRepo::new(&conn).get(id).unwrap().unwrap()
        }
        fn set_status(&self, id: i64, status: ItemStatus) {
            let conn = self.db.lock().unwrap();
            QueueRepo::new(&conn).set_status(id, status, None).unwrap();
        }
        fn next(&mut self) -> SourceSegment {
            self.channel.next().unwrap()
        }
        fn key(&mut self) -> String {
            self.next().source_key
        }
        fn advance(&self, ms: u64) {
            self.now.fetch_add(ms, Ordering::SeqCst);
        }
        /// `next()` followed by the broadcaster's `on_published`, so `state()` sees it.
        fn publish(&mut self) -> SourceSegment {
            let s = self.next();
            self.channel.on_published(&PublishedSegment {
                path: s.path.clone(),
                duration: s.duration,
                source_key: s.source_key.clone(),
                index: s.index,
                seq: 0,
                channel_time: 0.0,
            });
            s
        }
        /// Starts `a` and airs `segments` clip segments (4 s each) of it.
        async fn air(&mut self, item: &QueueItem, segments: usize) -> Fake {
            self.next();
            self.flush().await;
            let last = self.factory.lock().handles.len() - 1;
            let h = self.factory.handle(last);
            assert_eq!(h.item_id(), item.id);
            h.produce(segments);
            for _ in 0..segments {
                assert_eq!(self.key(), clip_key(item, 0));
            }
            h
        }
        /// Lets the spawned clip starts run, then hands their results to the channel.
        async fn flush(&mut self) {
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            self.channel.poll_launches();
        }
        /// One loop tick of the engine (runs a seek whose 700 ms of quiet have passed).
        fn tick(&mut self) {
            self.channel.on_tick();
        }
        fn control(&self) -> ControlState {
            self.channel.state().control
        }
        fn starts(&self) -> usize {
            self.factory.calls().len()
        }
        /// Releases the `i`-th held clip start (manual factory).
        fn release(&self, i: usize) {
            let gate = std::mem::replace(&mut self.factory.lock().gates[i], oneshot::channel().0);
            gate.send(()).unwrap();
        }
        /// Releases every held start; a cancelled one is gone already.
        fn release_all(&self) {
            for gate in std::mem::take(&mut self.factory.lock().gates) {
                let _ = gate.send(());
            }
        }
        /// Lets spawned starts run without handing their results to the channel.
        async fn yield_only(&self) {
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        }
        fn live_handles(&self) -> usize {
            let handles = self.factory.lock().handles.clone();
            handles.iter().filter(|h| !h.disposed()).count()
        }
    }

    fn clip_key(item: &QueueItem, attempt: u32) -> String {
        format!("clip-{}-{attempt}", item.id)
    }

    #[tokio::test]
    async fn empty_queue_loops_the_idle_slate() {
        let mut e = setup();
        let (a, b, c) = (e.next(), e.next(), e.next());
        assert_eq!(
            [a.source_key, b.source_key, c.source_key],
            ["idle", "idle", "idle"]
        );
        assert_eq!([a.index, b.index, c.index], [0, 1, 0]);
        assert_eq!(e.channel.state().status, ChannelStatus::Idle);
    }

    #[tokio::test]
    async fn new_film_buffers_first_then_airs_clip_segments_and_is_playing() {
        let mut e = setup();
        let item = e.add_item("a");
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        e.factory.handle(0).produce(2);
        let s1 = e.next();
        assert_eq!(s1.source_key, clip_key(&item, 0));
        assert_eq!(s1.index, 0);
        assert_eq!(e.next().index, 1);
        assert_eq!(e.get(item.id).status, ItemStatus::Playing);
    }

    #[tokio::test]
    async fn end_of_film_marks_played_and_shows_idle_slate() {
        let mut e = setup();
        let item = e.add_item("a");
        e.next();
        e.flush().await;
        let h = e.factory.handle(0);
        h.produce(1);
        e.next();
        h.set_state(SegmenterState::Done);
        assert_eq!(e.key(), "idle");
        assert_eq!(e.get(item.id).status, ItemStatus::Played);
        assert!(h.disposed());
    }

    #[tokio::test]
    async fn ffmpeg_error_retries_once_from_the_aired_position_then_fails() {
        let mut e = setup();
        let item = e.add_item("a");
        e.next();
        e.flush().await;
        let h0 = e.factory.handle(0);
        h0.produce(3);
        e.next();
        e.next();
        e.next();
        h0.set_state(SegmenterState::Failed);
        h0.set_error("Connection reset");
        // Failed on air: "Technical problem" while it is started again.
        assert_eq!(e.key(), "fallback");
        assert_eq!(e.key(), "fallback");
        e.flush().await;
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: item.id,
                start_at_s: 12.0,
                attempt: 1
            }
        );
        let h1 = e.factory.handle(1);
        h1.produce(1);
        assert_eq!(e.key(), clip_key(&item, 1));
        h1.set_state(SegmenterState::Failed);
        h1.set_error("Connection reset");
        assert_eq!(e.key(), "idle");
        let fresh = e.get(item.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert!(
            fresh
                .error_text(Lang::En)
                .unwrap()
                .contains("Connection reset")
        );
    }

    /// The address of a viewer's film is on the home network: it fails at once, no second try.
    #[tokio::test]
    async fn a_refused_address_fails_the_film_without_a_retry() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        {
            let mut f = e.factory.lock();
            f.fail_next = 1;
            f.fail_with = Some(Msg::error("local_address"));
        }
        e.next();
        e.flush().await;
        e.next();
        e.flush().await;
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert_eq!(
            fresh.error_text(Lang::Pl).unwrap(),
            "Linki do sieci lokalnej lub do tego komputera są niedozwolone."
        );
        let attempts_of_a = e
            .factory
            .calls()
            .iter()
            .filter(|c| c.item_id == a.id)
            .count();
        assert_eq!(attempts_of_a, 1, "no retry");
        assert!(e.factory.calls().iter().any(|c| c.item_id == b.id));
    }

    #[tokio::test]
    async fn failed_resolve_twice_marks_failed_and_moves_to_next_film() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.factory.lock().fail_next = 2;
        e.next();
        e.flush().await;
        e.next();
        e.flush().await;
        e.next();
        e.flush().await;
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert!(
            fresh
                .error_text(Lang::En)
                .unwrap()
                .contains("Time limit exceeded (30 s): yt-dlp")
        );
        assert_eq!(
            fresh.error_text(Lang::Pl).unwrap(),
            "Przekroczono limit czasu (30 s): yt-dlp"
        );
        // Kept as a message (key and fields), not as the text of one language.
        let raw = fresh.error.unwrap();
        assert!(raw.starts_with(crate::i18n::STORED_PREFIX), "{raw:?}");
        assert!(raw.contains("errors.process_timeout"), "{raw:?}");
        assert!(
            !raw.contains("Przekroczono") && !raw.contains("Time limit"),
            "{raw:?}"
        );
        assert_eq!(e.factory.last_call_item(), b.id);
    }

    #[tokio::test]
    async fn no_progress_for_30s_fails_the_film() {
        let mut e = setup();
        let item = e.add_item("a");
        e.next();
        e.flush().await;
        let h = e.factory.handle(0);
        h.produce(1);
        e.next();
        assert_eq!(e.key(), "buffering");
        e.advance(29_000);
        assert_eq!(e.key(), "buffering");
        e.advance(2_000);
        assert_eq!(e.key(), "idle");
        let fresh = e.get(item.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert_eq!(
            fresh.error_text(Lang::Pl).as_deref(),
            Some("Źródło nie dostarczało danych przez 30 s")
        );
        assert!(h.killed());
    }

    #[tokio::test]
    async fn prefetch_starts_the_next_film_60s_before_the_end_and_airs_it_without_buffering() {
        let mut e = setup();
        let a = e.add("a", Some(80.0), false);
        let b = e.add("b", Some(100.0), false);
        e.next();
        e.flush().await;
        let ha = e.factory.handle(0);
        ha.produce(20);
        for _ in 0..4 {
            e.next(); // 16 s
        }
        assert_eq!(e.factory.calls().len(), 1);
        e.next(); // 20 s, 60 s left
        e.flush().await;
        let ids: Vec<i64> = e.factory.calls().iter().map(|c| c.item_id).collect();
        assert_eq!(ids, [a.id, b.id]);
        assert_eq!(e.channel.prefetched_item_id(), Some(b.id));
        e.factory.handle(1).produce(1);
        for _ in 0..15 {
            e.next();
        }
        ha.set_state(SegmenterState::Done);
        let s = e.next();
        assert_eq!(s.source_key, clip_key(&b, 0));
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
        assert_eq!(e.get(b.id).status, ItemStatus::Playing);
        assert_eq!(e.channel.prefetched_item_id(), None);
    }

    #[tokio::test]
    async fn stats_describe_the_film_being_cut_the_next_one_and_the_process() {
        let mut e = setup();
        assert_eq!(e.channel.stats(), ChannelStats::default());
        e.add("a", Some(40.0), false);
        let b = e.add_item("b");
        e.next();
        e.flush().await;
        e.factory.handle(0).produce(10);
        e.next();
        e.flush().await;
        assert_eq!(e.factory.handle(1).item_id(), b.id);

        let stats = e.channel.stats();
        let on_air = stats.on_air.expect("a film is on air");
        assert_eq!(on_air.title, "a");
        assert_eq!(on_air.attempt, 0);
        assert_eq!(on_air.mode, Some(StreamMode::Transcode));
        assert!(on_air.position_s >= 0.0);
        let next = stats.next.expect("the next film is prepared");
        assert_eq!(next.title, "b");
        assert_eq!(next.state, NextState::Ready);
        assert_eq!(
            stats.ffmpeg,
            Some(FfmpegInfo {
                pid: 4321,
                speed: Some(1.5),
                cpu_percent: None,
                rss_bytes: None
            })
        );
        // The counters of the playlist belong to the broadcaster.
        assert_eq!(stats.hls, None);

        // A failing next film shows as failed.
        e.factory.handle(1).set_state(SegmenterState::Failed);
        assert_eq!(e.channel.stats().next.unwrap().state, NextState::Failed);
    }

    #[tokio::test]
    async fn reorder_kills_the_prefetched_clip_and_the_new_head_plays() {
        let mut e = setup();
        e.add("a", Some(40.0), false);
        let b = e.add_item("b");
        let c = e.add_item("c");
        e.next();
        e.flush().await;
        e.factory.handle(0).produce(10);
        e.next();
        e.flush().await;
        let prefetched = e.factory.handle(1);
        assert_eq!(prefetched.item_id(), b.id);
        {
            let conn = e.db.lock().unwrap();
            QueueRepo::new(&conn).move_item(c.id, 0).unwrap();
        }
        e.channel.on_queue_changed().unwrap();
        assert!(prefetched.disposed());
        for _ in 0..9 {
            e.next();
        }
        e.factory.handle(0).set_state(SegmenterState::Done);
        e.next();
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), c.id);
    }

    #[tokio::test]
    async fn removing_the_current_film_stops_it() {
        let mut e = setup();
        let a = e.add_item("a");
        e.next();
        e.flush().await;
        e.factory.handle(0).produce(5);
        e.next();
        e.set_status(a.id, ItemStatus::Removed);
        e.channel.on_queue_changed().unwrap();
        assert!(e.factory.handle(0).disposed());
        assert_eq!(e.key(), "idle");
    }

    #[tokio::test]
    async fn skip_ends_the_current_film_and_moves_on() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.next();
        e.flush().await;
        e.factory.handle(0).produce(5);
        e.next();
        e.channel.skip().unwrap();
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Played);
        assert!(
            fresh
                .error_text(Lang::En)
                .unwrap()
                .to_lowercase()
                .contains("skipped")
        );
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
    }

    #[tokio::test]
    async fn live_stream_does_not_prefetch() {
        let mut e = setup();
        e.add("live", None, true);
        e.add_item("b");
        e.next();
        e.flush().await;
        e.factory.handle(0).produce(30);
        for _ in 0..30 {
            e.next();
        }
        e.flush().await;
        assert_eq!(e.factory.calls().len(), 1);
    }

    #[tokio::test]
    async fn state_shows_what_viewers_see_three_segments_behind() {
        let mut e = setup();
        let a = e.add("a", Some(100.0), false);
        fn publish(e: &mut Env) {
            let s = e.next();
            e.channel.on_published(&PublishedSegment {
                path: s.path,
                duration: s.duration,
                source_key: s.source_key,
                index: s.index,
                seq: 0,
                channel_time: 0.0,
            });
        }
        publish(&mut e); // buffering
        e.flush().await;
        e.factory.handle(0).produce(10);
        for _ in 0..5 {
            publish(&mut e);
        }
        let st = e.channel.state();
        assert_eq!(st.status, ChannelStatus::Playing);
        let cur = st.current.unwrap();
        assert_eq!(cur.item_id, a.id);
        assert_eq!(cur.title, "a");
        assert_eq!(cur.duration_s, Some(100.0));
        assert_eq!(cur.requester_name.as_deref(), Some("Ola"));
        assert_eq!(cur.position_s, 8);
        assert!(!cur.is_live);
    }

    #[tokio::test]
    async fn restart_puts_the_interrupted_film_back_at_the_front() {
        let e = setup();
        let a = e.add_item("a");
        e.add_item("b");
        e.set_status(a.id, ItemStatus::Playing);
        let mut c2 = channel_for(&e.db, &e.factory, &e.now, &e.removed, &e.logs);
        c2.start().unwrap();
        let conn = e.db.lock().unwrap();
        let head = QueueRepo::new(&conn).next_queued(MAIN_CHANNEL_ID).unwrap();
        assert_eq!(head.unwrap().id, a.id);
    }

    // ---------- Beyond the TS suite ----------

    #[tokio::test]
    async fn a_clip_that_starts_after_being_cancelled_is_disposed() {
        let mut e = setup();
        e.factory.lock().manual = true;
        let a = e.add_item("a");
        assert_eq!(e.key(), "buffering");
        e.channel.skip().unwrap();
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
        let gate = e.factory.lock().gates.remove(0);
        gate.send(()).unwrap();
        e.flush().await;
        assert!(e.factory.handle(0).disposed());
        assert_eq!(e.key(), "idle");
    }

    #[tokio::test]
    async fn start_timeout_retries_once_then_fails() {
        let mut e = setup();
        e.factory.lock().manual = true;
        let a = e.add_item("a");
        assert_eq!(e.key(), "buffering");
        e.advance(120_000);
        assert_eq!(e.key(), "buffering");
        assert_eq!(e.factory.calls().len(), 1);
        e.advance(1_000);
        assert_eq!(e.key(), "buffering");
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: a.id,
                start_at_s: 0.0,
                attempt: 1
            }
        );
        e.advance(121_000);
        assert_eq!(e.key(), "idle");
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert_eq!(
            fresh.error_text(Lang::Pl).as_deref(),
            Some("Przekroczono czas uruchamiania źródła")
        );
    }

    #[tokio::test]
    async fn live_retry_restarts_from_zero() {
        let mut e = setup();
        let live = e.add("live", None, true);
        e.next();
        e.flush().await;
        let h = e.factory.handle(0);
        h.produce(3);
        for _ in 0..3 {
            e.next();
        }
        h.set_state(SegmenterState::Failed);
        assert_eq!(e.key(), "fallback");
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: live.id,
                start_at_s: 0.0,
                attempt: 1
            }
        );
    }

    #[tokio::test]
    async fn the_previous_clip_segment_is_removed_on_the_next_call_but_slates_are_not() {
        let mut e = setup();
        e.add_item("a");
        let slate = e.next();
        e.flush().await;
        e.factory.handle(0).produce(2);
        let s1 = e.next();
        assert!(e.removed.lock().unwrap().is_empty());
        let s2 = e.next();
        assert_eq!(*e.removed.lock().unwrap(), std::slice::from_ref(&s1.path));
        e.next(); // buffering slate
        assert_eq!(*e.removed.lock().unwrap(), [s1.path, s2.path]);
        e.next();
        assert!(!e.removed.lock().unwrap().contains(&slate.path));
        assert_eq!(e.removed.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn stop_disposes_the_current_and_prefetched_clips() {
        let mut e = setup();
        e.add("a", Some(40.0), false);
        e.add_item("b");
        e.next();
        e.flush().await;
        e.factory.handle(0).produce(2);
        e.next();
        e.flush().await;
        assert!(e.channel.prefetched_item_id().is_some());
        e.channel.stop();
        assert!(e.factory.handle(0).disposed());
        assert!(e.factory.handle(1).disposed());
        assert_eq!(e.channel.prefetched_item_id(), None);
    }

    /// An owner's DELETE that lands between the channel reading the queue head and marking it
    /// playing is not overwritten: the launched clip is dropped and the next film is picked.
    #[tokio::test]
    async fn a_removal_while_the_head_is_being_started_wins_and_the_next_film_is_picked() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        let db = Arc::clone(&e.db);
        let target = a.id;
        e.factory.lock().on_create = Some(Box::new(move |item: &QueueItem| {
            if item.id == target {
                let conn = db.lock().unwrap();
                QueueRepo::new(&conn)
                    .set_status(target, ItemStatus::Removed, None)
                    .unwrap();
            }
        }));
        assert_eq!(e.key(), "buffering");
        assert_eq!(e.get(a.id).status, ItemStatus::Removed);
        assert_eq!(e.get(b.id).status, ItemStatus::Playing);
        let ids: Vec<i64> = e.factory.calls().iter().map(|c| c.item_id).collect();
        assert_eq!(ids, [a.id, b.id]);
        e.flush().await;
        // The clip started for the removed film is disposed once its start completes.
        let handles: Vec<Fake> = e.factory.lock().handles.clone();
        let for_a = handles.iter().find(|h| h.item_id() == a.id).unwrap();
        assert!(for_a.disposed());
        let for_b = handles.iter().find(|h| h.item_id() == b.id).unwrap();
        assert!(!for_b.disposed());
        for_b.produce(1);
        assert_eq!(e.key(), clip_key(&b, 0));
    }

    /// A removal the channel has not been told about yet is not turned into "played" by a skip
    /// or the end of the film.
    #[tokio::test]
    async fn ending_a_film_does_not_overwrite_its_removal() {
        let mut e = setup();
        let a = e.add_item("a");
        e.next();
        e.flush().await;
        e.factory.handle(0).produce(2);
        e.next();
        e.set_status(a.id, ItemStatus::Removed);
        e.channel.skip().unwrap();
        assert_eq!(e.get(a.id).status, ItemStatus::Removed);
        assert_eq!(e.get(a.id).error, None);

        let b = e.add_item("b");
        e.next();
        e.flush().await;
        let hb = e.factory.handle(1);
        hb.produce(1);
        assert_eq!(e.key(), clip_key(&b, 0));
        e.set_status(b.id, ItemStatus::Removed);
        hb.set_state(SegmenterState::Done);
        assert_eq!(e.key(), "idle");
        assert_eq!(e.get(b.id).status, ItemStatus::Removed);
    }

    #[tokio::test]
    async fn current_item_id_is_the_film_being_aired_and_fail_item_marks_it_failed() {
        let mut e = setup();
        assert_eq!(e.channel.current_item_id(), None);
        let a = e.add_item("a");
        e.next();
        assert_eq!(e.channel.current_item_id(), Some(a.id));
        e.channel
            .fail_item(a.id, &Msg::new("item_errors.internal"))
            .unwrap();
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert_eq!(
            fresh.error_text(Lang::En).as_deref(),
            Some("Internal error")
        );
        assert!(fresh.finished_at.is_some());
        // A removed item stays removed.
        let b = e.add_item("b");
        e.set_status(b.id, ItemStatus::Removed);
        e.channel
            .fail_item(b.id, &Msg::new("item_errors.internal"))
            .unwrap();
        assert_eq!(e.get(b.id).status, ItemStatus::Removed);
    }

    #[tokio::test]
    async fn buffering_slate_index_restarts_when_switching_from_idle() {
        let mut e = setup();
        assert_eq!(e.next().index, 0);
        assert_eq!(e.next().index, 1);
        e.add_item("a");
        let s = e.next();
        assert_eq!((s.source_key.as_str(), s.index), ("buffering", 0));
        e.channel.skip().unwrap();
        let s = e.next();
        assert_eq!((s.source_key.as_str(), s.index), ("idle", 0));
    }

    // ---------- Slate replacement ----------

    fn replacement_slates(generation: u32) -> Slates {
        let dir = format!("/slates/g{generation}");
        Slates {
            idle: vec![info(&format!("{dir}/idle/0.ts"), 0)],
            buffering: vec![info(&format!("{dir}/buf/0.ts"), 0)],
            pause: vec![
                info(&format!("{dir}/pause/0.ts"), 0),
                info(&format!("{dir}/pause/1.ts"), 1),
            ],
            generation,
            fallback: Vec::new(),
            japanese_font_missing: false,
            font_missing: None,
        }
    }

    #[tokio::test]
    async fn replaced_slates_air_from_the_new_set_with_a_new_source_key() {
        let mut e = setup();
        assert_eq!(e.next().index, 0);
        let before = e.next();
        assert_eq!((before.source_key.as_str(), before.index), ("idle", 1));
        e.channel.replace_slates(replacement_slates(1));
        let after = e.next();
        assert_eq!(after.source_key, "idle-g1");
        assert_eq!(after.index, 0, "the new set starts from its first segment");
        assert_eq!(after.path, PathBuf::from("/slates/g1/idle/0.ts"));
        assert_eq!(e.next().source_key, "idle-g1");
        // Another replacement: yet another key.
        e.channel.replace_slates(replacement_slates(2));
        assert_eq!(e.next().source_key, "idle-g2");
    }

    #[tokio::test]
    async fn replacing_slates_does_not_touch_the_airing_film() {
        let mut e = setup();
        let a = e.add_item("a");
        let h = e.air(&a, 2).await;
        e.channel.replace_slates(replacement_slates(1));
        assert!(!h.disposed());
        assert_eq!(e.channel.current_item_id(), Some(a.id));
        h.produce(1);
        assert_eq!(e.key(), clip_key(&a, 0));
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
    }

    #[tokio::test]
    async fn the_pause_slate_survives_a_replacement_and_resume_still_works() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.pause(PauseReason::Tunnel);
        assert_eq!(e.key(), "pause");
        e.channel.replace_slates(replacement_slates(1));
        let s = e.next();
        assert_eq!((s.source_key.as_str(), s.index), ("pause-g1", 0));
        assert_eq!(s.path, PathBuf::from("/slates/g1/pause/0.ts"));
        assert_eq!(e.next().index, 1);
        // The paused film is remembered across the replacement and resumes from 12 s.
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "buffering-g1");
        e.flush().await;
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: a.id,
                start_at_s: 12.0,
                attempt: 0
            }
        );
        e.factory.handle(1).produce(2);
        assert_eq!(e.key(), format!("{}-r1", clip_key(&a, 0)));
    }

    // ---------- Pause and resume ----------

    #[tokio::test]
    async fn pause_remembers_the_position_stops_the_clip_and_airs_pause_slates() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await; // 12 s aired
        e.channel.pause(PauseReason::Tunnel);
        assert!(h0.disposed());
        let (s1, s2, s3) = (e.next(), e.next(), e.next());
        assert_eq!(
            [&s1.source_key, &s2.source_key, &s3.source_key],
            ["pause", "pause", "pause"]
        );
        assert_eq!([s1.index, s2.index, s3.index], [0, 1, 0]);
        // The database keeps the film as playing, and no other film was started.
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
        assert_eq!(e.factory.calls().len(), 1);
    }

    #[tokio::test]
    async fn resume_relaunches_the_same_film_from_the_remembered_position() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.pause(PauseReason::Tunnel);
        assert_eq!(e.key(), "pause");
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: a.id,
                start_at_s: 12.0,
                attempt: 0
            }
        );
        e.factory.handle(1).produce(2);
        assert_eq!(e.key(), format!("{}-r1", clip_key(&a, 0)));
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
        assert_eq!(e.channel.current_item_id(), Some(a.id));
    }

    #[tokio::test]
    async fn state_shows_the_paused_film_with_its_position_and_the_reasons() {
        let mut e = setup();
        let a = e.add_item("a");
        e.next();
        e.flush().await;
        e.factory.handle(0).produce(3);
        for _ in 0..3 {
            e.publish();
        }
        e.channel.pause(PauseReason::Tunnel);
        e.channel.pause(PauseReason::NoViewers);
        for _ in 0..4 {
            e.publish();
        }
        let st = e.channel.state();
        assert_eq!(st.status, ChannelStatus::Paused);
        assert!(st.paused);
        assert_eq!(
            st.pause_reasons,
            [PauseReason::Tunnel, PauseReason::NoViewers]
        );
        let cur = st.current.unwrap();
        assert_eq!((cur.item_id, cur.position_s), (a.id, 12));
        e.channel.resume(PauseReason::Tunnel);
        e.channel.resume(PauseReason::NoViewers);
        assert!(!e.channel.state().paused);
        assert!(e.channel.state().pause_reasons.is_empty());
    }

    #[tokio::test]
    async fn with_two_reasons_the_film_resumes_only_after_both_are_cleared() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 2).await;
        e.channel.pause(PauseReason::Tunnel);
        e.channel.pause(PauseReason::NoViewers);
        // The same reason twice is one reason.
        e.channel.pause(PauseReason::NoViewers);
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "pause");
        e.flush().await;
        assert_eq!(e.factory.calls().len(), 1);
        e.channel.resume(PauseReason::NoViewers);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.calls().len(), 2);
        assert_eq!(e.factory.calls()[1].start_at_s, 8.0);
    }

    #[tokio::test]
    async fn resuming_without_a_pause_does_nothing() {
        let mut e = setup();
        let a = e.add_item("a");
        let h = e.air(&a, 2).await;
        e.channel.resume(PauseReason::Tunnel);
        assert!(!h.disposed());
        e.flush().await;
        assert_eq!(e.factory.calls().len(), 1);
    }

    #[tokio::test]
    async fn a_live_stream_keeps_airing_through_a_pause_and_the_next_film_waits() {
        let mut e = setup();
        let live = e.add("live", None, true);
        let b = e.add_item("b");
        let h = e.air(&live, 2).await;
        e.channel.pause(PauseReason::NoViewers);
        assert!(!h.disposed());
        assert!(!e.channel.state().paused);
        h.produce(1);
        assert_eq!(e.key(), clip_key(&live, 0));
        // The stream ends: the queued film is held back behind the pause slate.
        h.set_state(SegmenterState::Done);
        assert_eq!(e.key(), "pause");
        assert_eq!(e.get(live.id).status, ItemStatus::Played);
        assert_eq!(e.get(b.id).status, ItemStatus::Queued);
        assert!(e.channel.state().paused);
        e.flush().await;
        assert_eq!(e.factory.calls().len(), 1);
        e.channel.resume(PauseReason::NoViewers);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
        assert_eq!(e.get(b.id).status, ItemStatus::Playing);
    }

    #[tokio::test]
    async fn pausing_an_empty_queue_keeps_the_idle_slate_and_holds_back_the_next_start() {
        let mut e = setup();
        e.channel.pause(PauseReason::Tunnel);
        assert_eq!(e.key(), "idle");
        assert_eq!(e.channel.state().status, ChannelStatus::Idle);
        let a = e.add_item("a");
        // A film is waiting now: the pause slate tells viewers why nothing starts.
        assert_eq!(e.key(), "pause");
        e.flush().await;
        assert!(e.factory.calls().is_empty());
        assert_eq!(e.get(a.id).status, ItemStatus::Queued);
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(
            e.factory.calls(),
            [Call {
                item_id: a.id,
                start_at_s: 0.0,
                attempt: 0
            }]
        );
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
    }

    #[tokio::test]
    async fn skip_while_paused_plays_out_the_paused_film_and_stays_paused() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.air(&a, 2).await;
        e.channel.pause(PauseReason::Tunnel);
        e.channel.skip().unwrap();
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Played);
        assert_eq!(
            fresh.error_text(Lang::En).as_deref(),
            Some("Skipped by the owner")
        );
        assert_eq!(e.key(), "pause");
        e.flush().await;
        assert_eq!(e.get(b.id).status, ItemStatus::Queued);
        assert_eq!(e.factory.calls().len(), 1);
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: b.id,
                start_at_s: 0.0,
                attempt: 0
            }
        );
    }

    #[tokio::test]
    async fn pausing_disposes_the_prefetched_clip_and_resume_prefetches_again() {
        let mut e = setup();
        let a = e.add("a", Some(40.0), false);
        let b = e.add_item("b");
        e.air(&a, 1).await;
        e.factory.handle(0).produce(1);
        e.next();
        e.flush().await;
        let prefetched = e.factory.handle(1);
        assert_eq!(prefetched.item_id(), b.id);
        e.channel.pause(PauseReason::Tunnel);
        assert!(prefetched.disposed());
        assert_eq!(e.channel.prefetched_item_id(), None);
        e.channel.resume(PauseReason::Tunnel);
        e.next();
        e.flush().await;
        e.factory.handle(2).produce(1);
        e.next();
        e.flush().await;
        // Resumed `a` is within 60 s of its end again: `b` is prefetched anew.
        let ids: Vec<i64> = e.factory.calls().iter().map(|c| c.item_id).collect();
        assert_eq!(ids, [a.id, b.id, a.id, b.id]);
        assert_eq!(e.channel.prefetched_item_id(), Some(b.id));
    }

    #[tokio::test]
    async fn removing_the_paused_film_drops_it_and_the_next_one_starts_on_resume() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.air(&a, 2).await;
        e.channel.pause(PauseReason::Tunnel);
        e.set_status(a.id, ItemStatus::Removed);
        e.channel.on_queue_changed().unwrap();
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
        assert_eq!(e.factory.calls().len(), 2);
        assert_eq!(e.get(a.id).status, ItemStatus::Removed);
    }

    #[tokio::test]
    async fn pausing_a_film_that_is_still_starting_resumes_it_from_its_start_position() {
        let mut e = setup();
        e.factory.lock().manual = true;
        let a = e.add_item("a");
        assert_eq!(e.key(), "buffering");
        e.channel.pause(PauseReason::Tunnel);
        assert_eq!(e.key(), "pause");
        // The start that was in flight finishes after the pause: its clip is disposed.
        let gate = e.factory.lock().gates.remove(0);
        gate.send(()).unwrap();
        e.flush().await;
        assert!(e.factory.handle(0).disposed());
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "buffering");
        assert_eq!(e.factory.calls().len(), 2);
        assert_eq!(e.factory.calls()[1].item_id, a.id);
        assert_eq!(e.factory.calls()[1].start_at_s, 0.0);
    }

    /// Pause and resume landing between two `next()` calls must still change the source key,
    /// otherwise the broadcaster would not mark a discontinuity for the new encoder process.
    #[tokio::test]
    async fn a_resumed_clip_has_a_different_source_key_even_without_a_segment_in_between() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 2).await;
        h0.produce(1);
        let last = e.next();
        assert_eq!(last.source_key, clip_key(&a, 0));
        e.channel.pause(PauseReason::Tunnel);
        e.channel.resume(PauseReason::Tunnel);
        e.flush().await;
        e.factory.handle(1).produce(2);
        let first_after = e.next();
        assert!(first_after.source_key.starts_with("clip-"));
        assert_ne!(first_after.source_key, last.source_key);
        let second_after = e.next();
        assert_eq!(second_after.source_key, first_after.source_key);
        // A second pause and resume changes it again.
        e.channel.pause(PauseReason::Tunnel);
        e.channel.resume(PauseReason::Tunnel);
        e.flush().await;
        e.factory.handle(2).produce(1);
        let third = e.next();
        assert_ne!(third.source_key, first_after.source_key);
        assert_ne!(third.source_key, last.source_key);
    }

    #[tokio::test]
    async fn pausing_a_clip_that_already_finished_marks_the_film_played_instead_of_parking_it() {
        let mut e = setup();
        let a = e.add_item("a");
        let h = e.air(&a, 2).await;
        h.set_state(SegmenterState::Done);
        e.channel.pause(PauseReason::Tunnel);
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
        assert_eq!(e.key(), "idle");
        e.channel.resume(PauseReason::Tunnel);
        e.flush().await;
        assert_eq!(e.factory.calls().len(), 1, "a finished film was relaunched");
        assert_eq!(e.key(), "idle");
    }

    #[tokio::test]
    async fn pausing_a_clip_that_failed_goes_through_the_retry_and_resumes_the_retry() {
        let mut e = setup();
        let a = e.add_item("a");
        let h = e.air(&a, 2).await;
        h.set_state(SegmenterState::Failed);
        h.set_error("Connection reset");
        e.channel.pause(PauseReason::Tunnel);
        e.flush().await;
        assert!(h.disposed());
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: a.id,
                start_at_s: 8.0,
                attempt: 1
            }
        );
        assert_eq!(e.key(), "pause");
        e.channel.resume(PauseReason::Tunnel);
        e.flush().await;
        assert_eq!(
            e.factory.calls()[2],
            Call {
                item_id: a.id,
                start_at_s: 8.0,
                attempt: 1
            }
        );
    }

    #[tokio::test]
    async fn pausing_a_clip_that_failed_after_its_retry_marks_the_film_failed() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 2).await;
        h0.set_state(SegmenterState::Failed);
        assert_eq!(e.key(), "fallback");
        e.flush().await;
        let h1 = e.factory.handle(1);
        h1.produce(1);
        assert_eq!(e.key(), clip_key(&a, 1));
        h1.set_state(SegmenterState::Failed);
        h1.set_error("boom");
        e.channel.pause(PauseReason::Tunnel);
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert!(fresh.error_text(Lang::En).unwrap().contains("boom"));
        assert_eq!(e.key(), "idle");
        e.channel.resume(PauseReason::Tunnel);
        e.flush().await;
        assert_eq!(e.factory.calls().len(), 2);
    }

    #[tokio::test]
    async fn pausing_a_film_whose_start_failed_goes_through_the_retry() {
        let mut e = setup();
        let a = e.add_item("a");
        e.factory.lock().fail_next = 1;
        e.next();
        e.flush().await;
        e.channel.pause(PauseReason::Tunnel);
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
        assert_eq!(e.factory.calls()[1].attempt, 1);
        e.channel.resume(PauseReason::Tunnel);
        e.flush().await;
        assert_eq!(e.factory.calls().len(), 3);
        assert_eq!(e.factory.calls()[2].attempt, 1);
    }

    #[tokio::test]
    async fn stop_while_paused_leaves_nothing_running() {
        let mut e = setup();
        let a = e.add_item("a");
        let h = e.air(&a, 2).await;
        e.channel.pause(PauseReason::Tunnel);
        e.channel.stop();
        assert!(h.disposed());
        assert_eq!(e.factory.calls().len(), 1);
    }

    // ---------- Owner's controls (stage 4) ----------

    fn relaunched_key(item: &QueueItem, attempt: u32, n: u32) -> String {
        format!("{}-r{n}", clip_key(item, attempt))
    }

    #[tokio::test]
    async fn resuming_the_manual_pause_leaves_the_tunnel_pause_in_place() {
        assert_eq!(PauseReason::Manual.as_str(), "manual");
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.pause(PauseReason::Manual);
        e.channel.pause(PauseReason::Tunnel);
        assert_eq!(
            e.channel.state().pause_reasons,
            [PauseReason::Tunnel, PauseReason::Manual]
        );
        e.channel.resume(PauseReason::Manual);
        assert_eq!(e.key(), "pause");
        e.flush().await;
        assert_eq!(e.starts(), 1, "the film must stay paused");
        let st = e.channel.state();
        assert!(st.paused);
        assert_eq!(st.pause_reasons, [PauseReason::Tunnel]);
        assert_eq!(st.control.position_s, 12.0);
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: a.id,
                start_at_s: 12.0,
                attempt: 0
            }
        );
    }

    // ---------- Pause at the producer's position ("armed" pause) ----------

    #[tokio::test]
    async fn a_pause_ahead_airs_up_to_the_segment_boundary_then_pauses_there() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await; // 12 s aired
        e.channel.pause_at(17.0);
        let c = e.control();
        assert!(c.pause_pending);
        assert!(!e.channel.state().paused);
        assert!(e.channel.state().pause_reasons.is_empty());
        h0.produce(3);
        // 12–16 s and 16–20 s still air: 17 s is inside the second one.
        assert_eq!(e.key(), clip_key(&a, 0));
        assert!(e.control().pause_pending);
        assert_eq!(e.key(), clip_key(&a, 0));
        assert_eq!(e.key(), "pause");
        assert!(h0.disposed());
        let c = e.control();
        assert!(!c.pause_pending);
        assert_eq!(c.position_s, 20.0);
        assert_eq!(e.channel.state().pause_reasons, [PauseReason::Manual]);
        // The resume goes on from the boundary: nothing missed, nothing repeated.
        e.channel.resume(PauseReason::Manual);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.calls()[1].start_at_s, 20.0);
    }

    #[tokio::test]
    async fn a_pause_on_a_boundary_pauses_right_after_that_segment() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await;
        e.channel.pause_at(16.0);
        h0.produce(2);
        assert_eq!(e.key(), clip_key(&a, 0));
        assert_eq!(e.key(), "pause");
        assert_eq!(e.control().position_s, 16.0);
    }

    #[tokio::test]
    async fn a_pause_at_or_behind_what_has_aired_pauses_at_once() {
        for at in [12.0, 3.0, 0.0] {
            let mut e = setup();
            let a = e.add_item("a");
            let h0 = e.air(&a, 3).await;
            e.channel.pause_at(at);
            assert!(h0.disposed(), "at {at}");
            let c = e.control();
            assert!(!c.pause_pending);
            assert_eq!(c.position_s, 12.0);
            assert_eq!(e.key(), "pause");
        }
    }

    #[tokio::test]
    async fn pausing_again_while_a_pause_is_pending_changes_nothing() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await;
        e.channel.pause_at(21.0);
        e.channel.pause_at(13.0);
        e.channel.pause(PauseReason::Manual);
        assert!(!h0.disposed());
        assert!(e.control().pause_pending);
        h0.produce(4);
        for _ in 0..3 {
            assert_eq!(e.key(), clip_key(&a, 0));
        }
        assert_eq!(e.key(), "pause");
        assert_eq!(e.control().position_s, 24.0);
    }

    #[tokio::test]
    async fn a_skip_or_resume_cancels_the_pending_pause() {
        // Resume: the film simply goes on.
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await;
        e.channel.pause_at(17.0);
        e.channel.resume(PauseReason::Manual);
        assert!(!e.control().pause_pending);
        h0.produce(3);
        for _ in 0..3 {
            assert_eq!(e.key(), clip_key(&a, 0));
        }
        // A skip: the next film plays.
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.air(&a, 3).await;
        e.channel.pause_at(17.0);
        e.channel.skip().unwrap();
        assert!(!e.control().pause_pending);
        e.next();
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
        let last = e.factory.lock().handles.len() - 1;
        e.factory.handle(last).produce(1);
        assert_eq!(e.key(), clip_key(&b, 0));
        assert!(e.channel.state().pause_reasons.is_empty());
    }

    #[tokio::test]
    async fn a_seek_during_a_pending_pause_moves_where_the_film_resumes() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await; // 12 s aired
        e.channel.pause_at(17.0);
        e.channel.seek_to(50.0);
        e.channel.seek_by(10.0); // from the new paused position
        let c = e.control();
        assert!(c.pause_pending && !c.seek_pending);
        assert_eq!(c.position_s, 60.0);
        // Nothing is relaunched: the film airs on up to the pause boundary (20 s), as before.
        e.advance(800);
        e.tick();
        e.flush().await;
        assert!(!h0.disposed());
        assert_eq!(e.starts(), 1);
        h0.produce(3);
        assert_eq!(e.key(), clip_key(&a, 0));
        assert_eq!(e.key(), clip_key(&a, 0));
        assert_eq!(e.key(), "pause");
        let c = e.control();
        assert!(!c.pause_pending);
        assert_eq!(c.position_s, 60.0);
        assert_eq!(e.channel.state().pause_reasons, [PauseReason::Manual]);
        e.channel.resume(PauseReason::Manual);
        e.flush().await;
        assert_eq!(e.factory.calls()[1].start_at_s, 60.0);
    }

    #[tokio::test]
    async fn a_resume_before_the_boundary_runs_the_seek_made_during_the_pending_pause() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await;
        e.channel.pause_at(17.0);
        e.channel.seek_to(50.0);
        e.channel.resume(PauseReason::Manual);
        let c = e.control();
        assert!(!c.pause_pending && c.seek_pending);
        assert_eq!(c.position_s, 50.0);
        e.advance(700);
        e.tick();
        e.flush().await;
        assert!(h0.disposed());
        assert_eq!(e.factory.calls()[1].start_at_s, 50.0);
        assert!(e.channel.state().pause_reasons.is_empty());
    }

    #[tokio::test]
    async fn a_restart_or_a_seek_to_the_end_during_a_pending_pause() {
        // ⏮: the film resumes from its start.
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await;
        e.channel.pause_at(17.0);
        e.channel.seek_to(0.0);
        assert_eq!(e.control().position_s, 0.0);
        h0.produce(2);
        e.key();
        e.key();
        assert_eq!(e.key(), "pause");
        e.channel.resume(PauseReason::Manual);
        e.flush().await;
        assert_eq!(e.factory.calls()[1].start_at_s, 0.0);
        // The last second: the film ends at the boundary, the channel stays paused for the next.
        let mut e = setup();
        let a = e.add_item("a");
        e.add_item("b");
        let h0 = e.air(&a, 3).await;
        e.channel.pause_at(17.0);
        e.channel.seek_to(99.5);
        h0.produce(2);
        e.key();
        e.key();
        assert_eq!(e.key(), "pause");
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
        assert_eq!(e.channel.state().pause_reasons, [PauseReason::Manual]);
        assert_eq!(e.starts(), 1, "the next film waits for the resume");
    }

    #[tokio::test]
    async fn an_automatic_pause_after_a_seek_during_a_pending_pause_parks_at_the_target() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.pause_at(30.0);
        e.channel.seek_to(70.0);
        e.channel.pause(PauseReason::Tunnel);
        assert_eq!(e.control().position_s, 70.0);
        e.channel.resume(PauseReason::Tunnel);
        e.channel.resume(PauseReason::Manual);
        e.flush().await;
        assert_eq!(e.factory.calls()[1].start_at_s, 70.0);
    }

    #[tokio::test]
    async fn an_automatic_pause_while_one_is_pending_pauses_at_once_and_keeps_the_manual_one() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await;
        e.channel.pause_at(30.0);
        e.channel.pause(PauseReason::Tunnel);
        assert!(h0.disposed());
        let c = e.control();
        assert!(!c.pause_pending);
        assert_eq!(c.position_s, 12.0);
        assert_eq!(
            e.channel.state().pause_reasons,
            [PauseReason::Tunnel, PauseReason::Manual]
        );
        e.channel.resume(PauseReason::Tunnel);
        assert_eq!(e.key(), "pause");
    }

    #[tokio::test]
    async fn a_film_that_ends_before_the_pending_pause_leaves_the_channel_paused() {
        let mut e = setup();
        let a = e.add("a", Some(14.0), false);
        e.add_item("b");
        let h0 = e.air(&a, 3).await;
        e.channel.pause_at(13.9);
        h0.set_state(SegmenterState::Done);
        assert_eq!(e.key(), "pause");
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
        // The next film waits for the resume.
        assert_eq!(e.channel.current_item_id(), None);
        assert_eq!(e.key(), "pause");
        assert_eq!(e.channel.state().pause_reasons, [PauseReason::Manual]);
    }

    #[tokio::test]
    async fn a_live_stream_ignores_the_position_of_a_pause() {
        let mut e = setup();
        let live = e.add("live", None, true);
        let h = e.air(&live, 2).await;
        e.channel.pause_at(30.0);
        assert!(!e.control().pause_pending);
        assert!(!h.disposed());
        h.produce(1);
        assert_eq!(e.key(), clip_key(&live, 0));
    }

    #[tokio::test]
    async fn a_seek_starts_the_film_from_the_target_under_a_new_source_key() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await; // 12 s aired
        e.channel.seek_to(50.0);
        let c = e.control();
        assert!(c.seek_pending);
        assert_eq!((c.item_id, c.position_s), (Some(a.id), 50.0));
        // Nothing happens before 700 ms of quiet.
        e.advance(699);
        e.tick();
        e.flush().await;
        assert_eq!(e.starts(), 1);
        assert!(!h0.disposed());
        e.advance(1);
        e.tick();
        assert!(h0.disposed(), "the old clip is stopped");
        assert!(!e.control().seek_pending);
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: a.id,
                start_at_s: 50.0,
                attempt: 0
            }
        );
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        e.factory.handle(1).produce(2);
        let first = e.next();
        assert_eq!(first.source_key, relaunched_key(&a, 0, 1));
        assert_ne!(first.source_key, clip_key(&a, 0));
        assert_eq!(e.control().position_s, 54.0);
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
    }

    /// Review Focus 1: a burst of relative seeks is one clip start.
    #[tokio::test]
    async fn eight_plus_10_s_seeks_within_2_s_start_one_clip_80_s_further() {
        let mut e = setup();
        let a = e.add("a", Some(300.0), false);
        let h0 = e.air(&a, 3).await; // 12 s aired
        for n in 1..=8 {
            e.channel.seek_by(10.0);
            let c = e.control();
            assert!(c.seek_pending);
            assert_eq!(c.position_s, 12.0 + 10.0 * f64::from(n));
            if n == 4 {
                // The old clip keeps airing while the seek waits; it does not move the target.
                h0.produce(1);
                assert_eq!(e.key(), clip_key(&a, 0));
            }
            e.advance(250);
            e.tick();
            e.flush().await;
            assert_eq!(e.starts(), 1, "started before the burst ended (n = {n})");
        }
        e.advance(449); // 699 ms after the last command
        e.tick();
        assert_eq!(e.starts(), 1);
        assert!(e.control().seek_pending);
        e.advance(1);
        e.tick();
        e.flush().await;
        assert_eq!(e.starts(), 2);
        assert_eq!(e.factory.calls()[1].start_at_s, 92.0);
        let c = e.control();
        assert!(!c.seek_pending);
        assert_eq!(c.position_s, 92.0);
        // Nothing else starts later.
        for _ in 0..10 {
            e.advance(250);
            e.tick();
        }
        e.flush().await;
        assert_eq!(e.starts(), 2);
        assert_eq!(e.live_handles(), 1);
    }

    #[tokio::test]
    async fn a_seek_while_paused_moves_the_remembered_position_without_starting_a_clip() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.pause(PauseReason::Manual);
        e.channel.seek_to(40.0);
        let c = e.control();
        assert_eq!((c.position_s, c.seek_pending), (40.0, false));
        e.channel.seek_by(-15.0);
        assert_eq!(e.control().position_s, 25.0);
        e.advance(1_000);
        e.tick();
        assert_eq!(e.key(), "pause");
        e.flush().await;
        assert_eq!(e.starts(), 1);
        e.channel.resume(PauseReason::Manual);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.calls()[1].start_at_s, 25.0);
        e.factory.handle(1).produce(1);
        assert_eq!(e.key(), relaunched_key(&a, 0, 1));
    }

    #[tokio::test]
    async fn pausing_with_a_seek_pending_parks_the_film_at_the_target() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.seek_to(60.0);
        e.channel.pause(PauseReason::Manual);
        let c = e.control();
        assert_eq!((c.position_s, c.seek_pending), (60.0, false));
        e.advance(1_000);
        e.tick();
        e.flush().await;
        assert_eq!(e.starts(), 1);
        e.channel.resume(PauseReason::Manual);
        e.next();
        e.flush().await;
        assert_eq!(e.factory.calls()[1].start_at_s, 60.0);
    }

    #[tokio::test]
    async fn a_target_before_the_start_is_the_start() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.seek_by(-60.0);
        assert_eq!(e.control().position_s, 0.0);
        e.advance(700);
        e.tick();
        assert_eq!(e.factory.calls()[1].start_at_s, 0.0);
        e.flush().await;
        e.channel.seek_to(-5.0);
        e.advance(700);
        e.tick();
        assert_eq!(e.factory.calls()[2].start_at_s, 0.0);
    }

    #[tokio::test]
    async fn a_target_in_the_last_second_ends_the_film_as_skipped_and_the_next_one_starts() {
        let mut e = setup();
        let a = e.add_item("a"); // 100 s
        let b = e.add_item("b");
        let h0 = e.air(&a, 2).await;
        e.channel.seek_to(99.0);
        assert_eq!(e.control().position_s, 99.0);
        e.advance(700);
        e.tick();
        assert!(h0.disposed());
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Played);
        assert_eq!(
            fresh.error_text(Lang::En).as_deref(),
            Some("Skipped by the owner")
        );
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
        assert_eq!(e.factory.calls()[1].start_at_s, 0.0);
        assert!(!e.control().seek_pending);

        // Far past the end, relative: the same; the shown target stops at the end.
        e.factory.handle(1).produce(1);
        assert_eq!(e.key(), clip_key(&b, 0));
        e.channel.seek_by(500.0);
        assert_eq!(e.control().position_s, 100.0);
        e.advance(700);
        e.tick();
        assert_eq!(e.get(b.id).status, ItemStatus::Played);
        assert_eq!(e.key(), "idle");
    }

    #[tokio::test]
    async fn a_seek_into_the_last_second_of_a_paused_film_skips_it_and_stays_paused() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.air(&a, 2).await;
        e.channel.pause(PauseReason::Manual);
        e.channel.seek_to(99.5);
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Played);
        assert_eq!(
            fresh.error_text(Lang::En).as_deref(),
            Some("Skipped by the owner")
        );
        assert_eq!(e.key(), "pause");
        e.flush().await;
        assert_eq!(e.starts(), 1);
        assert_eq!(e.get(b.id).status, ItemStatus::Queued);
    }

    #[tokio::test]
    async fn a_film_of_unknown_length_only_seeks_forward_without_an_upper_bound() {
        let mut e = setup();
        let a = e.add("a", None, false);
        e.air(&a, 2).await; // 8 s
        let c = e.control();
        assert_eq!(c.duration_s, None);
        assert!(c.can_seek);
        e.channel.seek_to(30.0);
        e.channel.seek_by(-5.0);
        assert!(!e.control().seek_pending, "only forward relative seeks");
        e.advance(700);
        e.tick();
        assert_eq!(e.starts(), 1);
        e.channel.seek_by(5_000.0);
        assert_eq!(e.control().position_s, 5_008.0);
        e.advance(700);
        e.tick();
        assert_eq!(e.factory.calls()[1].start_at_s, 5_008.0);
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
        e.flush().await;
        // Back to the start (⏮) works for it too.
        e.channel.seek_to(0.0);
        e.advance(700);
        e.tick();
        assert_eq!(e.factory.calls()[2].start_at_s, 0.0);
    }

    /// Review Focus 1: seeks while a clip start is in flight end with exactly one clip.
    #[tokio::test]
    async fn seeks_while_the_clip_is_still_starting_discard_the_old_start() {
        let mut e = setup();
        e.factory.lock().manual = true;
        let a = e.add_item("a");
        assert_eq!(e.key(), "buffering"); // start #0 in flight
        for _ in 0..8 {
            e.channel.seek_by(10.0);
            e.advance(250);
            e.tick();
        }
        e.advance(700);
        // The old start is cancelled; the new one waits until it is gone.
        e.tick();
        assert_eq!(e.starts(), 1);
        assert!(e.control().seek_pending);
        e.flush().await;
        e.tick();
        assert_eq!(e.starts(), 2);
        assert_eq!(e.factory.calls()[1].start_at_s, 80.0);
        // The cancelled start is gone: nothing waits on its gate any more.
        let gate = std::mem::replace(&mut e.factory.lock().gates[0], oneshot::channel().0);
        assert!(gate.send(()).is_err(), "start #0 was not cancelled");
        e.flush().await;
        assert!(e.factory.lock().handles.is_empty());
        // Another seek while start #1 is held in flight (its gate is gone: a new burst).
        e.channel.seek_to(20.0);
        e.advance(700);
        e.tick();
        e.flush().await;
        e.tick();
        assert_eq!(e.factory.calls()[2].start_at_s, 20.0);
        e.release_all();
        e.flush().await;
        assert_eq!(e.live_handles(), 1);
        let live = e
            .factory
            .lock()
            .handles
            .iter()
            .find(|h| !h.disposed())
            .cloned()
            .unwrap();
        assert_eq!(live.lock().start_at_s, 20.0);
        live.produce(1);
        assert_eq!(e.key(), relaunched_key(&a, 0, 2));
        assert!(!e.control().seek_pending);
    }

    #[tokio::test]
    async fn a_pending_seek_never_outlives_its_film() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.air(&a, 2).await;
        e.channel.seek_to(50.0);
        e.channel.skip().unwrap();
        assert!(!e.control().seek_pending);
        e.next();
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
        e.advance(700);
        e.tick();
        e.flush().await;
        assert_eq!(
            e.starts(),
            2,
            "the seek of the skipped film must not touch the next one"
        );
        // A removed film takes its pending seek with it.
        e.factory.handle(1).produce(1);
        e.next();
        e.channel.seek_to(30.0);
        e.set_status(b.id, ItemStatus::Removed);
        e.channel.on_queue_changed().unwrap();
        assert!(!e.control().seek_pending);
        e.advance(700);
        e.tick();
        assert_eq!(e.starts(), 2);
    }

    #[tokio::test]
    async fn a_seek_drops_the_prefetched_film_which_is_prefetched_again_near_the_end() {
        let mut e = setup();
        let a = e.add("a", Some(40.0), false);
        let b = e.add_item("b");
        e.air(&a, 1).await;
        e.factory.handle(0).produce(1);
        e.next();
        e.flush().await;
        let prefetched = e.factory.handle(1);
        assert_eq!(prefetched.item_id(), b.id);
        e.channel.seek_to(0.0);
        e.advance(700);
        e.tick();
        assert!(prefetched.disposed());
        assert_eq!(e.channel.prefetched_item_id(), None);
    }

    #[tokio::test]
    async fn play_now_from_the_queue_moves_it_first_and_skips_the_current_film() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        let c = e.add_item("c");
        e.air(&a, 2).await;
        e.channel.play_now(c.id).unwrap();
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Played);
        assert_eq!(
            fresh.error_text(Lang::En).as_deref(),
            Some("Skipped by the owner")
        );
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), c.id);
        assert_eq!(e.get(c.id).status, ItemStatus::Playing);
        assert_eq!(e.get(b.id).status, ItemStatus::Queued);
    }

    #[tokio::test]
    async fn play_now_from_history_adds_the_film_again_with_the_same_link_and_nick() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        let c = e.add_item("c");
        e.set_status(a.id, ItemStatus::Played);
        e.air(&b, 2).await;
        e.channel.play_now(a.id).unwrap();
        assert_eq!(e.get(b.id).status, ItemStatus::Played);
        e.next();
        e.flush().await;
        let again = {
            let conn = e.db.lock().unwrap();
            QueueRepo::new(&conn)
                .list_by_status(MAIN_CHANNEL_ID, &[ItemStatus::Playing])
                .unwrap()
        };
        assert_eq!(again.len(), 1);
        let again = &again[0];
        assert_ne!(again.id, a.id, "a new entry");
        assert_eq!(again.source_url, a.source_url);
        assert_eq!(again.title, a.title);
        assert_eq!(again.requester_name.as_deref(), Some("Ola"));
        assert_eq!(again.requester_key_hash, a.requester_key_hash);
        assert!(again.requester_key_hash.is_some());
        assert_eq!(e.factory.last_call_item(), again.id);
        // The old entry stays in the history; the rest of the queue waits.
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
        assert_eq!(e.get(c.id).status, ItemStatus::Queued);
    }

    /// Fix round 1 (2): a seek onto a clip whose process failed (not yet noticed by `next()`)
    /// goes through the retry accounting, from the seek target.
    #[tokio::test]
    async fn a_seek_onto_a_failed_clip_is_the_retry_from_the_target() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 2).await;
        h0.set_state(SegmenterState::Failed);
        h0.set_error("Connection reset");
        e.channel.seek_to(50.0);
        e.advance(700);
        e.tick();
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: a.id,
                start_at_s: 50.0,
                attempt: 1
            }
        );
        e.flush().await;
        let h1 = e.factory.handle(1);
        h1.produce(1);
        assert_eq!(e.key(), clip_key(&a, 1));
        // The retry failing too, then a seek: the film fails instead of starting again.
        h1.set_state(SegmenterState::Failed);
        e.channel.seek_to(20.0);
        e.advance(700);
        e.tick();
        assert_eq!(e.starts(), 2);
        assert_eq!(e.get(a.id).status, ItemStatus::Failed);
    }

    /// Fix round 1 (2): a start that failed (not yet noticed) is the retry from the target.
    #[tokio::test]
    async fn a_seek_onto_a_failed_start_is_the_retry_from_the_target() {
        let mut e = setup();
        let a = e.add_item("a");
        e.factory.lock().fail_next = 1;
        e.next();
        e.flush().await;
        e.channel.seek_to(40.0);
        e.advance(700);
        e.tick();
        assert_eq!(
            e.factory.calls()[1],
            Call {
                item_id: a.id,
                start_at_s: 40.0,
                attempt: 1
            }
        );
    }

    /// Fix round 1 (3): a seek never leaves two processes of the same film: the start still in
    /// flight is cancelled and the new one waits until it is gone.
    #[tokio::test]
    async fn a_seek_during_a_slow_start_never_runs_two_clips_of_the_film() {
        let mut e = setup();
        e.factory.lock().manual = true;
        let a = e.add_item("a");
        assert_eq!(e.key(), "buffering"); // start #0 in flight
        e.channel.seek_to(30.0);
        e.advance(700);
        e.tick();
        // Every start that is still held finishes now, before the channel looks at them.
        e.release_all();
        e.yield_only().await;
        assert!(e.live_handles() <= 1, "two clips of the film at once");
        e.flush().await;
        e.tick();
        e.release_all();
        e.yield_only().await;
        assert!(e.live_handles() <= 1, "two clips of the film at once");
        e.flush().await;
        assert_eq!(e.live_handles(), 1);
        assert_eq!(e.factory.calls().last().unwrap().start_at_s, 30.0);
        assert!(!e.control().seek_pending);
        let live = e
            .factory
            .lock()
            .handles
            .iter()
            .find(|h| !h.disposed())
            .cloned()
            .unwrap();
        assert_eq!(live.lock().start_at_s, 30.0);
        live.produce(1);
        assert_eq!(e.key(), relaunched_key(&a, 0, 1));
    }

    /// Fix round 1 (3): a start that finished just before the seek runs is disposed before the
    /// film starts again.
    #[tokio::test]
    async fn a_start_that_lands_as_the_seek_runs_is_disposed_before_the_relaunch() {
        let mut e = setup();
        e.factory.lock().manual = true;
        let a = e.add_item("a");
        assert_eq!(e.key(), "buffering");
        e.channel.seek_to(30.0);
        e.release(0);
        e.yield_only().await; // handle #0 exists but the channel has not taken it yet
        assert_eq!(e.live_handles(), 1);
        e.advance(700);
        e.tick();
        assert!(e.factory.handle(0).disposed());
        assert_eq!(e.starts(), 2);
        e.release_all();
        e.flush().await;
        assert_eq!(e.live_handles(), 1);
        e.factory.handle(1).produce(1);
        assert_eq!(e.key(), relaunched_key(&a, 0, 1));
    }

    /// Controller's ruling: play-now is the owner's explicit "play this now", so it clears the
    /// manual pause; automatic reasons stay and keep holding the film.
    #[tokio::test]
    async fn play_now_clears_the_manual_pause_but_not_the_automatic_ones() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        let c = e.add_item("c");
        e.air(&a, 2).await;
        e.channel.pause(PauseReason::Manual);
        e.channel.play_now(b.id).unwrap();
        assert!(e.channel.state().pause_reasons.is_empty());
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
        assert_eq!(e.starts(), 2, "the skipped paused film is not relaunched");

        e.factory.handle(1).produce(1);
        e.next();
        e.channel.pause(PauseReason::Manual);
        e.channel.pause(PauseReason::NoViewers);
        e.channel.play_now(c.id).unwrap();
        assert_eq!(e.channel.state().pause_reasons, [PauseReason::NoViewers]);
        assert_eq!(e.key(), "pause");
        e.flush().await;
        assert_eq!(e.starts(), 2);
        assert_eq!(e.get(c.id).status, ItemStatus::Queued);
        e.channel.resume(PauseReason::NoViewers);
        e.next();
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), c.id);
    }

    #[tokio::test]
    async fn play_now_on_an_idle_channel_just_starts_it_and_refuses_unplayable_items() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.channel.play_now(b.id).unwrap();
        e.next();
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
        assert_eq!(e.get(a.id).status, ItemStatus::Queued);
        e.set_status(a.id, ItemStatus::PendingApproval);
        assert!(e.channel.play_now(a.id).is_err());
        // Final wave M2: in the reader's language, not hard-coded Polish.
        let missing = e.channel.play_now(9_999).unwrap_err();
        assert_eq!(
            crate::i18n::error_msg(&missing).render(crate::i18n::Lang::En),
            "There is no such item."
        );
        assert_eq!(e.get(b.id).status, ItemStatus::Playing);
    }

    #[tokio::test]
    async fn a_live_stream_cannot_be_paused_or_seeked_but_can_be_skipped() {
        let mut e = setup();
        let live = e.add("live", None, true);
        let h = e.air(&live, 2).await;
        let c = e.control();
        assert_eq!(c.item_id, Some(live.id));
        assert!(c.is_live);
        assert!(!c.can_pause && !c.can_seek);
        e.channel.seek_to(30.0);
        e.channel.seek_by(10.0);
        assert!(!e.control().seek_pending);
        e.advance(700);
        e.tick();
        assert!(!h.disposed());
        assert_eq!(e.starts(), 1);
        e.channel.skip().unwrap();
        assert_eq!(e.get(live.id).status, ItemStatus::Played);
    }

    #[tokio::test]
    async fn control_state_without_a_film_allows_nothing() {
        let e = setup();
        let c = e.control();
        assert_eq!(c, ControlState::default());
        assert_eq!(c.item_id, None);
        assert!(!c.can_pause && !c.can_seek && !c.seek_pending);
    }

    #[tokio::test]
    async fn the_position_is_approximate_when_the_film_is_copied_and_exact_when_converted() {
        let mut e = setup();
        e.factory.lock().copies_video = true;
        let a = e.add_item("a");
        e.air(&a, 1).await;
        let c = e.control();
        assert_eq!(
            (c.duration_s, c.can_pause, c.can_seek),
            (Some(100.0), true, true)
        );
        assert!(c.position_approx);
        // Still known while the relaunched clip is starting, and while paused.
        e.channel.seek_to(30.0);
        e.advance(700);
        e.tick();
        assert!(e.control().position_approx);
        e.channel.pause(PauseReason::Manual);
        assert!(e.control().position_approx);
        e.channel.resume(PauseReason::Manual);

        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 1).await;
        assert!(!e.control().position_approx);
    }

    // ---------- Info ticker (stage 4, task 6) ----------

    /// A change of the ticker's layout starts the film again from where the channel is, once,
    /// under a new source key (a discontinuity), the old clip and the prefetched one stopped.
    #[tokio::test]
    async fn applying_the_ticker_starts_the_film_again_from_its_position() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        let h0 = e.air(&a, 3).await; // 12 s aired
        e.channel.apply_ticker();
        // Nothing waits for a seek: the controls show where the film is.
        let c = e.control();
        assert!(!c.seek_pending);
        assert_eq!(c.position_s, 12.0);
        // A burst of changes is one restart.
        e.advance(300);
        e.channel.apply_ticker();
        // The old clip keeps airing meanwhile.
        h0.produce(1);
        assert_eq!(e.key(), clip_key(&a, 0));
        e.advance(699);
        e.tick();
        e.flush().await;
        assert_eq!(e.starts(), 1);
        e.advance(1);
        e.tick();
        assert!(h0.disposed());
        // From where the channel is when it restarts (no part is aired twice).
        assert_eq!(
            e.factory.calls()[1..],
            [Call {
                item_id: a.id,
                start_at_s: 16.0,
                attempt: 0
            }]
        );
        e.flush().await;
        e.factory.handle(1).produce(1);
        assert_eq!(e.key(), relaunched_key(&a, 0, 1));
        assert_eq!(e.starts(), 2);
        assert_eq!(e.get(b.id).status, ItemStatus::Queued);
    }

    /// Final wave I1: a new public address restarts the film for the ticker's code only when
    /// its process does not draw the code of that address already.
    #[tokio::test]
    async fn a_new_address_restarts_the_film_only_when_its_code_is_out_of_date() {
        let mut e = setup();
        let a = e.add_item("a");
        let h0 = e.air(&a, 3).await;
        // Started after the change (by a resume): it draws the new code already.
        h0.lock().ticker_code = Some("https://nowy.example.com/".to_string());
        e.channel.apply_ticker_code("https://nowy.example.com");
        e.advance(700);
        e.tick();
        e.flush().await;
        assert_eq!(e.starts(), 1);
        assert!(!h0.disposed());
        // Another address: one restart, as for a new look.
        e.channel.apply_ticker_code("https://inny.example.com");
        e.advance(700);
        e.tick();
        e.flush().await;
        assert!(h0.disposed());
        assert_eq!(e.starts(), 2);
        // A clip whose code is not known (no ticker drawn) is restarted too.
        e.factory.handle(1).produce(1);
        e.channel.apply_ticker_code("https://inny.example.com");
        e.advance(700);
        e.tick();
        e.flush().await;
        assert_eq!(e.starts(), 3);
    }

    /// A seek and a new ticker look at once: one restart, at the seek's target. Pausing while
    /// a ticker restart waits parks the film where it is.
    #[tokio::test]
    async fn a_seek_and_a_ticker_restart_together_are_one_start_at_the_seek_target() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.seek_to(50.0);
        e.channel.apply_ticker();
        e.advance(700);
        e.tick();
        assert_eq!(e.factory.calls()[1].start_at_s, 50.0);
        e.flush().await;

        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.apply_ticker();
        e.channel.seek_by(10.0);
        e.advance(700);
        e.tick();
        assert_eq!(e.factory.calls()[1].start_at_s, 22.0);
        assert_eq!(e.starts(), 2);
        e.flush().await;

        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 3).await;
        e.channel.apply_ticker();
        e.channel.pause(PauseReason::Manual);
        assert_eq!(e.control().position_s, 12.0);
        e.advance(5_000);
        e.tick();
        e.channel.resume(PauseReason::Manual);
        e.flush().await;
        assert_eq!(e.factory.calls()[1].start_at_s, 12.0);
        assert_eq!(e.starts(), 2);
    }

    /// In the last second of a film a new ticker look is not worth a restart: it plays out.
    #[tokio::test]
    async fn a_ticker_restart_in_the_last_second_lets_the_film_play_out() {
        let mut e = setup();
        let a = e.add("a", Some(12.5), false);
        let h = e.air(&a, 3).await; // 12 s of 12.5
        e.channel.apply_ticker();
        e.advance(700);
        e.tick();
        assert!(!h.disposed());
        assert_eq!(e.starts(), 1);
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
    }

    /// The next film prepared with the old look is dropped and prepared again with the new
    /// one (a buffering slate before it is accepted).
    #[tokio::test]
    async fn a_ticker_restart_in_the_last_second_prepares_the_next_film_again() {
        let mut e = setup();
        let a = e.add("a", Some(12.5), false);
        let b = e.add_item("b");
        let h = e.air(&a, 3).await; // 12 s of 12.5: b is prefetched
        e.flush().await;
        assert_eq!(e.channel.prefetched_item_id(), Some(b.id));
        let prefetched = e.factory.handle(1);
        e.channel.apply_ticker();
        e.advance(700);
        e.tick();
        assert!(!h.disposed());
        assert!(prefetched.disposed(), "the old look is not aired");
        assert_eq!(e.channel.prefetched_item_id(), None);
        // The film ends: the next one starts with the new look.
        h.set_state(SegmenterState::Done);
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        assert_eq!(e.factory.last_call_item(), b.id);
        assert_eq!(e.starts(), 3);
    }

    /// The warning about a computer too slow for the ticker is logged once, for the film on
    /// air only (not for the next one being prepared).
    #[tokio::test]
    async fn a_slow_ticker_is_logged_once_and_only_for_the_film_on_air() {
        let mut e = setup();
        let a = e.add("a", Some(60.0), false);
        let b = e.add_item("b");
        let h = e.air(&a, 3).await; // 12 s of 60: b is prefetched
        e.flush().await;
        assert_eq!(e.channel.prefetched_item_id(), Some(b.id));
        let slow = |e: &Env| {
            e.logs
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("The computer cannot keep up with the info ticker"))
                .cloned()
                .collect::<Vec<_>>()
        };
        e.factory.handle(1).lock().ticker_slow = true;
        e.tick();
        assert!(slow(&e).is_empty(), "the prepared film is not on air");
        h.lock().ticker_slow = true;
        e.tick();
        e.tick();
        assert_eq!(
            slow(&e),
            [format!(
                "The computer cannot keep up with the info ticker (video #{} \"a\" converts at 1.50× for 30 s). Switch the ticker off or turn on conversion on the graphics card.",
                a.id
            )]
        );
    }

    #[tokio::test]
    async fn applying_the_ticker_to_a_live_stream_waits_for_the_next_item() {
        let mut e = setup();
        let live = e.add("live", None, true);
        let h = e.air(&live, 2).await;
        e.channel.apply_ticker();
        e.advance(5_000);
        e.tick();
        e.flush().await;
        assert_eq!(e.starts(), 1);
        assert!(!h.disposed());
    }

    #[tokio::test]
    async fn applying_the_ticker_while_paused_or_idle_starts_nothing() {
        let mut e = setup();
        e.channel.apply_ticker();
        e.advance(5_000);
        e.tick();
        assert_eq!(e.starts(), 0);
        let a = e.add_item("a");
        e.air(&a, 1).await;
        e.channel.pause(PauseReason::Manual);
        e.channel.apply_ticker();
        e.advance(5_000);
        e.tick();
        e.flush().await;
        assert_eq!(e.starts(), 1);
        // The resume starts it with the new layout anyway.
        e.channel.resume(PauseReason::Manual);
        e.flush().await;
        assert_eq!(e.starts(), 2);
    }

    /// The next film is started ahead of its turn as such (the factory reads it without the
    /// initial burst); a film started when it is due is not.
    #[tokio::test]
    async fn the_next_film_is_prepared_ahead_of_its_turn() {
        let mut e = setup();
        let a = e.add("a", Some(60.0), false);
        let b = e.add_item("b");
        e.air(&a, 1).await; // 56 s left: b is prepared ahead
        e.flush().await;
        assert_eq!(e.channel.prefetched_item_id(), Some(b.id));
        assert_eq!(e.factory.lock().ahead, [b.id]);
        // A film started on its turn is not "ahead".
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 1).await;
        assert!(e.factory.lock().ahead.is_empty());
    }

    #[tokio::test]
    async fn a_slow_ticker_of_the_film_on_air_shows_in_the_stats() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 1).await;
        assert!(!e.channel.stats().ticker_slow);
        e.factory.lock().handles[0].lock().ticker_slow = true;
        assert!(e.channel.stats().ticker_slow);
    }

    // ---------- The producer's monitor (spec 2026-10-21) ----------

    impl Fake {
        /// Like [`Self::produce`], with real files in `dir` (the monitor links them).
        fn produce_files(&self, dir: &Path, count: usize) {
            let mut s = self.lock();
            for _ in 0..count {
                let path = dir.join(format!("{}-{}-{}.ts", s.item_id, s.start_at_s, s.n));
                std::fs::write(&path, [0x47u8; 188]).unwrap();
                let index = s.n;
                s.pending.push(SegmentInfo {
                    path,
                    duration: 4.0,
                    index,
                });
                s.n += 1;
            }
        }
    }

    struct MonitorEnv {
        e: Env,
        tmp: tempfile::TempDir,
    }

    impl MonitorEnv {
        fn clips(&self) -> PathBuf {
            self.tmp.path().join("clips")
        }
        fn feed(&self) -> MonitorFeed {
            self.e.channel.monitor_feed()
        }
        fn rows(&self) -> Vec<(u64, i64, f64, bool)> {
            self.feed()
                .rows
                .iter()
                .map(|r| (r.seq, r.item_id, r.position_s, r.discontinuity))
                .collect()
        }
        fn last_handle(&self) -> Fake {
            let n = self.e.factory.lock().handles.len();
            self.e.factory.handle(n - 1)
        }
    }

    fn setup_monitor() -> MonitorEnv {
        setup_monitor_with(Arc::new(MonitorWatch::always()))
    }

    fn setup_monitor_with(watch: Arc<MonitorWatch>) -> MonitorEnv {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("clips")).unwrap();
        let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
        let factory = FakeFactory::default();
        let now = Arc::new(AtomicU64::new(0));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let logs = Arc::new(Mutex::new(Vec::new()));
        let mut o = ChannelOptions::new(
            MAIN_CHANNEL_ID,
            Arc::clone(&db),
            Arc::new(factory.clone()),
            slates(),
        );
        let clock = Arc::clone(&now);
        o.now = Some(Box::new(move || clock.load(Ordering::SeqCst)));
        let sink = Arc::clone(&removed);
        // The channel's own removal of aired segments, for real (the monitor keeps a link).
        o.remove_file = Some(Box::new(move |p: &Path| {
            let _ = std::fs::remove_file(p);
            sink.lock().unwrap().push(p.to_path_buf());
        }));
        o.monitor = Some(MonitorOptions {
            dir: tmp.path().join("clips").join("_monitor"),
            watch,
            first_seq: Some(0),
        });
        let mut channel = Channel::new(o);
        channel.start().unwrap();
        MonitorEnv {
            e: Env {
                db,
                factory,
                now,
                removed,
                logs,
                channel,
            },
            tmp,
        }
    }

    #[tokio::test]
    async fn the_monitor_lists_the_new_clip_before_it_airs_and_follows_what_airs() {
        let mut m = setup_monitor();
        let a = m.e.add_item("a");
        assert_eq!(m.e.key(), "buffering");
        m.e.flush().await;
        assert!(m.feed().active, "a film is starting");
        assert!(m.rows().is_empty());
        m.last_handle().produce_files(&m.clips(), 5);
        // One loop tick later the first segments are listed, before the broadcaster takes one.
        m.e.tick();
        assert_eq!(
            m.rows(),
            [
                (0, a.id, 0.0, false),
                (1, a.id, 4.0, false),
                (2, a.id, 8.0, false)
            ],
            "at most {LOOKAHEAD} segments ahead of the air"
        );
        assert_eq!(m.feed().aired_seq, None);
        // Two segments air: two more are listed, the aired ones stay (their file is gone).
        m.e.publish();
        m.e.publish();
        m.e.tick();
        let feed = m.feed();
        assert_eq!(feed.rows.len(), 5);
        assert_eq!(feed.rows[4].position_s, 16.0);
        assert_eq!(feed.aired_seq, Some(1));
        assert!(
            m.e.removed.lock().unwrap().len() == 1,
            "the first aired file was removed"
        );
        assert!(feed.rows.iter().all(|r| r.path.exists()), "{:?}", feed.rows);
        let text = feed.playlist(None);
        assert_eq!(
            text.matches(&format!("monitor/clip-{}-0/", a.id)).count(),
            5,
            "{text}"
        );
    }

    #[tokio::test]
    async fn after_a_seek_the_old_lookahead_goes_and_the_new_clip_follows_a_discontinuity() {
        let mut m = setup_monitor();
        let a = m.e.add_item("a");
        m.e.next();
        m.e.flush().await;
        m.last_handle().produce_files(&m.clips(), 4);
        m.e.tick();
        m.e.publish();
        m.e.publish();
        m.e.tick();
        let before = m.rows();
        assert_eq!(before.len(), 4, "{before:?}");
        m.e.channel.seek_to(60.0);
        m.e.advance(800);
        m.e.tick();
        m.e.flush().await;
        // The two rows that never aired went (the viewers never see them); the relaunched
        // clip's first segment is listed right away after the aired ones.
        m.last_handle().produce_files(&m.clips(), 1);
        m.e.tick();
        let rows = m.rows();
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[..2], before[..2]);
        assert_eq!(rows[2], (2, a.id, 60.0, true));
        let feed = m.feed();
        assert_eq!(feed.rows[2].clip, format!("clip-{}-0-r1", a.id));
        let text = feed.playlist(Some(2));
        assert!(text.contains("#EXT-X-DISCONTINUITY\n"), "{text}");
        assert!(text.contains("#EXT-X-START:TIME-OFFSET=8.000"), "{text}");
    }

    #[tokio::test]
    async fn the_antenna_is_the_row_the_viewers_see_and_no_film_means_an_empty_list() {
        let mut m = setup_monitor();
        assert!(!m.feed().active, "idle");
        let a = m.e.add("a", Some(20.0), false);
        m.e.next();
        m.e.flush().await;
        let h = m.last_handle();
        h.produce_files(&m.clips(), 5);
        m.e.tick();
        for _ in 0..4 {
            m.e.publish();
            m.e.tick();
        }
        // Viewers lag 3 segments: the second of the four aired.
        assert_eq!(m.e.channel.state().current.unwrap().position_s, 4);
        assert_eq!(m.feed().antenna_seq, Some(1));
        assert_eq!(m.feed().aired_seq, Some(3));
        // The film ends: nothing current, the list is empty (the rows wait for their time).
        h.set_state(SegmenterState::Done);
        m.e.publish();
        m.e.publish();
        m.e.tick();
        assert_eq!(m.e.channel.state().control.item_id, None, "{:?}", a.id);
        let feed = m.feed();
        assert!(!feed.active);
        assert!(!feed.playlist(None).contains("#EXTINF"));
        assert!(!feed.rows.is_empty(), "kept for their 90 s");
        m.e.advance(RETAIN_MS);
        m.e.tick();
        assert!(m.feed().rows.is_empty());
    }

    #[tokio::test]
    async fn while_paused_the_monitor_keeps_the_segment_the_film_resumes_from() {
        let mut m = setup_monitor();
        let a = m.e.add_item("a");
        m.e.next();
        m.e.flush().await;
        m.last_handle().produce_files(&m.clips(), 4);
        m.e.tick();
        m.e.publish();
        m.e.tick();
        m.e.channel.pause(PauseReason::Manual);
        m.e.tick();
        let feed = m.feed();
        assert!(feed.active, "a paused film");
        let at = m.e.channel.state().control.position_s;
        assert_eq!(at, 4.0);
        let row = feed
            .rows
            .iter()
            .find(|r| r.item_id == a.id && r.position_s == at)
            .expect("the first segment of the paused position");
        assert!(row.path.exists());
        let still = row.seq;
        // A pause longer than the retention time (review fix 1): the still picture stays.
        m.e.advance(2 * RETAIN_MS);
        m.e.tick();
        let rows: Vec<u64> = m.feed().rows.iter().map(|r| r.seq).collect();
        assert_eq!(rows, [still], "the aired row went, the still picture stays");
        assert!(m.feed().rows[0].path.exists());
        // Resumed: the relaunch is the next clip of the list; the still goes in its time.
        m.e.channel.resume(PauseReason::Manual);
        m.e.flush().await;
        m.last_handle().produce_files(&m.clips(), 1);
        m.e.tick();
        let last = m.feed().rows.last().cloned().unwrap();
        assert!(last.discontinuity && last.position_s == 4.0, "{last:?}");
        m.e.advance(1);
        m.e.tick();
        assert!(m.feed().rows.iter().all(|r| r.seq != still), "unpinned");
    }

    /// Review fix 13: without a viewer of the monitor the channel reads ffmpeg's list only when
    /// a segment is due (as without a monitor); aired segments are still kept for a later Sync.
    #[tokio::test]
    async fn without_a_viewer_the_monitor_reads_nothing_ahead() {
        let watch = Arc::new(MonitorWatch::default());
        let mut m = setup_monitor_with(Arc::clone(&watch));
        m.e.add_item("a");
        m.e.next();
        m.e.flush().await;
        m.last_handle().produce_files(&m.clips(), 5);
        m.e.tick();
        assert!(m.rows().is_empty(), "nothing listed ahead");
        assert_eq!(m.last_handle().lock().pending.len(), 5, "not even polled");
        m.e.publish();
        m.e.tick();
        assert_eq!(m.rows().len(), 1, "the aired one");
        watch.touch();
        m.e.tick();
        assert_eq!(m.rows().len(), 4, "watched: {LOOKAHEAD} ahead again");
    }

    fn setup_maybe_monitor(dir: Option<&Path>) -> Env {
        let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
        let factory = FakeFactory::default();
        let now = Arc::new(AtomicU64::new(0));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let logs = Arc::new(Mutex::new(Vec::new()));
        let mut o = ChannelOptions::new(
            MAIN_CHANNEL_ID,
            Arc::clone(&db),
            Arc::new(factory.clone()),
            slates(),
        );
        let clock = Arc::clone(&now);
        o.now = Some(Box::new(move || clock.load(Ordering::SeqCst)));
        let sink = Arc::clone(&removed);
        o.remove_file = Some(Box::new(move |p: &Path| {
            sink.lock().unwrap().push(p.to_path_buf())
        }));
        o.monitor = dir.map(|dir| MonitorOptions {
            dir: dir.to_path_buf(),
            watch: Arc::new(MonitorWatch::always()),
            first_seq: Some(0),
        });
        let mut channel = Channel::new(o);
        channel.start().unwrap();
        Env {
            db,
            factory,
            now,
            removed,
            logs,
            channel,
        }
    }

    /// Plays a film with seeks, a pause, a skip and the next film; what went to the
    /// broadcaster, what was removed and how the films ended.
    async fn public_stream_of(e: &mut Env) -> Vec<String> {
        let a = e.add_item("a");
        let b = e.add_item("b");
        let mut out = Vec::new();
        let seg =
            |s: SourceSegment| format!("{:?} {} {} {}", s.path, s.source_key, s.index, s.duration);
        out.push(seg(e.publish()));
        e.flush().await;
        e.factory.handle(0).produce(6);
        for _ in 0..3 {
            e.tick();
            out.push(seg(e.publish()));
        }
        e.channel.seek_by(20.0);
        e.advance(800);
        e.tick();
        e.flush().await;
        for _ in 0..2 {
            out.push(seg(e.publish()));
        }
        let last = e.factory.lock().handles.len() - 1;
        e.factory.handle(last).produce(3);
        for _ in 0..3 {
            e.tick();
            out.push(seg(e.publish()));
        }
        e.channel.pause(PauseReason::Manual);
        e.tick();
        for _ in 0..2 {
            out.push(seg(e.publish()));
        }
        e.channel.resume(PauseReason::Manual);
        e.flush().await;
        let last = e.factory.lock().handles.len() - 1;
        e.factory.handle(last).produce(2);
        for _ in 0..2 {
            e.tick();
            out.push(seg(e.publish()));
        }
        e.channel.skip().unwrap();
        out.push(seg(e.publish()));
        e.flush().await;
        let last = e.factory.lock().handles.len() - 1;
        e.factory.handle(last).produce(2);
        for _ in 0..3 {
            e.tick();
            out.push(seg(e.publish()));
        }
        out.extend(
            e.removed
                .lock()
                .unwrap()
                .iter()
                .map(|p| format!("removed {p:?}")),
        );
        out.push(format!("{:?} {:?}", e.get(a.id).status, e.get(b.id).status));
        out.push(format!("{:?}", e.factory.calls()));
        out
    }

    /// Review fix 13: the app runs the channel with the monitor on and watched; what the
    /// viewers get is exactly what they get without it.
    #[tokio::test]
    async fn the_public_stream_is_the_same_with_the_monitor_on() {
        let tmp = tempfile::tempdir().unwrap();
        let off = public_stream_of(&mut setup_maybe_monitor(None)).await;
        let mut on_env = setup_maybe_monitor(Some(tmp.path()));
        let on = public_stream_of(&mut on_env).await;
        assert!(off.len() > 20, "{off:?}");
        assert_eq!(off, on);
        assert!(
            on_env.channel.monitor_feed().next_seq > 5,
            "the monitor did list them"
        );
    }

    #[tokio::test]
    async fn without_the_monitor_the_channel_lists_nothing() {
        let mut e = setup();
        let a = e.add_item("a");
        e.air(&a, 2).await;
        e.tick();
        let feed = e.channel.monitor_feed();
        assert!(feed.rows.is_empty() && feed.antenna_seq.is_none());
    }

    // ---------- In and out points ----------

    fn trim(e: &Env, id: i64, in_s: Option<f64>, out_s: Option<f64>) {
        let conn = e.db.lock().unwrap();
        assert!(QueueRepo::new(&conn).set_trim(id, in_s, out_s).unwrap());
    }

    #[tokio::test]
    async fn a_trimmed_film_starts_at_its_in_point_and_ends_at_its_out_point() {
        let mut e = setup();
        let a = e.add_item("a");
        trim(&e, a.id, Some(10.0), Some(30.0));
        e.next();
        e.flush().await;
        assert_eq!(e.factory.calls()[0].start_at_s, 10.0);
        let h = e.factory.handle(0);
        h.produce(8);
        // 10, 14, 18, 22, 26 (up to 30), then the out point ends it.
        for _ in 0..5 {
            assert_eq!(e.key(), clip_key(&a, 0));
        }
        assert_eq!(e.control().out_s, Some(30.0));
        assert_eq!(e.key(), "idle");
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
    }

    #[tokio::test]
    async fn seeks_stay_inside_the_points_and_the_restart_goes_to_the_in_point() {
        let mut e = setup();
        let a = e.add_item("a");
        trim(&e, a.id, Some(10.0), Some(50.0));
        e.air(&a, 2).await;
        e.channel.seek_to(0.0);
        assert_eq!(e.control().position_s, 10.0);
        e.advance(SEEK_QUIET_MS);
        e.tick();
        assert_eq!(e.factory.calls()[1].start_at_s, 10.0);
        e.flush().await;
        // The last second before the out point ends the film like a skip.
        e.channel.seek_to(49.5);
        e.advance(SEEK_QUIET_MS);
        e.tick();
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
    }

    #[tokio::test]
    async fn the_next_film_is_prepared_from_its_in_point_before_the_out_point() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        trim(&e, a.id, None, Some(70.0));
        trim(&e, b.id, Some(5.0), None);
        // 0..12 aired: 58 s left before the out point, within the minute of the prefetch.
        e.air(&a, 3).await;
        let ahead = e.factory.calls();
        assert_eq!(ahead.len(), 2);
        assert_eq!((ahead[1].item_id, ahead[1].start_at_s), (b.id, 5.0));
        // A new in point of the prepared film prepares it again.
        trim(&e, b.id, Some(6.0), None);
        e.channel.on_queue_changed().unwrap();
        assert_eq!(e.channel.prefetched_item_id(), None);
    }

    #[tokio::test]
    async fn a_later_out_point_starts_the_film_again_where_it_is_an_earlier_one_ends_it() {
        let mut e = setup();
        let a = e.add_item("a");
        trim(&e, a.id, None, Some(30.0));
        e.air(&a, 2).await;
        trim(&e, a.id, None, Some(60.0));
        e.channel.on_queue_changed().unwrap();
        e.advance(SEEK_QUIET_MS);
        e.tick();
        assert_eq!(e.factory.calls()[1].start_at_s, 8.0);
        e.flush().await;
        let h = e.factory.handle(1);
        h.produce(2);
        assert_eq!(e.key(), relaunched_key(&a, 0, 1));
        trim(&e, a.id, None, Some(12.0));
        e.channel.on_queue_changed().unwrap();
        assert_eq!(e.starts(), 2, "an earlier out point needs no new start");
        assert_eq!(e.key(), "idle");
        assert_eq!(e.get(a.id).status, ItemStatus::Played);
    }

    // ---------- The next film ready ----------

    #[tokio::test]
    async fn the_next_film_is_resolved_ahead_once_the_film_on_air_has_aired_a_while() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        e.air(&a, 2).await;
        assert!(e.factory.lock().prepared.is_empty(), "8 s aired: not yet");
        let h = e.factory.handle(0);
        h.produce(2);
        e.key();
        e.key();
        assert_eq!(e.factory.lock().prepared, vec![b.id]);
        let next = e.channel.stats().next.unwrap();
        assert_eq!((next.item_id, next.state), (b.id, NextState::Resolving));
        e.flush().await;
        assert_eq!(e.channel.stats().next.unwrap().state, NextState::Resolved);
        h.produce(1);
        e.key();
        assert_eq!(e.factory.lock().prepared.len(), 1, "once per film");
    }

    #[tokio::test]
    async fn a_prepared_clip_that_failed_is_started_once_more() {
        let mut e = setup();
        let a = e.add_item("a");
        let b = e.add_item("b");
        // 48 s aired of 100: the next film is prepared.
        let h = e.air(&a, 12).await;
        assert_eq!(e.factory.lock().ahead, vec![b.id]);
        e.flush().await;
        e.factory.handle(1).set_state(SegmenterState::Failed);
        e.advance(PREFETCH_RETRY_MS);
        h.produce(1);
        e.key();
        assert_eq!(e.factory.lock().ahead, vec![b.id, b.id]);
        assert_eq!(e.channel.stats().next.unwrap().state, NextState::Preparing);
    }

    // ---------- Start at a time ----------

    fn schedule(e: &Env, id: i64, in_ms: u64) {
        let conn = e.db.lock().unwrap();
        let at = i64::try_from(WALL0 + e.now.load(Ordering::SeqCst) + in_ms).unwrap();
        assert!(QueueRepo::new(&conn).set_start_at(id, Some(at)).unwrap());
    }

    #[tokio::test]
    async fn a_film_with_a_start_time_waits_behind_a_countdown_and_starts_on_time_prepared() {
        let mut e = setup();
        let a = e.add_item("a");
        schedule(&e, a.id, 120_000);
        assert_eq!(e.key(), "idle", "the countdown is being made");
        e.flush().await;
        assert_eq!(e.factory.lock().countdowns, vec![(a.id, 120.0)]);
        assert_eq!(e.starts(), 0, "the film waits");
        let countdown = e.factory.handle(0);
        countdown.produce(30);
        let s = e.publish();
        assert_eq!(s.source_key, format!("countdown-{}-1", a.id));
        e.publish();
        e.publish();
        assert_eq!(e.channel.state().status, ChannelStatus::Countdown);
        // The last minute: the film is prepared from its in point.
        e.advance(61_000);
        assert_eq!(e.key(), format!("countdown-{}-1", a.id));
        assert_eq!(e.factory.lock().ahead, vec![a.id]);
        // Its time: the prepared film airs at once, the countdown goes.
        e.advance(59_500);
        e.flush().await;
        e.factory.handle(1).produce(1);
        assert_eq!(e.key(), clip_key(&a, 0));
        assert!(countdown.disposed());
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);
    }

    #[tokio::test]
    async fn a_countdown_that_ran_out_starts_the_film_and_a_pause_or_play_now_drops_it() {
        let mut e = setup();
        let a = e.add_item("a");
        schedule(&e, a.id, 30_000);
        e.next();
        e.flush().await;
        let countdown = e.factory.handle(0);
        countdown.produce(1);
        assert_eq!(e.key(), format!("countdown-{}-1", a.id));
        // Its segments are all out a little before the time: the film starts.
        countdown.set_state(SegmenterState::Done);
        e.advance(28_500);
        e.next();
        assert_eq!(e.get(a.id).status, ItemStatus::Playing);

        let b = e.add_item("b");
        e.channel.skip().unwrap();
        schedule(&e, b.id, 60_000);
        e.next();
        e.flush().await;
        e.channel.pause(PauseReason::Manual);
        assert_eq!(e.key(), "pause");
        assert!(
            e.factory.handle(2).disposed(),
            "a paused countdown is dropped"
        );
        e.channel.play_now(b.id).unwrap();
        assert_eq!(e.get(b.id).start_at, None, "now wins over the start time");
        e.next();
        assert_eq!(e.get(b.id).status, ItemStatus::Playing);
    }
}
