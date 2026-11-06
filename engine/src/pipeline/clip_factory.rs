//! The real clip factory: resolve, probe and start an ffmpeg cut. Port of
//! `src/pipeline/clipFactory.ts`, extended with the info ticker (stage 4).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rusqlite::Connection;

use crate::config::VideoEncoder;
use crate::db::{AddedBy, ChannelSettings, ItemStatus, QueueItem, QueueRepo, SettingsRepo};
use crate::i18n::{Lang, Msg};
use crate::live::LiveValue;
use crate::pipeline::channel::{BoxFuture, ClipFactory, ClipHandle};
use crate::pipeline::ffmpeg_args::{ClipArgsOptions, MAX_HEIGHT, ReadRate, build_clip_args};
use crate::pipeline::probe::{ProbeError, VideoInfo, VideoMode, decide_video_mode, probe_video};
use crate::pipeline::segmenter::{ClipSegmenter, SegmentInfo, SegmenterState};
use crate::pipeline::slates::{FontChoice, font_for, qr_image, request_url, system_fonts_dir};
use crate::pipeline::temp_downloads::{Lease, TempDownloads, is_tiktok};
use crate::pipeline::ticker::{
    CAPTION_FILE, FONTS_DIR, OverlayContext, QR_FILE, QR_PX, QrOverlay, Schedule, SlowWatch,
    TEXT_FILE, TickerContent, TickerMode, TickerQr, TickerWriter, compose_text, overlay_filter,
    prepare_after_s, window_budget_px,
};
use crate::resolver::ytdlp::{FALLBACK_YOUTUBE_CLIENT, youtube_client};
use crate::resolver::{MediaResolver, ResolvedStream, StreamInput};
use crate::rules::{HostResolver, LinkError, check_media_url, check_media_urls};
use crate::stats::{ClipStats, ProbeSummary, StreamMode};

/// Retries when removing a clip directory (Windows may hold a file lock for a moment).
const REMOVE_RETRIES: u32 = 3;
const REMOVE_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Source read rate: 1.5× real time leaves headroom without downloading the whole film.
pub const READRATE: f64 = 1.5;
/// Source read rate of a clip with the info ticker: real time, so ffmpeg (and the text it
/// draws) stays only the initial burst (20 s) ahead of the stream; at 1.5× it would draw the
/// text minutes before it airs and finish a long film with its last part's text frozen.
pub const TICKER_READRATE: f64 = 1.0;

/// How often a running ticker re-reads what it shows (and ffmpeg's progress).
const TICKER_REFRESH: Duration = Duration::from_millis(500);

pub type SharedLogFn = Arc<dyn Fn(Msg) + Send + Sync>;

pub struct ClipFactoryDeps {
    pub resolver: Arc<dyn MediaResolver>,
    pub ffmpeg_path: PathBuf,
    pub ffprobe_path: PathBuf,
    /// Read when each clip starts: the desktop app may change it while the channel runs.
    pub encoder: LiveValue<VideoEncoder>,
    pub segment_seconds: u32,
    pub clips_dir: PathBuf,
    /// Where yt-dlp downloads the films that cannot be read from their address (TikTok).
    pub temp_dir: PathBuf,
    pub db: Arc<Mutex<Connection>>,
    pub channel_id: i64,
    pub log: Option<SharedLogFn>,
    /// The info ticker's Latin font (`FONT_FILE`, like the slates).
    pub font_file: PathBuf,
    /// The public address (the request page in the ticker and its QR code); read while the
    /// ticker runs.
    pub public_url: LiveValue<String>,
    /// Looks up the host of the media a viewer's link led to (the home-network protection).
    pub host_resolver: Arc<dyn HostResolver>,
    /// Whether the media of a viewer's link is checked (always, but with the fake resolver of
    /// the black-box tests, whose media are local files).
    pub check_viewer_media: bool,
}

/// Log entry written when a clip starts.
pub fn clip_start_line(
    item: &QueueItem,
    mode: VideoMode,
    info: &VideoInfo,
    start_at_s: f64,
) -> Msg {
    let action = match mode {
        VideoMode::Copy => Msg::new("log.clip_action_copy"),
        VideoMode::Transcode => Msg::new("log.clip_action_transcode"),
    };
    let silent = if info.has_audio {
        Msg::text("")
    } else {
        Msg::new("log.clip_no_audio")
    };
    let resume = if start_at_s > 0.0 {
        // Math.round
        Msg::new("log.clip_resume").arg("seconds", (start_at_s + 0.5).floor() as i64)
    } else {
        Msg::text("")
    };
    Msg::new("log.clip_start")
        .arg("id", item.id)
        .arg("title", &item.title)
        .arg_msg("action", action)
        .arg("codec", &info.codec)
        .arg("height", info.height)
        .arg("fps", info.fps)
        .arg_msg("silent", silent)
        .arg_msg("resume", resume)
}

/// Copy or convert: the ticker is drawn on a converted picture, so it forces the conversion
/// (like `forceTranscode`).
pub fn clip_video_mode(info: &VideoInfo, settings: &ChannelSettings) -> VideoMode {
    decide_video_mode(info, settings.force_transcode || settings.ticker.enabled)
}

/// How fast the source is read: a live stream as it comes, any clip without the ticker at
/// [`READRATE`]; a clip with the ticker at real time ([`TICKER_READRATE`]), catching up at
/// [`READRATE`] after a stall (ffmpeg's own catch-up, 1.05×, would take minutes to win back
/// a few seconds), and without the initial burst when it is the next film prepared
/// `ahead` of its turn (the prefetch window is its cushion; with the burst too, its text
/// would be drawn about 80 s before it airs).
pub fn clip_readrate(live: bool, ticker: bool, ahead: bool) -> Option<ReadRate> {
    match (live, ticker) {
        (true, _) => None,
        (false, true) => Some(ReadRate {
            rate: TICKER_READRATE,
            initial_burst_s: if ahead {
                0.0
            } else {
                crate::pipeline::ffmpeg_args::INITIAL_BURST_S
            },
            catchup: Some(READRATE),
        }),
        (false, false) => Some(ReadRate::new(READRATE)),
    }
}

/// Marks in ffmpeg's error output of the ticker's filters (ffmpeg names a filter of a graph
/// `Parsed_<filter>_<n>`) and of the files they read: only such a failure starts the clip
/// again without the ticker. Plain words ("movie", "font") would also match a film's file name.
const TICKER_ERROR_MARKS: [&str; 8] = [
    "[parsed_drawtext_",
    "[parsed_drawbox_",
    "[parsed_movie_",
    "[parsed_overlay_",
    "[parsed_color_",
    "'ticker.txt'",
    "'qr.txt'",
    "'qr.png'",
];

/// ffmpeg's last error lines say the ticker's filters failed (not the source or the encoder).
pub fn ticker_failure(stderr_tail: &[String]) -> bool {
    stderr_tail.iter().any(|line| {
        let line = line.to_lowercase();
        TICKER_ERROR_MARKS.iter().any(|mark| line.contains(mark))
    })
}

/// How long ffmpeg cuts a clip started at `start_at_s`: up to the film's out point (`None`: to
/// the end, also for a live stream or a start at or past the out point).
pub fn out_point_limit(item: &QueueItem, live: bool, start_at_s: f64) -> Option<f64> {
    let out = item.out_s.filter(|_| !live)?;
    (out > start_at_s).then_some(out - start_at_s)
}

