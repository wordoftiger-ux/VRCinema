//! The air observer (Settings → Advanced, spec `2026-11-06-advanced-design.md`): follows the
//! engine's snapshots — the published segments and the film each shows — without touching the
//! channel or the broadcaster, and
//!
//! - keeps the broadcast journal (`db::journal`): a segment of another film (or a slate) closes
//!   the open row, a film's segment opens a row for it;
//! - when the alarms are on, analyses every published film segment with ffmpeg (`blackdetect`,
//!   `silencedetect`) and raises an alarm once the picture has been black, or the sound silent,
//!   for the set time. Runs add up the segments' own durations, so any segment length works.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::db::journal::JournalRepo;
use crate::db::{QueueRepo, SettingsRepo};
use crate::engine::Snapshot;
use crate::i18n::Msg;
use crate::pipeline::advanced::AdvancedSettings;
use crate::pipeline::air::AirSegment;

pub type SharedLog = Arc<dyn Fn(Msg) + Send + Sync>;

/// A segment counts as black (silent) when at least this share of it is.
const COVERED: f64 = 0.9;
/// Longest analysis of one segment.
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(20);
/// Segments waiting for analysis at most, 16 s of them (more are skipped: the computer is
/// behind).
const QUEUE: usize = crate::pipeline::segments_for(16);
/// How often the settings are read again.
const SETTINGS_EVERY: Duration = Duration::from_secs(2);

/// The alarms on show (`/api/desktop/info`, `/api/admin/environment`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Alarms {
    pub black: bool,
    pub silence: bool,
}

/// Shared between the observer and the HTTP routes.
#[derive(Debug, Default)]
pub struct AlarmBoard {
    alarms: Mutex<Alarms>,
}

impl AlarmBoard {
    pub fn get(&self) -> Alarms {
        *self.alarms.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, alarms: Alarms) {
        *self.alarms.lock().unwrap_or_else(PoisonError::into_inner) = alarms;
    }
}

/// What one segment's analysis found (seconds).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Findings {
    pub black_s: f64,
    pub silent_s: f64,
}

/// Seconds of black picture and silence in ffmpeg's output of `blackdetect` and
/// `silencedetect` for a segment of `duration` seconds (a start without an end lasts to the
/// end of the segment).
pub fn parse_findings(stderr: &str, duration: f64) -> Findings {
    let value = |line: &str, key: &str| -> Option<f64> {
        let at = line.find(key)? + key.len();
        line[at..]
            .trim_start()
            .split(|c: char| c.is_whitespace() || c == '|')
            .next()?
            .parse()
            .ok()
    };
    let mut f = Findings::default();
    let (mut black_open, mut silence_open) = (None, None);
    for line in stderr.lines() {
        if line.contains("black_start:") {
            if let Some(d) = value(line, "black_duration:") {
                f.black_s += d;
            } else {
                black_open = value(line, "black_start:");
            }
        } else if let Some(start) = value(line, "silence_start:") {
            silence_open = Some(start);
        } else if let Some(d) = value(line, "silence_duration:") {
            f.silent_s += d;
            silence_open = None;
        }
    }
    if let Some(start) = black_open {
        f.black_s += (duration - start).max(0.0);
    }
    if let Some(start) = silence_open {
        f.silent_s += (duration - start).max(0.0);
    }
    f.black_s = f.black_s.min(duration);
    f.silent_s = f.silent_s.min(duration);
    f
}

/// The running lengths of black picture and silence and the alarms they raise.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlarmRuns {
    pub black_s: f64,
    pub silence_s: f64,
}

impl AlarmRuns {
    /// One analysed film segment of `duration` seconds; returns the alarms now.
    pub fn feed(&mut self, duration: f64, found: Findings, adv: &AdvancedSettings) -> Alarms {
        let covered = |s: f64| duration > 0.0 && s >= COVERED * duration;
        self.black_s = if covered(found.black_s) {
            self.black_s + duration
        } else {
            0.0
        };
        self.silence_s = if covered(found.silent_s) {
            self.silence_s + duration
        } else {
            0.0
        };
        Alarms {
            black: self.black_s >= f64::from(adv.alarm_black_s),
            silence: self.silence_s >= f64::from(adv.alarm_silence_s),
        }
    }

    /// A slate, a pause or the alarms switched off: nothing runs.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// ffmpeg's arguments analysing one segment file.
pub fn analysis_args(file: &Path, silence_db: i32) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostats", "-nostdin", "-i"]
        .iter()
        .map(OsString::from)
        .collect();
    args.push(file.into());
    for a in [
        "-vf".to_string(),
        "blackdetect=d=0.1:pix_th=0.10:pic_th=0.98".to_string(),
        "-af".to_string(),
        format!("silencedetect=n={silence_db}dB:d=0.1"),
        "-f".to_string(),
        "null".to_string(),
        "-".to_string(),
    ] {
        args.push(a.into());
    }
    args
}

