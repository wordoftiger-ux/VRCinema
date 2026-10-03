//! Channel orchestration: decides what airs and feeds the broadcaster. Port of
//! `src/pipeline/channel.ts`.
//!
//! Rule: `next()` always returns something — a clip segment or a slate — so the channel never
//! goes silent.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Connection;
use tokio::sync::mpsc;

use crate::db::{ItemStatus, QueueItem, QueueRepo};
use crate::pipeline::broadcaster::{LogFn, PublishedSegment, SegmentSource, SourceSegment};
use crate::pipeline::segmenter::{SegmentInfo, SegmenterState};
use crate::pipeline::slates::{SlateKind, Slates};

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
    fn kill(&mut self);
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
}

impl ChannelStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Playing => "playing",
            Self::Buffering => "buffering",
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
    pub status: ChannelStatus,
    pub current: Option<NowPlaying>,
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
    /// Film second the current process started from.
    base_s: f64,
    handle: Option<Box<dyn ClipHandle>>,
    start_error: Option<String>,
    started_at: u64,
    ready: VecDeque<SegmentInfo>,
    /// Seconds aired from the current process.
    emitted_s: f64,
    stalled_since: Option<u64>,
}

impl ActiveClip {
    fn position_s(&self) -> f64 {
        self.base_s + self.emitted_s
    }

    /// Seconds left until the end of the film; `None` for live streams and unknown durations.
    fn remaining_s(&self) -> Option<f64> {
        if self.item.is_live {
            return None;
        }
        let duration = self
            .item
            .duration_s
            .or_else(|| self.handle.as_ref().and_then(|h| h.duration_s()))?;
        Some(duration - self.position_s())
    }

    /// Accounts one aired segment of this clip.
    fn emit(&mut self, seg: &SegmentInfo) -> (SourceSegment, SegmentMeta) {
        let position_s = self.position_s();
        self.emitted_s += seg.duration;
        self.stalled_since = None;
        let out = SourceSegment {
            path: seg.path.clone(),
            duration: seg.duration,
            source_key: format!("clip-{}-{}", self.item.id, self.attempt),
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

fn slate_slot(kind: SlateKind) -> usize {
    match kind {
        SlateKind::Idle => 0,
        SlateKind::Buffering => 1,
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
    slate_index: [usize; 2],
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
            slate_index: [0; 2],
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
        for mut clip in [self.current.take(), self.prefetched.take()]
            .into_iter()
            .flatten()
        {
            clip.dispose();
        }
    }

    pub fn skip(&mut self) -> anyhow::Result<()> {
        self.finish_current(ItemStatus::Played, Some(SKIPPED))
    }

    /// Called after queue changes (removal, reorder, approval).
    pub fn on_queue_changed(&mut self) -> anyhow::Result<()> {
        if let Some(id) = self.current.as_ref().map(|c| c.item.id) {
            let fresh = self.queue(|q| q.get(id))?;
            if fresh.is_none_or(|f| f.status != ItemStatus::Playing) {
                self.drop_current();
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
        let idle = ChannelState {
            status: ChannelStatus::Idle,
            current: None,
        };
        let lag = self
            .o
            .viewer_lag_segments
            .unwrap_or(DEFAULT_VIEWER_LAG_SEGMENTS);
        let Some(meta) = self.aired.get(self.aired.len().saturating_sub(lag)) else {
            return idle;
        };
        match (meta.kind, &meta.item) {
            (MetaKind::Slate(SlateKind::Idle), _) => idle,
            (_, None) => ChannelState {
                status: ChannelStatus::Buffering,
                current: None,
            },
            (kind, Some(item)) => ChannelState {
                status: if kind == MetaKind::Clip {
                    ChannelStatus::Playing
                } else {
                    ChannelStatus::Buffering
                },
                current: Some(NowPlaying {
                    item_id: item.id,
                    title: item.title.clone(),
                    duration_s: item.duration_s,
                    position_s: js_round(meta.position_s),
                    requester_name: item.requester_name.clone(),
                    is_live: item.is_live,
                }),
            },
        }
    }

    /// The item being started in the background ahead of time (shown in the admin panel).
    pub fn prefetched_item_id(&self) -> Option<i64> {
        self.prefetched.as_ref().map(|c| c.item.id)
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

    fn slate(&mut self, kind: SlateKind) -> anyhow::Result<SourceSegment> {
        let list = match kind {
            SlateKind::Idle => &self.o.slates.idle,
            SlateKind::Buffering => &self.o.slates.buffering,
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
            SlateKind::Idle => None,
        };
        let position_s = self.current.as_ref().map_or(0.0, ActiveClip::position_s);
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
            source_key: kind.as_str().to_string(),
            index: index as u64,
        })
    }

    /// The current film ended or there is none: take the prefetched one or the queue head.
    fn promote(&mut self) -> anyhow::Result<()> {
        let channel_id = self.o.channel_id;
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
        clip.item = head;
        self.current = Some(clip);
        let fresh = self.queue(|q| {
            q.set_status(id, ItemStatus::Playing, None)?;
            q.get(id)
        })?;
        if let (Some(fresh), Some(current)) = (fresh, self.current.as_mut()) {
            current.item = fresh;
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
            base_s: start_at_s,
            handle: None,
            start_error: None,
            started_at: (self.now)(),
            ready: VecDeque::new(),
            emitted_s: 0.0,
            stalled_since: None,
        };
        let start = self.o.factory.create(item, start_at_s, attempt);
        let tx = self.launch_tx.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move {
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
        if let Some(mut handle) = c.handle.take() {
            handle.dispose();
        }
        if c.attempt == 0 {
            // One retry from where it stopped (resolved anew, the old address may have expired).
            let resume_at = if c.item.is_live { 0.0 } else { c.position_s() };
            self.current = Some(self.launch(c.item, resume_at, 1));
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
        let result = self.queue(|q| q.set_status(c.item.id, status, error));
        c.dispose();
        result
    }

    fn drop_current(&mut self) {
        if let Some(mut c) = self.current.take() {
            c.dispose();
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

    #[derive(Default)]
    struct FactoryState {
        handles: Vec<Fake>,
        calls: Vec<Call>,
        fail_next: u32,
        /// When set, every create() waits until its gate is released.
        manual: bool,
        gates: Vec<oneshot::Sender<()>>,
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
            if s.fail_next > 0 {
                s.fail_next -= 1;
                return Box::pin(async { Err("nie da się rozpoznać".to_string()) });
            }
            let gate = s.manual.then(|| {
                let (tx, rx) = oneshot::channel();
                s.gates.push(tx);
                rx
            });
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
        /// Lets the spawned clip starts run, then hands their results to the channel.
        async fn flush(&mut self) {
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            self.channel.poll_launches();
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
}