/// Length of the film for the progress line: none for a live stream.
pub fn progress_duration(live: bool, duration_s: Option<f64>) -> Option<f64> {
    if live { None } else { duration_s }
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

/// Removes `dir` once `seg`'s ffmpeg has really exited (Windows keeps open files locked).
fn remove_after_exit(seg: Arc<ClipSegmenter>, dir: PathBuf) {
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

/// The ticker of a running clip: the task keeping its text up to date and its speed warning.
struct TickerRun {
    task: tokio::task::AbortHandle,
    slow: Arc<AtomicBool>,
    /// The request address whose QR code this ticker draws (`""`: no code).
    code: String,
}

/// The same clip without the ticker, started when ffmpeg cannot draw it (e.g. a broken font)
/// before the first segment: no attempt of the film is used up.
struct PlainRestart {
    args: Vec<OsString>,
    out_dir: PathBuf,
    mode: VideoMode,
    encoder: VideoEncoder,
}

/// A running ffmpeg cut of one clip.
struct RealClipHandle {
    seg: Arc<ClipSegmenter>,
    out_dir: PathBuf,
    ffmpeg_path: PathBuf,
    duration_s: Option<f64>,
    copies_video: bool,
    probe: ProbeSummary,
    mode: StreamMode,
    /// `None` when the video is copied.
    encoder: Option<VideoEncoder>,
    ticker: Option<TickerRun>,
    fallback: Option<PlainRestart>,
    /// A segment came out (after that a failure is a normal failure, not the ticker's).
    produced: bool,
    item_id: i64,
    title: String,
    log: Option<SharedLogFn>,
    disposed: bool,
    /// The downloaded film this clip plays (removed a while after its last clip is gone).
    _download: Option<Lease>,
}

impl RealClipHandle {
    /// ffmpeg failed before its first segment because of the ticker: it is started again
    /// without. Any other failure (the source, the encoder) is reported as usual.
    fn restart_pending(&self) -> bool {
        self.fallback.is_some()
            && !self.produced
            && self.seg.state() == SegmenterState::Failed
            && ticker_failure(&self.seg.stderr_tail())
    }

    fn stop_ticker(&mut self) {
        if let Some(ticker) = self.ticker.take() {
            ticker.task.abort();
            ticker.slow.store(false, Ordering::Relaxed);
        }
    }

    fn restart_plain(&mut self) {
        let Some(plain) = self.fallback.take() else {
            return;
        };
        let error = self
            .seg
            .error()
            .unwrap_or_else(|| Msg::new("item_errors.ffmpeg_stopped"));
        self.stop_ticker();
        if let Some(log) = &self.log {
            log(Msg::new("log.ticker_failed_plain")
                .arg("id", self.item_id)
                .arg("title", &self.title)
                .arg_msg("error", error));
        }
        remove_after_exit(Arc::clone(&self.seg), self.out_dir.clone());
        self.seg = Arc::new(ClipSegmenter::start(
            &self.ffmpeg_path,
            plain.args,
            plain.out_dir.clone(),
            None,
        ));
        self.out_dir = plain.out_dir;
        self.copies_video = plain.mode == VideoMode::Copy;
        self.mode = stream_mode(plain.mode);
        self.encoder = (plain.mode == VideoMode::Transcode).then_some(plain.encoder);
    }
}

fn stream_mode(mode: VideoMode) -> StreamMode {
    match mode {
        VideoMode::Copy => StreamMode::Copy,
        VideoMode::Transcode => StreamMode::Transcode,
    }
}

impl ClipHandle for RealClipHandle {
    fn state(&self) -> SegmenterState {
        if self.restart_pending() {
            return SegmenterState::Running;
        }
        self.seg.state()
    }

    fn error(&self) -> Option<Msg> {
        if self.restart_pending() {
            return None;
        }
        self.seg.error()
    }

    fn duration_s(&self) -> Option<f64> {
        self.duration_s
    }

    fn copies_video(&self) -> bool {
        self.copies_video
    }

    fn poll(&mut self) -> Vec<SegmentInfo> {
        let fresh = self.seg.poll();
        if !fresh.is_empty() {
            self.produced = true;
            self.fallback = None;
        }
        if self.restart_pending() {
            self.restart_plain();
        }
        fresh
    }

    fn kill(&mut self) {
        self.seg.kill();
    }

    fn stats(&self) -> ClipStats {
        ClipStats {
            probe: Some(self.probe.clone()),
            mode: Some(self.mode),
            encoder: self.encoder.map(|e| e.as_str().to_string()),
            pid: self.seg.pid(),
            speed: self.seg.speed(),
            ticker_slow: self.ticker_slow(),
        }
    }

    fn ticker_slow(&self) -> bool {
        self.ticker
            .as_ref()
            .is_some_and(|t| t.slow.load(Ordering::Relaxed))
    }

    fn ticker_code(&self) -> Option<String> {
        self.ticker.as_ref().map(|t| t.code.clone())
    }

    /// Non-blocking: kills ffmpeg, and a background task removes the directory once the process
    /// has really exited (Windows keeps open files locked until then).
    fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        self.stop_ticker();
        self.fallback = None;
        self.seg.kill();
        remove_after_exit(Arc::clone(&self.seg), self.out_dir.clone());
    }
}

impl Drop for RealClipHandle {
    /// A handle dropped without `dispose()` (e.g. a restarted channel) must not leave ffmpeg
    /// running or its files behind.
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Analyses a source before its clip starts; injectable so tests need no real ffprobe.
#[async_trait::async_trait]
pub trait VideoProber: Send + Sync {
    async fn probe(&self, ffprobe: &Path, inputs: &[StreamInput]) -> Result<VideoInfo, ProbeError>;
}

/// The real thing: [`probe_video`].
pub struct FfprobeProber;

#[async_trait::async_trait]
impl VideoProber for FfprobeProber {
    async fn probe(&self, ffprobe: &Path, inputs: &[StreamInput]) -> Result<VideoInfo, ProbeError> {
        probe_video(ffprobe, inputs).await
    }
}

/// Cheap to clone: every field is shared.
#[derive(Clone)]
pub struct RealClipFactory {
    deps: Arc<ClipFactoryDeps>,
    counter: Arc<AtomicU64>,
    /// Which of the owner's messages the ticker shows next (interval mode), across clips.
    rotation: Arc<AtomicUsize>,
    prober: Arc<dyn VideoProber>,
    downloads: TempDownloads,
}

pub fn create_clip_factory(deps: ClipFactoryDeps) -> RealClipFactory {
    let downloads = TempDownloads::new(deps.temp_dir.clone());
    RealClipFactory {
        deps: Arc::new(deps),
        counter: Arc::new(AtomicU64::new(0)),
        rotation: Arc::new(AtomicUsize::new(0)),
        prober: Arc::new(FfprobeProber),
        downloads,
    }
}

impl RealClipFactory {
    /// Tests: probes with `prober` instead of ffprobe.
    pub fn with_prober(mut self, prober: Arc<dyn VideoProber>) -> Self {
        self.prober = prober;
        self
    }

    /// Shares the downloaded films with the window's Preview ([`crate::preview_source`]): a
    /// film downloaded for one is played by the other (one download per film).
    pub fn with_downloads(mut self, downloads: TempDownloads) -> Self {
        self.downloads = downloads;
        self
    }
}

/// A viewer's link: what it led to must not be the owner's network, and the programs get exactly
/// the address that was checked (not the text the extractor wrote, which they could read
/// differently), before ffprobe or ffmpeg touch it.
async fn checked_inputs(
    d: &ClipFactoryDeps,
    viewer: bool,
    mut stream: ResolvedStream,
) -> Result<ResolvedStream, Msg> {
    if viewer {
        let raw: Vec<&str> = stream.inputs.iter().map(|i| i.url.as_str()).collect();
        let checked = check_media_urls(&raw, d.host_resolver.as_ref())
            .await
            .map_err(|e| Msg::error(e.code()))?;
        for (input, url) in stream.inputs.iter_mut().zip(checked) {
            input.url = url;
        }
    }
    Ok(stream)
}

/// The yt-dlp client of a YouTube stream (its first `googlevideo.com` input), `None` for
/// any other stream.
fn stream_youtube_client(stream: &ResolvedStream) -> Option<String> {
    stream.inputs.iter().find_map(|i| youtube_client(&i.url))
}

/// One line per YouTube resolution: which yt-dlp client made the address and whether yt-dlp
/// had a JavaScript runtime (the address itself never reaches the log).
fn log_youtube_client(d: &ClipFactoryDeps, item: &QueueItem, client: &str, source: &str) {
    let Some(log) = &d.log else {
        return;
    };
    let key = if d.resolver.js_runtime_passed(source) == Some(true) {
        "log.ytdlp_client"
    } else {
        "log.ytdlp_client_no_js"
    };
    log(Msg::new(key)
        .arg("id", item.id)
        .arg("title", &item.title)
        .arg("client", client));
}

/// Probes the resolved stream. YouTube refusing the address (HTTP 403/401) makes yt-dlp
/// resolve once more with [`FALLBACK_YOUTUBE_CLIENT`]; when that one fails too, the last
/// probe error is returned (inner `Err`), for the download fallback or the start's failure
/// (and the channel's retry of the film). The outer `Err`: the fallback's resolution failed.
async fn probe_or_fallback(
    d: &ClipFactoryDeps,
    prober: &dyn VideoProber,
    item: &QueueItem,
    source: &str,
    viewer: bool,
    stream: ResolvedStream,
) -> Result<Result<(ResolvedStream, VideoInfo), ProbeError>, Msg> {
    let client = stream_youtube_client(&stream);
    if let Some(client) = &client {
        log_youtube_client(d, item, client, source);
    }
    let error = match prober.probe(&d.ffprobe_path, &stream.inputs).await {
        Ok(info) => return Ok(Ok((stream, info))),
        Err(e) => e,
    };
    let Some(client) = client.filter(|_| error.access_denied()) else {
        return Ok(Err(error));
    };
    let Some(again) = d.resolver.resolve_with_fallback_client(source).await else {
        return Ok(Err(error));
    };
    if let Some(log) = &d.log {
        log(Msg::new("log.ytdlp_fallback_client")
            .arg("id", item.id)
            .arg("title", &item.title)
            .arg("client", client)
            .arg("fallback", FALLBACK_YOUTUBE_CLIENT));
    }
    let again = checked_inputs(d, viewer, again.map_err(|e| e.0)?).await?;
    if let Some(client) = stream_youtube_client(&again) {
        log_youtube_client(d, item, &client, source);
    }
    Ok(prober
        .probe(&d.ffprobe_path, &again.inputs)
        .await
        .map(|info| (again, info)))
}

/// What a clip plays: the stream, its analysis and the downloaded film it holds, if any.
type Probed = (ResolvedStream, VideoInfo, Option<Lease>);

/// Resolves and probes the source; a TikTok film, or one whose address was refused to
/// ffprobe (HTTP 403/401), is downloaded by yt-dlp and played from the disk.
async fn resolve_and_probe(
    d: &ClipFactoryDeps,
    downloads: &TempDownloads,
    prober: &dyn VideoProber,
    item: &QueueItem,
    source: &str,
    viewer: bool,
) -> Result<Probed, Msg> {
    let stream = d.resolver.resolve(source).await.map_err(|e| e.0)?;
    let stream = checked_inputs(d, viewer, stream).await?;
    let live = item.is_live || stream.is_live;
    let duration_s = stream.duration_s;
    // TikTok's addresses answer 403 to everything but yt-dlp: not even probed.
    if !live
        && is_tiktok(source)
        && let Some(found) =
            download_and_probe(d, downloads, prober, item, source, duration_s).await
    {
        return found;
    }
    match probe_or_fallback(d, prober, item, source, viewer, stream).await? {
        Ok((stream, info)) => Ok((stream, info, None)),
        // The address was refused (any site): yt-dlp may still fetch the film itself. When
        // that fails too, the refusal is what the start fails with.
        Err(error) if !live && error.access_denied() => {
            match download_and_probe(d, downloads, prober, item, source, duration_s).await {
                Some(Ok(found)) => Ok(found),
                _ => Err(error.message()),
            }
        }
        Err(error) => Err(error.message()),
    }
}

/// yt-dlp downloads `source` into the temp folder and the file is probed; `None`: the
/// resolver of this source cannot download.
async fn download_and_probe(
    d: &ClipFactoryDeps,
    downloads: &TempDownloads,
    prober: &dyn VideoProber,
    item: &QueueItem,
    source: &str,
    duration_s: Option<f64>,
) -> Option<Result<Probed, Msg>> {
    let stem = match downloads.stem(item.id) {
        Ok(stem) => stem,
        Err(e) => return Some(Err(Msg::error("source_message").arg("detail", e))),
    };
    Some(match d.resolver.download(source, &stem).await? {
        Ok(file) => probe_download(d, prober, downloads.adopt(item.id, file), duration_s).await,
        Err(e) => Err(e.0),
    })
}

/// Probes a downloaded film (a new one, or the item's earlier download on a resume or a seek).
async fn probe_download(
    d: &ClipFactoryDeps,
    prober: &dyn VideoProber,
    film: Lease,
    duration_s: Option<f64>,
) -> Result<Probed, Msg> {
    let stream = ResolvedStream {
        inputs: vec![StreamInput {
            url: film.path().to_string_lossy().into_owned(),
            headers: Vec::new(),
        }],
        title: String::new(),
        duration_s,
        is_live: false,
    };
    let info = prober
        .probe(&d.ffprobe_path, &stream.inputs)
        .await
        .map_err(|e| e.message())?;
    Ok((stream, info, Some(film)))
}

/// Where a running ticker reads what it shows.
#[derive(Clone)]
struct ContentSource {
    db: Arc<Mutex<Connection>>,
    channel_id: i64,
    item_id: i64,
    public_url: LiveValue<String>,
    /// The font has Japanese: without it, Japanese texts are drawn in English.
    japanese_font: bool,
}

impl ContentSource {
    fn read(&self) -> rusqlite::Result<TickerContent> {
        let (settings, next) = {
            let conn = self.db.lock().unwrap_or_else(PoisonError::into_inner);
            let settings = SettingsRepo::new(&conn).get(self.channel_id)?;
            // The next film is the first queued one that is not this one (a prefetched clip
            // is itself still queued).
            let next = QueueRepo::new(&conn)
                .list_by_status(self.channel_id, &[ItemStatus::Queued])?
                .into_iter()
                .find(|i| i.id != self.item_id)
                .map(|i| i.title);
            (settings, next)
        };
        let lang = match settings.slate_language {
            Lang::Ja if !self.japanese_font => Lang::En,
            lang => lang,
        };
        Ok(TickerContent {
            lang,
            settings: settings.ticker,
            next_title: next,
            address: request_url(&self.public_url.get()),
        })
    }
}

/// The ticker of a clip about to start: its filters and the writer of its texts.
struct PreparedTicker {
    overlay: String,
    /// The request address whose QR code the overlay draws (`""`: no code).
    code: String,
    writer: TickerWriter,
    source: ContentSource,
}

/// A copy of `font` in `<clips>/_fonts`, named by its file name and size (a changed font gets
/// a new name, a running ffmpeg may hold the old one), as ffmpeg sees it from a clip folder.
fn place_ticker_font(font: &Path, clips_dir: &Path) -> std::io::Result<String> {
    let size = std::fs::metadata(font)?.len();
    let stem: String = font
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ext = if font
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ttc"))
    {
        "ttc"
    } else {
        "ttf"
    };
    let name = format!("{stem}-{size}.{ext}");
    let dir = clips_dir.join(FONTS_DIR);
    let target = dir.join(&name);
    if std::fs::metadata(&target).is_ok_and(|m| m.len() == size) {
        return Ok(format!("../{FONTS_DIR}/{name}"));
    }
    std::fs::create_dir_all(&dir)?;
    // Two clips may start at once: each copies to its own file, the last rename wins.
    static COPIES: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        COPIES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::copy(font, &tmp)?;
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        if !std::fs::metadata(&target).is_ok_and(|m| m.len() == size) {
            return Err(e);
        }
    }
    Ok(format!("../{FONTS_DIR}/{name}"))
}

/// Height of the converted picture (see `build_clip_args`: never above 1080).
fn output_height(info: &VideoInfo) -> u32 {
    if info.height == 0 {
        MAX_HEIGHT
    } else {
        info.height.min(MAX_HEIGHT)
    }
}

struct TickerStart<'a> {
    item: &'a QueueItem,
    settings: &'a ChannelSettings,
    info: &'a VideoInfo,
    duration_s: Option<f64>,
    start_at_s: f64,
    out_dir: &'a Path,
}