/// A journal row being written.
#[derive(Debug, Clone, PartialEq)]
struct OpenRow {
    row: i64,
    item_id: i64,
    to_s: Option<f64>,
}

pub struct AirWatchOptions {
    pub db: Arc<Mutex<Connection>>,
    pub channel_id: i64,
    pub ffmpeg_path: PathBuf,
    pub log: Option<SharedLog>,
    pub board: Arc<AlarmBoard>,
}

/// Starts the observer; it ends when the engine's snapshots end (or when aborted).
pub fn spawn_air_watch(
    snapshots: watch::Receiver<Arc<Snapshot>>,
    o: AirWatchOptions,
) -> JoinHandle<()> {
    tokio::spawn(run(snapshots, o))
}

fn now_ms() -> i64 {
    crate::db::now_ms()
}

struct Watcher {
    o: AirWatchOptions,
    open: Option<OpenRow>,
    settings: AdvancedSettings,
    settings_at: Option<std::time::Instant>,
    analyses: mpsc::Sender<(PathBuf, f64)>,
}

impl Watcher {
    fn settings(&mut self) -> &AdvancedSettings {
        let stale = self
            .settings_at
            .is_none_or(|at| at.elapsed() >= SETTINGS_EVERY);
        if stale {
            let conn = self.o.db.lock().unwrap_or_else(PoisonError::into_inner);
            if let Ok(s) = SettingsRepo::new(&conn).get(self.o.channel_id) {
                self.settings = s.advanced;
            }
            self.settings_at = Some(std::time::Instant::now());
        }
        &self.settings
    }

    fn log(&self, msg: Msg) {
        if let Some(log) = &self.o.log {
            log(msg);
        }
    }

    /// Closes the open row at `at` (Unix ms).
    fn close(&mut self, at: i64) {
        let Some(open) = self.open.take() else {
            return;
        };
        let conn = self.o.db.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(e) = JournalRepo::new(&conn).close(open.row, at, open.to_s) {
            drop(conn);
            self.log(Msg::new("log.journal_failed").arg("error", e));
        }
    }

    fn journal(&mut self, seg: &AirSegment, at: i64) {
        let end_s = seg.position_s.map(|p| p + seg.duration);
        if let (Some(open), Some(id)) = (&mut self.open, seg.item_id)
            && open.item_id == id
        {
            open.to_s = end_s.or(open.to_s);
            return;
        }
        self.close(at);
        let Some(id) = seg.item_id else {
            return;
        };
        let conn = self.o.db.lock().unwrap_or_else(PoisonError::into_inner);
        let opened = QueueRepo::new(&conn).get(id).and_then(|item| match item {
            Some(item) => JournalRepo::new(&conn)
                .open(self.o.channel_id, &item, at, seg.position_s)
                .map(Some),
            None => Ok(None),
        });
        drop(conn);
        match opened {
            Ok(Some(row)) => {
                self.open = Some(OpenRow {
                    row,
                    item_id: id,
                    to_s: end_s,
                });
            }
            Ok(None) => {}
            Err(e) => self.log(Msg::new("log.journal_failed").arg("error", e)),
        }
    }

    fn analyse(&mut self, seg: &AirSegment, live_dir: &Path) {
        let file = live_dir.join(format!("{}.ts", seg.seq));
        // A full queue: the computer is behind; this segment is not looked at.
        let _ = self.analyses.try_send((file, seg.duration));
    }
}

async fn run(mut snapshots: watch::Receiver<Arc<Snapshot>>, o: AirWatchOptions) {
    let (tx, rx) = mpsc::channel(QUEUE);
    // `Some(item)` per segment sent: a film segment (analysed) or `None` (a slate: runs reset).
    let analyser = tokio::spawn(analyse_loop(
        rx,
        Arc::clone(&o.db),
        o.channel_id,
        o.ffmpeg_path.clone(),
        o.log.clone(),
        Arc::clone(&o.board),
    ));
    let mut w = Watcher {
        o,
        open: None,
        settings: AdvancedSettings::default(),
        settings_at: None,
        analyses: tx,
    };
    let mut last_seq: Option<u64> = None;
    while snapshots.changed().await.is_ok() {
        let snap = Arc::clone(&snapshots.borrow_and_update());
        let segments = &snap.air.segments;
        let Some(newest) = segments.last().map(|s| s.seq) else {
            continue;
        };
        // A new run of the loop numbers its segments anew.
        if last_seq.is_some_and(|l| newest < l) {
            last_seq = None;
        }
        let fresh: Vec<&AirSegment> = segments
            .iter()
            .filter(|s| last_seq.is_none_or(|l| s.seq > l))
            .collect();
        if fresh.is_empty() {
            continue;
        }
        last_seq = Some(newest);
        let at = now_ms();
        let alarms_on = w.settings().alarms_enabled;
        for seg in fresh {
            w.journal(seg, at);
            if alarms_on {
                if seg.item_id.is_some() {
                    w.analyse(seg, &snap.live_dir);
                } else {
                    // A slate: an empty path resets the runs.
                    let _ = w.analyses.try_send((PathBuf::new(), seg.duration));
                }
            }
        }
        if !alarms_on {
            w.o.board.set(Alarms::default());
        }
    }
    w.close(now_ms());
    analyser.abort();
}

