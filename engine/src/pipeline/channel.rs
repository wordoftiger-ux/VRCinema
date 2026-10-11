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
use crate::pipeline::broadcaster::{LogFn, PublishedSegment, SegmentSource, SourceSegment};
use crate::pipeline::segmenter::{SegmentInfo, SegmenterState};
use crate::pipeline::slates::{SlateKind, Slates};
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
    fn error(&self) -> Option<String>;
    /// Source duration from ffprobe (may fill in a duration missing from the queue).
    fn duration_s(&self) -> Option<f64>;
    fn poll(&mut self) -> Vec<SegmentInfo>;
    /// The video is copied, not converted: a start midway lands on the keyframe before it.
    fn copies_video(&self) -> bool {
        false
    }
    fn kill(&mut self);
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
    ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, String>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    Idle,
    Starting,
    Playing,
    Buffering,
    /// Viewers see the pause slate. The queue item stays `playing` in the database.
    Paused,
}

impl ChannelStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Playing => "playing",
            Self::Buffering => "buffering",
            Self::Paused => "paused",
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
    /// viewers are a few segments behind). While a seek waits, its target.
    pub position_s: f64,
    pub duration_s: Option<f64>,
    pub can_pause: bool,
    pub can_seek: bool,
    /// A seek waits for 700 ms of quiet before the film is started again from its target.
    pub seek_pending: bool,
    /// The video is copied: a seek lands on the keyframe before its target.
    pub position_approx: bool,
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

