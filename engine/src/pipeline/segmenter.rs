//! One ffmpeg process cutting a single clip into HLS segments. Port of `src/pipeline/segmenter.ts`.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::sync::{Notify, watch};

use crate::i18n::Msg;
use crate::process;
use crate::stats::parse_speed;

/// How many trailing non-empty stderr lines are kept for the error message.
const STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentInfo {
    pub path: PathBuf,
    pub duration: f64,
    pub index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmenterState {
    Idle,
    Running,
    Done,
    Failed,
    Killed,
}

/// JavaScript `Number(text)` for the CSV time columns: blank means 0, junk is `None`.
fn parse_js_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0.0);
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Reads the segment muxer CSV list: `name,start,end` on every complete line.
/// The last element after splitting on `\n` is an incomplete line (or empty) and is ignored.
pub fn parse_segment_csv(text: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    for line in &lines[..lines.len() - 1] {
        let parts: Vec<&str> = line.trim().split(',').collect();
        if parts.len() < 3 {
            continue;
        }
        let (Some(start), Some(end)) = (
            parse_js_number(parts[parts.len() - 2]),
            parse_js_number(parts[parts.len() - 1]),
        ) else {
            continue;
        };
        let name = parts[..parts.len() - 2].join(",");
        if name.is_empty() {
            continue;
        }
        // Math.round semantics (halves go up), 6 decimals.
        let duration = ((end - start) * 1e6 + 0.5).floor() / 1e6;
        out.push((name, duration));
    }
    out
}

#[derive(Debug, Clone)]
struct Snapshot {
    state: SegmenterState,
    /// Why it failed: ffmpeg's last words, else its exit code.
    error: Option<Msg>,
    /// Set once the child has really exited and been reaped (or never started).
    exited: bool,
    /// ffmpeg's last stderr lines once it has exited.
    stderr_tail: Vec<String>,
}

/// ffmpeg process cutting one clip into segments. States: Idle -> Running -> Done | Failed | Killed.
/// A background task waits for the child and publishes the final state; [`ClipSegmenter::poll`]
/// returns the segments that appeared since the previous call.
#[derive(Debug)]
pub struct ClipSegmenter {
    out_dir: PathBuf,
    consumed: Mutex<usize>,
    status: watch::Sender<Snapshot>,
    kill: Arc<Notify>,
    /// The ffmpeg process, once it runs.
    pid: Option<u32>,
    /// The latest progress report.
    progress: Progress,
}

/// What ffmpeg's `-progress` reports, kept as `f64` bits ([`NOT_YET`]: nothing reported).
#[derive(Debug, Clone)]
struct Progress {
    /// `speed=` (1.0: as fast as it plays).
    speed: Arc<AtomicU64>,
    /// `out_time_us=` in seconds: how far the output got.
    out_time_s: Arc<AtomicU64>,
}

impl Progress {
    fn new() -> Self {
        Self {
            speed: Arc::new(AtomicU64::new(NOT_YET)),
            out_time_s: Arc::new(AtomicU64::new(NOT_YET)),
        }
    }
}

fn read_cell(cell: &AtomicU64) -> Option<f64> {
    match cell.load(Ordering::Relaxed) {
        NOT_YET => None,
        bits => Some(f64::from_bits(bits)),
    }
}

