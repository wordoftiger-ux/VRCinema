//! The producer's monitor ("Podgląd", spec `2026-10-21-monitor-design.md`): the newest clip
//! segments for the program's window, a few seconds after ffmpeg wrote them instead of after
//! the viewers' delay. No process of its own: it lists the segments of the clip the channel is
//! cutting, and keeps the ones it listed for a while after they aired.
//!
//! Every segment the monitor lists gets a row: its own sequence number (dense and going on
//! across clips, so the list is a well-formed live HLS playlist: a new clip starts after a
//! discontinuity, and the window starts its player again for it), the clip, the film position it starts at, and a hard link in the monitor's
//! folder. The link keeps the segment readable after the channel removes the clip's file (once
//! the broadcaster read it) or the whole clip folder (seek, skip, end of film) — so the clip's
//! own cleanup is exactly as before and the public stream is untouched. Where a link cannot be
//! made (a file system without hard links) the row points at the clip's file and lives only as
//! long as that file.
//!
//! Retention (the links): a row of the current clip that has not aired yet stays; an aired row,
//! or any row of a clip that is no longer current, stays [`RETAIN_MS`] (90 s) and then goes; the
//! links together never take more than [`MAX_BYTES`] (256 MiB) and the list never has more than
//! [`MAX_ROWS`] rows — the oldest go first. Rows only ever leave from the front, so the
//! numbering stays dense. The channel lists at most [`LOOKAHEAD`] segments that have not aired
//! yet (about 12 s), so what is retained is about 90 s of the stream plus that.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::pipeline::segmenter::SegmentInfo;

/// How many segments of the current clip that have not aired yet the monitor lists.
pub const LOOKAHEAD: usize = 3;
/// How long a row stays after its segment aired (or its clip stopped being current).
pub const RETAIN_MS: u64 = 90_000;
/// Upper bound on the size of the retained links.
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// Upper bound on the rows of the list.
pub const MAX_ROWS: usize = 120;
/// `EXT-X-TARGETDURATION` of a list without segments.
const DEFAULT_TARGET_S: u64 = 4;

/// One segment the monitor lists.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorRow {
    /// Media sequence number (dense; only the numbers of rows that left before they aired, the
    /// old clip's lookahead, are given again, to the next clip's rows).
    pub seq: u64,
    /// The encoder process (the channel's source key, e.g. `clip-12-0-r1`).
    pub clip: String,
    /// The segment's file name in the clip folder (`seg_00003.ts`).
    pub name: String,
    /// Where it is read from: the monitor's link, or the clip's own file.
    pub path: PathBuf,
    pub duration: f64,
    pub item_id: i64,
    /// Film second the segment starts at (the channel's own bookkeeping, as for `positionS`).
    pub position_s: f64,
    /// Another clip comes before it in the list.
    pub discontinuity: bool,
}

/// What the HTTP routes need: the rows, oldest first, and where the channel is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonitorFeed {
    pub rows: Vec<MonitorRow>,
    /// Discontinuities that left the front of the list (`EXT-X-DISCONTINUITY-SEQUENCE`).
    pub discontinuity_sequence: u64,
    /// The number the next row gets (`EXT-X-MEDIA-SEQUENCE` of an empty list).
    pub next_seq: u64,
    /// The longest segment ever listed, rounded (`EXT-X-TARGETDURATION` never shrinks); 0: none.
    pub target_duration: u64,
    /// A film is on air or paused; without one the list is empty (nothing to preview).
    pub active: bool,
    /// The row the viewers see now (the channel's estimate, as for `current.positionS`).
    pub antenna_seq: Option<u64>,
    /// The newest row handed to the broadcaster.
    pub aired_seq: Option<u64>,
}

impl MonitorFeed {
    /// The row `name` of `clip`, if the list still has it.
    pub fn row(&self, clip: &str, name: &str) -> Option<&MonitorRow> {
        self.rows.iter().find(|r| r.clip == clip && r.name == name)
    }

