//! The real clip factory: resolve, probe and start an ffmpeg cut. Port of
//! `src/pipeline/clipFactory.ts`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rusqlite::Connection;

use crate::config::VideoEncoder;
use crate::db::{QueueItem, SettingsRepo};
use crate::pipeline::channel::{BoxFuture, ClipFactory, ClipHandle};
use crate::pipeline::ffmpeg_args::{ClipArgsOptions, build_clip_args};
use crate::pipeline::probe::{VideoInfo, VideoMode, decide_video_mode, probe_video};
use crate::pipeline::segmenter::{ClipSegmenter, SegmentInfo, SegmenterState};
use crate::resolver::MediaResolver;

/// Retries when removing a clip directory (Windows may hold a file lock for a moment).
const REMOVE_RETRIES: u32 = 3;
const REMOVE_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Source read rate: 1.5× real time leaves headroom without downloading the whole film.
pub const READRATE: f64 = 1.5;

pub type SharedLogFn = Arc<dyn Fn(&str) + Send + Sync>;

pub struct ClipFactoryDeps {
    pub resolver: Arc<dyn MediaResolver>,
    pub ffmpeg_path: PathBuf,
    pub ffprobe_path: PathBuf,
    /// Read when each clip starts: the desktop app may change it while the channel runs.
    pub encoder: crate::live::LiveValue<VideoEncoder>,
    pub segment_seconds: u32,
    pub clips_dir: PathBuf,
    pub db: Arc<Mutex<Connection>>,
    pub channel_id: i64,
    pub log: Option<SharedLogFn>,
}

/// Log line written when a clip starts.
pub fn clip_start_line(
    item: &QueueItem,
    mode: VideoMode,
    info: &VideoInfo,
    start_at_s: f64,
) -> String {
    let action = match mode {
        VideoMode::Copy => "przepakowanie",
        VideoMode::Transcode => "konwersja",
    };
    let silent = if info.has_audio {
        ""
    } else {
        ", bez dźwięku"
    };
    let resume = if start_at_s > 0.0 {
        // Math.round
        format!(", wznowienie od {} s", (start_at_s + 0.5).floor() as i64)
    } else {
        String::new()
    };
    format!(
        "Film #{} \"{}\": {action} ({} {}p {} kl./s{silent}){resume}",
        item.id, item.title, info.codec, info.height, info.fps
    )
}

/// Removes a clip directory, best effort with a few retries. A leftover directory is cleared at
/// the next server start.
async fn remove_dir(dir: &Path) {
    for attempt in 0..=REMOVE_RETRIES {
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) if attempt < REMOVE_RETRIES => {
                tokio::time::sleep(REMOVE_RETRY_DELAY * (attempt + 1)).await;
            }
            Err(_) => return,
        }
    }
}

/// A running ffmpeg cut of one clip.
struct RealClipHandle {
    seg: Arc<ClipSegmenter>,
    out_dir: PathBuf,
    duration_s: Option<f64>,
    disposed: bool,
}

impl ClipHandle for RealClipHandle {
    fn state(&self) -> SegmenterState {
        self.seg.state()
    }

    fn error(&self) -> Option<String> {
        self.seg.error()
    }

    fn duration_s(&self) -> Option<f64> {
        self.duration_s
    }

    fn poll(&mut self) -> Vec<SegmentInfo> {
        self.seg.poll()
    }

    fn kill(&mut self) {
        self.seg.kill();
    }

