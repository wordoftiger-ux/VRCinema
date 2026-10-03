//! One ffmpeg process cutting a single clip into HLS segments. Port of `src/pipeline/segmenter.ts`.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{Notify, watch};

use crate::process;

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

impl SegmenterState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Killed)
    }
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
    error: Option<String>,
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
        });
        let segmenter = ClipSegmenter {
            out_dir,
            consumed: Mutex::new(0),
            status,
            kill: Arc::new(Notify::new()),
        };
        if let Err(e) = std::fs::create_dir_all(&segmenter.out_dir) {
            segmenter.finish(SegmenterState::Failed, Some(e.to_string()));
            return segmenter;
        }

        let mut command = process::command(ffmpeg);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(cwd) = &cwd {
            command.current_dir(cwd);
        }
        let mut child = match process::spawn(&mut command) {
            Ok(child) => child,
            Err(e) => {
                segmenter.finish(SegmenterState::Failed, Some(e.to_string()));
                return segmenter;
            }
        };

        let stderr = child.stderr.take();
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
            let (state, error) = match exit {
                Ok(code) if code.success() => (SegmenterState::Done, None),
                Ok(code) => {
                    let error = tail.back().cloned().unwrap_or_else(|| {
                        let code = code.code().map_or_else(|| "?".into(), |c| c.to_string());
                        format!("ffmpeg zakończył się kodem {code}")
                    });
                    (SegmenterState::Failed, Some(error))
                }
                Err(e) => (SegmenterState::Failed, Some(e.to_string())),
            };
            // A kill() that already moved the state to Killed wins over the exit status.
            status.send_if_modified(|s| {
                if s.state != SegmenterState::Running {
                    return false;
                }
                s.state = state;
                s.error = error;
                true
            });
        });
        segmenter
    }

    fn finish(&self, state: SegmenterState, error: Option<String>) {
        self.status.send_if_modified(|s| {
            if s.state != SegmenterState::Running {
                return false;
            }
            s.state = state;
            s.error = error;
            true
        });
    }

    pub fn state(&self) -> SegmenterState {
        self.status.borrow().state
    }

    pub fn error(&self) -> Option<String> {
        self.status.borrow().error.clone()
    }

    /// Resolves with the terminal state (Done, Failed or Killed).
    pub async fn wait(&self) -> SegmenterState {
        let mut rx = self.status.subscribe();
        loop {
            let state = rx.borrow_and_update().state;
            if state.is_terminal() {
                return state;
            }
            if rx.changed().await.is_err() {
                return self.state();
            }
        }
    }

    /// Segments that appeared in `list.csv` since the previous call.
    pub fn poll(&self) -> Vec<SegmentInfo> {
        let Ok(bytes) = std::fs::read(self.out_dir.join("list.csv")) else {
            return Vec::new();
        };
        let all = parse_segment_csv(&String::from_utf8_lossy(&bytes));
        let mut consumed = self.consumed.lock().unwrap_or_else(|e| e.into_inner());
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

/// Keeps the last [`STDERR_TAIL_LINES`] non-empty trimmed stderr lines until EOF.
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
                    tail.push_back(line.to_string());
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
        assert!(seg.error().is_some_and(|e| !e.is_empty()));
        assert_eq!(seg.wait().await, SegmenterState::Failed);
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
        let seg = ClipSegmenter::start(&ffmpeg, args, dir.path().to_path_buf(), None);
        assert_eq!(seg.state(), SegmenterState::Running);
        tokio::time::sleep(Duration::from_millis(300)).await;
        seg.kill();
        assert_eq!(seg.state(), SegmenterState::Killed);
        assert_eq!(seg.wait().await, SegmenterState::Killed);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(seg.state(), SegmenterState::Killed);
        assert_eq!(seg.error(), None);
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
        });
        let seg = ClipSegmenter::start(&ffmpeg, args, dir.path().to_path_buf(), None);
        assert_eq!(seg.wait().await, SegmenterState::Failed);
        assert!(seg.error().is_some_and(|e| !e.is_empty()));
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
        });
        let seg = ClipSegmenter::start(&ffmpeg, args, out_dir, None);
        let state = seg.wait().await;
        assert_eq!(state, SegmenterState::Done, "{:?}", seg.error());
        let segments = seg.poll();
        assert!(!segments.is_empty());
        assert!(segments[0].path.exists());
    }
}
