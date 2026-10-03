//! Real-time broadcaster (port of `src/pipeline/broadcaster.ts`).

use std::path::{Path, PathBuf};

use crate::pipeline::playlist::{LivePlaylistInput, PlaylistSegment, build_live_playlist};
use crate::pipeline::tsrewrite::{TsRewriter, content_end, min_dts};

/// Start of the channel timeline (10 s) so shifted DTS values are never negative.
pub const START_PTS: i64 = 10 * 90_000;

/// A segment delivered by the source (the Channel).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSegment {
    pub path: PathBuf,
    pub duration: f64,
    /// Source identifier, e.g. `"clip-12-0"` or `"idle"`. A change means a discontinuity.
    pub source_key: String,
    /// Segment number within the source. The next number continues the same timeline.
    pub index: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublishedSegment {
    pub path: PathBuf,
    pub duration: f64,
    pub source_key: String,
    pub index: u64,
    pub seq: u64,
    /// Channel time (s) at the start of the segment.
    pub channel_time: f64,
}

pub trait SegmentSource {
    /// Returns the next segment to broadcast. It must always return something (e.g. a slate);
    /// an error is logged and retried on the next tick.
    fn next(&mut self) -> anyhow::Result<SourceSegment>;
    /// Called after a segment has been published.
    fn on_published(&mut self, seg: &PublishedSegment);
}

const INITIAL_SEGMENTS: usize = 3;
const MAX_SOURCE_ATTEMPTS: usize = 5;
const MAX_PUBLISH_PER_TICK: usize = 100;
const DEFAULT_RETAIN_S: u64 = 60;
/// How much later a file that could not be deleted is retried.
const REQUEUE_DELAY_MS: u64 = 4_000;
/// Largest difference (90 kHz) between the clock and the end of the last packet for which we
/// trust the packets over the clock: 1 s.
const JOIN_WINDOW: i64 = 90_000;

pub type LogFn = Box<dyn Fn(&str) + Send>;

pub struct BroadcasterOptions {
    pub dir: PathBuf,
    pub window_size: usize,
    pub segment_seconds: f64,
    pub path_prefix: String,
    pub discontinuity_tags: Box<dyn Fn() -> bool + Send>,
    /// How many seconds to keep files after they leave the window (default 60).
    pub retain_s: Option<u64>,
    pub log: Option<LogFn>,
}

struct Retired {
    name: String,
    at: u64,
}

pub struct Broadcaster {
    o: BroadcasterOptions,
    now: Box<dyn Fn() -> u64 + Send>,
    rewriter: TsRewriter,
    published: Vec<PlaylistSegment>,
    retired: Vec<Retired>,
    seq: u64,
    clock: f64,
    offset: i64,
    last_end: Option<i64>,
    last: Option<SourceSegment>,
    target_duration: f64,
    discontinuity_sequence: u64,
    next_publish_at: u64,
}

impl Broadcaster {
    pub fn new(o: BroadcasterOptions, now: Box<dyn Fn() -> u64 + Send>) -> Self {
        let target_duration = o.segment_seconds;
        Self {
            o,
            now,
            rewriter: TsRewriter::new(),
            published: Vec::new(),
            retired: Vec::new(),
            seq: 0,
            clock: 0.0,
            offset: 0,
            last_end: None,
            last: None,
            target_duration,
            discontinuity_sequence: 0,
            next_publish_at: 0,
        }
    }

    fn log(&self, msg: &str) {
        if let Some(log) = &self.o.log {
            log(msg);
        }
    }

    fn retain_ms(&self) -> u64 {
        self.o
            .retain_s
            .unwrap_or(DEFAULT_RETAIN_S)
            .saturating_mul(1000)
    }

    /// Clears the directory and publishes 3 segments in advance.
    pub fn start_initial(&mut self, source: &mut dyn SegmentSource) {
        let _ = std::fs::remove_dir_all(&self.o.dir);
        if let Err(e) = std::fs::create_dir_all(&self.o.dir) {
            self.log(&format!(
                "Nie można utworzyć katalogu {}: {e}",
                self.o.dir.display()
            ));
        }
        let mut first = 0.0;
        for i in 0..INITIAL_SEGMENTS {
            let d = self.publish_one(source);
            if i == 0 {
                first = d;
            }
        }
        self.next_publish_at = (self.now)() + secs_to_ms(first);
    }