/// Writes the ticker's files into the clip folder and builds its filters; `None` (logged): the
/// clip starts without the ticker (no font, files cannot be written).
fn prepare_ticker(
    d: &ClipFactoryDeps,
    rotation: &Arc<AtomicUsize>,
    s: &TickerStart<'_>,
) -> Option<PreparedTicker> {
    let log = |msg: Msg| {
        if let Some(log) = &d.log {
            log(msg);
        }
    };
    let ticker = &s.settings.ticker;
    let mut source = ContentSource {
        db: Arc::clone(&d.db),
        channel_id: d.channel_id,
        item_id: s.item.id,
        public_url: d.public_url.clone(),
        japanese_font: true,
    };
    let content = match source.read() {
        Ok(content) => content,
        Err(e) => {
            log(Msg::new("log.ticker_text_failed").arg("error", e));
            return None;
        }
    };
    let candidates = |content: &TickerContent| -> Vec<String> {
        let now = crate::pipeline::ticker::NowNext {
            title: s.item.title.clone(),
            requester: crate::pipeline::ticker::Requester::of(s.item),
            next_title: content.next_title.clone(),
            address: content.address.clone(),
        };
        let count = match ticker.mode {
            TickerMode::Interval => ticker.messages.len().max(1),
            TickerMode::Continuous => 1,
        };
        (0..count)
            .map(|i| compose_text(content.lang, &now, ticker, i))
            .collect()
    };
    let japanese_dir = system_fonts_dir();
    let mut texts = candidates(&content);
    let mut font = font_for(content.lang, &texts.join(" "), &d.font_file, &japanese_dir);
    if matches!(font, FontChoice::JapaneseMissing(_)) && content.lang == Lang::Ja {
        // Japanese without a Japanese font: the texts in English (like the slates).
        let english = TickerContent {
            lang: Lang::En,
            ..content.clone()
        };
        texts = candidates(&english);
        font = font_for(Lang::En, &texts.join(" "), &d.font_file, &japanese_dir);
    }
    source.japanese_font = font.has_japanese();
    if !font.file().is_file() {
        log(Msg::new("log.ticker_font_missing")
            .arg("path", font.file().display())
            .arg("id", s.item.id)
            .arg("title", &s.item.title));
        return None;
    }
    let font_name = match place_ticker_font(font.file(), &d.clips_dir) {
        Ok(name) => name,
        Err(e) => {
            log(Msg::new("log.ticker_text_failed").arg("error", e));
            return None;
        }
    };
    // What the running ticker reads from now on (Japanese without its font: English).
    let content = TickerContent {
        lang: match content.lang {
            Lang::Ja if !source.japanese_font => Lang::En,
            lang => lang,
        },
        ..content
    };
    let height = output_height(s.info);
    let k = f64::from(height) / f64::from(MAX_HEIGHT);
    let qr_size = if ticker.qr != TickerQr::Off && !content.address.is_empty() {
        let written = qr_image(&content.address, (QR_PX * k).round() as u32)
            .map_err(|e| e.message())
            .and_then(|(png, size)| {
                std::fs::create_dir_all(s.out_dir)
                    .and_then(|()| std::fs::write(s.out_dir.join(QR_FILE), png))
                    .map(|()| size)
                    .map_err(Msg::text)
            });
        match written {
            Ok(size) => Some(size),
            Err(e) => {
                log(Msg::new("log.ticker_text_failed").arg_msg("error", e));
                None
            }
        }
    } else {
        None
    };
    let budget = window_budget_px(ticker, &texts);
    let schedule = Schedule {
        mode: ticker.mode,
        interval_s: f64::from(ticker.interval_s),
        prepare_after_s: prepare_after_s(ticker, budget),
    };
    let mut writer = TickerWriter::new(
        s.out_dir.to_path_buf(),
        s.item,
        schedule,
        (ticker.mode == TickerMode::Interval).then_some(budget),
        Arc::clone(rotation),
        source.japanese_font,
    );
    if let Err(e) =
        std::fs::create_dir_all(s.out_dir).and_then(|()| writer.update(None, &content).map(drop))
    {
        log(Msg::new("log.ticker_text_failed").arg("error", e));
        return None;
    }
    let overlay = overlay_filter(
        ticker,
        &OverlayContext {
            duration_s: s.duration_s,
            start_at_s: s.start_at_s,
            text_file: TEXT_FILE,
            font: &font_name,
            qr: qr_size.map(|size_px| QrOverlay {
                png: QR_FILE,
                size_px,
                caption_file: CAPTION_FILE,
            }),
            frame_height: height,
            text_budget_px: budget,
        },
    );
    let code = if qr_size.is_some() {
        content.address.clone()
    } else {
        String::new()
    };
    Some(PreparedTicker {
        overlay,
        code,
        writer,
        source,
    })
}

/// Keeps the ticker's texts up to date while the clip runs and watches the conversion speed
/// (`watch_speed`: not for a live stream, which never runs faster than real time).
async fn run_ticker(
    mut writer: TickerWriter,
    source: ContentSource,
    seg: Arc<ClipSegmenter>,
    slow: Arc<AtomicBool>,
    watch_speed: bool,
    log: Option<SharedLogFn>,
) {
    let log = |msg: Msg| {
        if let Some(log) = &log {
            log(msg);
        }
    };
    let started = tokio::time::Instant::now();
    let mut watch = SlowWatch::default();
    let mut reported = false;
    while seg.state() == SegmenterState::Running {
        tokio::time::sleep(TICKER_REFRESH).await;
        if let Ok(content) = source.read() {
            let t = seg.out_time_s();
            // Replacing the file may wait for ffmpeg reading it: not on a runtime thread.
            let Ok((back, result)) = tokio::task::spawn_blocking(move || {
                let result = writer.update(t, &content);
                (writer, result)
            })
            .await
            else {
                break;
            };
            writer = back;
            if let Err(e) = result
                && !reported
            {
                reported = true;
                log(Msg::new("log.ticker_text_failed").arg("error", e));
            }
        }
        if !watch_speed {
            continue;
        }
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        // The channel logs it for the film on air (not for one being prepared).
        if let Some(slow_now) = watch.observe(ms, seg.speed()) {
            slow.store(slow_now, Ordering::Relaxed);
        }
    }
    slow.store(false, Ordering::Relaxed);
}