const STALL_ERROR: &str = "Źródło nie dostarczało danych przez 30 s";
const START_TIMEOUT_ERROR: &str = "Przekroczono czas uruchamiania źródła";
const SKIPPED: &str = "Pominięty przez właściciela";
const FFMPEG_STOPPED: &str = "ffmpeg przerwał pracę";
const UNKNOWN_ERROR: &str = "Nieznany błąd";

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
    start_error: Option<String>,
    started_at: u64,
    ready: VecDeque<SegmentInfo>,
    /// Seconds aired from the current process.
    emitted_s: f64,
    stalled_since: Option<u64>,
    /// What an earlier process of this film found out (kept while the new one starts).
    earlier: Probed,
    /// The background start, so a seek can cancel it.
    start: Option<tokio::task::AbortHandle>,
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

    /// Seconds left until the end of the film; `None` for live streams and unknown durations.
    fn remaining_s(&self) -> Option<f64> {
        Some(self.duration_s()? - self.position_s())
    }

    /// Accounts one aired segment of this clip.
    fn emit(&mut self, seg: &SegmentInfo) -> (SourceSegment, SegmentMeta) {
        let position_s = self.position_s();
        self.emitted_s += seg.duration;
        self.stalled_since = None;
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
    fn failure(&self) -> Option<String> {
        if let Some(error) = &self.start_error {
            return Some(error.clone());
        }
        let handle = self.handle.as_ref()?;
        matches!(
            handle.state(),
            SegmenterState::Failed | SegmenterState::Killed
        )
        .then(|| handle.error().unwrap_or_else(|| FFMPEG_STOPPED.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetaKind {
    Clip,
    Slate(SlateKind),
}

#[derive(Debug, Clone)]
struct SegmentMeta {
    kind: MetaKind,
    item: Option<QueueItem>,
    position_s: f64,
}

/// Result of an asynchronous clip start.
struct Launch {
    clip_id: u64,
    result: Result<Box<dyn ClipHandle>, String>,
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
}

#[derive(Debug, Clone, Copy)]
enum SeekRequest {
    To(f64),
    By(f64),
}

impl SeekRequest {
    /// The target from `from_s`, never before the start; `None` when the request is not finite
    /// or the film's length is unknown and the request is not a forward relative seek (or a
    /// seek back to the start).
    fn target(self, from_s: f64, duration_s: Option<f64>) -> Option<f64> {
        let target = match self {
            Self::To(to) => to,
            Self::By(delta) => from_s + delta,
        };
        if !target.is_finite() {
            return None;
        }
        let allowed = duration_s.is_some()
            || match self {
                Self::To(to) => to <= 0.0,
                Self::By(delta) => delta >= 0.0,
            };
        allowed.then_some(target.max(0.0))
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
    slate_index: [usize; 3],
    last_slate_kind: Option<SlateKind>,
    previous_clip_segment: Option<PathBuf>,
    pending_meta: HashMap<PathBuf, SegmentMeta>,
    aired: VecDeque<SegmentMeta>,
    next_clip_id: u64,
    launch_tx: mpsc::UnboundedSender<Launch>,
    launch_rx: mpsc::UnboundedReceiver<Launch>,
}

impl Channel {
    pub fn new(mut o: ChannelOptions) -> Self {
        let now = o.now.take().unwrap_or_else(default_clock);
        let (launch_tx, launch_rx) = mpsc::unbounded_channel();
        Self {
            o,
            now,
            current: None,
            prefetched: None,
            parked: None,
            pause_reasons: BTreeSet::new(),
            pending_seek: None,
            slate_index: [0; 3],
            last_slate_kind: None,
            previous_clip_segment: None,
            pending_meta: HashMap::new(),
            aired: VecDeque::new(),
            next_clip_id: 0,
            launch_tx,
            launch_rx,
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
        for mut clip in [self.current.take(), self.prefetched.take()]
            .into_iter()
            .flatten()
        {
            clip.dispose();
        }
    }

    pub fn skip(&mut self) -> anyhow::Result<()> {
        if let Some(p) = self.parked.take() {
            // Paused film: its clip is already gone. The channel stays paused for the next one.
            return self
                .queue(|q| q.finish(p.item.id, ItemStatus::Played, Some(SKIPPED)))
                .map(|_| ());
        }
        self.finish_current(ItemStatus::Played, Some(SKIPPED))
    }

    /// Airs `slates` from now on (a new address was drawn on them). Films are not touched; the
    /// next slate segment comes from the new set, from its first segment, under a new source key.
    pub fn replace_slates(&mut self, slates: Slates) {
        self.o.slates = slates;
        self.slate_index = [0; 3];
        self.last_slate_kind = None;
    }

    /// Called after queue changes (removal, reorder, approval).
    pub fn on_queue_changed(&mut self) -> anyhow::Result<()> {
        if let Some(id) = self.current.as_ref().map(|c| c.item.id) {
            let fresh = self.queue(|q| q.get(id))?;
            if fresh.is_none_or(|f| f.status != ItemStatus::Playing) {
                self.drop_current();
            }
        }
        if let Some(id) = self.parked.as_ref().map(|p| p.item.id) {
            let fresh = self.queue(|q| q.get(id))?;
            if fresh.is_none_or(|f| f.status != ItemStatus::Playing) {
                self.parked = None;
            }
        }
        if let Some(id) = self.prefetched.as_ref().map(|c| c.item.id) {
            let channel_id = self.o.channel_id;
            let head = self.queue(|q| q.next_queued(channel_id))?;
            if head.is_none_or(|h| h.id != id) {
                self.drop_prefetched();
            }
        }
        Ok(())
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
                    is_live: item.is_live,
                });
                match (meta.kind, now_playing) {
                    (MetaKind::Slate(SlateKind::Idle), _) => (ChannelStatus::Idle, None),
                    (MetaKind::Slate(SlateKind::Pause), current) => {
                        (ChannelStatus::Paused, current)
                    }
                    (_, None) => (ChannelStatus::Buffering, None),
                    (MetaKind::Clip, current) => (ChannelStatus::Playing, current),
                    (MetaKind::Slate(SlateKind::Buffering), current) => {
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
            };
        }
        let Some(c) = &self.current else {
            return ControlState::default();
        };
        let live = c.item.is_live;
        let probed = c.probed();
        let pending = self.pending_seek_of(c.item.id);
        let position_s = match pending {
            Some(target) => probed.duration_s.map_or(target, |d| target.min(d)),
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
        }
    }

    /// Target of the seek waiting for film `item_id`.
    fn pending_seek_of(&self, item_id: i64) -> Option<f64> {
        self.pending_seek
            .as_ref()
            .filter(|s| s.item_id == item_id)
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
        if let Some(p) = &self.parked {
            let Some(target) = request.target(p.position_s, p.probed.duration_s) else {
                self.log(&format!(
                    "Pominięto skok {request:?}: niedozwolony dla tego filmu"
                ));
                return;
            };
            if ends_film(target, p.probed.duration_s) {
                self.log(&format!(
                    "Skok na koniec wstrzymanego filmu #{} \"{}\" — film pominięty",
                    p.item.id, p.item.title
                ));
                if let Err(e) = self.skip() {
                    self.log(&format!("Błąd kanału przy skoku: {e}"));
                }
            } else if let Some(p) = self.parked.as_mut() {
                p.position_s = target;
            }
            return;
        }
        let Some(c) = self.current.as_ref().filter(|c| !c.item.is_live) else {
            self.log(&format!(
                "Pominięto skok {request:?}: nic nie leci albo to transmisja na żywo"
            ));
            return;
        };
        let from = self
            .pending_seek_of(c.item.id)
            .unwrap_or_else(|| c.position_s());
        let Some(target_s) = request.target(from, c.duration_s()) else {
            self.log(&format!(
                "Pominięto skok {request:?}: niedozwolony dla tego filmu"
            ));
            return;
        };
        self.pending_seek = Some(PendingSeek {
            item_id: c.item.id,
            target_s,
            due_at: (self.now)().saturating_add(SEEK_QUIET_MS),
        });
    }

    /// Called by the engine loop every tick: runs a seek whose quiet has passed.
    pub fn on_tick(&mut self) {
        if let Err(e) = self.run_due_seek() {
            self.log(&format!("Błąd kanału przy skoku: {e}"));
        }
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
        if ends_film(seek.target_s, c.duration_s()) {
            self.log(&format!(
                "Skok na koniec filmu #{} \"{}\" — film pominięty",
                c.item.id, c.item.title
            ));
            return self.finish(c, ItemStatus::Played, Some(SKIPPED));
        }
        // The prefetched film may now be far from its turn; it is prefetched again near the end.
        self.drop_prefetched();
        if let Some(error) = c.failure() {
            // The process had already failed: the usual retry accounting, from the target.
            c.base_s = seek.target_s;
            c.emitted_s = 0.0;
            c.ready.clear();
            self.current = Some(c);
            return self.handle_failure(error);
        }
        let mut off = Parked::off_air(c);
        self.log(&format!(
            "Skok: film #{} \"{}\" z {:.0} s na {:.0} s",
            off.item.id, off.item.title, off.position_s, seek.target_s
        ));
        off.position_s = seek.target_s;
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
            .ok_or_else(|| anyhow::anyhow!("Nie ma pozycji #{item_id}."))?;
        let chosen = match item.status {
            ItemStatus::Queued => {
                self.queue(|q| q.move_item(item.id, 0))?;
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
                    })
                })?;
                again.id
            }
            status => anyhow::bail!(
                "Pozycji #{item_id} nie można odtworzyć teraz (status {}).",
                status.as_str()
            ),
        };
        self.log(&format!("Odtwórz teraz: \"{}\" (#{chosen})", item.title));
        self.skip()?;
        // The owner's explicit "play this now" ends the manual pause (the paused film was just
        // skipped, so nothing is relaunched); automatic reasons keep holding the film.
        self.resume(PauseReason::Manual);
        self.on_queue_changed()
    }

    /// Takes the current film off the air and shows the pause slate until every reason is
    /// resumed. A live stream is not paused (it keeps airing; the next film waits for the
    /// resume). The queue item stays `playing`; the clip is disposed, the position remembered.
    pub fn pause(&mut self, reason: PauseReason) {
        let first = self.pause_reasons.is_empty();
        self.pause_reasons.insert(reason);
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
        let target = seek.filter(|s| s.item_id == c.item.id).map(|s| s.target_s);
        if let Some(target) = target
            && ends_film(target, c.duration_s())
        {
            if let Err(e) = self.finish(c, ItemStatus::Played, Some(SKIPPED)) {
                self.log(&format!("Błąd kanału przy pauzie: {e}"));
            }
            return;
        }
        let mut p = Parked::off_air(c);
        if let Some(target) = target {
            p.position_s = target;
        }
        self.log(&format!(
            "Pauza ({}): film #{} \"{}\" wstrzymany na {:.0} s",
            reason.as_str(),
            p.item.id,
            p.item.title,
            p.position_s
        ));
        self.parked = Some(p);
    }

    /// A clip that already ended or failed must not be parked: a finished film is finished
    /// (played), a failure goes through the normal retry accounting.
    fn settle_before_parking(&mut self) {
        self.poll_launches();
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
                        failure = Some(error.unwrap_or_else(|| FFMPEG_STOPPED.to_string()));
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
            self.log(&format!("Błąd kanału przy pauzie: {e}"));
        }
    }

    /// Clears `reason`; once no reason is left the paused film restarts from where it stopped
    /// (behind the buffering slate) and queued films start again.
    pub fn resume(&mut self, reason: PauseReason) {
        if !self.pause_reasons.remove(&reason) || !self.pause_reasons.is_empty() {
            return;
        }
        if let Some(p) = self.parked.take() {
            self.log(&format!(
                "Wznowienie: film #{} \"{}\" od {:.0} s",
                p.item.id, p.item.title, p.position_s
            ));
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
        let next = self.prefetched.as_ref().map(|c| NextInfo {
            title: c.item.title.clone(),
            state: if c.failure().is_some() {
                NextState::Failed
            } else if c.is_starting() {
                NextState::Preparing
            } else {
                NextState::Ready
            },
            ready_segments: c.ready.len(),
        });
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
            on_air: on_air.map(|(info, _)| info),
            hls: None,
            next,
            ffmpeg,
        }
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
    pub fn fail_item(&mut self, id: i64, error: &str) -> anyhow::Result<()> {
        self.queue(|q| q.finish(id, ItemStatus::Failed, Some(error)))
            .map(|_| ())
    }

    /// Hands finished clip starts to their clips; a start whose clip was dropped meanwhile is
    /// disposed.
    pub fn poll_launches(&mut self) {
        while let Ok(launch) = self.launch_rx.try_recv() {
            let target = [self.current.as_mut(), self.prefetched.as_mut()]
                .into_iter()
                .flatten()
                .find(|c| c.id == launch.clip_id);
            match (target, launch.result) {
                (Some(clip), Ok(handle)) => clip.handle = Some(handle),
                (Some(clip), Err(e)) => {
                    clip.start_error = Some(if e.is_empty() {
                        UNKNOWN_ERROR.to_string()
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

    fn log(&self, msg: &str) {
        if let Some(log) = &self.o.log {
            log(msg);
        }
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
            if self.current.is_none() {
                if !self.pause_reasons.is_empty() {
                    return self.paused_slate();
                }
                self.promote()?;
            }
            let Some(c) = self.current.as_mut() else {
                return self.slate(SlateKind::Idle);
            };

            if let Some(error) = c.start_error.clone() {
                self.handle_failure(error)?;
                continue;
            }
            if let Some(handle) = c.handle.as_mut() {
                let fresh = handle.poll();
                c.ready.extend(fresh);
            }

            if let Some(seg) = c.ready.pop_front() {
                let (out, meta) = c.emit(&seg);
                let remaining = c.remaining_s();
                self.last_slate_kind = None;
                self.previous_clip_segment = Some(seg.path.clone());
                self.pending_meta.insert(seg.path, meta);
                self.maybe_prefetch(remaining);
                return Ok(out);
            }

            let now = (self.now)();
            match c.handle.as_ref().map(|h| (h.state(), h.error())) {
                Some((SegmenterState::Done, _)) => {
                    self.finish_current(ItemStatus::Played, None)?;
                    continue;
                }
                Some((SegmenterState::Failed | SegmenterState::Killed, error)) => {
                    self.handle_failure(error.unwrap_or_else(|| FFMPEG_STOPPED.to_string()))?;
                    continue;
                }
                Some(_) => {}
                None => {
                    if now.saturating_sub(c.started_at) > start_timeout_ms {
                        self.handle_failure(START_TIMEOUT_ERROR.to_string())?;
                        continue;
                    }
                    return self.slate(SlateKind::Buffering);
                }
            }
            let stalled_since = *c.stalled_since.get_or_insert(now);
            if now.saturating_sub(stalled_since) > stall_timeout_ms {
                if let Some(handle) = c.handle.as_mut() {
                    handle.kill();
                }
                self.finish_current(ItemStatus::Failed, Some(STALL_ERROR))?;
                continue;
            }
            return self.slate(SlateKind::Buffering);
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

    fn slate(&mut self, kind: SlateKind) -> anyhow::Result<SourceSegment> {
        let list = match kind {
            SlateKind::Idle => &self.o.slates.idle,
            SlateKind::Buffering => &self.o.slates.buffering,
            SlateKind::Pause => &self.o.slates.pause,
        };
        if list.is_empty() {
            anyhow::bail!("Plansza \"{}\" nie ma fragmentów.", kind.as_str());
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
            SlateKind::Buffering => self.current.as_ref().map(|c| c.item.clone()),
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
            let mut clip = match self.prefetched.take() {
                Some(clip) => clip,
                None => self.launch(head.clone(), 0.0, 0),
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

    fn maybe_prefetch(&mut self, remaining_s: Option<f64>) {
        if self.prefetched.is_some() {
            return;
        }
        let Some(remaining_s) = remaining_s else {
            return;
        };
        if remaining_s > self.o.prefetch_s.unwrap_or(DEFAULT_PREFETCH_S) {
            return;
        }
        let channel_id = self.o.channel_id;
        match self.queue(|q| q.next_queued(channel_id)) {
            Ok(Some(head)) => self.prefetched = Some(self.launch(head, 0.0, 0)),
            Ok(None) => {}
            Err(e) => self.log(&format!("Błąd kolejki przy wyprzedzaniu: {e}")),
        }
    }

    /// Starts the clip in a background task; the result arrives through `poll_launches`.
    fn launch(&mut self, item: QueueItem, start_at_s: f64, attempt: u32) -> ActiveClip {
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
        };
        let start = self.o.factory.create(item, start_at_s, attempt);
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
            Err(e) => clip.start_error = Some(e.to_string()),
        }
        clip
    }

    fn handle_failure(&mut self, error: String) -> anyhow::Result<()> {
        let Some(mut c) = self.current.take() else {
            return Ok(());
        };
        self.log(&format!(
            "Film #{} \"{}\": {error}",
            c.item.id, c.item.title
        ));
        let probed = c.probed();
        if let Some(mut handle) = c.handle.take() {
            handle.dispose();
        }
        if c.attempt == 0 {
            // One retry from where it stopped (resolved anew, the old address may have expired).
            let resume_at = if c.item.is_live { 0.0 } else { c.position_s() };
            let mut retry = self.launch(c.item, resume_at, 1);
            retry.earlier = probed;
            self.current = Some(retry);
            return Ok(());
        }
        self.finish(c, ItemStatus::Failed, Some(&error))
    }

    fn finish_current(&mut self, status: ItemStatus, error: Option<&str>) -> anyhow::Result<()> {
        match self.current.take() {
            Some(c) => self.finish(c, status, error),
            None => Ok(()),
        }
    }

    fn finish(
        &mut self,
        mut c: ActiveClip,
        status: ItemStatus,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        // Conditional: an owner's removal (status `removed`) is never overwritten.
        let result = self.queue(|q| q.finish(c.item.id, status, error));
        c.dispose();
        self.forget_seek_of(c.item.id);
        result.map(|_| ())
    }

    fn drop_current(&mut self) {
        if let Some(mut c) = self.current.take() {
            c.dispose();
            self.forget_seek_of(c.item.id);
        }
    }

    /// A seek waiting for a film that is gone never runs.
    fn forget_seek_of(&mut self, item_id: i64) {
        if self.pending_seek_of(item_id).is_some() {
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
                self.log(&format!("Błąd kanału: {e}"));
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
        let meta = match self.pending_meta.get(&seg.path) {
            None => return,
            Some(meta) if meta.kind == MetaKind::Clip => self.pending_meta.remove(&seg.path),
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
    use crate::stats::StreamMode;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::oneshot;

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
        }
    }

    struct FakeState {
        state: SegmenterState,
        error: Option<String>,
        killed: bool,
        disposed: bool,
        pending: Vec<SegmentInfo>,
        n: usize,
        item_id: i64,
        start_at_s: f64,
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
            self.lock().error = Some(error.into());
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
        fn error(&self) -> Option<String> {
            self.fake.lock().error.clone()
        }
        fn duration_s(&self) -> Option<f64> {
            self.duration_s
        }
        fn copies_video(&self) -> bool {
            self.copies_video
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
        /// When set, every create() waits until its gate is released.
        manual: bool,
        gates: Vec<oneshot::Sender<()>>,
        /// Runs inside create(), i.e. between the channel reading the queue head and starting it.
        on_create: Option<CreateHook>,
        /// Clips started from now on copy the video (instead of converting it).
        copies_video: bool,
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
        fn create(
            &self,
            item: QueueItem,
            start_at_s: f64,
            attempt: u32,
        ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, String>> {
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
                return Box::pin(async { Err("nie da się rozpoznać".to_string()) });
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
        channel: Channel,
    }

    fn channel_for(
        db: &Arc<Mutex<Connection>>,
        factory: &FakeFactory,
        now: &Arc<AtomicU64>,
        removed: &Arc<Mutex<Vec<PathBuf>>>,
    ) -> Channel {
        let mut o = ChannelOptions::new(
            MAIN_CHANNEL_ID,
            Arc::clone(db),
            Arc::new(factory.clone()),
            slates(),
        );
        let clock = Arc::clone(now);
        o.now = Some(Box::new(move || clock.load(Ordering::SeqCst)));
        let removed = Arc::clone(removed);
        o.remove_file = Some(Box::new(move |p: &Path| {
            removed.lock().unwrap().push(p.to_path_buf())
        }));
        Channel::new(o)
    }

    fn setup() -> Env {
        let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
        let factory = FakeFactory::default();
        let now = Arc::new(AtomicU64::new(0));
        let removed = Arc::new(Mutex::new(Vec::new()));
        let mut channel = channel_for(&db, &factory, &now, &removed);
        channel.start().unwrap();
        Env {
            db,
            factory,
            now,
            removed,
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
        assert_eq!(e.key(), "buffering");
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
        assert!(fresh.error.unwrap().contains("Connection reset"));
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
        assert!(fresh.error.unwrap().contains("nie da się rozpoznać"));
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
            fresh.error.as_deref(),
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
        assert!(fresh.error.unwrap().to_lowercase().contains("pomini"));
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
        let mut c2 = channel_for(&e.db, &e.factory, &e.now, &e.removed);
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
            fresh.error.as_deref(),
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
        assert_eq!(e.key(), "buffering");
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
        e.channel.fail_item(a.id, "Błąd wewnętrzny").unwrap();
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert_eq!(fresh.error.as_deref(), Some("Błąd wewnętrzny"));
        assert!(fresh.finished_at.is_some());
        // A removed item stays removed.
        let b = e.add_item("b");
        e.set_status(b.id, ItemStatus::Removed);
        e.channel.fail_item(b.id, "Błąd wewnętrzny").unwrap();
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
        assert_eq!(fresh.error.as_deref(), Some("Pominięty przez właściciela"));
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
        assert_eq!(e.key(), "buffering");
        e.flush().await;
        let h1 = e.factory.handle(1);
        h1.produce(1);
        assert_eq!(e.key(), clip_key(&a, 1));
        h1.set_state(SegmenterState::Failed);
        h1.set_error("boom");
        e.channel.pause(PauseReason::Tunnel);
        let fresh = e.get(a.id);
        assert_eq!(fresh.status, ItemStatus::Failed);
        assert!(fresh.error.unwrap().contains("boom"));
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
        assert_eq!(fresh.error.as_deref(), Some(SKIPPED));
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
        assert_eq!(fresh.error.as_deref(), Some(SKIPPED));
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
        assert_eq!(fresh.error.as_deref(), Some(SKIPPED));
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
        assert!(e.channel.play_now(9_999).is_err());
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
}