impl ClipSegmenter {
    /// Creates `out_dir` and starts ffmpeg right away (must be called inside a tokio runtime).
    /// `cwd` is the child's working directory (relative paths in filters resolve against it).
    /// A failure to start is reported as the `Failed` state with the error message.
    pub fn start(
        ffmpeg: &Path,
        args: Vec<OsString>,
        out_dir: PathBuf,
        cwd: Option<PathBuf>,
    ) -> Self {
        let (status, _) = watch::channel(Snapshot {
            state: SegmenterState::Running,
            error: None,
            exited: false,
            stderr_tail: Vec::new(),
        });
        let mut segmenter = ClipSegmenter {
            out_dir,
            consumed: Mutex::new(0),
            status,
            kill: Arc::new(Notify::new()),
            pid: None,
            progress: Progress::new(),
        };
        if let Err(e) = std::fs::create_dir_all(&segmenter.out_dir) {
            segmenter.finish(SegmenterState::Failed, Some(output(e)));
            return segmenter;
        }

        let mut command = process::command(ffmpeg);
        command
            .args(&args)
            .stdin(Stdio::null())
            // `-progress pipe:1` (see `build_clip_args`) reports on stdout, so stderr stays
            // errors only; ffmpegs without the option write nothing here.
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &cwd {
            command.current_dir(cwd);
        }
        let mut child = match process::spawn(&mut command) {
            Ok(child) => child,
            Err(e) => {
                segmenter.finish(SegmenterState::Failed, Some(output(e)));
                return segmenter;
            }
        };

        segmenter.pid = child.id();
        let stderr = child.stderr.take();
        if let Some(progress) = child.stdout.take() {
            tokio::spawn(read_progress(progress, segmenter.progress.clone()));
        }
        let status = segmenter.status.clone();
        let kill = Arc::clone(&segmenter.kill);
        tokio::spawn(async move {
            let tail = tokio::spawn(read_stderr_tail(stderr));
            let exit = tokio::select! {
                exit = child.wait() => exit,
                () = kill.notified() => {
                    let _ = child.kill().await;
                    child.wait().await
                }
            };
            let tail = tail.await.unwrap_or_default();
            let lines: Vec<String> = tail.iter().cloned().collect();
            let (state, error) = match exit {
                Ok(code) if code.success() => (SegmenterState::Done, None),
                Ok(code) => {
                    let error = tail.back().map_or_else(
                        || {
                            let code = code.code().map_or_else(|| "?".into(), |c| c.to_string());
                            Msg::new("item_errors.ffmpeg_exit").arg("code", code)
                        },
                        output,
                    );
                    (SegmenterState::Failed, Some(error))
                }
                Err(e) => (SegmenterState::Failed, Some(output(e))),
            };
            // A kill() that already moved the state to Killed wins over the exit status.
            status.send_modify(|s| {
                if s.state == SegmenterState::Running {
                    s.state = state;
                    s.error = error;
                }
                s.exited = true;
                s.stderr_tail = lines;
            });
        });
        segmenter
    }

    /// ffmpeg's last stderr lines (empty until it has exited).
    pub fn stderr_tail(&self) -> Vec<String> {
        self.status.borrow().stderr_tail.clone()
    }

    /// Failure before any child exists: terminal state and exit signal at once.
    fn finish(&self, state: SegmenterState, error: Option<Msg>) {
        self.status.send_modify(|s| {
            s.state = state;
            s.error = error;
            s.exited = true;
        });
    }

    /// The ffmpeg process id while it runs (it can be reused once the process is gone).
    pub fn pid(&self) -> Option<u32> {
        let s = self.status.borrow();
        (s.state == SegmenterState::Running && !s.exited)
            .then_some(self.pid)
            .flatten()
    }

    /// ffmpeg's `speed=` from its latest progress report (1.0: as fast as it plays).
    pub fn speed(&self) -> Option<f64> {
        read_cell(&self.progress.speed)
    }

    /// How far ffmpeg's output got, in seconds of the clip (`out_time` of its latest progress
    /// report; the filters' `t` runs slightly ahead of it).
    pub fn out_time_s(&self) -> Option<f64> {
        read_cell(&self.progress.out_time_s)
    }

    pub fn state(&self) -> SegmenterState {
        self.status.borrow().state
    }

    /// Why ffmpeg failed (for the person: in their language).
    pub fn error(&self) -> Option<Msg> {
        self.status.borrow().error.clone()
    }

    /// Resolves with the final state (Done, Failed or Killed) once ffmpeg has actually exited
    /// and been reaped, so its files and working directory are released. `state()` may already
    /// report Killed earlier, right after `kill()`.
    pub async fn wait(&self) -> SegmenterState {
        let mut rx = self.status.subscribe();
        loop {
            let (state, exited) = {
                let s = rx.borrow_and_update();
                (s.state, s.exited)
            };
            if exited {
                return state;
            }
            if rx.changed().await.is_err() {
                return self.state();
            }
        }
    }

