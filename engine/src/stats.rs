//! "Nerd stats" (spec section 5): counters the channel keeps in memory and the shapes the
//! `GET /api/admin/channels/main/stats` route reports. Viewer addresses are counted, never
//! stored in a result.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::pipeline::tsrewrite::ts_ahead;
use crate::tunnel::{TunnelState, TunnelStats};

/// An address with a fetch this recent counts as watching.
pub const WATCHING_WINDOW: Duration = Duration::from_secs(30);
/// Samples of the bytes history (one per second).
pub const HISTORY_LEN: usize = 60;
/// `bytes_per_s` is averaged over this many seconds.
const RATE_WINDOW_S: u64 = 5;
/// Most addresses remembered at once (bounds memory under a flood). Beyond it a new address is
/// still counted in the fetches and bytes but not as a watcher, so `watching` saturates here.
pub const MAX_TRACKED_ADDRESSES: usize = 4096;

/// What the viewers' `.ts` fetches amount to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewersView {
    /// Distinct addresses with a fetch in the last 30 s, at most [`MAX_TRACKED_ADDRESSES`].
    pub watching: usize,
    /// Fetches in the last 60 s.
    pub fetches_per_min: u64,
    /// Bytes served per second, averaged over the last 5 s.
    pub bytes_per_s: u64,
    /// Bytes served in each of the last 60 seconds, the newest last.
    pub history: Vec<u64>,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    second: u64,
    bytes: u64,
    fetches: u64,
}

#[derive(Debug, Default)]
struct ViewerInner {
    last_seen: HashMap<IpAddr, Instant>,
    /// One bucket per second that had a fetch, oldest first.
    buckets: VecDeque<Bucket>,
    /// Second of the latest sweep of `last_seen`.
    pruned_second: u64,
}

/// Counts segment fetches per viewer address and per second. Recording takes a short lock; the
/// addresses themselves are only ever counted.
#[derive(Debug)]
pub struct ViewerStats {
    base: Instant,
    inner: Mutex<ViewerInner>,
}

impl Default for ViewerStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewerStats {
    pub fn new() -> Self {
        Self::starting_at(Instant::now())
    }

    /// Seconds are counted from `base` (tests start the clock themselves).
    pub fn starting_at(base: Instant) -> Self {
        Self {
            base,
            inner: Mutex::new(ViewerInner::default()),
        }
    }

    /// `ip` fetched a segment of `bytes` bytes.
    pub fn record(&self, ip: IpAddr, bytes: u64) {
        self.record_at(ip, bytes, Instant::now());
    }

    pub fn record_at(&self, ip: IpAddr, bytes: u64, now: Instant) {
        self.add(Some(ip), bytes, now);
    }

    /// A fetch from an address that could not be read: counted, but nobody to remember.
    pub fn record_unknown(&self, bytes: u64) {
        self.add(None, bytes, Instant::now());
    }

    fn add(&self, ip: Option<IpAddr>, bytes: u64, now: Instant) {
        let second = self.second_of(now);
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        match inner.buckets.back_mut() {
            Some(b) if b.second == second => {
                b.bytes = b.bytes.saturating_add(bytes);
                b.fetches += 1;
            }
            _ => inner.buckets.push_back(Bucket {
                second,
                bytes,
                fetches: 1,
            }),
        }
        while inner
            .buckets
            .front()
            .is_some_and(|b| b.second + HISTORY_LEN as u64 <= second)
        {
            inner.buckets.pop_front();
        }
        // Old addresses are dropped at most once a second, so a request never pays for a sweep
        // of the whole table.
        if inner.pruned_second != second {
            inner.pruned_second = second;
            inner
                .last_seen
                .retain(|_, seen| now.saturating_duration_since(*seen) <= WATCHING_WINDOW);
        }
        if let Some(ip) = ip {
            if let Some(seen) = inner.last_seen.get_mut(&ip) {
                *seen = now;
            } else if inner.last_seen.len() < MAX_TRACKED_ADDRESSES {
                inner.last_seen.insert(ip, now);
            }
        }
    }