    /// The HLS playlist (never `ENDLIST`: the monitor goes on with the next clip). `from`: the
    /// player starts at that row (`EXT-X-START`), when the list has it. Comments for the
    /// window: `#VRCINEMA-ANTENNA:<seq>`, `#VRCINEMA-AIRED:<seq>` and before every segment
    /// `#VRCINEMA-AT:<item id>,<film position>`. Without a film (`active` false) the list has
    /// no segments.
    pub fn playlist(&self, from: Option<u64>) -> String {
        let rows: &[MonitorRow] = if self.active { &self.rows } else { &[] };
        // HLS: an EXTINF rounded to an integer must not exceed TARGETDURATION.
        let target = rows
            .iter()
            .map(|r| r.duration.round().max(1.0) as u64)
            .chain([self.target_duration])
            .max()
            .filter(|t| *t > 0)
            .unwrap_or(DEFAULT_TARGET_S);
        // An idle list names the first row still kept, so the number never goes back.
        let first = self.rows.first().map_or(self.next_seq, |r| r.seq);
        let mut text = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{target}\n\
             #EXT-X-MEDIA-SEQUENCE:{first}\n#EXT-X-DISCONTINUITY-SEQUENCE:{}\n",
            self.discontinuity_sequence
        );
        if rows.is_empty() {
            return text;
        }
        if let Some(from) = from {
            let mut offset = 0.0;
            for r in rows {
                if r.seq == from {
                    text.push_str(&format!(
                        "#EXT-X-START:TIME-OFFSET={offset:.3},PRECISE=YES\n"
                    ));
                    break;
                }
                offset += r.duration;
            }
        }
        let listed = |seq: Option<u64>| seq.filter(|s| rows.iter().any(|r| r.seq == *s));
        if let Some(seq) = listed(self.antenna_seq) {
            text.push_str(&format!("#VRCINEMA-ANTENNA:{seq}\n"));
        }
        if let Some(seq) = listed(self.aired_seq) {
            text.push_str(&format!("#VRCINEMA-AIRED:{seq}\n"));
        }
        for r in rows {
            if r.discontinuity {
                text.push_str("#EXT-X-DISCONTINUITY\n");
            }
            text.push_str(&format!(
                "#VRCINEMA-AT:{},{:.3}\n#EXTINF:{:.6},\nmonitor/{}/{}\n",
                r.item_id, r.position_s, r.duration, r.clip, r.name
            ));
        }
        text
    }
}

/// How long after the window's last monitor request the channel keeps reading ffmpeg's list
/// every tick for the monitor (without a viewer of the monitor nothing is polled ahead).
pub const WATCH_MS: u64 = 5_000;

/// When the window last asked for the monitor (the HTTP routes touch it, the channel reads it).
#[derive(Debug, Default)]
pub struct MonitorWatch {
    /// Milliseconds since [`epoch`] + 1 of the last request; 0: never; `u64::MAX`: always.
    last: AtomicU64,
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn since_epoch_ms() -> u64 {
    u64::try_from(epoch().elapsed().as_millis())
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1)
}

impl MonitorWatch {
    /// A watch that always counts as watched (tests of the channel with the monitor on).
    pub fn always() -> Self {
        Self {
            last: AtomicU64::new(u64::MAX),
        }
    }

    /// The window asked for the monitor now.
    pub fn touch(&self) {
        let now = since_epoch_ms();
        let _ = self
            .last
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
                (last != u64::MAX).then_some(now)
            });
    }

    /// The window asked for the monitor within the last [`WATCH_MS`].
    pub fn watched(&self) -> bool {
        match self.last.load(Ordering::Relaxed) {
            0 => false,
            u64::MAX => true,
            last => since_epoch_ms().saturating_sub(last) < WATCH_MS,
        }
    }
}

/// Where the channel's monitor keeps its links and whom it asks whether anybody watches.
#[derive(Debug, Clone)]
pub struct MonitorOptions {
    /// Folder for the links (each channel gets its own subfolder).
    pub dir: PathBuf,
    pub watch: Arc<MonitorWatch>,
    /// The first row number; `None`: go on from the newest number of this process (a rebuilt
    /// channel or a restarted server never hands out a number the window has seen).
    pub first_seq: Option<u64>,
}

struct Entry {
    row: MonitorRow,
    /// `row.path` is the monitor's own link (removed with the row).
    owned: bool,
    bytes: u64,
    /// Handed to the broadcaster.
    aired: bool,
    /// `None`: a row of the current clip that has not aired yet.
    expires_at: Option<u64>,
}

/// The rows and their links; owned by the channel, which tells it what airs.
pub struct MonitorTimeline {
    dir: PathBuf,
    entries: VecDeque<Entry>,
    next_seq: u64,
    discontinuity_sequence: u64,
    current: Option<String>,
    /// The clip of the newest row ever listed (a new clip after it starts with a
    /// discontinuity, also when the list is empty by then).
    last_clip: Option<String>,
    /// The row a paused film resumes from: kept however long the pause lasts.
    pinned: Option<u64>,
    /// `EXT-X-TARGETDURATION`: only ever grows.
    target_s: u64,
    bytes: u64,
    retain_ms: u64,
    max_bytes: u64,
    max_rows: usize,
}

/// Folders of channels in this process (a rebuilt channel gets a fresh one).
static INSTANCES: AtomicU64 = AtomicU64::new(0);
/// The highest row number handed out in this process, plus one.
static NEXT_SEQ: AtomicU64 = AtomicU64::new(0);