    /// Segments that appeared in `list.csv` since the previous call.
    pub fn poll(&self) -> Vec<SegmentInfo> {
        // Hold the lock across read + parse + update so concurrent polls cannot hand out a
        // segment twice or move `consumed` backwards.
        let mut consumed = self.consumed.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(bytes) = std::fs::read(self.out_dir.join("list.csv")) else {
            return Vec::new();
        };
        let all = parse_segment_csv(&String::from_utf8_lossy(&bytes));
        let fresh = all
            .iter()
            .enumerate()
            .skip(*consumed)
            .map(|(index, (name, duration))| SegmentInfo {
                path: self.out_dir.join(name),
                duration: *duration,
                index,
            })
            .collect();
        *consumed = all.len();
        fresh
    }

    /// Kills a running ffmpeg; the state becomes Killed immediately and stays so.
    pub fn kill(&self) {
        let moved = self.status.send_if_modified(|s| {
            if s.state != SegmenterState::Running {
                return false;
            }
            s.state = SegmenterState::Killed;
            s.error = None;
            true
        });
        if moved {
            // notify_one stores a permit, so the waiting task sees it even if it is not parked yet.
            self.kill.notify_one();
        }
    }
}

/// Nothing reported yet.
const NOT_YET: u64 = u64::MAX;

/// `out_time_us=` of a progress line, in seconds.
fn parse_out_time(line: &str) -> Option<f64> {
    let us: i64 = line
        .trim()
        .strip_prefix("out_time_us=")?
        .trim()
        .parse()
        .ok()?;
    Some(us.max(0) as f64 / 1_000_000.0)
}

/// Follows ffmpeg's `-progress` report (`key=value` lines) and keeps the latest `speed` and
/// `out_time`.
async fn read_progress<R: AsyncRead + Unpin>(progress: R, cells: Progress) {
    /// A longer line is read in pieces (bounded memory); the report's lines are short.
    const MAX_LINE: u64 = 4096;
    let mut reader = BufReader::new(progress);
    let mut buf = Vec::new();
    // Read as bytes until the end of the pipe: a line that is not UTF-8 (a metadata title) must
    // not stop the reading, or ffmpeg would block on a full pipe.
    loop {
        buf.clear();
        match (&mut reader)
            .take(MAX_LINE)
            .read_until(b'\n', &mut buf)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                if let Some(value) = parse_speed(&line) {
                    cells.speed.store(value.to_bits(), Ordering::Relaxed);
                } else if let Some(value) = parse_out_time(&line) {
                    cells.out_time_s.store(value.to_bits(), Ordering::Relaxed);
                }
            }
        }
    }
}

/// Words of ffmpeg or the system, shown as they are.
fn output(text: impl std::fmt::Display) -> Msg {
    Msg::error("source_message").arg("detail", text)
}