    pub fn snapshot(&self, now: Instant) -> ViewersView {
        let current = self.second_of(now);
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let watching = inner
            .last_seen
            .values()
            .filter(|seen| now.saturating_duration_since(**seen) <= WATCHING_WINDOW)
            .count();
        let mut history = vec![0u64; HISTORY_LEN];
        let mut fetches = 0;
        let mut recent_bytes = 0u64;
        for b in &inner.buckets {
            let age = current.saturating_sub(b.second);
            if b.second > current || age >= HISTORY_LEN as u64 {
                continue;
            }
            history[HISTORY_LEN - 1 - age as usize] = b.bytes;
            fetches += b.fetches;
            if age < RATE_WINDOW_S {
                recent_bytes = recent_bytes.saturating_add(b.bytes);
            }
        }
        ViewersView {
            watching,
            fetches_per_min: fetches,
            bytes_per_s: recent_bytes / RATE_WINDOW_S,
            history,
        }
    }

    /// Addresses currently remembered (tests).
    pub fn tracked(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_seen
            .len()
    }

    fn second_of(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.base).as_secs()
    }
}

/// How far (90 kHz, 1 s) a segment may start before the previous one ended: audio runs ahead of
/// video and B-frames reorder, so neighbouring segments overlap a little. A step back deeper
/// into the previous segment than this is a regression; a smaller one is not noticed.
const DTS_TOLERANCE: i64 = 90_000;

/// Whether the published timeline ever stepped back (the channel promises it never does).
/// Fed with the span (first and last DTS) of every published segment, in publishing order.
/// Timestamps are wrapped modulo 2^33 (about 26.5 h), so they are compared on the wrapped
/// timeline: a drop in the numbers across the wrap is not a step back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtsMonitor {
    previous_end: Option<u64>,
    continuous: bool,
}

impl Default for DtsMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl DtsMonitor {
    pub fn new() -> Self {
        Self {
            previous_end: None,
            continuous: true,
        }
    }

    /// A segment was published that covers `span` (see `tsrewrite::dts_span`). Starting more
    /// than [`DTS_TOLERANCE`] before the previous segment's end turns the flag off for good.
    pub fn observe(&mut self, span: Option<(u64, u64)>) {
        let Some((start, end)) = span else {
            return;
        };
        if self
            .previous_end
            .is_some_and(|previous| ts_ahead(previous, start) < -DTS_TOLERANCE)
        {
            self.continuous = false;
        }
        self.previous_end = Some(end);
    }

    pub fn is_continuous(&self) -> bool {
        self.continuous
    }
}

/// The playlist the broadcaster publishes, in numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HlsCounters {
    /// Media sequence of the first segment in the window.
    pub sequence: u64,
    /// Segments in the playlist.
    pub window: usize,
    pub segment_s: f64,
    /// Discontinuity tags published since the broadcaster started.
    pub discontinuities: u64,
    /// End of the last published packet on the channel timeline, in seconds.
    pub last_pts: Option<f64>,
    /// No segment ever started before the one published before it.
    pub dts_continuous: bool,
}

/// Parsed analysis of the source of the film being cut.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeSummary {
    pub codec: String,
    pub height: u32,
    pub fps: f64,
    pub pix_fmt: String,
    pub has_audio: bool,
    pub duration_s: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamMode {
    Copy,
    Transcode,
}

/// What a running clip knows about itself (see `ClipHandle::stats`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipStats {
    pub probe: Option<ProbeSummary>,
    pub mode: Option<StreamMode>,
    /// The video encoder; `None` when the video is copied.
    pub encoder: Option<String>,
    pub pid: Option<u32>,
    /// ffmpeg's `speed=` (1.0: as fast as it plays).
    pub speed: Option<f64>,
    /// The conversion the info ticker needs has been slower than real time for 30 s.
    pub ticker_slow: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnAirInfo {
    pub title: String,
    pub source: String,
    /// Where the channel is cutting the film (viewers are a few segments behind).
    pub position_s: f64,
    pub attempt: u32,
    pub probe: Option<ProbeSummary>,
    pub mode: Option<StreamMode>,
    pub encoder: Option<String>,
    /// Cut segments waiting for the broadcaster.
    pub ready_segments: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NextState {
    /// Resolving and analysing the source ahead of time (no clip yet).
    Resolving,
    /// Resolved and analysed ahead of time: its start skips that step.
    Resolved,
    /// Resolving and analysing the source.
    Preparing,
    /// Cutting segments.
    Ready,
    Failed,
}

/// The film prepared for after the current one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextInfo {
    pub item_id: i64,
    pub title: String,
    pub state: NextState,
    pub ready_segments: usize,
}