    pub fn tick(&mut self, source: &mut dyn SegmentSource) {
        let mut guard = 0;
        while (self.now)() >= self.next_publish_at && guard < MAX_PUBLISH_PER_TICK {
            guard += 1;
            let d = self.publish_one(source);
            self.next_publish_at += secs_to_ms(d);
        }
        self.cleanup();
    }

    /// Channel time in seconds (sum of the published segments).
    pub fn channel_time(&self) -> f64 {
        self.clock
    }

    pub fn dir(&self) -> &Path {
        &self.o.dir
    }

    pub fn playlist(&self) -> String {
        build_live_playlist(&LivePlaylistInput {
            segments: self.published.clone(),
            target_duration: self.target_duration,
            discontinuity_sequence: self.discontinuity_sequence,
            path_prefix: self.o.path_prefix.clone(),
        })
    }

    /// Names of all segment files that may still be requested (retired + window, oldest first).
    pub fn known_segments(&self) -> Vec<String> {
        self.retired
            .iter()
            .map(|r| r.name.clone())
            .chain(self.published.iter().map(|s| s.name.clone()))
            .collect()
    }

    pub fn segment_path(&self, name: &str) -> Option<PathBuf> {
        if !is_segment_name(name) {
            return None;
        }
        let known = self.published.iter().any(|s| s.name == name)
            || self.retired.iter().any(|r| r.name == name);
        known.then(|| self.o.dir.join(name))
    }

    /// Publishes one segment and returns its length in seconds.
    fn publish_one(&mut self, source: &mut dyn SegmentSource) -> f64 {
        for _ in 0..MAX_SOURCE_ATTEMPTS {
            let seg = match source.next() {
                Ok(seg) => seg,
                Err(e) => {
                    // A source error must not take the server down: retry on the next tick.
                    self.log(&format!("Błąd źródła fragmentów: {e}"));
                    return 1.0;
                }
            };
            match self.publish(source, &seg) {
                Ok(d) => return d,
                Err(e) => self.log(&format!(
                    "Pominięto uszkodzony fragment {}: {e}",
                    seg.path.display()
                )),
            }
        }
        // The source keeps returning bad data: short pause, retry on the next tick.
        1.0
    }

    fn publish(
        &mut self,
        source: &mut dyn SegmentSource,
        seg: &SourceSegment,
    ) -> anyhow::Result<f64> {
        let input = std::fs::read(&seg.path)?;
        let continuing = self.last.as_ref().is_some_and(|l| {
            seg.source_key == l.source_key && l.index.checked_add(1) == Some(seg.index)
        });
        let mut offset = self.offset;
        if !continuing {
            let base = min_dts(&input).unwrap_or(0) as i64;
            let by_clock = (START_PTS as f64 + self.clock * 90_000.0).round() as i64;
            let mut target = by_clock;
            if let Some(joined) = self.last_end {
                // Declared durations (EXTINF) differ from the content by fractions of a second in
                // both directions. Near the clock we join right after the last packet; the
                // timeline never goes backwards.
                target = if (joined - by_clock).abs() <= JOIN_WINDOW {
                    joined
                } else {
                    by_clock.max(joined)
                };
                self.clock = (target - START_PTS) as f64 / 90_000.0;
            }
            offset = target - base;
        }
        let input_end = content_end(&input);
        let output = self.rewriter.rewrite(&input, offset)?;
        let source_changed = self
            .last
            .as_ref()
            .is_some_and(|l| seg.source_key != l.source_key);

        let seq = self.seq;
        self.seq += 1;
        let name = format!("{seq}.ts");
        std::fs::write(self.o.dir.join(&name), output)?;

        let published = PublishedSegment {
            path: seg.path.clone(),
            duration: seg.duration,
            source_key: seg.source_key.clone(),
            index: seg.index,
            seq,
            channel_time: self.clock,
        };
        self.offset = offset;
        if let Some(end) = input_end {
            self.last_end = Some(self.last_end.unwrap_or(0).max(end as i64 + offset));
        }
        self.last = Some(seg.clone());
        self.clock += seg.duration;
        // HLS: EXTINF rounded to an integer must not exceed TARGETDURATION.
        self.target_duration = self.target_duration.max(seg.duration.round());
        let discontinuity = source_changed && (self.o.discontinuity_tags)();
        self.published.push(PlaylistSegment {
            seq,
            name,
            duration: seg.duration,
            discontinuity,
        });
        while self.published.len() > self.o.window_size {
            let removed = self.published.remove(0);
            if removed.discontinuity {
                self.discontinuity_sequence += 1;
            }
            let at = (self.now)();
            self.retired.push(Retired {
                name: removed.name,
                at,
            });
        }
        source.on_published(&published);
        Ok(seg.duration)
    }