/// Keeps the last [`STDERR_TAIL_LINES`] non-empty trimmed stderr lines until EOF, with
/// addresses cut to their host and first path segment.
async fn read_stderr_tail<R: AsyncRead + Unpin>(stderr: Option<R>) -> VecDeque<String> {
    let mut tail = VecDeque::new();
    let Some(stderr) = stderr else {
        return tail;
    };
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim();
                if !line.is_empty() {
                    if tail.len() == STDERR_TAIL_LINES {
                        tail.pop_front();
                    }
                    // ffmpeg quotes the inputs' signed addresses (the owner's IP address).
                    tail.push_back(crate::redact::redact_urls(line));
                }
            }
        }
    }
    tail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VideoEncoder;
    use crate::pipeline::ffmpeg_args::{ClipArgsOptions, build_clip_args};
    use crate::pipeline::probe::{VideoInfo, VideoMode};
    use crate::resolver::types::StreamInput;
    use std::time::Duration;

    #[tokio::test]
    async fn the_stderr_tail_keeps_no_signed_addresses() {
        let stderr: &[u8] = b"[tls] something\n[https @ 0x1] https://rr7---sn-x.googlevideo.com/videoplayback?ip=188.47.115.50&sig=abc: Server returned 403 Forbidden (access denied)\n";
        let tail: Vec<String> = read_stderr_tail(Some(stderr)).await.into();
        assert_eq!(
            tail,
            [
                "[tls] something",
                "[https @ 0x1] https://rr7---sn-x.googlevideo.com/videoplayback: Server returned 403 Forbidden (access denied)"
            ]
        );
    }

    pub(crate) fn ffmpeg_bin() -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
        ["ffmpeg.exe", "ffmpeg"]
            .iter()
            .map(|name| root.join("bin").join(name))
            .find(|p| p.exists())
    }

    fn info(codec: &str, height: u32, fps: f64, has_audio: bool) -> VideoInfo {
        VideoInfo {
            codec: codec.into(),
            height,
            pix_fmt: "yuv420p".into(),
            fps,
            has_audio,
            duration_s: Some(10.0),
            portrait: false,
        }
    }

    fn input(url: &str) -> StreamInput {
        StreamInput {
            url: url.into(),
            headers: vec![],
        }
    }

    #[test]
    fn csv_reads_name_start_end_rows() {
        let rows =
            parse_segment_csv("seg_00000.ts,0.000000,4.004000\nseg_00001.ts,4.004000,8.000000\n");
        assert_eq!(
            rows,
            vec![
                ("seg_00000.ts".to_string(), 4.004),
                ("seg_00001.ts".to_string(), 3.996)
            ]
        );
    }

    #[test]
    fn csv_skips_the_incomplete_last_line() {
        assert_eq!(parse_segment_csv("seg_00000.ts,0,4\nseg_0000").len(), 1);
        assert_eq!(parse_segment_csv("seg_00000.ts,0,4").len(), 0);
        assert_eq!(parse_segment_csv("").len(), 0);
    }

    #[test]
    fn csv_name_may_contain_commas_and_bad_rows_are_skipped() {
        let rows = parse_segment_csv("a,b.ts,1,3.5\r\nbroken\nx.ts,nan,4\n,1,2\nok.ts,1,2\n");
        assert_eq!(
            rows,
            vec![("a,b.ts".to_string(), 2.5), ("ok.ts".to_string(), 1.0)]
        );
    }

    #[tokio::test]
    async fn spawn_failure_is_failed_with_the_io_message() {
        let dir = tempfile::tempdir().unwrap();
        let seg = ClipSegmenter::start(
            Path::new("definitely-not-a-real-program-xyz"),
            vec![],
            dir.path().to_path_buf(),
            None,
        );
        assert_eq!(seg.state(), SegmenterState::Failed);
        assert!(seg.error().is_some_and(|e| !e.to_string().is_empty()));
        assert_eq!(seg.wait().await, SegmenterState::Failed);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_silent_failure_names_the_exit_code_in_the_readers_language() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = std::env::var_os("ComSpec").map_or_else(
            || PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            PathBuf::from,
        );
        let args = ["/c", "exit 3"].map(OsString::from).to_vec();
        let seg = ClipSegmenter::start(&cmd, args, dir.path().to_path_buf(), None);
        assert_eq!(seg.wait().await, SegmenterState::Failed);
        let error = seg.error().expect("a reason");
        assert_eq!(
            error.render(crate::i18n::Lang::Pl),
            "ffmpeg zakończył się kodem 3"
        );
        assert_eq!(
            error.render(crate::i18n::Lang::En),
            "ffmpeg ended with code 3"
        );
    }

    #[tokio::test]
    async fn kill_sets_killed_and_a_later_exit_does_not_overwrite_it() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        // An endless synthetic source: only kill() can end it.
        let args: Vec<OsString> = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25",
            "-f",
            "null",
            "-",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        // cwd = the directory: on Windows a live process holds it, so removing it only works
        // once ffmpeg is really gone.
        let seg = ClipSegmenter::start(
            &ffmpeg,
            args,
            dir.path().to_path_buf(),
            Some(dir.path().to_path_buf()),
        );
        assert_eq!(seg.state(), SegmenterState::Running);
        tokio::time::sleep(Duration::from_millis(300)).await;
        seg.kill();
        assert_eq!(seg.state(), SegmenterState::Killed);
        assert_eq!(seg.wait().await, SegmenterState::Killed);
        // wait() resolved => the child has exited and been reaped.
        std::fs::remove_dir_all(dir.path()).expect("directory still held by ffmpeg after wait()");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(seg.state(), SegmenterState::Killed);
        assert_eq!(seg.error(), None);
    }

    #[tokio::test]
    async fn pid_and_speed_come_from_the_running_process_and_the_progress_report() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let args: Vec<OsString> = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-progress",
            "pipe:1",
            "-nostats",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25",
            "-f",
            "null",
            "-",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let seg = ClipSegmenter::start(
            &ffmpeg,
            args,
            dir.path().to_path_buf(),
            Some(dir.path().to_path_buf()),
        );
        assert!(seg.pid().is_some_and(|pid| pid > 0));
        assert_eq!(seg.speed(), None, "nothing reported yet");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while seg.speed().is_none() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let speed = seg.speed().expect("a speed from the progress report");
        // Real time input (-re): about 1x.
        assert!((0.3..3.0).contains(&speed), "speed {speed}");
        seg.kill();
        seg.wait().await;
        assert_eq!(seg.pid(), None, "no pid once the process is gone");
    }

    #[tokio::test]
    async fn progress_reading_survives_bytes_that_are_not_utf8() {
        use tokio::io::AsyncWriteExt;
        let (mut tx, rx) = tokio::io::duplex(64);
        let cells = Progress::new();
        let reader = tokio::spawn(read_progress(rx, cells.clone()));
        // A title with a broken encoding, then more reports, more than the pipe holds.
        tx.write_all(b"stream_0_0_q=\xff\xfe\xfd\nspeed=2x\n")
            .await
            .unwrap();
        for _ in 0..200 {
            tx.write_all(b"frame=1\nout_time_us=N/A\nspeed=1.5x\n")
                .await
                .unwrap();
        }
        tx.write_all(b"out_time_ms=999\nout_time_us=12500000\nout_time=00:00:12.500000\n")
            .await
            .unwrap();
        drop(tx);
        reader.await.unwrap();
        assert_eq!(read_cell(&cells.speed), Some(1.5));
        assert_eq!(read_cell(&cells.out_time_s), Some(12.5));
    }

    /// A segmenter that never started a child, to exercise `poll()` on hand-written lists.
    fn poll_only(dir: &Path) -> ClipSegmenter {
        let (status, _) = watch::channel(Snapshot {
            state: SegmenterState::Done,
            error: None,
            exited: true,
            stderr_tail: Vec::new(),
        });
        ClipSegmenter {
            out_dir: dir.to_path_buf(),
            consumed: Mutex::new(0),
            status,
            kill: Arc::new(Notify::new()),
            pid: None,
            progress: Progress::new(),
        }
    }

    #[test]
    fn poll_returns_only_new_complete_rows_with_contiguous_indices() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("list.csv");
        let seg = poll_only(dir.path());
        assert!(seg.poll().is_empty(), "no list.csv yet");

        std::fs::write(
            &list,
            "seg_00000.ts,0.000000,4.000000\nseg_00001.ts,4.000000,8.500000\nseg_00002.ts,8.5",
        )
        .unwrap();
        let first = seg.poll();
        assert_eq!(
            first,
            vec![
                SegmentInfo {
                    path: dir.path().join("seg_00000.ts"),
                    duration: 4.0,
                    index: 0
                },
                SegmentInfo {
                    path: dir.path().join("seg_00001.ts"),
                    duration: 4.5,
                    index: 1
                },
            ]
        );
        assert!(seg.poll().is_empty(), "partial third line is not returned");

        std::fs::write(
            &list,
            "seg_00000.ts,0.000000,4.000000\nseg_00001.ts,4.000000,8.500000\n\
             seg_00002.ts,8.500000,12.000000\nseg_00003.ts,12.000000,13.250000\n",
        )
        .unwrap();
        let next = seg.poll();
        assert_eq!(next.iter().map(|s| s.index).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(next[0].path, dir.path().join("seg_00002.ts"));
        assert_eq!(next[0].duration, 3.5);
        assert_eq!(next[1].duration, 1.25);
        assert!(seg.poll().is_empty());
    }

    #[test]
    fn concurrent_polls_never_return_a_segment_twice() {
        let dir = tempfile::tempdir().unwrap();
        let rows: String = (0..200)
            .map(|i| format!("seg_{i:05}.ts,{i}.000000,{}.000000\n", i + 1))
            .collect();
        std::fs::write(dir.path().join("list.csv"), rows).unwrap();
        let seg = Arc::new(poll_only(dir.path()));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let seg = Arc::clone(&seg);
                std::thread::spawn(move || {
                    (0..20)
                        .flat_map(|_| seg.poll())
                        .map(|s| s.index)
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut all: Vec<usize> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        all.sort_unstable();
        assert_eq!(all, (0..200).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn cuts_10s_test_pattern_into_segments_and_ends_done() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let args = build_clip_args(&ClipArgsOptions {
            inputs: vec![input("testsrc2=size=640x360:rate=25:duration=10")],
            input_format: Some("lavfi".into()),
            mode: VideoMode::Transcode,
            info: info("rawvideo", 360, 25.0, false),
            encoder: VideoEncoder::Libx264,
            segment_seconds: 4,
            out_dir: dir.path().to_path_buf(),
            start_at_s: 0.0,
            readrate: None,
            overlay: None,
        });
        let seg = ClipSegmenter::start(&ffmpeg, args, dir.path().to_path_buf(), None);
        assert_eq!(seg.wait().await, SegmenterState::Done);
        let segments = seg.poll();
        assert!(segments.len() >= 2, "{segments:?}");
        assert_eq!(segments[0].index, 0);
        assert!(segments[0].path.exists());
        let total: f64 = segments.iter().map(|s| s.duration).sum();
        assert!(total > 9.5, "{total}");
        assert!(seg.poll().is_empty());
    }

    #[tokio::test]
    async fn bad_input_ends_failed_with_a_message() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nie-ma.mp4");
        let args = build_clip_args(&ClipArgsOptions {
            inputs: vec![input(&missing.to_string_lossy())],
            input_format: None,
            mode: VideoMode::Copy,
            info: info("h264", 360, 25.0, true),
            encoder: VideoEncoder::Libx264,
            segment_seconds: 4,
            out_dir: dir.path().to_path_buf(),
            start_at_s: 0.0,
            readrate: None,
            overlay: None,
        });
        let seg = ClipSegmenter::start(&ffmpeg, args, dir.path().to_path_buf(), None);
        assert_eq!(seg.wait().await, SegmenterState::Failed);
        assert!(seg.error().is_some_and(|e| !e.to_string().is_empty()));
    }

    /// Review Focus 1: a non-ASCII file name inside a directory whose name contains a space.
    #[tokio::test]
    async fn cuts_a_file_with_polish_name_in_a_directory_with_a_space() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let src_dir = root.path().join("katalog ze spacją");
        std::fs::create_dir_all(&src_dir).unwrap();
        let source = src_dir.join("Żółć 1.mkv");
        let made = process::run_process(
            &ffmpeg,
            &[
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=25:duration=6",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ]
            .iter()
            .map(OsString::from)
            .chain([source.clone().into_os_string()])
            .collect::<Vec<_>>(),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert_eq!(made.code, Some(0), "{}", made.stderr);
        assert!(source.exists());

        let out_dir = src_dir.join("wyjście 1");
        let args = build_clip_args(&ClipArgsOptions {
            inputs: vec![input(&source.to_string_lossy())],
            input_format: None,
            mode: VideoMode::Transcode,
            info: info("h264", 240, 25.0, false),
            encoder: VideoEncoder::Libx264,
            segment_seconds: 4,
            out_dir: out_dir.clone(),
            start_at_s: 0.0,
            readrate: None,
            overlay: None,
        });
        let seg = ClipSegmenter::start(&ffmpeg, args, out_dir, None);
        let state = seg.wait().await;
        assert_eq!(state, SegmenterState::Done, "{:?}", seg.error());
        let segments = seg.poll();
        assert!(!segments.is_empty());
        assert!(segments[0].path.exists());
    }
}