async fn start_clip(
    factory: RealClipFactory,
    item: QueueItem,
    start_at_s: f64,
    attempt: u32,
    ahead: bool,
) -> Result<Box<dyn ClipHandle>, Msg> {
    let RealClipFactory {
        deps: d,
        counter,
        rotation,
        prober,
        downloads,
    } = factory;
    let viewer = item.added_by == AddedBy::Public && d.check_viewer_media;
    let refused = |e: LinkError| Msg::error(e.code());
    // A viewer's link is looked up again now (yt-dlp fetches it anew): its name may have started
    // to point into the owner's network since the request. The owner's films are not limited.
    let source = if viewer {
        check_media_url(&item.source_url, d.host_resolver.as_ref())
            .await
            .map_err(refused)?
    } else {
        item.source_url.clone()
    };
    let (stream, info, download) = match downloads.reuse(item.id) {
        // A resume or a seek of a film downloaded for an earlier clip of the item.
        Some(film) => probe_download(&d, prober.as_ref(), film, item.duration_s).await?,
        None => resolve_and_probe(&d, &downloads, prober.as_ref(), &item, &source, viewer).await?,
    };
    let settings = {
        let conn = d.db.lock().unwrap_or_else(PoisonError::into_inner);
        SettingsRepo::new(&conn)
            .get(d.channel_id)
            .map_err(Msg::text)?
    };
    let live = item.is_live || stream.is_live;
    let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
    // ffmpeg runs in this folder when it draws the ticker, so it must not be relative.
    let out_dir = d.clips_dir.join(format!("{}-{attempt}-{n}", item.id));
    let out_dir = std::path::absolute(&out_dir).unwrap_or(out_dir);
    let start_at_s = if live { 0.0 } else { start_at_s };
    let duration_s = info.duration_s.or(stream.duration_s);
    let ticker = if settings.ticker.enabled {
        prepare_ticker(
            &d,
            &rotation,
            &TickerStart {
                item: &item,
                settings: &settings,
                info: &info,
                // The progress line ends at the out point.
                duration_s: progress_duration(live, item.end_s(duration_s.or(item.duration_s))),
                start_at_s,
                out_dir: &out_dir,
            },
        )
    } else {
        None
    };
    let plain_mode = decide_video_mode(&info, settings.force_transcode);
    let mode = if ticker.is_some() {
        clip_video_mode(&info, &settings)
    } else {
        plain_mode
    };
    if let Some(log) = &d.log {
        log(clip_start_line(&item, mode, &info, start_at_s));
    }
    let encoder = d.encoder.get();
    let options = ClipArgsOptions {
        inputs: stream.inputs,
        input_format: None,
        mode,
        info: info.clone(),
        encoder,
        segment_seconds: d.segment_seconds,
        out_dir: out_dir.clone(),
        start_at_s,
        readrate: clip_readrate(live, ticker.is_some(), ahead),
        overlay: None,
        duration_limit_s: out_point_limit(&item, live, start_at_s),
    };
    let (seg, ticker, fallback) = match ticker {
        Some(prepared) => {
            let plain_dir = PathBuf::from(format!("{}-plain", out_dir.display()));
            let fallback = PlainRestart {
                args: build_clip_args(&ClipArgsOptions {
                    mode: plain_mode,
                    out_dir: plain_dir.clone(),
                    readrate: clip_readrate(live, false, ahead),
                    ..options.clone()
                }),
                out_dir: plain_dir,
                mode: plain_mode,
                encoder,
            };
            let args = build_clip_args(&ClipArgsOptions {
                overlay: Some(prepared.overlay),
                ..options
            });
            let seg = Arc::new(ClipSegmenter::start(
                &d.ffmpeg_path,
                args,
                out_dir.clone(),
                Some(out_dir.clone()),
            ));
            let slow = Arc::new(AtomicBool::new(false));
            let task = tokio::spawn(run_ticker(
                prepared.writer,
                prepared.source,
                Arc::clone(&seg),
                Arc::clone(&slow),
                !live,
                d.log.clone(),
            ));
            let run = TickerRun {
                task: task.abort_handle(),
                slow,
                code: prepared.code,
            };
            (seg, Some(run), Some(fallback))
        }
        None => {
            let args = build_clip_args(&options);
            let seg = ClipSegmenter::start(&d.ffmpeg_path, args, out_dir.clone(), None);
            (Arc::new(seg), None, None)
        }
    };
    Ok(Box::new(RealClipHandle {
        seg,
        out_dir,
        ffmpeg_path: d.ffmpeg_path.clone(),
        duration_s,
        copies_video: mode == VideoMode::Copy,
        probe: ProbeSummary {
            codec: info.codec.clone(),
            height: info.height,
            fps: info.fps,
            pix_fmt: info.pix_fmt.clone(),
            has_audio: info.has_audio,
            duration_s: info.duration_s,
        },
        mode: stream_mode(mode),
        encoder: (mode == VideoMode::Transcode).then_some(encoder),
        ticker,
        fallback,
        produced: false,
        item_id: item.id,
        title: item.title.clone(),
        log: d.log.clone(),
        disposed: false,
        _download: download,
    }))
}

/// The countdown's timer as drawtext's text: `m:ss`, or `h:mm:ss` for an hour or more, of the
/// seconds left (`total_s` minus the frame time, rounded up).
pub fn countdown_timer_text(total_s: f64) -> String {
    let left = format!("ceil({total_s:.3}-t)");
    if total_s >= 3600.0 {
        format!(
            "%{{eif:trunc({left}/3600):d}}:%{{eif:mod(trunc({left}/60),60):d:2}}:%{{eif:mod({left},60):d:2}}"
        )
    } else {
        format!("%{{eif:trunc({left}/60):d}}:%{{eif:mod({left},60):d:2}}")
    }
}

/// lavfi graph of the countdown: the slate's background, "Starting in", the timer and the
/// film's title (read from files in ffmpeg's folder; the title without `%` expansion), and
/// silence of the same length (one input, so the real-time pace holds for both).
pub fn countdown_graph(font: Option<&str>, total_s: f64) -> String {
    let mut chain = vec![format!("color=c=0x14161c:s=1920x1080:r=30:d={total_s:.3}")];
    if let Some(font) = font {
        let common = format!("fontfile={font}:x=(w-text_w)/2");
        chain.push(format!(
            "drawtext={common}:fontcolor=0xb8bcc8:textfile=title.txt:fontsize=64:y=(h/2)-260"
        ));
        chain.push(format!(
            "drawtext={common}:fontcolor=white:textfile=timer.txt:fontsize=220:y=(h/2)-150"
        ));
        chain.push(format!(
            "drawtext={common}:fontcolor=0xb8bcc8:textfile=film.txt:expansion=none:fontsize=48:y=(h/2)+150"
        ));
    }
    format!(
        "{}[out0];anullsrc=channel_layout=stereo:sample_rate=48000,atrim=end={total_s:.3}[out1]",
        chain.join(",")
    )
}

/// The countdown before `item`: ffmpeg draws it at real time from a lavfi graph.
async fn start_countdown(
    factory: RealClipFactory,
    item: QueueItem,
    total_s: f64,
) -> Result<Box<dyn ClipHandle>, Msg> {
    let d = factory.deps;
    let lang = {
        let conn = d.db.lock().unwrap_or_else(PoisonError::into_inner);
        SettingsRepo::new(&conn)
            .get(d.channel_id)
            .map_err(Msg::text)?
            .slate_language
    };
    let n = factory.counter.fetch_add(1, Ordering::SeqCst) + 1;
    let out_dir = d.clips_dir.join(format!("countdown-{}-{n}", item.id));
    let out_dir = std::path::absolute(&out_dir).unwrap_or(out_dir);
    std::fs::create_dir_all(&out_dir).map_err(Msg::text)?;
    let title = crate::i18n::t(lang, "slates.countdown_title", &[]);
    let film = item.title.replace(['\r', '\n'], " ");
    let font = font_for(
        lang,
        &format!("{title} {film}"),
        &d.font_file,
        &system_fonts_dir(),
    );
    let font_name = if font.file().is_file() {
        let name = place_ticker_font(font.file(), &d.clips_dir).map_err(Msg::text)?;
        for (file, text) in [
            ("title.txt", title),
            ("timer.txt", countdown_timer_text(total_s)),
            ("film.txt", film),
        ] {
            std::fs::write(out_dir.join(file), text).map_err(Msg::text)?;
        }
        Some(name)
    } else {
        None
    };
    let args = build_clip_args(&ClipArgsOptions {
        inputs: vec![StreamInput {
            url: countdown_graph(font_name.as_deref(), total_s),
            headers: Vec::new(),
        }],
        input_format: Some("lavfi".into()),
        mode: VideoMode::Transcode,
        info: VideoInfo {
            codec: "rawvideo".into(),
            height: 1080,
            pix_fmt: "yuv420p".into(),
            fps: 30.0,
            has_audio: true,
            duration_s: Some(total_s),
            ..VideoInfo::default()
        },
        encoder: VideoEncoder::Libx264,
        segment_seconds: d.segment_seconds,
        out_dir: out_dir.clone(),
        start_at_s: 0.0,
        readrate: Some(ReadRate::new(TICKER_READRATE)),
        overlay: None,
        duration_limit_s: None,
    });
    let seg = ClipSegmenter::start(&d.ffmpeg_path, args, out_dir.clone(), Some(out_dir.clone()));
    Ok(Box::new(RealClipHandle {
        seg: Arc::new(seg),
        out_dir,
        ffmpeg_path: d.ffmpeg_path.clone(),
        duration_s: Some(total_s),
        copies_video: false,
        probe: ProbeSummary::default(),
        mode: StreamMode::Transcode,
        encoder: Some(VideoEncoder::Libx264),
        ticker: None,
        fallback: None,
        produced: false,
        item_id: item.id,
        title: item.title.clone(),
        log: d.log.clone(),
        disposed: false,
        _download: None,
    }))
}

impl ClipFactory for RealClipFactory {
    fn create_countdown(
        &self,
        item: QueueItem,
        seconds: f64,
    ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
        Box::pin(start_countdown(self.clone(), item, seconds))
    }

    fn create(
        &self,
        item: QueueItem,
        start_at_s: f64,
        attempt: u32,
    ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
        Box::pin(start_clip(self.clone(), item, start_at_s, attempt, false))
    }