    /// Non-blocking: kills ffmpeg, and a background task removes the directory once the process
    /// has really exited (Windows keeps open files locked until then).
    fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        self.seg.kill();
        let seg = Arc::clone(&self.seg);
        let dir = self.out_dir.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move {
                    seg.wait().await;
                    remove_dir(&dir).await;
                });
            }
            Err(_) => {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

impl Drop for RealClipHandle {
    /// A handle dropped without `dispose()` (e.g. a restarted channel) must not leave ffmpeg
    /// running or its files behind.
    fn drop(&mut self) {
        self.dispose();
    }
}

pub struct RealClipFactory {
    deps: Arc<ClipFactoryDeps>,
    counter: Arc<AtomicU64>,
}

pub fn create_clip_factory(deps: ClipFactoryDeps) -> RealClipFactory {
    RealClipFactory {
        deps: Arc::new(deps),
        counter: Arc::new(AtomicU64::new(0)),
    }
}

async fn start_clip(
    d: Arc<ClipFactoryDeps>,
    counter: Arc<AtomicU64>,
    item: QueueItem,
    start_at_s: f64,
    attempt: u32,
) -> Result<Box<dyn ClipHandle>, String> {
    let stream = d
        .resolver
        .resolve(&item.source_url)
        .await
        .map_err(|e| e.to_string())?;
    let info = probe_video(&d.ffprobe_path, &stream.inputs)
        .await
        .map_err(|e| e.to_string())?;
    let force_transcode = {
        let conn = d.db.lock().unwrap_or_else(PoisonError::into_inner);
        SettingsRepo::new(&conn)
            .get(d.channel_id)
            .map_err(|e| e.to_string())?
            .force_transcode
    };
    let mode = decide_video_mode(&info, force_transcode);
    let live = item.is_live || stream.is_live;
    let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
    let out_dir = d.clips_dir.join(format!("{}-{attempt}-{n}", item.id));
    if let Some(log) = &d.log {
        log(&clip_start_line(&item, mode, &info, start_at_s));
    }
    let args = build_clip_args(&ClipArgsOptions {
        inputs: stream.inputs,
        input_format: None,
        mode,
        info: info.clone(),
        encoder: d.encoder.get(),
        segment_seconds: d.segment_seconds,
        out_dir: out_dir.clone(),
        start_at_s: if live { 0.0 } else { start_at_s },
        readrate: if live { None } else { Some(READRATE) },
    });
    let seg = ClipSegmenter::start(&d.ffmpeg_path, args, out_dir.clone(), None);
    Ok(Box::new(RealClipHandle {
        seg: Arc::new(seg),
        out_dir,
        duration_s: info.duration_s.or(stream.duration_s),
        disposed: false,
    }))
}

impl ClipFactory for RealClipFactory {
    fn create(
        &self,
        item: QueueItem,
        start_at_s: f64,
        attempt: u32,
    ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, String>> {
        Box::pin(start_clip(
            Arc::clone(&self.deps),
            Arc::clone(&self.counter),
            item,
            start_at_s,
            attempt,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};
    use crate::pipeline::segmenter::SegmenterState;
    use crate::resolver::{MediaMeta, ResolveError, ResolvedStream, StreamInput};
    use async_trait::async_trait;
    use std::ffi::OsString;
    use std::time::{Duration, Instant};

    fn item(id: i64, title: &str) -> QueueItem {
        QueueItem {
            id,
            channel_id: MAIN_CHANNEL_ID,
            position: 1,
            source_url: "https://youtu.be/x".into(),
            title: title.into(),
            duration_s: Some(60.0),
            is_live: false,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
        }
    }

    fn video(codec: &str, height: u32, fps: f64, has_audio: bool) -> VideoInfo {
        VideoInfo {
            codec: codec.into(),
            height,
            pix_fmt: "yuv420p".into(),
            fps,
            has_audio,
            duration_s: Some(60.0),
        }
    }

    #[test]
    fn log_line_for_a_copied_film_from_the_start() {
        assert_eq!(
            clip_start_line(
                &item(7, "Kot"),
                VideoMode::Copy,
                &video("h264", 720, 25.0, true),
                0.0
            ),
            "Film #7 \"Kot\": przepakowanie (h264 720p 25 kl./s)"
        );
    }

    #[test]
    fn log_line_for_a_transcoded_silent_film_resumed_midway() {
        assert_eq!(
            clip_start_line(
                &item(3, "Pies"),
                VideoMode::Transcode,
                &video("vp9", 1080, 29.97, false),
                12.5
            ),
            "Film #3 \"Pies\": konwersja (vp9 1080p 29.97 kl./s, bez dźwięku), wznowienie od 13 s"
        );
    }

    fn bin(name: &str) -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
        [format!("{name}.exe"), name.to_string()]
            .iter()
            .map(|n| root.join("bin").join(n))
            .find(|p| p.exists())
    }

    /// Resolves every URL to the given local file, or fails with a fixed message.
    struct FileResolver(Option<PathBuf>);

    #[async_trait]
    impl MediaResolver for FileResolver {
        async fn fetch_meta(&self, _url: &str) -> Result<MediaMeta, ResolveError> {
            Err(ResolveError::new("nieużywane"))
        }
        async fn resolve(&self, _url: &str) -> Result<ResolvedStream, ResolveError> {
            let Some(file) = &self.0 else {
                return Err(ResolveError::new("Nie udało się rozpoznać źródła."));
            };
            Ok(ResolvedStream {
                inputs: vec![StreamInput {
                    url: file.to_string_lossy().into_owned(),
                    headers: vec![],
                }],
                title: "plik".into(),
                duration_s: Some(6.0),
                is_live: false,
            })
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        clips: PathBuf,
        factory: RealClipFactory,
        item: QueueItem,
        lines: Arc<Mutex<Vec<String>>>,
    }

    fn fixture(
        source: Option<PathBuf>,
        dir: tempfile::TempDir,
        ffmpeg: PathBuf,
        ffprobe: PathBuf,
    ) -> Fixture {
        let conn = open_database(Path::new(":memory:")).unwrap();
        let item = QueueRepo::new(&conn)
            .add(&NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url: "https://youtu.be/x".into(),
                title: "Test".into(),
                duration_s: Some(6.0),
                is_live: false,
                status: ItemStatus::Queued,
                added_by: AddedBy::Admin,
                requester_name: None,
                requester_ip: None,
            })
            .unwrap();
        let clips = dir.path().join("clips");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let factory = create_clip_factory(ClipFactoryDeps {
            resolver: Arc::new(FileResolver(source)),
            ffmpeg_path: ffmpeg,
            ffprobe_path: ffprobe,
            encoder: crate::live::LiveValue::new(VideoEncoder::Libx264),
            segment_seconds: 4,
            clips_dir: clips.clone(),
            db: Arc::new(Mutex::new(conn)),
            channel_id: MAIN_CHANNEL_ID,
            log: Some(Arc::new(move |l: &str| {
                sink.lock().unwrap().push(l.to_string())
            })),
        });
        Fixture {
            _dir: dir,
            clips,
            factory,
            item,
            lines,
        }
    }

    #[tokio::test]
    async fn a_resolve_error_is_returned_as_its_message() {
        let dir = tempfile::tempdir().unwrap();
        let f = fixture(None, dir, "ffmpeg".into(), "ffprobe".into());
        let err = f
            .factory
            .create(f.item.clone(), 0.0, 0)
            .await
            .err()
            .unwrap();
        assert_eq!(err, "Nie udało się rozpoznać źródła.");
    }

    async fn wait_until(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        cond()
    }

    async fn make_source(ffmpeg: &Path, file: &Path, seconds: u32) {
        let args: Vec<OsString> = [
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size=320x240:rate=25:duration={seconds}"),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "50",
        ]
        .iter()
        .map(OsString::from)
        .chain([file.as_os_str().to_owned()])
        .collect();
        let made = crate::process::run_process(ffmpeg, &args, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(made.code, Some(0), "{}", made.stderr);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cuts_a_local_file_and_dispose_removes_the_clip_directory() {
        let (Some(ffmpeg), Some(ffprobe)) = (bin("ffmpeg"), bin("ffprobe")) else {
            eprintln!("skip: bin/ffmpeg or bin/ffprobe missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("źródło 1.mp4");
        make_source(&ffmpeg, &source, 6).await;
        let f = fixture(Some(source), dir, ffmpeg, ffprobe);

        let mut handle = f.factory.create(f.item.clone(), 0.0, 0).await.unwrap();
        let out_dir = f.clips.join(format!("{}-0-1", f.item.id));
        assert!(out_dir.exists(), "{out_dir:?}");
        assert_eq!(handle.duration_s().map(f64::round), Some(6.0));
        assert_eq!(
            *f.lines.lock().unwrap(),
            [format!(
                "Film #{} \"Test\": przepakowanie (h264 240p 25 kl./s, bez dźwięku)",
                f.item.id
            )]
        );

        let mut segments = Vec::new();
        let finished = wait_until(
            || {
                segments.extend(handle.poll());
                handle.state() != SegmenterState::Running
            },
            Duration::from_secs(60),
        )
        .await;
        assert!(finished);
        segments.extend(handle.poll());
        assert_eq!(handle.state(), SegmenterState::Done, "{:?}", handle.error());
        assert!(!segments.is_empty());
        assert!(segments[0].path.starts_with(&out_dir));

        handle.dispose();
        assert!(wait_until(|| !out_dir.exists(), Duration::from_secs(10)).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn disposing_a_running_resumed_clip_kills_it_and_removes_the_directory() {
        let (Some(ffmpeg), Some(ffprobe)) = (bin("ffmpeg"), bin("ffprobe")) else {
            eprintln!("skip: bin/ffmpeg or bin/ffprobe missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("long.mp4");
        make_source(&ffmpeg, &source, 40).await;
        let f = fixture(Some(source), dir, ffmpeg, ffprobe);

        // Second clip of this factory: the counter in the directory name goes up.
        let first = f.factory.create(f.item.clone(), 0.0, 0).await.unwrap();
        drop(first);
        let mut handle = f.factory.create(f.item.clone(), 4.0, 1).await.unwrap();
        let out_dir = f.clips.join(format!("{}-1-2", f.item.id));
        assert!(out_dir.exists(), "{out_dir:?}");
        assert!(f.lines.lock().unwrap()[1].ends_with(", wznowienie od 4 s"));
        // readrate 1.5 on a 40 s source: still running after a moment.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(handle.state(), SegmenterState::Running);
        handle.dispose();
        assert_eq!(handle.state(), SegmenterState::Killed);
        assert!(wait_until(|| !out_dir.exists(), Duration::from_secs(10)).await);
        // Dropping an undisposed handle also cleans up.
        assert!(
            wait_until(
                || !f.clips.join(format!("{}-0-1", f.item.id)).exists(),
                Duration::from_secs(10)
            )
            .await
        );
    }
}