async fn analyse_loop(
    mut rx: mpsc::Receiver<(PathBuf, f64)>,
    db: Arc<Mutex<Connection>>,
    channel_id: i64,
    ffmpeg: PathBuf,
    log: Option<SharedLog>,
    board: Arc<AlarmBoard>,
) {
    let mut runs = AlarmRuns::default();
    while let Some((file, duration)) = rx.recv().await {
        let adv = {
            let conn = db.lock().unwrap_or_else(PoisonError::into_inner);
            SettingsRepo::new(&conn)
                .get(channel_id)
                .map(|s| s.advanced)
                .unwrap_or_default()
        };
        if !adv.alarms_enabled || file.as_os_str().is_empty() {
            runs.reset();
            board.set(Alarms::default());
            continue;
        }
        let args = analysis_args(&file, adv.alarm_silence_db);
        let Ok(out) = crate::process::run_process(&ffmpeg, &args, ANALYSIS_TIMEOUT).await else {
            continue;
        };
        if out.code != Some(0) {
            // The segment left the window before it was read: not known, nothing changes.
            continue;
        }
        let before = board.get();
        let now = runs.feed(duration, parse_findings(&out.stderr, duration), &adv);
        if let Some(log) = &log {
            if now.black && !before.black {
                log(Msg::new("log.alarm_black").arg("seconds", runs.black_s.round() as i64));
            }
            if now.silence && !before.silence {
                log(Msg::new("log.alarm_silence").arg("seconds", runs.silence_s.round() as i64));
            }
        }
        board.set(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK_ALL: &str = "[blackdetect @ 0x1] black_start:0 black_end:4 black_duration:4\n";

    #[test]
    fn ffmpeg_output_gives_black_and_silent_seconds() {
        let f = parse_findings(
            "Input #0, mpegts\n\
             [silencedetect @ 0x2] silence_start: 0.5\n\
             [silencedetect @ 0x2] silence_end: 2.5 | silence_duration: 2\n\
             [blackdetect @ 0x1] black_start:1 black_end:2.5 black_duration:1.5\n\
             [silencedetect @ 0x2] silence_start: 3\n",
            4.0,
        );
        assert_eq!(
            f,
            Findings {
                black_s: 1.5,
                silent_s: 3.0
            }
        );
        // A black start without an end (an ffmpeg that does not flush) lasts to the end.
        let f = parse_findings("[blackdetect @ 0x1] black_start:0.2\n", 4.0);
        assert!((f.black_s - 3.8).abs() < 1e-9);
        assert_eq!(parse_findings(BLACK_ALL, 4.0).black_s, 4.0);
    }

    #[test]
    fn runs_add_segment_durations_up_to_the_thresholds() {
        let adv = AdvancedSettings {
            alarms_enabled: true,
            alarm_black_s: 10,
            alarm_silence_s: 3,
            ..AdvancedSettings::default()
        };
        let mut runs = AlarmRuns::default();
        let black = Findings {
            black_s: 3.8,
            silent_s: 0.0,
        };
        assert_eq!(runs.feed(4.0, black, &adv), Alarms::default());
        assert_eq!(runs.feed(4.0, black, &adv), Alarms::default());
        assert!(runs.feed(4.0, black, &adv).black, "12 s ≥ 10 s");
        // One normal segment clears it.
        assert!(!runs.feed(4.0, Findings::default(), &adv).black);
        // One-second segments add up the same way.
        let silent = Findings {
            black_s: 0.0,
            silent_s: 1.0,
        };
        assert!(!runs.feed(1.0, silent, &adv).silence);
        assert!(!runs.feed(1.0, silent, &adv).silence);
        assert!(runs.feed(1.0, silent, &adv).silence);
        runs.reset();
        assert_eq!(runs, AlarmRuns::default());
    }

    #[test]
    fn the_analysis_reads_the_file_with_both_detectors() {
        let args: Vec<String> = analysis_args(Path::new("live/12.ts"), -45)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"silencedetect=n=-45dB:d=0.1".to_string()));
        assert!(args.iter().any(|a| a.starts_with("blackdetect=")));
        assert_eq!(args.last().unwrap(), "-");
    }
}