    fn create_ahead(
        &self,
        item: QueueItem,
        start_at_s: f64,
    ) -> BoxFuture<'static, Result<Box<dyn ClipHandle>, Msg>> {
        Box::pin(start_clip(self.clone(), item, start_at_s, 0, true))
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
            requester_key_hash: None,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
            in_s: None,
            out_s: None,
            start_at: None,
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
            portrait: false,
            ..VideoInfo::default()
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
            )
            .to_string(),
            "Video #7 \"Kot\": repackaging (h264 720p 25 fps)"
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
            )
            .to_string(),
            "Video #3 \"Pies\": transcoding (vp9 1080p 29.97 fps, no audio), resuming from 13 s"
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
                return Err(ResolveError::new("Could not recognise the source."));
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
        db: Arc<Mutex<Connection>>,
    }

    fn fixture(
        source: Option<PathBuf>,
        dir: tempfile::TempDir,
        ffmpeg: PathBuf,
        ffprobe: PathBuf,
    ) -> Fixture {
        fixture_with_font(source, dir, ffmpeg, ffprobe, default_font())
    }

    fn default_font() -> PathBuf {
        if cfg!(windows) {
            "C:/Windows/Fonts/arial.ttf".into()
        } else {
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into()
        }
    }

    fn fixture_with_font(
        source: Option<PathBuf>,
        dir: tempfile::TempDir,
        ffmpeg: PathBuf,
        ffprobe: PathBuf,
        font_file: PathBuf,
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
                requester_key_hash: None,
            })
            .unwrap();
        let clips = dir.path().join("clips");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let db = Arc::new(Mutex::new(conn));
        let factory = create_clip_factory(ClipFactoryDeps {
            resolver: Arc::new(FileResolver(source)),
            ffmpeg_path: ffmpeg,
            ffprobe_path: ffprobe,
            encoder: crate::live::LiveValue::new(VideoEncoder::Libx264),
            segment_seconds: 4,
            clips_dir: clips.clone(),
            temp_dir: clips.join("..").join("temp"),
            db: Arc::clone(&db),
            channel_id: MAIN_CHANNEL_ID,
            log: Some(Arc::new(move |l: Msg| {
                sink.lock().unwrap().push(l.to_string())
            })),
            font_file,
            public_url: crate::live::LiveValue::new("https://kanal.example.com".to_string()),
            host_resolver: Arc::new(crate::rules::TableHostResolver::new(&[])),
            check_viewer_media: true,
        });
        Fixture {
            _dir: dir,
            clips,
            factory,
            item,
            lines,
            db,
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
        assert_eq!(err.to_string(), "Could not recognise the source.");
    }

    /// Resolves every URL to the given address(es), as yt-dlp would.
    struct UrlResolver(Vec<&'static str>);

    #[async_trait]
    impl MediaResolver for UrlResolver {
        async fn fetch_meta(&self, _url: &str) -> Result<MediaMeta, ResolveError> {
            Err(ResolveError::new("nieużywane"))
        }
        async fn resolve(&self, _url: &str) -> Result<ResolvedStream, ResolveError> {
            Ok(ResolvedStream {
                inputs: self
                    .0
                    .iter()
                    .map(|url| StreamInput {
                        url: (*url).to_string(),
                        headers: vec![],
                    })
                    .collect(),
                title: "film".into(),
                duration_s: Some(6.0),
                is_live: false,
            })
        }
    }

    /// A factory whose tools do not exist: it can only get as far as the checks.
    fn factory_for_media(urls: Vec<&'static str>) -> RealClipFactory {
        let hosts = crate::rules::TableHostResolver::new(&[
            ("cdn.example", &["93.184.216.34"]),
            ("evil.example", &["192.168.0.10"]),
        ]);
        create_clip_factory(ClipFactoryDeps {
            resolver: Arc::new(UrlResolver(urls)),
            ffmpeg_path: "no-such-ffmpeg".into(),
            ffprobe_path: "no-such-ffprobe".into(),
            encoder: crate::live::LiveValue::new(VideoEncoder::Libx264),
            segment_seconds: 4,
            clips_dir: std::env::temp_dir().join("vrcinema-never-made"),
            temp_dir: std::env::temp_dir().join("vrcinema-never-made"),
            db: Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap())),
            channel_id: MAIN_CHANNEL_ID,
            log: None,
            font_file: default_font(),
            public_url: crate::live::LiveValue::new(String::new()),
            host_resolver: Arc::new(hosts),
            check_viewer_media: true,
        })
    }

    async fn start_error(factory: &RealClipFactory, added_by: AddedBy) -> Msg {
        start_error_of(
            factory,
            QueueItem {
                added_by,
                ..item(1, "Test")
            },
        )
        .await
    }

    async fn start_error_of(factory: &RealClipFactory, item: QueueItem) -> Msg {
        factory
            .create(item, 0.0, 0)
            .await
            .err()
            .expect("the start must fail")
    }

    #[tokio::test]
    async fn the_media_of_a_viewers_link_must_not_be_on_the_home_network() {
        for url in [
            "http://192.168.0.5/v.mp4",
            "https://evil.example/v.m3u8",
            "file:///C:/secret.mp4",
            // The URL standard reads the backslash as the end of the host; ffmpeg does not.
            r"http://93.184.216.34\@192.168.1.1/v.mp4",
            r"http://user\@192.168.1.1/v.mp4",
        ] {
            let factory = factory_for_media(vec![url]);
            let err = start_error(&factory, AddedBy::Public).await;
            assert!(
                matches!(
                    err.code(),
                    Some("local_address" | "bad_scheme" | "bad_address")
                ),
                "{url}: {err:?}"
            );
        }
        let err = start_error(
            &factory_for_media(vec![
                "https://cdn.example/v.mp4",
                "http://192.168.0.5/audio.m4a",
            ]),
            AddedBy::Public,
        )
        .await;
        assert_eq!(err.code(), Some("local_address"), "every input is checked");
        assert_eq!(
            err.render(Lang::Pl),
            "Linki do sieci lokalnej lub do tego komputera są niedozwolone."
        );
    }

    #[tokio::test]
    async fn the_same_address_is_fine_for_the_owners_film_and_a_public_one_goes_on() {
        // The owner's film gets past the check (and fails later, on the missing ffprobe).
        let err = start_error(
            &factory_for_media(vec!["http://192.168.0.5/v.mp4"]),
            AddedBy::Admin,
        )
        .await;
        assert_eq!(err.render(Lang::En), "program not found", "{err:?}");
        // A public address passes the check as well.
        let err = start_error(
            &factory_for_media(vec!["https://cdn.example/v.mp4"]),
            AddedBy::Public,
        )
        .await;
        assert_eq!(err.render(Lang::En), "program not found", "{err:?}");
    }

    /// The viewer's own link is looked up again when the film starts: yt-dlp fetches it anew, and
    /// the name may point into the owner's network by now. The owner's link is not looked up.
    #[tokio::test]
    async fn the_name_of_the_viewers_link_is_looked_up_again_before_the_resolver_runs() {
        let factory = factory_for_media(vec!["https://cdn.example/v.mp4"]);
        let with_source = |added_by, source: &str| QueueItem {
            added_by,
            source_url: source.into(),
            ..item(1, "Test")
        };
        let err = start_error_of(
            &factory,
            with_source(AddedBy::Public, "https://evil.example/watch?v=1"),
        )
        .await;
        assert_eq!(err.code(), Some("local_address"), "{err:?}");
        let err = start_error_of(
            &factory,
            with_source(AddedBy::Public, "http://nas/watch?v=1"),
        )
        .await;
        assert_eq!(err.code(), Some("local_address"), "{err:?}");
        // The same name for the owner is not looked at.
        let err = start_error_of(
            &factory,
            with_source(AddedBy::Admin, "https://evil.example/watch?v=1"),
        )
        .await;
        assert_eq!(err.render(Lang::En), "program not found", "{err:?}");
    }

    // ---- YouTube refusing an address: the fallback client ----

    /// A signed address as yt-dlp hands it out (the IP address and tokens made up).
    fn googlevideo(host: &str, client: &str) -> String {
        format!(
            "https://{host}/videoplayback?expire=1792700650&ip=188.47.115.50&id=o-AI6&itag=137&c={client}&sig=AE0s2JYw%3D"
        )
    }

    type Argv = Arc<Mutex<Vec<Vec<String>>>>;

    /// yt-dlp: the TVHTML5 address normally, `fallback` with the extractor args.
    struct YtDlpRunner {
        calls: Argv,
        fallback: String,
    }

    #[async_trait]
    impl crate::resolver::ytdlp::ProcessRunner for YtDlpRunner {
        async fn run(
            &self,
            _cmd: &Path,
            args: &[OsString],
            _timeout: Duration,
        ) -> Result<crate::process::ProcessOutput, crate::process::ProcessError> {
            let args: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let url = if args.iter().any(|a| a == "--extractor-args") {
                self.fallback.clone()
            } else {
                googlevideo("rr7---sn-u2oxu.googlevideo.com", "TVHTML5")
            };
            self.calls.lock().unwrap().push(args);
            Ok(crate::process::ProcessOutput {
                code: Some(0),
                stdout: serde_json::json!({"title": "film", "duration": 6, "url": url}).to_string(),
                stderr: String::new(),
            })
        }
    }

    /// ffprobe: refuses (as it reports a 403, addresses already cut) every address that contains
    /// one of `denied`, or fails with `other`; reads the rest.
    struct FakeProber {
        denied: Vec<&'static str>,
        other: Option<&'static str>,
        probed: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl VideoProber for FakeProber {
        async fn probe(
            &self,
            _ffprobe: &Path,
            inputs: &[StreamInput],
        ) -> Result<VideoInfo, ProbeError> {
            let url = inputs[0].url.clone();
            self.probed.lock().unwrap().push(url.clone());
            if let Some(other) = self.other {
                return Err(ProbeError::Failed(other.into()));
            }
            if self.denied.iter().any(|d| url.contains(d)) {
                return Err(ProbeError::Failed(crate::redact::redact_urls(&format!(
                    "{url}: Server returned 403 Forbidden (access denied)"
                ))));
            }
            Ok(video("h264", 720, 25.0, true))
        }
    }

    struct YouTube {
        _dir: tempfile::TempDir,
        factory: RealClipFactory,
        ytdlp: Argv,
        probed: Arc<Mutex<Vec<String>>>,
        lines: Arc<Mutex<Vec<String>>>,
    }

    fn youtube_factory(
        resolver: Option<Arc<dyn MediaResolver>>,
        fallback: &str,
        denied: Vec<&'static str>,
        other: Option<&'static str>,
    ) -> YouTube {
        let dir = tempfile::tempdir().unwrap();
        let ytdlp = Argv::default();
        let resolver = resolver.unwrap_or_else(|| {
            Arc::new(
                crate::resolver::ytdlp::YtDlpResolver::new(
                    "yt-dlp".into(),
                    Some("deno:/deno".into()),
                    None,
                )
                .with_runner(Box::new(YtDlpRunner {
                    calls: Arc::clone(&ytdlp),
                    fallback: fallback.to_string(),
                })),
            )
        });
        let hosts = crate::rules::TableHostResolver::new(&[
            ("youtu.be", &["142.250.0.1"]),
            ("rr7---sn-u2oxu.googlevideo.com", &["142.250.0.2"]),
            ("rr3---sn-u2oxu.googlevideo.com", &["142.250.0.3"]),
            ("evil.example", &["192.168.0.10"]),
        ]);
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let probed = Arc::new(Mutex::new(Vec::new()));
        let factory = create_clip_factory(ClipFactoryDeps {
            resolver,
            ffmpeg_path: "no-such-ffmpeg".into(),
            ffprobe_path: "no-such-ffprobe".into(),
            encoder: crate::live::LiveValue::new(VideoEncoder::Libx264),
            segment_seconds: 4,
            clips_dir: dir.path().join("clips"),
            temp_dir: dir.path().join("temp"),
            db: Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap())),
            channel_id: MAIN_CHANNEL_ID,
            log: Some(Arc::new(move |l: Msg| {
                sink.lock().unwrap().push(l.to_string())
            })),
            font_file: default_font(),
            public_url: crate::live::LiveValue::new(String::new()),
            host_resolver: Arc::new(hosts),
            check_viewer_media: true,
        })
        .with_prober(Arc::new(FakeProber {
            denied,
            other,
            probed: Arc::clone(&probed),
        }));
        YouTube {
            _dir: dir,
            factory,
            ytdlp,
            probed,
            lines,
        }
    }

    fn visionos() -> String {
        googlevideo("rr3---sn-u2oxu.googlevideo.com", "VISIONOS")
    }

    async fn start(y: &YouTube, added_by: AddedBy) -> Result<Box<dyn ClipHandle>, Msg> {
        let item = QueueItem {
            added_by,
            ..item(128, "Ado")
        };
        y.factory.create(item, 0.0, 0).await
    }

    fn no_secrets(text: &str) {
        for secret in ["188.47.115.50", "ip=", "sig=", "expire="] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }

    #[tokio::test]
    async fn a_refused_youtube_address_is_resolved_again_with_the_fallback_client() {
        for added_by in [AddedBy::Admin, AddedBy::Public] {
            let y = youtube_factory(None, &visionos(), vec!["c=TVHTML5"], None);
            let mut handle = start(&y, added_by).await.expect("the fallback plays");
            handle.dispose();
            let calls = y.ytdlp.lock().unwrap().clone();
            assert_eq!(calls.len(), 2, "{added_by:?}");
            assert!(!calls[0].iter().any(|a| a == "--extractor-args"));
            let i = calls[1]
                .iter()
                .position(|a| a == "--extractor-args")
                .unwrap();
            assert_eq!(calls[1][i + 1], "youtube:player_client=visionos");
            let probed = y.probed.lock().unwrap().clone();
            assert_eq!(probed.len(), 2);
            assert!(probed[1].contains("c=VISIONOS"), "{probed:?}");
            let lines = y.lines.lock().unwrap().clone();
            assert_eq!(
                lines[..3],
                [
                    "Video #128 \"Ado\": YouTube address from yt-dlp client TVHTML5 (with a JavaScript runtime)",
                    "Video #128 \"Ado\": YouTube refused the address of yt-dlp client TVHTML5; trying client visionos",
                    "Video #128 \"Ado\": YouTube address from yt-dlp client VISIONOS (with a JavaScript runtime)",
                ],
                "{lines:?}"
            );
            for line in &lines {
                no_secrets(line);
            }
        }
    }

    #[tokio::test]
    async fn when_the_fallback_is_refused_too_the_start_fails_without_the_signed_address() {
        let y = youtube_factory(None, &visionos(), vec!["googlevideo.com"], None);
        let err = start(&y, AddedBy::Admin).await.err().expect("both refused");
        let text = err.render(Lang::En);
        assert_eq!(
            text,
            "ffprobe: https://rr3---sn-u2oxu.googlevideo.com/videoplayback: Server returned 403 Forbidden (access denied)"
        );
        no_secrets(&text);
        let calls = y.ytdlp.lock().unwrap().clone();
        assert_eq!(calls.len(), 3, "one fallback client, then one download");
        assert!(!calls[1].iter().any(|a| a == "--max-filesize"));
        assert!(calls[2].iter().any(|a| a == "--max-filesize"), "{calls:?}");
        assert_eq!(y.probed.lock().unwrap().len(), 2, "no film was downloaded");
    }

    /// yt-dlp for TikTok: the address (refused to ffprobe) for `-j`, the film for a download.
    struct TikTokRunner {
        calls: Argv,
    }

    #[async_trait]
    impl crate::resolver::ytdlp::ProcessRunner for TikTokRunner {
        async fn run(
            &self,
            _cmd: &Path,
            args: &[OsString],
            _timeout: Duration,
        ) -> Result<crate::process::ProcessOutput, crate::process::ProcessError> {
            let args: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let stdout = match args.iter().position(|a| a == "-o") {
                Some(i) => {
                    let film = args[i + 1].replace("%(ext)s", "mp4");
                    std::fs::write(film, b"film").unwrap();
                    String::new()
                }
                None => serde_json::json!({
                    "title": "film", "duration": 6,
                    "url": "https://v16.tiktokcdn.example/v.mp4?sig=1",
                })
                .to_string(),
            };
            self.calls.lock().unwrap().push(args);
            Ok(crate::process::ProcessOutput {
                code: Some(0),
                stdout,
                stderr: String::new(),
            })
        }
    }

    fn tiktok_factory() -> YouTube {
        let ytdlp = Argv::default();
        let resolver = Arc::new(
            crate::resolver::ytdlp::YtDlpResolver::new("yt-dlp".into(), None, None).with_runner(
                Box::new(TikTokRunner {
                    calls: Arc::clone(&ytdlp),
                }),
            ),
        );
        YouTube {
            ytdlp,
            ..youtube_factory(Some(resolver), "", vec!["tiktokcdn"], None)
        }
    }

    fn tiktok_item() -> QueueItem {
        QueueItem {
            source_url: "https://www.tiktok.com/@polishballers/video/7698743057218407702".into(),
            ..item(129, "TikTok")
        }
    }

    #[tokio::test]
    async fn a_tiktok_film_is_downloaded_and_played_from_the_temp_folder() {
        let y = tiktok_factory();
        let temp = y._dir.path().join("temp");
        let mut first = y.factory.create(tiktok_item(), 0.0, 0).await.unwrap();
        let film = temp.join("129-1.mp4");
        assert!(film.is_file());
        // A seek: the same file, without yt-dlp.
        let mut seek = y.factory.create(tiktok_item(), 3.0, 0).await.unwrap();
        first.dispose();
        seek.dispose();
        let calls = y.ytdlp.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[0].iter().any(|a| a == "-j"));
        assert!(calls[1].iter().any(|a| a == "--max-filesize"));
        let film = std::path::absolute(&film).unwrap();
        let probed = y.probed.lock().unwrap().clone();
        assert_eq!(
            probed,
            [film.to_string_lossy(), film.to_string_lossy()],
            "the refused address is never probed"
        );
        drop((first, seek));
        assert!(film.is_file(), "kept for a while after its last clip");
    }

    #[tokio::test]
    async fn a_refused_address_of_any_site_falls_back_to_a_download() {
        let y = tiktok_factory();
        let item = QueueItem {
            source_url: "https://www.cda.pl/video/1".into(),
            ..tiktok_item()
        };
        let mut handle = y.factory.create(item, 0.0, 0).await.expect("downloaded");
        handle.dispose();
        let probed = y.probed.lock().unwrap().clone();
        assert_eq!(probed.len(), 2);
        assert!(probed[1].ends_with("129-1.mp4"), "{probed:?}");

        // A live stream is never downloaded.
        let y = tiktok_factory();
        let live = QueueItem {
            is_live: true,
            ..tiktok_item()
        };
        assert!(y.factory.create(live, 0.0, 0).await.is_err());
        assert_eq!(y.ytdlp.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_fallback_address_of_a_viewers_link_is_checked_as_well() {
        let y = youtube_factory(
            None,
            "https://evil.example/videoplayback?c=VISIONOS",
            vec!["c=TVHTML5"],
            None,
        );
        let err = start(&y, AddedBy::Public).await.err().unwrap();
        assert_eq!(err.code(), Some("local_address"), "{err:?}");
        assert_eq!(y.probed.lock().unwrap().len(), 1, "never probed");
    }

    #[tokio::test]
    async fn other_failures_and_other_resolvers_get_no_fallback() {
        // Not a refusal.
        let y = youtube_factory(
            None,
            &visionos(),
            vec![],
            Some("Invalid data found when processing input"),
        );
        let err = start(&y, AddedBy::Admin).await.err().unwrap();
        assert_eq!(
            err.render(Lang::En),
            "ffprobe: Invalid data found when processing input"
        );
        assert_eq!(y.ytdlp.lock().unwrap().len(), 1);

        // A refused address that is not YouTube's.
        let y = youtube_factory(
            Some(Arc::new(UrlResolver(vec![
                "https://cdn.example/v.mp4?token=1",
            ]))),
            &visionos(),
            vec!["cdn.example"],
            None,
        );
        let err = start(&y, AddedBy::Admin).await.err().unwrap();
        assert_eq!(
            err.render(Lang::En),
            "ffprobe: https://cdn.example/v.mp4: Server returned 403 Forbidden (access denied)"
        );
        assert_eq!(y.probed.lock().unwrap().len(), 1);

        // A YouTube address from a resolver without a fallback (not yt-dlp).
        let y = youtube_factory(
            Some(Arc::new(UrlResolver(vec![
                "https://rr7---sn-u2oxu.googlevideo.com/videoplayback?c=TVHTML5",
            ]))),
            &visionos(),
            vec!["googlevideo.com"],
            None,
        );
        assert!(start(&y, AddedBy::Admin).await.is_err());
        assert_eq!(y.probed.lock().unwrap().len(), 1);
        assert_eq!(
            y.lines.lock().unwrap()[..],
            [
                "Video #128 \"Ado\": YouTube address from yt-dlp client TVHTML5 (without a JavaScript runtime)"
            ]
        );
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
                "Video #{} \"Test\": repackaging (h264 240p 25 fps, no audio)",
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
        assert!(f.lines.lock().unwrap()[1].ends_with(", resuming from 4 s"));
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

    // ---------- Info ticker (stage 4, task 6) ----------

    #[test]
    fn the_ticker_forces_the_conversion() {
        let info = video("h264", 720, 25.0, true);
        let mut settings = ChannelSettings::default();
        assert_eq!(clip_video_mode(&info, &settings), VideoMode::Copy);
        settings.ticker.enabled = true;
        assert_eq!(clip_video_mode(&info, &settings), VideoMode::Transcode);
        settings.ticker.enabled = false;
        settings.force_transcode = true;
        assert_eq!(clip_video_mode(&info, &settings), VideoMode::Transcode);
    }

    #[test]
    fn the_countdown_shows_minutes_or_hours_and_an_out_point_limits_a_clip() {
        assert_eq!(
            countdown_timer_text(150.0),
            "%{eif:trunc(ceil(150.000-t)/60):d}:%{eif:mod(ceil(150.000-t),60):d:2}"
        );
        assert!(countdown_timer_text(3600.0).matches("%{eif").count() == 3);
        let graph = countdown_graph(Some("../_fonts/f.ttf"), 90.0);
        assert!(graph.contains("d=90.000") && graph.contains("atrim=end=90.000[out1]"));
        assert!(graph.contains("textfile=film.txt:expansion=none"));
        assert!(!countdown_graph(None, 5.0).contains("drawtext"));
        let mut film = item(1, "a");
        film.out_s = Some(50.0);
        assert_eq!(out_point_limit(&film, false, 20.0), Some(30.0));
        assert_eq!(out_point_limit(&film, false, 60.0), None);
        assert_eq!(out_point_limit(&film, true, 20.0), None);
    }

    #[test]
    fn a_live_stream_has_no_progress_line() {
        assert_eq!(progress_duration(true, Some(60.0)), None);
        assert_eq!(progress_duration(false, Some(60.0)), Some(60.0));
        assert_eq!(progress_duration(false, None), None);
    }

    fn enable_ticker(f: &Fixture, patch: serde_json::Value) {
        let conn = f.db.lock().unwrap();
        let mut body = serde_json::json!({ "tickerEnabled": true });
        body.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let patch = crate::db::sanitize_settings_patch(&body).unwrap();
        SettingsRepo::new(&conn)
            .update(MAIN_CHANNEL_ID, &patch)
            .unwrap();
    }

    /// Polls the handle until ffmpeg is done; the handle never reports a failure on the way.
    async fn run_to_the_end(handle: &mut Box<dyn ClipHandle>) -> Vec<SegmentInfo> {
        let mut segments = Vec::new();
        let mut failed = false;
        let finished = wait_until(
            || {
                segments.extend(handle.poll());
                failed |= handle.state() == SegmenterState::Failed;
                handle.state() != SegmenterState::Running
            },
            Duration::from_secs(120),
        )
        .await;
        assert!(finished);
        segments.extend(handle.poll());
        assert!(!failed, "{:?}", handle.error());
        assert_eq!(handle.state(), SegmenterState::Done, "{:?}", handle.error());
        segments
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_ticker_text_names_the_film_and_follows_the_queue_messages_and_language() {
        let (Some(ffmpeg), Some(ffprobe)) = (bin("ffmpeg"), bin("ffprobe")) else {
            eprintln!("skip: bin/ffmpeg or bin/ffprobe missing");
            return;
        };
        if !default_font().is_file() {
            eprintln!("skip: no default font");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("film.mp4");
        make_source(&ffmpeg, &source, 60).await;
        let f = fixture(Some(source), dir, ffmpeg, ffprobe);
        enable_ticker(
            &f,
            serde_json::json!({ "tickerMode": "continuous", "tickerMessages": ["Witajcie!"] }),
        );
        {
            let conn = f.db.lock().unwrap();
            QueueRepo::new(&conn)
                .add(&NewItem {
                    channel_id: MAIN_CHANNEL_ID,
                    source_url: "https://youtu.be/y".into(),
                    title: "Następny".into(),
                    duration_s: Some(6.0),
                    is_live: false,
                    status: ItemStatus::Queued,
                    added_by: AddedBy::Public,
                    requester_name: Some("Kasia".into()),
                    requester_ip: None,
                    requester_key_hash: None,
                })
                .unwrap();
        }
        let mut handle = f.factory.create(f.item.clone(), 0.0, 0).await.unwrap();
        let out_dir = f.clips.join(format!("{}-0-1", f.item.id));
        let read = |name: &str| {
            crate::pipeline::ticker::TickerText::read(&out_dir.join(name)).unwrap_or_default()
        };
        assert_eq!(
            read(TEXT_FILE),
            "► Now: Test   ·   added by Host   ·   next: Następny   ·   Witajcie!"
        );
        assert_eq!(read(CAPTION_FILE), "Add a video");
        assert!(out_dir.join(QR_FILE).is_file());
        // The channel knows which address the code on air is for (final wave I1).
        assert_eq!(
            handle.ticker_code().as_deref(),
            Some("https://kanal.example.com/")
        );
        assert!(!handle.copies_video(), "the ticker forces the conversion");
        assert!(
            f.lines.lock().unwrap()[0].contains("transcoding"),
            "{:?}",
            f.lines.lock().unwrap()
        );
        // Content changes reach the running clip through its text file.
        {
            let conn = f.db.lock().unwrap();
            let patch = crate::db::sanitize_settings_patch(&serde_json::json!({
                "tickerMessages": ["Nowa wiadomość"], "slateLanguage": "pl"
            }))
            .unwrap();
            SettingsRepo::new(&conn)
                .update(MAIN_CHANNEL_ID, &patch)
                .unwrap();
        }
        let expected = "► Teraz: Test   ·   dodane przez Gospodarza   ·   następnie: Następny   ·   Nowa wiadomość";
        assert!(
            wait_until(|| read(TEXT_FILE) == expected, Duration::from_secs(5)).await,
            "{}",
            read(TEXT_FILE)
        );
        assert_eq!(read(CAPTION_FILE), "Dodaj film");
        // ffmpeg draws it: segments come out while the file is being rewritten.
        let mut segments = Vec::new();
        assert!(
            wait_until(
                || {
                    segments.extend(handle.poll());
                    !segments.is_empty()
                },
                Duration::from_secs(60)
            )
            .await,
            "{:?}",
            handle.error()
        );
        assert_eq!(handle.state(), SegmenterState::Running);
        handle.dispose();
        assert!(wait_until(|| !out_dir.exists(), Duration::from_secs(10)).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn without_its_font_the_clip_starts_without_the_ticker_and_it_is_logged() {
        let (Some(ffmpeg), Some(ffprobe)) = (bin("ffmpeg"), bin("ffprobe")) else {
            eprintln!("skip: bin/ffmpeg or bin/ffprobe missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("film.mp4");
        make_source(&ffmpeg, &source, 6).await;
        let missing = dir.path().join("brak").join("czcionka.ttf");
        let f = fixture_with_font(Some(source), dir, ffmpeg, ffprobe, missing);
        enable_ticker(&f, serde_json::json!({}));
        let mut handle = f.factory.create(f.item.clone(), 0.0, 0).await.unwrap();
        // The same attempt, no ticker files, the video copied as without the ticker.
        let out_dir = f.clips.join(format!("{}-0-1", f.item.id));
        assert!(out_dir.exists());
        assert!(!out_dir.join(TEXT_FILE).exists());
        assert_eq!(handle.ticker_code(), None, "no ticker, no code");
        assert!(handle.copies_video());
        let lines = f.lines.lock().unwrap().clone();
        assert!(
            lines[0].starts_with("The info ticker font (")
                && lines[0].contains(&format!(
                    "video #{} \"Test\" plays without the ticker",
                    f.item.id
                )),
            "{lines:?}"
        );
        assert!(lines[1].contains("repackaging"), "{lines:?}");
        assert!(!run_to_the_end(&mut handle).await.is_empty());
        handle.dispose();
    }

    /// ffmpeg cannot draw the ticker (this ffmpeg falls back to another font by itself, so the
    /// failure here is a text file it cannot read): the handle starts the clip again without
    /// the ticker, the channel never sees the failure (no attempt of the film is used up) and
    /// the log says why.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_clip_that_cannot_draw_its_ticker_starts_again_without_it() {
        let Some(ffmpeg) = bin("ffmpeg") else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("film.mp4");
        make_source(&ffmpeg, &source, 6).await;
        let out_dir = dir.path().join("1-0-1");
        let plain_dir = dir.path().join("1-0-1-plain");
        let options = ClipArgsOptions {
            inputs: vec![StreamInput {
                url: source.to_string_lossy().into_owned(),
                headers: vec![],
            }],
            input_format: None,
            mode: VideoMode::Transcode,
            info: video("h264", 240, 25.0, false),
            encoder: VideoEncoder::Libx264,
            segment_seconds: 4,
            out_dir: out_dir.clone(),
            start_at_s: 0.0,
            readrate: None,
            overlay: Some("drawtext=textfile=brak.txt".into()),
            duration_limit_s: None,
        };
        let fallback = PlainRestart {
            args: build_clip_args(&ClipArgsOptions {
                mode: VideoMode::Copy,
                out_dir: plain_dir.clone(),
                overlay: None,
                ..options.clone()
            }),
            out_dir: plain_dir.clone(),
            mode: VideoMode::Copy,
            encoder: VideoEncoder::Libx264,
        };
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let seg = ClipSegmenter::start(
            &ffmpeg,
            build_clip_args(&options),
            out_dir.clone(),
            Some(dir.path().to_path_buf()),
        );
        let mut handle: Box<dyn ClipHandle> = Box::new(RealClipHandle {
            seg: Arc::new(seg),
            out_dir: out_dir.clone(),
            ffmpeg_path: ffmpeg,
            duration_s: Some(6.0),
            copies_video: false,
            probe: ProbeSummary {
                codec: "h264".into(),
                height: 240,
                fps: 25.0,
                pix_fmt: "yuv420p".into(),
                has_audio: false,
                duration_s: Some(6.0),
            },
            mode: StreamMode::Transcode,
            encoder: Some(VideoEncoder::Libx264),
            ticker: None,
            fallback: Some(fallback),
            produced: false,
            item_id: 1,
            title: "Test".into(),
            log: Some(Arc::new(move |l: Msg| {
                sink.lock().unwrap().push(l.to_string())
            })),
            disposed: false,
            _download: None,
        });
        let segments = run_to_the_end(&mut handle).await;
        assert!(!segments.is_empty());
        assert!(
            segments.iter().all(|s| s.path.starts_with(&plain_dir)),
            "{segments:?}"
        );
        assert!(
            handle.copies_video(),
            "without the ticker the video is copied"
        );
        assert_eq!(handle.stats().mode, Some(StreamMode::Copy));
        let lines = lines.lock().unwrap().clone();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("Video #1 \"Test\" could not start with the info ticker (")
                && lines[0].ends_with("It starts again without the ticker."),
            "{lines:?}"
        );
        assert!(wait_until(|| !out_dir.exists(), Duration::from_secs(10)).await);
        handle.dispose();
        assert!(wait_until(|| !plain_dir.exists(), Duration::from_secs(10)).await);
    }

    /// A failure after the first segment is a normal failure (the channel's retry).
    #[tokio::test(flavor = "multi_thread")]
    async fn after_its_first_segment_a_clip_with_the_ticker_fails_like_any_other() {
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path().join("1-0-1");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(out_dir.join("list.csv"), "seg_00000.ts,0.000000,4.000000\n").unwrap();
        // "ffmpeg" that cannot start: the state is Failed at once.
        let seg = ClipSegmenter::start(
            &dir.path().join("brak-ffmpeg.exe"),
            vec![],
            out_dir.clone(),
            None,
        );
        let mut handle = RealClipHandle {
            seg: Arc::new(seg),
            out_dir: out_dir.clone(),
            ffmpeg_path: dir.path().join("brak-ffmpeg.exe"),
            duration_s: None,
            copies_video: false,
            probe: ProbeSummary {
                codec: "h264".into(),
                height: 240,
                fps: 25.0,
                pix_fmt: "yuv420p".into(),
                has_audio: false,
                duration_s: None,
            },
            mode: StreamMode::Transcode,
            encoder: Some(VideoEncoder::Libx264),
            ticker: None,
            fallback: Some(PlainRestart {
                args: vec![],
                out_dir: dir.path().join("1-0-1-plain"),
                mode: VideoMode::Copy,
                encoder: VideoEncoder::Libx264,
            }),
            produced: false,
            item_id: 1,
            title: "Test".into(),
            log: None,
            disposed: false,
            _download: None,
        };
        assert_eq!(handle.poll().len(), 1);
        assert_eq!(handle.state(), SegmenterState::Failed);
        assert!(handle.error().is_some());
        assert!(!dir.path().join("1-0-1-plain").exists());
    }

    // ---------- Fix round 1 ----------

    /// A clip with the ticker is read at real time, so ffmpeg stays only the initial burst
    /// ahead of what airs and a new text reaches the stream soon; plain clips keep 1.5×.
    #[test]
    fn a_clip_with_the_ticker_is_read_at_real_time() {
        let ticker = |ahead| clip_readrate(false, true, ahead).unwrap();
        // Real time, catching up at 1.5× after a stall; the usual 20 s burst, none for the
        // next film prepared ahead (the prefetch window is its cushion).
        assert_eq!(
            ticker(false),
            ReadRate {
                rate: 1.0,
                initial_burst_s: 20.0,
                catchup: Some(READRATE)
            }
        );
        assert_eq!(
            ticker(true),
            ReadRate {
                rate: 1.0,
                initial_burst_s: 0.0,
                catchup: Some(READRATE)
            }
        );
        for ahead in [false, true] {
            assert_eq!(
                clip_readrate(false, false, ahead),
                Some(ReadRate::new(READRATE))
            );
            assert_eq!(clip_readrate(true, true, ahead), None);
            assert_eq!(clip_readrate(true, false, ahead), None);
        }
    }

    /// With the real ffmpeg: the clip with the ticker is read at the pace it plays after the
    /// 20 s burst (the 40 s film takes at least about 20 s, about 13 s at 1.5×; the catch-up
    /// after a stall never runs ahead of that schedule, a busy computer only makes it longer),
    /// so ffmpeg draws a text at most about the burst ahead of when it airs; and the text
    /// keeps following changes until the end of the film.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_ticker_text_follows_changes_until_the_end_of_a_clip_read_at_real_time() {
        let (Some(ffmpeg), Some(ffprobe)) = (bin("ffmpeg"), bin("ffprobe")) else {
            eprintln!("skip: bin/ffmpeg or bin/ffprobe missing");
            return;
        };
        if !default_font().is_file() {
            eprintln!("skip: no default font");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("film.mp4");
        make_source(&ffmpeg, &source, 40).await;
        let f = fixture(Some(source), dir, ffmpeg, ffprobe);
        enable_ticker(&f, serde_json::json!({ "tickerMode": "continuous" }));
        let started = Instant::now();
        let mut handle = f.factory.create(f.item.clone(), 0.0, 0).await.unwrap();
        let out_dir = f.clips.join(format!("{}-0-1", f.item.id));
        let read = || {
            crate::pipeline::ticker::TickerText::read(&out_dir.join(TEXT_FILE)).unwrap_or_default()
        };
        // When each segment appeared.
        let mut seen: Vec<Instant> = Vec::new();
        let mut changed = false;
        let mut updated_while_running = false;
        let deadline = Instant::now() + Duration::from_secs(120);
        while handle.state() == SegmenterState::Running {
            assert!(Instant::now() < deadline, "the clip did not end");
            let fresh = handle.poll();
            seen.extend(fresh.iter().map(|_| Instant::now()));
            // Six segments out (24 s): the last part of the film, a few seconds before ffmpeg
            // has read the whole source.
            if !changed && seen.len() >= 6 {
                changed = true;
                let conn = f.db.lock().unwrap();
                let patch = crate::db::sanitize_settings_patch(
                    &serde_json::json!({ "tickerMessages": ["Ostatnia chwila"] }),
                )
                .unwrap();
                SettingsRepo::new(&conn)
                    .update(MAIN_CHANNEL_ID, &patch)
                    .unwrap();
            }
            if changed && read().ends_with("Ostatnia chwila") {
                updated_while_running = true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        seen.extend(handle.poll().iter().map(|_| Instant::now()));
        assert_eq!(handle.state(), SegmenterState::Done, "{:?}", handle.error());
        assert!(seen.len() >= 8, "{} segments", seen.len());
        let took = started.elapsed().as_secs_f64();
        assert!(took >= 18.0, "the 40 s film came out in {took:.1} s");
        assert!(
            updated_while_running,
            "the text did not follow a change in the last part of the film"
        );
        handle.dispose();
    }

    /// A clip with the ticker that fails before its first segment for a reason that has
    /// nothing to do with the ticker (the source cannot be opened) fails as before: no restart
    /// without the ticker, no log line about it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failure_unrelated_to_the_ticker_is_reported_as_before() {
        let Some(ffmpeg) = bin("ffmpeg") else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path().join("1-0-1");
        let plain_dir = dir.path().join("1-0-1-plain");
        let options = ClipArgsOptions {
            inputs: vec![StreamInput {
                url: dir.path().join("brak.mp4").to_string_lossy().into_owned(),
                headers: vec![],
            }],
            input_format: None,
            mode: VideoMode::Transcode,
            info: video("h264", 240, 25.0, false),
            encoder: VideoEncoder::Libx264,
            segment_seconds: 4,
            out_dir: out_dir.clone(),
            start_at_s: 0.0,
            readrate: None,
            overlay: Some("drawbox=x=0:y=0:w=10:h=10:t=fill".into()),
            duration_limit_s: None,
        };
        let lines = Arc::new(Mutex::new(Vec::new()));
        let mut handle = test_handle(
            &ffmpeg,
            &options,
            Some(dir.path().to_path_buf()),
            PlainRestart {
                args: build_clip_args(&ClipArgsOptions {
                    mode: VideoMode::Copy,
                    out_dir: plain_dir.clone(),
                    overlay: None,
                    ..options.clone()
                }),
                out_dir: plain_dir.clone(),
                mode: VideoMode::Copy,
                encoder: VideoEncoder::Libx264,
            },
            &lines,
        );
        let ended = wait_until(
            || {
                handle.poll();
                handle.state() != SegmenterState::Running
            },
            Duration::from_secs(30),
        )
        .await;
        assert!(ended);
        assert_eq!(handle.state(), SegmenterState::Failed);
        assert!(handle.error().is_some());
        assert!(!plain_dir.exists());
        assert!(
            lines.lock().unwrap().is_empty(),
            "{:?}",
            lines.lock().unwrap()
        );
        handle.dispose();
    }

    #[test]
    fn only_errors_from_the_ticker_filters_count_as_the_ticker_failing() {
        let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(ticker_failure(&lines(&[
            "[Parsed_drawtext_6 @ 000001] Cannot read file 'ticker.txt': No such file or directory",
            "Error opening output files: No such file or directory",
        ])));
        assert!(ticker_failure(&lines(&[
            "[Parsed_movie_3 @ 0001] Failed to avformat_open_input 'qr.png'"
        ])));
        assert!(!ticker_failure(&lines(&[
            "[in#0 @ 0001] Error opening input: No such file or directory",
            "Error opening input file C:\\movies\\overlay font movie.mp4.",
        ])));
        assert!(!ticker_failure(&lines(&[
            "[https @ 0001] HTTP error 403 Forbidden"
        ])));
    }

    /// A handle as the factory makes it for a clip with the ticker (without its text task).
    fn test_handle(
        ffmpeg: &Path,
        options: &ClipArgsOptions,
        cwd: Option<PathBuf>,
        fallback: PlainRestart,
        lines: &Arc<Mutex<Vec<String>>>,
    ) -> Box<dyn ClipHandle> {
        let sink = Arc::clone(lines);
        let seg = ClipSegmenter::start(
            ffmpeg,
            build_clip_args(options),
            options.out_dir.clone(),
            cwd,
        );
        Box::new(RealClipHandle {
            seg: Arc::new(seg),
            out_dir: options.out_dir.clone(),
            ffmpeg_path: ffmpeg.to_path_buf(),
            duration_s: Some(6.0),
            copies_video: false,
            probe: ProbeSummary {
                codec: "h264".into(),
                height: 240,
                fps: 25.0,
                pix_fmt: "yuv420p".into(),
                has_audio: false,
                duration_s: Some(6.0),
            },
            mode: StreamMode::Transcode,
            encoder: Some(VideoEncoder::Libx264),
            ticker: None,
            fallback: Some(fallback),
            produced: false,
            item_id: 1,
            title: "Test".into(),
            log: Some(Arc::new(move |l: Msg| {
                sink.lock().unwrap().push(l.to_string())
            })),
            disposed: false,
            _download: None,
        })
    }
}