impl MonitorTimeline {
    /// Links go to a fresh folder under `root` (made on the first row, removed on drop).
    pub fn new(root: &Path, first_seq: Option<u64>) -> Self {
        let n = INSTANCES.fetch_add(1, Ordering::Relaxed);
        Self {
            dir: root.join(format!("{}-{n}", std::process::id())),
            entries: VecDeque::new(),
            next_seq: first_seq.unwrap_or_else(|| NEXT_SEQ.load(Ordering::Relaxed)),
            discontinuity_sequence: 0,
            current: None,
            last_clip: None,
            pinned: None,
            target_s: 0,
            bytes: 0,
            retain_ms: RETAIN_MS,
            max_bytes: MAX_BYTES,
            max_rows: MAX_ROWS,
        }
    }

    /// Other limits (tests).
    pub fn with_limits(mut self, retain_ms: u64, max_bytes: u64, max_rows: usize) -> Self {
        self.retain_ms = retain_ms;
        self.max_bytes = max_bytes;
        self.max_rows = max_rows;
        self
    }

    /// Lists one more segment of `clip` (the current clip): its row number.
    pub fn push(&mut self, clip: &str, item_id: i64, seg: &SegmentInfo, position_s: f64) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        NEXT_SEQ.fetch_max(self.next_seq, Ordering::Relaxed);
        let name = seg
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (path, owned) = match self.link(&seg.path, seq) {
            Some(link) => (link, true),
            None => (seg.path.clone(), false),
        };
        let bytes = if owned {
            std::fs::metadata(&path).map_or(0, |m| m.len())
        } else {
            0
        };
        let discontinuity = self.last_clip.as_deref().is_some_and(|last| last != clip);
        self.last_clip = Some(clip.to_string());
        // HLS: an EXTINF rounded to an integer must not exceed TARGETDURATION.
        self.target_s = self.target_s.max(seg.duration.round().max(1.0) as u64);
        self.bytes += bytes;
        self.entries.push_back(Entry {
            row: MonitorRow {
                seq,
                clip: clip.to_string(),
                name,
                path,
                duration: seg.duration,
                item_id,
                position_s,
                discontinuity,
            },
            owned,
            bytes,
            aired: false,
            expires_at: None,
        });
        seq
    }

    /// A hard link to `file` in the monitor's folder; `None` when it cannot be made.
    fn link(&self, file: &Path, seq: u64) -> Option<PathBuf> {
        std::fs::create_dir_all(&self.dir).ok()?;
        let link = self.dir.join(format!("{seq}.ts"));
        // A leftover of a row that left the list with this number.
        let _ = std::fs::remove_file(&link);
        std::fs::hard_link(file, &link).ok()?;
        Some(link)
    }

    /// The row's segment went to the broadcaster at `now`.
    pub fn aired(&mut self, seq: u64, now: u64) {
        let until = now.saturating_add(self.retain_ms);
        if let Some(e) = self.entries.iter_mut().rev().find(|e| e.row.seq == seq) {
            e.aired = true;
            e.expires_at = Some(e.expires_at.map_or(until, |at| at.max(until)));
        }
    }

    /// The newest row of `item_id` starting at `position_s` (a paused film's resume point).
    pub fn find(&self, item_id: i64, position_s: f64) -> Option<u64> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.row.item_id == item_id && (e.row.position_s - position_s).abs() < 1e-6)
            .map(|e| e.row.seq)
    }

    /// Keeps `seq` regardless of its time (a paused film's still picture); `None`: nothing.
    pub fn pin(&mut self, seq: Option<u64>) {
        self.pinned = seq;
    }

    /// `clip` is the clip the channel cuts now (`None`: none). The rows of the one before it
    /// that never aired go at once (the viewers never see them; a pinned one stays), the
    /// others get their time limit from `now`.
    pub fn set_current(&mut self, clip: Option<&str>, now: u64) {
        if self.current.as_deref() == clip {
            return;
        }
        let until = now.saturating_add(self.retain_ms);
        if let Some(old) = self.current.take() {
            while let Some(back) = self.entries.back() {
                if back.row.clip != old || back.aired || self.pinned == Some(back.row.seq) {
                    break;
                }
                if let Some(e) = self.entries.pop_back() {
                    // Numbered on from the last row left (the window starts its player again
                    // for the next clip).
                    self.next_seq = e.row.seq;
                    if e.owned {
                        let _ = std::fs::remove_file(&e.row.path);
                    }
                    self.bytes = self.bytes.saturating_sub(e.bytes);
                }
            }
        }
        for e in &mut self.entries {
            if e.expires_at.is_none() && clip != Some(e.row.clip.as_str()) {
                e.expires_at = Some(until);
            }
        }
        self.current = clip.map(str::to_string);
    }

    /// Removes the rows (and links) past their time (not the pinned one), of a clip file that
    /// is gone (no link), beyond the size or beyond the count — always from the front, so the
    /// numbering of what is left stays dense.
    pub fn sweep(&mut self, now: u64) {
        while let Some(front) = self.entries.front() {
            let expired =
                front.expires_at.is_some_and(|at| at <= now) && self.pinned != Some(front.row.seq);
            let missing = !front.owned && front.aired && !front.row.path.exists();
            let too_big = self.bytes > self.max_bytes;
            let too_many = self.entries.len() > self.max_rows;
            if !(expired || missing || too_big || too_many) {
                break;
            }
            if let Some(e) = self.entries.pop_front() {
                self.forget(e);
            }
        }
        // The new first row's discontinuity, if any, stays on it (still counted from the tag).
    }

    fn forget(&mut self, e: Entry) {
        if e.owned {
            let _ = std::fs::remove_file(&e.row.path);
        }
        self.bytes = self.bytes.saturating_sub(e.bytes);
        if e.row.discontinuity {
            self.discontinuity_sequence += 1;
        }
    }

    pub fn feed(
        &self,
        active: bool,
        antenna_seq: Option<u64>,
        aired_seq: Option<u64>,
    ) -> MonitorFeed {
        MonitorFeed {
            rows: self.entries.iter().map(|e| e.row.clone()).collect(),
            discontinuity_sequence: self.discontinuity_sequence,
            next_seq: self.next_seq,
            target_duration: self.target_s,
            active,
            antenna_seq,
            aired_seq,
        }
    }
}