/// The encoder process; CPU and memory are filled in per request (see [`process_usage`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegInfo {
    pub pid: u32,
    pub speed: Option<f64>,
    pub cpu_percent: Option<f64>,
    pub rss_bytes: Option<u64>,
}

/// Everything the channel loop publishes for the stats route.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelStats {
    pub on_air: Option<OnAirInfo>,
    pub hls: Option<HlsCounters>,
    pub next: Option<NextInfo>,
    pub ffmpeg: Option<FfmpegInfo>,
    /// The film on air converts slower than real time because of the info ticker (the app
    /// shows a banner).
    pub ticker_slow: bool,
    /// The film being started (its first start, not a relaunch after a pause or a seek), while
    /// it has not aired long: the panel's "Now loading" card.
    pub loading: Option<LoadingInfo>,
}

/// The start of a film as the channel sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadingInfo {
    pub item_id: i64,
    /// Since the start was asked for (milliseconds).
    pub elapsed_ms: u64,
    /// Seconds of it cut so far (0: nothing yet).
    pub emitted_s: f64,
}

/// The tunnel, as the stats show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatsView {
    /// `connecting`, `online`, `reconnecting` or `error`.
    pub state: &'static str,
    /// Seconds since the tunnel last came online; `None` while it is not.
    pub online_s: Option<u64>,
    /// How many times cloudflared had to be started again.
    pub reconnects: u64,
    pub connections: usize,
    /// Cloudflare locations of the connections (`waw01`, ...).
    pub locations: Vec<String>,
}

impl TunnelStatsView {
    /// The tunnel as the window sees it (`None`: switched off) with the manager's numbers.
    pub fn of(state: &TunnelState, stats: &TunnelStats) -> Option<Self> {
        let (name, online) = match state {
            TunnelState::Off => return None,
            TunnelState::Connecting => ("connecting", false),
            TunnelState::Online { .. } => ("online", true),
            TunnelState::Reconnecting => ("reconnecting", false),
            TunnelState::Error { .. } => ("error", false),
        };
        Some(Self {
            state: name,
            online_s: stats.online_for.filter(|_| online).map(|d| d.as_secs()),
            reconnects: stats.reconnects,
            connections: stats.locations.len(),
            locations: stats.locations.clone(),
        })
    }
}

/// `speed=1.9x` of ffmpeg's `-progress` report (`speed=N/A` and anything else: `None`).
pub fn parse_speed(line: &str) -> Option<f64> {
    let value = line.trim().strip_prefix("speed=")?.trim();
    let value = value.strip_suffix('x')?.trim();
    value.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// CPU time and memory of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessUsage {
    /// Kernel plus user time since the process started.
    pub cpu_time: Duration,
    /// Working set.
    pub rss_bytes: u64,
    /// When the process started (FILETIME ticks): tells a reused pid from the process sampled.
    pub started: u64,
}

/// CPU time and working set of `pid` (Windows; `None` elsewhere or when the process is gone).
#[cfg(windows)]
pub fn process_usage(pid: u32) -> Option<ProcessUsage> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: plain Win32 calls on a handle opened and closed here; the out-structures are
    // zeroed locals of the sizes the calls expect.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let times = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let memory = K32GetProcessMemoryInfo(handle, &mut counters, counters.cb);
        CloseHandle(handle);
        if times == 0 || memory == 0 {
            return None;
        }
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        // FILETIME counts 100 ns.
        let hundred_ns = ticks(kernel).saturating_add(ticks(user));
        Some(ProcessUsage {
            cpu_time: Duration::from_nanos(hundred_ns.saturating_mul(100)),
            rss_bytes: counters.WorkingSetSize as u64,
            started: ticks(created),
        })
    }
}