    fn cleanup(&mut self) {
        let now = (self.now)();
        let limit = now.saturating_sub(self.retain_ms());
        while self.retired.first().is_some_and(|r| r.at < limit) {
            let Retired { name, .. } = self.retired.remove(0);
            match std::fs::remove_file(self.o.dir.join(&name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    // The file may be being sent to a viewer (Windows blocks deleting it); try
                    // again on a later tick.
                    self.retired.insert(
                        0,
                        Retired {
                            name,
                            at: limit + REQUEUE_DELAY_MS,
                        },
                    );
                    break;
                }
            }
        }
    }
}

/// Matches `^\d+\.ts$` (ASCII digits only).
fn is_segment_name(name: &str) -> bool {
    name.strip_suffix(".ts")
        .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()))
}

fn secs_to_ms(secs: f64) -> u64 {
    (secs * 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tsrewrite::min_dts;
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

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

    struct FakeSource {
        queue: VecDeque<SourceSegment>,
        counter: u32,
        src_dir: PathBuf,
        fail: Arc<AtomicBool>,
        seen: Arc<Mutex<Vec<u64>>>,
    }

    impl FakeSource {
        fn new(src_dir: &std::path::Path) -> Self {
            Self {
                queue: VecDeque::new(),
                counter: 0,
                src_dir: src_dir.to_path_buf(),
                fail: Arc::new(AtomicBool::new(false)),
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn make_with(&mut self, key: &str, index: u64, duration: f64, pts: u64) -> SourceSegment {
            let p = self
                .src_dir
                .join(format!("{key}-{index}-{}.ts", self.counter));
            self.counter += 1;
            fs::write(&p, ts_with_pts(pts)).unwrap();
            SourceSegment {
                path: p,
                duration,
                source_key: key.to_string(),
                index,
            }
        }

        fn make(&mut self, key: &str, index: u64) -> SourceSegment {
            self.make_with(key, index, 4.0, 126_000)
        }

        fn push(&mut self, seg: SourceSegment) {
            self.queue.push_back(seg);
        }
    }

    impl SegmentSource for FakeSource {
        fn next(&mut self) -> anyhow::Result<SourceSegment> {
            if self.fail.load(Ordering::SeqCst) {
                anyhow::bail!("baza niedostępna");
            }
            Ok(match self.queue.pop_front() {
                Some(s) => s,
                None => self.make("idle", 0),
            })
        }

        fn on_published(&mut self, seg: &PublishedSegment) {
            self.seen.lock().unwrap().push(seg.seq);
        }
    }

    struct Env {
        dir: tempfile::TempDir,
        src_dir: tempfile::TempDir,
        now: Arc<AtomicU64>,
    }

    impl Env {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                src_dir: tempfile::tempdir().unwrap(),
                now: Arc::new(AtomicU64::new(1_000_000)),
            }
        }

        fn live_dir(&self) -> PathBuf {
            self.dir.path().join("live")
        }

        fn source(&self) -> FakeSource {
            FakeSource::new(self.src_dir.path())
        }

        fn options(&self, prefix: &str, tags: bool) -> BroadcasterOptions {
            BroadcasterOptions {
                dir: self.live_dir(),
                window_size: 10,
                segment_seconds: 4.0,
                path_prefix: prefix.to_string(),
                discontinuity_tags: Box::new(move || tags),
                retain_s: None,
                log: None,
            }
        }

        fn broadcaster(&self, tags: bool) -> Broadcaster {
            self.broadcaster_with(self.options("main/", tags))
        }

        fn broadcaster_with(&self, o: BroadcasterOptions) -> Broadcaster {
            let now = self.now.clone();
            Broadcaster::new(o, Box::new(move || now.load(Ordering::SeqCst)))
        }

        fn advance(&self, ms: u64) {
            self.now.fetch_add(ms, Ordering::SeqCst);
        }

        fn read_min_dts(&self, n: u64) -> u64 {
            min_dts(&fs::read(self.live_dir().join(format!("{n}.ts"))).unwrap()).unwrap()
        }
    }

    fn lines(b: &Broadcaster) -> Vec<String> {
        b.playlist().split('\n').map(str::to_string).collect()
    }

    fn ts_lines(b: &Broadcaster) -> Vec<String> {
        lines(b)
            .into_iter()
            .filter(|l| l.ends_with(".ts"))
            .collect()
    }

    #[test]
    fn publishes_three_segments_at_start_then_in_real_time() {
        let env = Env::new();
        let mut src = env.source();
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        assert_eq!(ts_lines(&b), vec!["main/0.ts", "main/1.ts", "main/2.ts"]);
        env.advance(3_999);
        b.tick(&mut src);
        assert_eq!(ts_lines(&b).len(), 3);
        env.advance(1);
        b.tick(&mut src);
        assert_eq!(ts_lines(&b).len(), 4);
        env.advance(8_000);
        b.tick(&mut src);
        assert_eq!(ts_lines(&b).len(), 6);
        assert!(env.live_dir().join("5.ts").exists());
    }

    #[test]
    fn keeps_a_window_of_10_segments_and_advances_media_sequence() {
        let env = Env::new();
        let mut src = env.source();
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        env.advance(12 * 4_000);
        b.tick(&mut src);
        let l = lines(&b);
        assert_eq!(l.iter().filter(|x| x.ends_with(".ts")).count(), 10);
        assert!(l.contains(&"#EXT-X-MEDIA-SEQUENCE:5".to_string()));
    }

    #[test]
    fn marks_a_discontinuity_on_source_change_but_not_when_the_same_slate_loops() {
        let env = Env::new();
        let mut src = env.source();
        for (k, i) in [
            ("idle", 0),
            ("idle", 1),
            ("idle", 0),
            ("clip-1", 0),
            ("clip-1", 1),
        ] {
            let s = src.make(k, i);
            src.push(s);
        }
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        env.advance(8_000);
        b.tick(&mut src);
        let l = lines(&b);
        let disc = l.iter().position(|x| x == "#EXT-X-DISCONTINUITY").unwrap();
        assert_eq!(l[disc + 2], "main/3.ts");
        assert_eq!(l.iter().filter(|x| *x == "#EXT-X-DISCONTINUITY").count(), 1);
    }

    #[test]
    fn no_discontinuity_tags_when_disabled() {
        let env = Env::new();
        let mut src = env.source();
        for k in ["a", "b", "c"] {
            let s = src.make(k, 0);
            src.push(s);
        }
        let mut b = env.broadcaster(false);
        b.start_initial(&mut src);
        assert!(!b.playlist().contains("#EXT-X-DISCONTINUITY\n"));
    }

    #[test]
    fn counts_discontinuity_sequence_for_markers_that_left_the_window() {
        let env = Env::new();
        let mut src = env.source();
        for k in ["a", "b"] {
            let s = src.make(k, 0);
            src.push(s);
        }
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        env.advance(11 * 4_000);
        b.tick(&mut src);
        // markers at seq 1 (b) and seq 2 (back to the slate) left the window
        assert!(b.playlist().contains("#EXT-X-DISCONTINUITY-SEQUENCE:2"));
    }

    #[test]
    fn shifts_timestamps_so_the_channel_timeline_is_continuous() {
        let env = Env::new();
        let mut src = env.source();
        let s = src.make_with("a", 0, 4.0, 126_000);
        src.push(s);
        let s = src.make_with("a", 1, 4.0, 486_000);
        src.push(s);
        let s = src.make_with("b", 0, 4.0, 9_000_000);
        src.push(s);
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        assert_eq!(env.read_min_dts(0), START_PTS as u64);
        assert_eq!(env.read_min_dts(1), START_PTS as u64 + 360_000);
        // a new source with a completely different timeline lands exactly on the channel clock
        assert_eq!(env.read_min_dts(2), START_PTS as u64 + 720_000);
    }

    #[test]
    fn a_new_source_does_not_start_before_the_last_packet_of_the_previous_one() {
        let env = Env::new();
        let mut src = env.source();
        // The segment declares 4 s, but its audio reaches 4.05 s.
        let a = src.make_with("a", 0, 4.0, 126_000);
        let mut data = ts_with_pts(126_000);
        data.extend(ts_with_pts(126_000 + 364_500));
        fs::write(&a.path, data).unwrap();
        src.push(a);
        let s = src.make_with("b", 0, 4.0, 5_000_000);
        src.push(s);
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        let last_of_a = START_PTS as u64 + 364_500;
        let first_of_b = env.read_min_dts(1);
        assert!(first_of_b > last_of_a);
        assert!(first_of_b - last_of_a <= 4_500);
    }

    #[test]
    fn when_a_segment_declares_more_than_it_contains_the_new_source_joins_right_after_the_last_packet()
     {
        let env = Env::new();
        let mut src = env.source();
        let a = src.make_with("a", 0, 4.067, 126_000);
        let mut data = ts_with_pts(126_000);
        data.extend(ts_with_pts(126_000 + 357_000));
        fs::write(&a.path, data).unwrap();
        src.push(a);
        let s = src.make_with("b", 0, 4.0, 5_000_000);
        src.push(s);
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        assert_eq!(env.read_min_dts(1), START_PTS as u64 + 357_000 + 3_000);
    }

    #[test]
    fn target_duration_rounded_extinf_does_not_exceed_the_value() {
        let env = Env::new();
        let mut src = env.source();
        let s = src.make_with("a", 0, 4.066667, 126_000);
        src.push(s);
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        assert!(b.playlist().contains("#EXT-X-TARGETDURATION:4"));
    }

    #[test]
    fn target_duration_grows_for_a_long_segment_and_never_shrinks() {
        let env = Env::new();
        let mut src = env.source();
        let s = src.make_with("a", 0, 6.5, 126_000);
        src.push(s);
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        env.advance(60_000);
        b.tick(&mut src);
        assert!(b.playlist().contains("#EXT-X-TARGETDURATION:7"));
    }

    #[test]
    fn deletes_old_files_after_leaving_the_window_and_the_retention_time() {
        let env = Env::new();
        let mut src = env.source();
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        env.advance(11 * 4_000);
        b.tick(&mut src);
        assert!(env.live_dir().join("0.ts").exists());
        env.advance(61_000);
        b.tick(&mut src);
        assert!(!env.live_dir().join("0.ts").exists());
    }

    #[test]
    fn segment_path_lets_only_known_segments_through() {
        let env = Env::new();
        let mut src = env.source();
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        assert_eq!(b.segment_path("1.ts"), Some(env.live_dir().join("1.ts")));
        assert_eq!(b.segment_path("99.ts"), None);
        assert_eq!(b.segment_path("../secret.ts"), None);
        assert_eq!(b.segment_path("1.ts\n"), None);
        assert_eq!(
            b.known_segments(),
            vec!["0.ts".to_string(), "1.ts".to_string(), "2.ts".to_string()]
        );
    }

    #[test]
    fn notifies_the_source_with_the_segment_number_once_published() {
        let env = Env::new();
        let mut src = env.source();
        let seen = src.seen.clone();
        let mut b = env.broadcaster_with(env.options("", true));
        b.start_initial(&mut src);
        assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn a_source_error_neither_stops_the_broadcast_nor_escapes_tick() {
        let env = Env::new();
        let mut src = env.source();
        let fail = src.fail.clone();
        let logs = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = logs.clone();
        let mut o = env.options("", true);
        o.log = Some(Box::new(move |m| sink.lock().unwrap().push(m.to_string())));
        let mut b = env.broadcaster_with(o);
        b.start_initial(&mut src);
        fail.store(true, Ordering::SeqCst);
        env.advance(4_000);
        b.tick(&mut src);
        assert!(
            logs.lock()
                .unwrap()
                .iter()
                .any(|l| l == "Błąd źródła fragmentów: baza niedostępna")
        );
        fail.store(false, Ordering::SeqCst);
        env.advance(5_000);
        b.tick(&mut src);
        assert!(ts_lines(&b).len() > 3);
    }

    #[test]
    fn skips_a_broken_source_segment_and_takes_the_next_one() {
        let env = Env::new();
        let mut src = env.source();
        let broken = env.src_dir.path().join("broken.ts");
        fs::write(&broken, vec![0u8; 188]).unwrap();
        src.push(SourceSegment {
            path: broken,
            duration: 4.0,
            source_key: "x".to_string(),
            index: 0,
        });
        let mut b = env.broadcaster(true);
        b.start_initial(&mut src);
        assert_eq!(ts_lines(&b).len(), 3);
    }
}