impl Drop for MonitorTimeline {
    /// The channel stopped (or was rebuilt): its links go (a leftover folder is cleared with
    /// the clips at the next start).
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(dir: &Path, name: &str, bytes: usize) -> SegmentInfo {
        let path = dir.join(name);
        std::fs::write(&path, vec![0x47u8; bytes]).unwrap();
        SegmentInfo {
            path,
            duration: 4.0,
            index: 0,
        }
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        clips: PathBuf,
        root: PathBuf,
    }

    fn fx() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let clips = tmp.path().join("clips").join("7-0-1");
        std::fs::create_dir_all(&clips).unwrap();
        let root = tmp.path().join("clips").join("_monitor");
        Fx {
            _tmp: tmp,
            clips,
            root,
        }
    }

    #[test]
    fn rows_are_numbered_in_order_and_a_new_clip_starts_after_a_discontinuity() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("clip-7-0"), 0);
        for i in 0..3 {
            let s = seg(&f.clips, &format!("seg_{i:05}.ts"), 188);
            assert_eq!(m.push("clip-7-0", 7, &s, 10.0 + 4.0 * i as f64), i as u64);
        }
        for i in 0..3 {
            m.aired(i, 500);
        }
        // A seek: the next process of the same film, from 60 s.
        m.set_current(Some("clip-7-0-r1"), 1000);
        let s = seg(&f.clips, "seg_00000b.ts", 188);
        assert_eq!(m.push("clip-7-0-r1", 7, &s, 60.0), 3);
        let feed = m.feed(true, None, None);
        let seqs: Vec<u64> = feed.rows.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, [0, 1, 2, 3]);
        let disc: Vec<bool> = feed.rows.iter().map(|r| r.discontinuity).collect();
        assert_eq!(disc, [false, false, false, true]);
        assert_eq!(feed.rows[1].position_s, 14.0);
        assert_eq!(feed.rows[3].clip, "clip-7-0-r1");
        assert_eq!(feed.next_seq, 4);

        let text = feed.playlist(None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "#EXTM3U");
        assert!(lines.contains(&"#EXT-X-MEDIA-SEQUENCE:0"), "{text}");
        assert!(lines.contains(&"#EXT-X-DISCONTINUITY-SEQUENCE:0"), "{text}");
        assert!(lines.contains(&"#EXT-X-TARGETDURATION:4"), "{text}");
        assert!(!text.contains("ENDLIST"), "a live list: {text}");
        let uris: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| !l.starts_with('#'))
            .collect();
        assert_eq!(
            uris,
            [
                "monitor/clip-7-0/seg_00000.ts",
                "monitor/clip-7-0/seg_00001.ts",
                "monitor/clip-7-0/seg_00002.ts",
                "monitor/clip-7-0-r1/seg_00000b.ts",
            ]
        );
        // The discontinuity comes right before the first segment of the new clip.
        let at = lines
            .iter()
            .position(|l| *l == "monitor/clip-7-0-r1/seg_00000b.ts")
            .unwrap();
        assert_eq!(lines[at - 3], "#EXT-X-DISCONTINUITY", "{text}");
        assert_eq!(lines[at - 2], "#VRCINEMA-AT:7,60.000");
        assert_eq!(lines[at - 1], "#EXTINF:4.000000,");
        assert_eq!(
            text.matches("#EXT-X-DISCONTINUITY\n").count(),
            1,
            "only between the clips: {text}"
        );
    }

    #[test]
    fn the_rows_outlive_the_clip_files_through_their_links() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("clip-7-0"), 0);
        let s = seg(&f.clips, "seg_00000.ts", 1000);
        m.push("clip-7-0", 7, &s, 0.0);
        // The channel removes the clip folder (a seek): the monitor still has the segment.
        std::fs::remove_dir_all(&f.clips).unwrap();
        let feed = m.feed(true, None, None);
        let row = feed.row("clip-7-0", "seg_00000.ts").expect("listed");
        assert_eq!(std::fs::read(&row.path).unwrap().len(), 1000);
        assert!(row.path.starts_with(&f.root), "{:?}", row.path);
        assert!(feed.row("clip-7-0", "seg_00001.ts").is_none());
        assert!(feed.row("clip-7-1", "seg_00000.ts").is_none());
        // Dropping the timeline (the channel stops) removes its links.
        drop(m);
        assert!(std::fs::read_dir(&f.root).unwrap().next().is_none());
    }

    #[test]
    fn aired_rows_stay_for_the_retention_time_then_leave_from_the_front() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("clip-7-0"), 0);
        for i in 0..4 {
            let s = seg(&f.clips, &format!("seg_{i:05}.ts"), 188);
            m.push("clip-7-0", 7, &s, 4.0 * i as f64);
        }
        m.aired(0, 1_000);
        m.aired(1, 5_000);
        m.sweep(1_000 + RETAIN_MS - 1);
        assert_eq!(m.feed(true, None, None).rows.len(), 4, "not yet");
        m.sweep(1_000 + RETAIN_MS);
        let feed = m.feed(true, None, None);
        assert_eq!(feed.rows.first().map(|r| r.seq), Some(1));
        let link = f.root.read_dir().unwrap().count();
        assert!(link >= 1);
        // Rows that have not aired stay however old they are, while their clip is current.
        m.sweep(10_000_000);
        let seqs: Vec<u64> = m
            .feed(true, None, None)
            .rows
            .iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(seqs, [2, 3]);
        assert!(
            m.feed(true, None, None)
                .playlist(None)
                .contains("#EXT-X-MEDIA-SEQUENCE:2")
        );
        // Once the clip is no longer current, its rows that never aired go at once.
        m.set_current(None, 10_000_000);
        m.sweep(10_000_000 + RETAIN_MS);
        assert!(m.feed(true, None, None).rows.is_empty());
        // The links went with the rows.
        let left: Vec<_> = std::fs::read_dir(m.dir.clone())
            .map(|d| d.flatten().collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn the_retained_links_are_bounded_in_size_and_count() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0)).with_limits(RETAIN_MS, 2500, 50);
        m.set_current(Some("clip-7-0"), 0);
        for i in 0..4 {
            let s = seg(&f.clips, &format!("seg_{i:05}.ts"), 1000);
            m.push("clip-7-0", 7, &s, 4.0 * i as f64);
            m.aired(i, 0);
        }
        m.sweep(1);
        let seqs: Vec<u64> = m
            .feed(true, None, None)
            .rows
            .iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(seqs, [2, 3], "2 x 1000 bytes fit in 2500");

        let mut m = MonitorTimeline::new(&f.root, Some(0)).with_limits(RETAIN_MS, MAX_BYTES, 3);
        m.set_current(Some("clip-7-0"), 0);
        for i in 0..5 {
            let s = seg(&f.clips, &format!("seg_{i:05}.ts"), 10);
            m.push("clip-7-0", 7, &s, 4.0 * i as f64);
        }
        m.sweep(1);
        let seqs: Vec<u64> = m
            .feed(true, None, None)
            .rows
            .iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(seqs, [2, 3, 4]);
    }

    #[test]
    fn a_discontinuity_that_leaves_the_front_is_counted() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("a"), 0);
        m.push("a", 1, &seg(&f.clips, "a0.ts", 10), 0.0);
        m.aired(0, 0);
        m.set_current(Some("b"), 0);
        m.push("b", 1, &seg(&f.clips, "b0.ts", 10), 30.0);
        m.aired(1, 0);
        m.set_current(Some("c"), 0);
        m.push("c", 2, &seg(&f.clips, "c0.ts", 10), 0.0);
        m.sweep(RETAIN_MS);
        let feed = m.feed(true, None, None);
        assert_eq!(feed.rows.len(), 1);
        // "b" left with its discontinuity; "c" keeps its own on the first segment.
        assert_eq!(feed.discontinuity_sequence, 1);
        let text = feed.playlist(None);
        assert!(text.contains("#EXT-X-DISCONTINUITY-SEQUENCE:1\n"), "{text}");
        assert!(text.contains("#EXT-X-DISCONTINUITY\n"), "{text}");
    }

    #[test]
    fn without_a_film_the_list_is_empty_but_well_formed() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("a"), 0);
        m.push("a", 1, &seg(&f.clips, "a0.ts", 10), 0.0);
        let text = m.feed(false, None, None).playlist(None);
        assert_eq!(
            text,
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXT-X-DISCONTINUITY-SEQUENCE:0\n"
        );
        let empty = MonitorFeed::default().playlist(None);
        assert!(empty.starts_with("#EXTM3U\n") && !empty.contains("#EXTINF"));
    }

    #[test]
    fn the_list_names_the_antenna_the_aired_row_and_where_to_start() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("a"), 0);
        for i in 0..3 {
            let mut s = seg(&f.clips, &format!("s{i}.ts"), 10);
            s.duration = [4.0, 4.5, 3.25][i];
            m.push("a", 9, &s, [0.0, 4.0, 8.5][i]);
        }
        let text = m.feed(true, Some(0), Some(1)).playlist(Some(2));
        assert!(text.contains("#VRCINEMA-ANTENNA:0\n"), "{text}");
        assert!(text.contains("#VRCINEMA-AIRED:1\n"), "{text}");
        assert!(
            text.contains("#EXT-X-TARGETDURATION:5\n"),
            "4.5 rounds up: {text}"
        );
        assert!(
            text.contains("#EXT-X-START:TIME-OFFSET=8.500,PRECISE=YES\n"),
            "starts at the third segment: {text}"
        );
        assert!(text.contains("#VRCINEMA-AT:9,8.500\n#EXTINF:3.250000,\nmonitor/a/s2.ts\n"));
        // A row the list no longer has: the player picks its own start.
        assert!(
            !m.feed(true, None, None)
                .playlist(Some(99))
                .contains("EXT-X-START")
        );
    }

    fn seqs(m: &MonitorTimeline) -> Vec<u64> {
        m.feed(true, None, None)
            .rows
            .iter()
            .map(|r| r.seq)
            .collect()
    }

    /// Review fix 5: material the viewers never see (the old clip's lookahead after a seek)
    /// leaves the list at once; the next clip numbers on from the last row left.
    #[test]
    fn a_new_clip_drops_the_old_clips_rows_that_never_aired() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("a"), 0);
        for i in 0..4 {
            m.push(
                "a",
                1,
                &seg(&f.clips, &format!("a{i}.ts"), 10),
                4.0 * i as f64,
            );
        }
        m.aired(0, 0);
        m.aired(1, 0);
        let gone = m.feed(true, None, None).rows[3].path.clone();
        m.set_current(Some("b"), 100);
        assert_eq!(seqs(&m), [0, 1]);
        assert!(!gone.exists(), "its link went too");
        assert_eq!(m.push("b", 1, &seg(&f.clips, "b0.ts", 10), 60.0), 2);
        let feed = m.feed(true, None, None);
        assert!(feed.rows[2].discontinuity);
        assert_eq!(feed.next_seq, 3);
    }

    /// Review fix 1: the still picture of a long pause stays past the retention time.
    #[test]
    fn a_pinned_row_outlives_the_retention_time() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("a"), 0);
        for i in 0..3 {
            m.push(
                "a",
                1,
                &seg(&f.clips, &format!("a{i}.ts"), 10),
                4.0 * i as f64,
            );
        }
        m.aired(0, 0);
        // Paused at 4 s: the row of 4 s stays, the later one (never aired) goes.
        assert_eq!(m.find(1, 4.0), Some(1));
        m.pin(Some(1));
        m.set_current(None, 0);
        assert_eq!(seqs(&m), [0, 1]);
        m.sweep(10 * RETAIN_MS);
        assert_eq!(seqs(&m), [1], "the aired row went, the pinned one stays");
        assert!(m.feed(true, None, None).rows[0].path.exists());
        // Resumed: it goes like any other.
        m.pin(None);
        m.sweep(10 * RETAIN_MS);
        assert!(seqs(&m).is_empty());
        assert_eq!(m.find(1, 4.0), None);
    }

    /// Review fix 3: without links an aired row leaves once the channel removed its file.
    #[test]
    fn aired_rows_without_a_link_leave_once_their_file_is_gone() {
        let f = fx();
        std::fs::create_dir_all(f.root.parent().unwrap()).unwrap();
        std::fs::write(&f.root, b"x").unwrap();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("a"), 0);
        let s0 = seg(&f.clips, "a0.ts", 10);
        let s1 = seg(&f.clips, "a1.ts", 10);
        m.push("a", 1, &s0, 0.0);
        m.push("a", 1, &s1, 4.0);
        m.aired(0, 0);
        m.aired(1, 0);
        std::fs::remove_file(&s0.path).unwrap();
        m.sweep(1);
        assert_eq!(seqs(&m), [1]);
    }

    /// Review fixes 6 and 7.
    #[test]
    fn the_target_never_shrinks_and_a_clip_after_an_empty_list_keeps_its_discontinuity() {
        let f = fx();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("a"), 0);
        let mut long = seg(&f.clips, "a0.ts", 10);
        long.duration = 8.3;
        m.push("a", 1, &long, 0.0);
        m.aired(0, 0);
        m.sweep(RETAIN_MS);
        assert!(seqs(&m).is_empty());
        m.set_current(Some("b"), RETAIN_MS);
        m.push("b", 2, &seg(&f.clips, "b0.ts", 10), 0.0);
        let feed = m.feed(true, None, None);
        assert!(
            feed.rows[0].discontinuity,
            "after another clip, also one that left"
        );
        assert!(feed.playlist(None).contains("#EXT-X-TARGETDURATION:8\n"));
        // An idle list does not go back below the rows it still has.
        assert!(
            m.feed(false, None, None)
                .playlist(None)
                .contains("#EXT-X-MEDIA-SEQUENCE:1\n")
        );
    }

    /// Review fix 2: a rebuilt channel (or a restarted server) of the same program numbers on.
    #[test]
    fn numbering_goes_on_across_the_timelines_of_the_process() {
        let f = fx();
        let mut first = MonitorTimeline::new(&f.root, None);
        first.set_current(Some("a"), 0);
        let mut last = 0;
        for i in 0..3 {
            last = first.push("a", 1, &seg(&f.clips, &format!("a{i}.ts"), 10), 0.0);
        }
        drop(first);
        let mut second = MonitorTimeline::new(&f.root, None);
        second.set_current(Some("a"), 0);
        let next = second.push("a", 1, &seg(&f.clips, "x.ts", 10), 0.0);
        assert!(next > last, "{next} after {last}");
    }

    #[test]
    fn the_watch_counts_recent_requests_only() {
        let w = MonitorWatch::default();
        assert!(!w.watched());
        w.touch();
        assert!(w.watched());
        assert!(MonitorWatch::always().watched());
    }

    /// With the real ffmpeg: after a seek the first segment of the new clip shows up in the
    /// list within a few seconds, and it really is the film at the target (the test film's
    /// brightness counts its seconds).
    #[tokio::test(flavor = "multi_thread")]
    async fn after_a_seek_the_new_clips_first_segment_is_listed_and_shows_the_target() {
        use crate::config::VideoEncoder;
        use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};
        use crate::pipeline::broadcaster::SegmentSource;
        use crate::pipeline::channel::{Channel, ChannelOptions};
        use crate::pipeline::clip_factory::{ClipFactoryDeps, create_clip_factory};
        use crate::pipeline::slates::Slates;
        use crate::resolver::{
            MediaMeta, MediaResolver, ResolveError, ResolvedStream, StreamInput,
        };
        use std::ffi::OsString;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("bin");
        let (ffmpeg, ffprobe) = (root.join("ffmpeg.exe"), root.join("ffprobe.exe"));
        if !ffmpeg.exists() || !ffprobe.exists() {
            eprintln!("skip: bin/ffmpeg or bin/ffprobe missing");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        // 40 s whose brightness is 6 × the second (keyframe every second).
        let film = tmp.path().join("film.mp4");
        let args: Vec<OsString> = [
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=160x120:r=25:d=40,geq=lum='floor(T)*6':cb=128:cr=128",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-pix_fmt",
            "yuv420p",
        ]
        .iter()
        .map(OsString::from)
        .chain([film.clone().into_os_string()])
        .collect();
        let made = crate::process::run_process(&ffmpeg, &args, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(made.code, Some(0), "{}", made.stderr);

        struct FileResolver(PathBuf);
        #[async_trait::async_trait]
        impl MediaResolver for FileResolver {
            async fn fetch_meta(&self, _url: &str) -> Result<MediaMeta, ResolveError> {
                Err(ResolveError::new("unused"))
            }
            async fn resolve(&self, _url: &str) -> Result<ResolvedStream, ResolveError> {
                Ok(ResolvedStream {
                    inputs: vec![StreamInput {
                        url: self.0.to_string_lossy().into_owned(),
                        headers: vec![],
                    }],
                    title: "film".into(),
                    duration_s: Some(40.0),
                    is_live: false,
                })
            }
        }
        let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
        {
            let conn = db.lock().unwrap();
            QueueRepo::new(&conn)
                .add(&NewItem {
                    channel_id: MAIN_CHANNEL_ID,
                    source_url: "https://youtu.be/x".into(),
                    title: "Film".into(),
                    duration_s: Some(40.0),
                    is_live: false,
                    status: ItemStatus::Queued,
                    added_by: AddedBy::Admin,
                    requester_name: None,
                    requester_ip: None,
                    requester_key_hash: None,
                })
                .unwrap();
        }
        let clips = tmp.path().join("clips");
        let factory = create_clip_factory(ClipFactoryDeps {
            resolver: Arc::new(FileResolver(film)),
            ffmpeg_path: ffmpeg.clone(),
            ffprobe_path: ffprobe,
            encoder: crate::live::LiveValue::new(VideoEncoder::Libx264),
            segment_seconds: 4,
            clips_dir: clips.clone(),
            db: Arc::clone(&db),
            channel_id: MAIN_CHANNEL_ID,
            log: None,
            font_file: PathBuf::from("C:/Windows/Fonts/arial.ttf"),
            public_url: crate::live::LiveValue::new(String::new()),
            host_resolver: Arc::new(crate::rules::TableHostResolver::new(&[])),
            check_viewer_media: true,
        });
        let slate = |name: &str| SegmentInfo {
            path: tmp.path().join(name),
            duration: 4.0,
            index: 0,
        };
        let slates = Slates {
            idle: vec![slate("idle.ts")],
            buffering: vec![slate("buffering.ts")],
            pause: vec![slate("pause.ts")],
            generation: 0,
            japanese_font_missing: false,
            font_missing: None,
        };
        let mut o = ChannelOptions::new(MAIN_CHANNEL_ID, db, Arc::new(factory), slates);
        o.monitor = Some(MonitorOptions {
            dir: clips.join("_monitor"),
            watch: Arc::new(MonitorWatch::always()),
            first_seq: None,
        });
        let mut channel = Channel::new(o);
        channel.start().unwrap();
        // The loop: a tick every 250 ms; with `airing`, a segment for the broadcaster every 4 s
        // (as on air).
        async fn run_until(
            channel: &mut Channel,
            airing: bool,
            what: &str,
            mut done: impl FnMut(&MonitorFeed) -> bool,
        ) -> Duration {
            let started = Instant::now();
            let mut last_next = Instant::now() - Duration::from_secs(5);
            loop {
                if airing && last_next.elapsed() >= Duration::from_secs(4) {
                    last_next = Instant::now();
                    let _ = channel.next();
                }
                channel.on_tick();
                if done(&channel.monitor_feed()) {
                    return started.elapsed();
                }
                assert!(
                    started.elapsed() < Duration::from_secs(60),
                    "timeout: {what}"
                );
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        run_until(&mut channel, true, "the first clip is listed", |f| {
            f.rows.len() >= 2
        })
        .await;
        let first_clip = channel.monitor_feed().rows[0].clip.clone();
        channel.seek_to(20.0);
        // From now on the broadcaster asks for nothing: only the loop's ticks bring the new clip
        // to the list (on air it would wait up to a segment for the next request).
        let took = run_until(&mut channel, false, "the new clip is listed", |f| {
            f.rows.iter().any(|r| r.clip != first_clip)
        })
        .await;
        eprintln!("the first segment after the seek was listed {took:?} after the seek");
        // 700 ms of quiet, then the start of ffmpeg and its first 4 s segment (read at 1.5×, with
        // the initial burst): about a second here; generous under the load of the whole suite.
        assert!(took < Duration::from_secs(15), "{took:?}");
        let feed = channel.monitor_feed();
        let row = feed.rows.iter().find(|r| r.clip != first_clip).unwrap();
        assert!(row.discontinuity, "{row:?}");
        assert_eq!(row.position_s, 20.0);
        assert!(
            feed.playlist(None)
                .contains(&format!("monitor/{}/{}", row.clip, row.name))
        );
        // Its first picture: brightness 6 × 20 (the keyframe at 20 s; 19 s at worst).
        let out = std::process::Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&row.path)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=1:1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gray",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let luma = i32::from(out.stdout[0]);
        assert!(
            (108..=126).contains(&luma),
            "brightness {luma}: second {}",
            luma / 6
        );
        channel.stop();
        drop(channel);
        // The stopped ffmpeg lets go of the film a moment later: the folder goes then (a
        // TempDir dropped at once would stay behind in %TEMP%).
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::fs::remove_dir_all(tmp.path()).is_err() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!tmp.path().exists(), "the test folder was not removed");
    }

    #[test]
    fn a_segment_that_cannot_be_linked_is_served_from_the_clip_while_it_exists() {
        let f = fx();
        // The monitor's folder cannot be made (a file is in the way): no links.
        std::fs::create_dir_all(f.root.parent().unwrap()).unwrap();
        std::fs::write(&f.root, b"x").unwrap();
        let mut m = MonitorTimeline::new(&f.root, Some(0));
        m.set_current(Some("a"), 0);
        let s = seg(&f.clips, "a0.ts", 10);
        m.push("a", 1, &s, 0.0);
        let feed = m.feed(true, None, None);
        assert_eq!(feed.rows[0].path, s.path);
        m.aired(0, 0);
        m.sweep(RETAIN_MS);
        // The clip's own file is never removed by the monitor.
        assert!(s.path.exists());
    }
}