#[cfg(not(windows))]
pub fn process_usage(_pid: u32) -> Option<ProcessUsage> {
    None
}

/// CPU and memory of a process worth showing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampledUsage {
    pub cpu_percent: Option<f64>,
    pub rss_bytes: u64,
}
/// A pid not sampled for this long may belong to another process by now.
const SAMPLE_EXPIRY: Duration = Duration::from_secs(10);
/// A CPU percentage is measured over at least this long (two pollers a few ms apart share one
/// longer window instead of each measuring a tiny one).
const MIN_WINDOW: Duration = Duration::from_millis(500);

#[derive(Debug)]
struct PidSamples {
    /// Creation time of the process first sampled under this pid.
    started: u64,
    /// The sample CPU % is measured from.
    baseline: (Duration, Instant),
    latest: (Duration, Instant),
    seen: Instant,
}

/// Turns samples of processes' CPU time into a percentage of the whole machine (all logical
/// cores = 100 %, like Task Manager's CPU column), and tells a reused pid from the process that
/// was sampled (by its creation time). Baselines are kept per pid.
#[derive(Debug, Default)]
pub struct CpuSampler {
    pids: Mutex<HashMap<u32, PidSamples>>,
}

impl CpuSampler {
    /// CPU % since the baseline sample of `pid` (`None` on the first sample) and its memory.
    /// `None` when `pid` now belongs to another process than the one sampled first.
    pub fn sample(
        &self,
        pid: u32,
        usage: ProcessUsage,
        now: Instant,
        cores: usize,
    ) -> Option<SampledUsage> {
        let mut pids = self.pids.lock().unwrap_or_else(PoisonError::into_inner);
        pids.retain(|_, e| now.saturating_duration_since(e.seen) <= SAMPLE_EXPIRY);
        let entry = match pids.entry(pid) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(PidSamples {
                    started: usage.started,
                    baseline: (usage.cpu_time, now),
                    latest: (usage.cpu_time, now),
                    seen: now,
                });
                return Some(SampledUsage {
                    cpu_percent: None,
                    rss_bytes: usage.rss_bytes,
                });
            }
            std::collections::hash_map::Entry::Occupied(slot) => slot.into_mut(),
        };
        if entry.started != usage.started {
            return None;
        }
        if now.saturating_duration_since(entry.latest.1) >= MIN_WINDOW {
            entry.baseline = entry.latest;
        }
        entry.latest = (usage.cpu_time, now);
        entry.seen = now;
        let wall = now.saturating_duration_since(entry.baseline.1);
        let cpu_percent = (wall >= MIN_WINDOW).then(|| {
            let used = usage
                .cpu_time
                .saturating_sub(entry.baseline.0)
                .as_secs_f64();
            let percent = used / wall.as_secs_f64() / cores.max(1) as f64 * 100.0;
            (percent * 10.0).round() / 10.0
        });
        Some(SampledUsage {
            cpu_percent,
            rss_bytes: usage.rss_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn three_addresses_within_30_s_are_three_watchers() {
        let t0 = Instant::now();
        let stats = ViewerStats::starting_at(t0);
        stats.record_at(ip(1), 100, t0 + secs(1));
        stats.record_at(ip(2), 100, t0 + secs(10));
        stats.record_at(ip(3), 100, t0 + secs(20));
        // The same address again is still one.
        stats.record_at(ip(1), 100, t0 + secs(25));
        assert_eq!(stats.snapshot(t0 + secs(30)).watching, 3);
    }

    #[test]
    fn an_address_last_seen_31_s_ago_does_not_count() {
        let t0 = Instant::now();
        let stats = ViewerStats::starting_at(t0);
        stats.record_at(ip(1), 100, t0);
        stats.record_at(ip(2), 100, t0 + secs(1));
        assert_eq!(stats.snapshot(t0 + secs(30)).watching, 2);
        // ip(1) was seen 31 s ago, ip(2) 30 s ago.
        assert_eq!(stats.snapshot(t0 + secs(31)).watching, 1);
        assert_eq!(stats.snapshot(t0 + secs(32)).watching, 0);
    }

    #[test]
    fn history_has_60_samples_newest_last_and_old_ones_drop_out() {
        let t0 = Instant::now();
        let stats = ViewerStats::starting_at(t0);
        stats.record_at(ip(1), 500, t0 + secs(100));
        stats.record_at(ip(1), 300, t0 + secs(100));
        stats.record_at(ip(1), 70, t0 + secs(103));
        let view = stats.snapshot(t0 + secs(105));
        assert_eq!(view.history.len(), HISTORY_LEN);
        assert_eq!(view.history[HISTORY_LEN - 1], 0);
        assert_eq!(view.history[HISTORY_LEN - 1 - 2], 70);
        assert_eq!(view.history[HISTORY_LEN - 1 - 5], 800);
        assert_eq!(view.history.iter().sum::<u64>(), 870);
        assert_eq!(view.fetches_per_min, 3);
        // 60 s later the bytes have left the history.
        let later = stats.snapshot(t0 + secs(165));
        assert_eq!(later.history, vec![0; HISTORY_LEN]);
        assert_eq!(later.fetches_per_min, 0);
    }

    #[test]
    fn bytes_per_second_average_the_last_five_seconds() {
        let t0 = Instant::now();
        let stats = ViewerStats::starting_at(t0);
        stats.record_at(ip(1), 4000, t0 + secs(10));
        stats.record_at(ip(2), 1000, t0 + secs(12));
        // Seconds 10..=14 hold 5000 bytes.
        assert_eq!(stats.snapshot(t0 + secs(14)).bytes_per_s, 1000);
        // Second 10 has left the 5 s window.
        assert_eq!(stats.snapshot(t0 + secs(15)).bytes_per_s, 200);
        assert_eq!(stats.snapshot(t0 + secs(30)).bytes_per_s, 0);
    }

    #[test]
    fn the_view_does_not_contain_addresses() {
        let t0 = Instant::now();
        let stats = ViewerStats::starting_at(t0);
        stats.record_at(ip(77), 10, t0);
        let json = serde_json::to_string(&stats.snapshot(t0)).unwrap();
        assert!(!json.contains("10.0.0.77"));
    }

    const W: u64 = 1 << 33;

    /// Contiguous 4 s segments of 90 kHz ticks starting at `from` (wrapped).
    fn segments(from: u64, count: u64) -> Vec<Option<(u64, u64)>> {
        (0..count)
            .map(|i| {
                let start = from + i * 360_000;
                Some((start % W, (start + 350_000) % W))
            })
            .collect()
    }

    fn continuous_after(spans: &[Option<(u64, u64)>]) -> bool {
        let mut m = DtsMonitor::new();
        for span in spans {
            m.observe(*span);
        }
        m.is_continuous()
    }

    #[test]
    fn dts_continuity_turns_false_on_a_step_back_and_stays() {
        let mut m = DtsMonitor::new();
        assert!(m.is_continuous());
        for span in segments(900_000, 3) {
            m.observe(span);
        }
        m.observe(None);
        assert!(m.is_continuous());
        m.observe(Some((200_000, 550_000)));
        assert!(!m.is_continuous());
        for span in segments(2_000_000, 3) {
            m.observe(span);
        }
        assert!(!m.is_continuous(), "stays false");
    }

    #[test]
    fn dts_continuity_survives_the_wrap_after_26_hours() {
        // Segments before, across and after 2^33: the wrapped values drop, the timeline does not.
        assert!(continuous_after(&segments(W - 5 * 360_000, 12)));
        // One segment whose own span straddles the wrap.
        assert!(continuous_after(&[
            Some((W - 460_000, W - 110_000)),
            Some((W - 100_000, 250_000)),
            Some((260_000, 610_000)),
        ]));
    }

    #[test]
    fn dts_continuity_notices_a_step_back_across_the_wrap_too() {
        let mut spans = segments(W - 2 * 360_000, 4);
        spans.push(Some((W - 400_000, W - 50_000)));
        assert!(!continuous_after(&spans));
    }

    #[test]
    fn a_segment_starting_inside_the_previous_ones_span_is_a_step_back() {
        // The next segment starts after the previous one began but long before it ended.
        assert!(!continuous_after(&[
            Some((1_000_000, 1_350_000)),
            Some((1_100_000, 1_450_000)),
        ]));
        // Small overlaps (audio running ahead of video, B-frames) are not.
        assert!(continuous_after(&[
            Some((1_000_000, 1_350_000)),
            Some((1_300_000, 1_650_000)),
        ]));
    }

    #[test]
    fn the_tunnel_view_follows_the_state_and_hides_a_stale_uptime() {
        let stats = TunnelStats {
            online_for: Some(secs(125)),
            reconnects: 2,
            locations: vec!["waw01".into(), "vie01".into()],
        };
        let online = TunnelState::Online {
            url: "https://x.example".into(),
        };
        let view = TunnelStatsView::of(&online, &stats).unwrap();
        assert_eq!(view.state, "online");
        assert_eq!(view.online_s, Some(125));
        assert_eq!(view.reconnects, 2);
        assert_eq!(view.connections, 2);
        assert_eq!(view.locations, ["waw01", "vie01"]);
        let view = TunnelStatsView::of(&TunnelState::Reconnecting, &stats).unwrap();
        assert_eq!((view.state, view.online_s), ("reconnecting", None));
        assert_eq!(TunnelStatsView::of(&TunnelState::Off, &stats), None);
    }

    #[test]
    fn speed_is_read_from_the_progress_report() {
        assert_eq!(parse_speed("speed=1.9x"), Some(1.9));
        assert_eq!(parse_speed("speed=   2x"), Some(2.0));
        assert_eq!(parse_speed("speed=0.995x\r"), Some(0.995));
        assert_eq!(parse_speed("speed=N/A"), None);
        assert_eq!(parse_speed("fps=25.0"), None);
        assert_eq!(parse_speed("progress=continue"), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_cpu_time_and_memory_of_this_process_are_read() {
        // Some work, so the CPU time is not zero even on a coarse clock.
        let mut x = 0u64;
        for i in 0..20_000_000u64 {
            x = x.wrapping_add(i * i);
        }
        assert_ne!(x, 1);
        let usage = process_usage(std::process::id()).expect("usage of own process");
        assert!(usage.cpu_time > Duration::ZERO);
        assert!(usage.rss_bytes > 0);
    }

    #[test]
    fn a_pid_that_does_not_exist_has_no_usage() {
        assert_eq!(process_usage(u32::MAX - 1), None);
    }

    fn usage(cpu_s: u64, started: u64) -> ProcessUsage {
        ProcessUsage {
            cpu_time: secs(cpu_s),
            rss_bytes: 1000,
            started,
        }
    }

    #[test]
    fn cpu_percent_needs_two_samples_of_the_same_process() {
        let t0 = Instant::now();
        let sampler = CpuSampler::default();
        let first = sampler.sample(7, usage(10, 1), t0, 4).unwrap();
        assert_eq!(first.cpu_percent, None);
        // 2 CPU-seconds in 1 s on 4 cores: 50 %.
        let second = sampler.sample(7, usage(12, 1), t0 + secs(1), 4).unwrap();
        assert_eq!(second.cpu_percent, Some(50.0));
        assert_eq!(second.rss_bytes, 1000);
    }

    #[test]
    fn a_reused_pid_is_not_the_process_that_was_sampled() {
        let t0 = Instant::now();
        let sampler = CpuSampler::default();
        sampler.sample(7, usage(10, 111), t0, 4).unwrap();
        // Same pid, another creation time: another process, nothing to show.
        assert_eq!(sampler.sample(7, usage(500, 222), t0 + secs(1), 4), None);
        // The one sampled first is still known.
        assert!(sampler.sample(7, usage(12, 111), t0 + secs(2), 4).is_some());
        // A pid not sampled for a while may belong to a new process.
        let later = t0 + secs(2) + SAMPLE_EXPIRY + secs(1);
        let fresh = sampler.sample(7, usage(1, 333), later, 4).unwrap();
        assert_eq!(fresh.cpu_percent, None);
    }

    #[test]
    fn baselines_are_kept_per_pid() {
        let t0 = Instant::now();
        let sampler = CpuSampler::default();
        sampler.sample(1, usage(10, 1), t0, 1).unwrap();
        sampler.sample(2, usage(100, 2), t0 + secs(1), 1).unwrap();
        // Process 1 again: measured from its own earlier sample, not from process 2's.
        let p1 = sampler.sample(1, usage(12, 1), t0 + secs(2), 1).unwrap();
        assert_eq!(p1.cpu_percent, Some(100.0));
        let p2 = sampler.sample(2, usage(101, 2), t0 + secs(3), 1).unwrap();
        assert_eq!(p2.cpu_percent, Some(50.0));
    }

    #[test]
    fn two_pollers_close_together_do_not_measure_a_tiny_window() {
        let t0 = Instant::now();
        let sampler = CpuSampler::default();
        let ms = Duration::from_millis;
        let busy = |cpu_ms: u64| ProcessUsage {
            cpu_time: ms(cpu_ms),
            rss_bytes: 1,
            started: 1,
        };
        sampler.sample(1, busy(0), t0, 1).unwrap();
        // The process is busy all the time. A polls every second, B 20 ms after A: B must not
        // measure a 20 ms window (a wild number), nor take A's baseline away.
        let a = sampler.sample(1, busy(1000), t0 + secs(1), 1).unwrap();
        assert_eq!(a.cpu_percent, Some(100.0));
        let b = sampler
            .sample(1, busy(1020), t0 + secs(1) + ms(20), 1)
            .unwrap();
        assert_eq!(b.cpu_percent, Some(100.0));
        let a2 = sampler.sample(1, busy(2000), t0 + secs(2), 1).unwrap();
        let percent = a2.cpu_percent.unwrap();
        assert!((95.0..=105.0).contains(&percent), "{percent}");
    }

    #[test]
    fn a_flood_of_distinct_addresses_is_capped() {
        let t0 = Instant::now();
        let stats = ViewerStats::starting_at(t0);
        let started = Instant::now();
        for n in 0..100_000u32 {
            let addr = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + n));
            // 100k fetches spread over 29 s.
            let at = t0 + Duration::from_millis(u64::from(n) * 29_000 / 100_000);
            stats.record_at(addr, 10, at);
            assert!(stats.tracked() <= MAX_TRACKED_ADDRESSES);
        }
        assert!(
            started.elapsed() < secs(10),
            "recording must not be O(n) per request: {:?}",
            started.elapsed()
        );
        let view = stats.snapshot(t0 + secs(29));
        // "watching" saturates at the cap; every fetch and byte is still counted.
        assert_eq!(view.watching, MAX_TRACKED_ADDRESSES);
        assert_eq!(view.fetches_per_min, 100_000);
        assert_eq!(view.history.iter().sum::<u64>(), 1_000_000);
        // Once the old ones expire, new addresses are tracked again.
        stats.record_at(ip(1), 10, t0 + secs(70));
        assert_eq!(stats.snapshot(t0 + secs(70)).watching, 1);
        assert_eq!(stats.tracked(), 1);
    }

    #[test]
    fn a_known_address_is_refreshed_even_when_the_table_is_full() {
        let t0 = Instant::now();
        let stats = ViewerStats::starting_at(t0);
        for n in 0..MAX_TRACKED_ADDRESSES as u32 {
            stats.record_at(IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + n)), 1, t0);
        }
        let known = IpAddr::V4(Ipv4Addr::from(0x0a00_0000));
        stats.record_at(known, 1, t0 + secs(25));
        // The others are old after 31 s, the refreshed one is not.
        stats.record_at(ip(200), 1, t0 + secs(31));
        assert_eq!(stats.snapshot(t0 + secs(31)).watching, 2);
    }
}
