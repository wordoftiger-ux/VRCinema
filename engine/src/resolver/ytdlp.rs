//! yt-dlp resolver (port of `src/resolver/ytdlp.ts`).

use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use super::{MediaMeta, MediaResolver, PlaylistEntry, ResolveError, ResolvedStream, StreamInput};
use crate::i18n::Msg;
use crate::live::LiveValue;
use crate::process::{ProcessError, ProcessOutput, run_process, run_process_stoppable};
use crate::redact::redact_urls;

/// Prefers H.264 up to 1080p (can be remuxed without transcoding), otherwise anything up to 1080p.
pub const FORMAT_SELECTOR: &str = "bv*[vcodec^=avc1][height<=1080]+ba/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]/bv*+ba/b";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// yt-dlp's YouTube client asked for when the address of the usual one is refused (HTTP 403):
/// since yt-dlp 2026.10.21 its default (TVHTML5) addresses answer 403 to ffprobe and ffmpeg,
/// with or without a JavaScript runtime, while VISIONOS addresses play.
pub const FALLBACK_YOUTUBE_CLIENT: &str = "visionos";

/// The YouTube client a `googlevideo.com` address was made for (its `c=` parameter, `?`
/// when missing); `None` for any other address.
pub fn youtube_client(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "googlevideo.com" && !host.ends_with(".googlevideo.com") {
        return None;
    }
    Some(
        parsed
            .query_pairs()
            .find(|(k, _)| k == "c")
            .map_or_else(|| "?".to_string(), |(_, v)| v.into_owned()),
    )
}
/// The title stored for a video without one (shared with the old server's database); the
/// interface and the ticker show it as `player.untitled` in their language.
pub const NO_TITLE: &str = "Bez tytułu";

/// Runs a program to completion; injectable so tests need no real yt-dlp.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(
        &self,
        cmd: &Path,
        args: &[OsString],
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessError>;

    /// Like [`Self::run`], stopped (with everything it started) when `stop` completes;
    /// `Ok(None)`: stopped. The default drops the run (the child dies with its handle).
    async fn run_stoppable(
        &self,
        cmd: &Path,
        args: &[OsString],
        timeout: Duration,
        stop: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Result<Option<ProcessOutput>, ProcessError> {
        tokio::select! {
            r = self.run(cmd, args, timeout) => r.map(Some),
            () = stop => Ok(None),
        }
    }
}

/// The real thing: [`run_process`].
pub struct RealRunner;

#[async_trait]
impl ProcessRunner for RealRunner {
    async fn run(
        &self,
        cmd: &Path,
        args: &[OsString],
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessError> {
        run_process(cmd, args, timeout).await
    }

    async fn run_stoppable(
        &self,
        cmd: &Path,
        args: &[OsString],
        timeout: Duration,
        stop: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Result<Option<ProcessOutput>, ProcessError> {
        run_process_stoppable(cmd, args, timeout, stop).await
    }
}

/// A finite number greater than zero.
fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0)
}

fn non_empty_title(v: Option<&Value>) -> String {
    match v.and_then(Value::as_str).map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => NO_TITLE.to_string(),
    }
}

/// String-valued entries of a headers object, in their original order.
fn headers(v: Option<&Value>) -> Vec<(String, String)> {
    match v.and_then(Value::as_object) {
        Some(map) => map
            .iter()
            .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
        None => Vec::new(),
    }
}

fn is_live(j: &Value) -> bool {
    j.get("live_status").and_then(Value::as_str) == Some("is_live")
        || j.get("is_live").and_then(Value::as_bool) == Some(true)
}

pub fn parse_ytdlp_meta(j: &Value) -> MediaMeta {
    let title = non_empty_title(j.get("title"));
    if j.get("_type").and_then(Value::as_str) == Some("playlist") {
        let entries = j
            .get("entries")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                let webpage_url = e.get("webpage_url").and_then(Value::as_str);
                let url = webpage_url.or_else(|| e.get("url").and_then(Value::as_str))?;
                Some(PlaylistEntry {
                    url: url.to_string(),
                    title: non_empty_title(e.get("title")),
                    duration_s: num(e.get("duration")),
                    is_live: is_live(e),
                })
            })
            .collect();
        return MediaMeta {
            title,
            duration_s: None,
            is_live: false,
            is_playlist: true,
            entries: Some(entries),
        };
    }
    MediaMeta {
        title,
        duration_s: num(j.get("duration")),
        is_live: is_live(j),
        is_playlist: false,
        entries: None,
    }
}

pub fn parse_ytdlp_resolve(j: &Value) -> Result<ResolvedStream, ResolveError> {
    let no_url = || ResolveError::coded("ytdlp_no_stream");
    let formats = j
        .get("requested_formats")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty());
    let inputs = if let Some(formats) = formats {
        formats
            .iter()
            .map(|f| {
                let url = f.get("url").and_then(Value::as_str).ok_or_else(no_url)?;
                Ok(StreamInput {
                    url: url.to_string(),
                    headers: headers(f.get("http_headers")),
                })
            })
            .collect::<Result<Vec<_>, ResolveError>>()?
    } else if let Some(url) = j.get("url").and_then(Value::as_str) {
        vec![StreamInput {
            url: url.to_string(),
            headers: headers(j.get("http_headers")),
        }]
    } else {
        return Err(no_url());
    };
    Ok(ResolvedStream {
        inputs,
        title: non_empty_title(j.get("title")),
        duration_s: num(j.get("duration")),
        is_live: is_live(j),
    })
}

/// The last `ERROR:` line of yt-dlp's output (else its last line) as the reason.
pub fn ytdlp_error_message(stderr: &str) -> ResolveError {
    let last_error = stderr.lines().rfind(|l| l.starts_with("ERROR:"));
    if let Some(line) = last_error {
        let reason = line["ERROR:".len()..].trim();
        // A site yt-dlp has no extractor for (its generic one found no video either).
        if reason.starts_with("Unsupported URL") {
            return ResolveError::coded("site_not_supported");
        }
        return ResolveError::new(redact_urls(reason));
    }
    match stderr.trim().lines().next_back() {
        Some(line) if !line.is_empty() => ResolveError::new(redact_urls(line)),
        _ => ResolveError::coded("ytdlp_unknown"),
    }
}

/// The largest film yt-dlp downloads to disk ([`MediaResolver::download`], `--max-filesize`).
pub const MAX_DOWNLOAD_SIZE: &str = "200M";

/// How long such a download may take: inside the channel's limit for a start (120 s), which
/// also covers the resolution before it.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(100);

/// The largest film downloaded for the window's Preview ([`MediaResolver::download_preview`]):
/// longer films than the air's own downloads (a larger one plays from the engine's segments).
pub const PREVIEW_MAX_DOWNLOAD_SIZE: &str = "2G";

/// [`PREVIEW_MAX_DOWNLOAD_SIZE`] in bytes, enforced on the files themselves while yt-dlp
/// writes them (`--max-filesize` trusts the size a site announces, which may be missing).
pub const PREVIEW_MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How long a download for the Preview may take (it runs beside the air, not before it).
pub const PREVIEW_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How often the files of a download are measured against its byte limit.
const SIZE_CHECK_EVERY: Duration = Duration::from_millis(500);

/// Waits until the files of the download `stem` grow beyond `max_bytes` together.
async fn grown_beyond(stem: PathBuf, max_bytes: u64) {
    loop {
        tokio::time::sleep(SIZE_CHECK_EVERY).await;
        let total: u64 = files_of(&stem)
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok())
            .map(|m| m.len())
            .sum();
        if total > max_bytes {
            return;
        }
    }
}

/// Removes what a stopped download left, retrying a few times (Windows may hold a lock for a
/// moment after the processes exited). A file that still stays is removed at the next start
/// (the temp folder is emptied then); until then it takes disk space the free-space check of
/// the next download sees.
async fn remove_leftovers(stem: &Path) {
    for attempt in 0..4u32 {
        let left: Vec<PathBuf> = files_of(stem)
            .into_iter()
            .filter(|f| std::fs::remove_file(f).is_err())
            .collect();
        if left.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500) * (attempt + 1)).await;
    }
}

/// yt-dlp's output template for the file `<stem>.<ext>`; a `%` of the path is doubled (yt-dlp
/// would read it as the start of a field).
pub fn output_template(stem: &Path) -> OsString {
    let path = stem.to_string_lossy().replace('%', "%%");
    OsString::from(format!("{path}.%(ext)s"))
}

/// What a download of `stem` left in its folder: the film and any unfinished parts.
pub fn files_of(stem: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (stem.parent(), stem.file_name()) else {
        return Vec::new();
    };
    let prefix = format!("{}.", name.to_string_lossy());
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
        .collect()
}

/// The finished film among the files of `stem`: `<stem>.<ext>`, not a leftover (`.part`,
/// `.ytdl`, a format downloaded for merging such as `<stem>.f137.mp4`).
pub fn finished_download(stem: &Path, files: &[PathBuf]) -> Option<PathBuf> {
    let name = stem.file_name()?.to_string_lossy().into_owned();
    files
        .iter()
        .find(|f| {
            let Some(file) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                return false;
            };
            let Some(ext) = file.strip_prefix(&name).and_then(|r| r.strip_prefix('.')) else {
                return false;
            };
            !ext.is_empty()
                && !ext.contains('.')
                && !["part", "ytdl", "temp", "tmp"].contains(&ext.to_ascii_lowercase().as_str())
        })
        .cloned()
}

/// Why a download left no film: yt-dlp's words about the size limit (it stops with exit code
/// 0), its error, or no stream at all (a live stream is filtered out).
pub fn download_failure(out: &ProcessOutput) -> ResolveError {
    let too_large = out
        .stdout
        .lines()
        .chain(out.stderr.lines())
        .find(|l| l.contains("max-filesize"));
    if let Some(line) = too_large {
        let line = line.trim();
        let line = line.strip_prefix("[download]").unwrap_or(line).trim();
        return ResolveError::new(redact_urls(line));
    }
    if out.code != Some(0) {
        return ytdlp_error_message(&out.stderr);
    }
    ResolveError::coded("ytdlp_no_stream")
}

/// Removes what a download of `stem` left behind, but the film it keeps (when it fails or is
/// cancelled midway: the channel gave up on the start).
struct Leftovers<'a> {
    stem: &'a Path,
    keep: Option<PathBuf>,
}

impl Drop for Leftovers<'_> {
    fn drop(&mut self) {
        for file in files_of(self.stem) {
            if self.keep.as_ref() != Some(&file) {
                let _ = std::fs::remove_file(file);
            }
        }
    }
}

/// Why yt-dlp could not run (or ran too long).
fn process_error(e: ProcessError) -> ResolveError {
    match e {
        ProcessError::Timeout { secs, cmd } => Msg::error("process_timeout")
            .arg("seconds", secs)
            .arg("command", cmd)
            .into(),
        ProcessError::Io(e) => ResolveError::new(e.to_string()),
    }
}

pub struct YtDlpResolver {
    pub path: PathBuf,
    /// Verbatim value of `--js-runtimes` (e.g. `node:C:\...\node.exe` or `deno`).
    pub js_runtime: Option<String>,
    /// Read at every call: the desktop app may change it while the server runs.
    pub cookies: LiveValue<Option<PathBuf>>,
    pub timeout: Duration,
    pub runner: Box<dyn ProcessRunner>,
    /// ffmpeg for merging a download of separate video and audio (`--ffmpeg-location`, given
    /// only when the file exists).
    pub ffmpeg: Option<PathBuf>,
}

impl YtDlpResolver {
    pub fn new(path: PathBuf, js_runtime: Option<String>, cookies: Option<PathBuf>) -> Self {
        Self {
            path,
            js_runtime,
            cookies: LiveValue::new(cookies),
            timeout: DEFAULT_TIMEOUT,
            runner: Box::new(RealRunner),
            ffmpeg: None,
        }
    }

    pub fn with_ffmpeg(mut self, ffmpeg: PathBuf) -> Self {
        self.ffmpeg = Some(ffmpeg);
        self
    }

    /// Shares the cookies setting with the app (see [`crate::live`]).
    pub fn with_cookies(mut self, cookies: LiveValue<Option<PathBuf>>) -> Self {
        self.cookies = cookies;
        self
    }

    pub fn with_runner(mut self, runner: Box<dyn ProcessRunner>) -> Self {
        self.runner = runner;
        self
    }

    /// Full argv: common flags, mode-specific args, `--`, URL (always last).
    fn argv(&self, mode: &[impl AsRef<std::ffi::OsStr>], url: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = ["--no-progress", "--no-warnings", "--socket-timeout", "20"]
            .iter()
            .map(OsString::from)
            .collect();
        if let Some(runtime) = self.runtime() {
            args.push("--js-runtimes".into());
            args.push(runtime.into());
        }
        if let Some(cookies) = self.cookies.get().filter(|c| !c.as_os_str().is_empty()) {
            args.push("--cookies".into());
            args.push(cookies.into());
        }
        args.extend(mode.iter().map(|a| a.as_ref().to_os_string()));
        args.push("--".into());
        args.push(url.into());
        args
    }

    /// The JavaScript runtime given to yt-dlp, if any.
    fn runtime(&self) -> Option<&str> {
        self.js_runtime.as_deref().filter(|r| !r.is_empty())
    }

    async fn exec(&self, mode: &[&str], url: &str) -> Result<Value, ResolveError> {
        let args = self.argv(mode, url);
        let out = self
            .runner
            .run(&self.path, &args, self.timeout)
            .await
            .map_err(process_error)?;
        if out.code != Some(0) {
            return Err(ytdlp_error_message(&out.stderr));
        }
        serde_json::from_str(&out.stdout).map_err(|_| ResolveError::coded("ytdlp_bad_data"))
    }

    /// The arguments of a download into `<stem>.<ext>` (see [`MediaResolver::download`]).
    fn download_argv(&self, url: &str, stem: &Path, max_size: &str) -> Vec<OsString> {
        let mut mode: Vec<OsString> = [
            "--no-playlist",
            "-f",
            FORMAT_SELECTOR,
            "--max-filesize",
            max_size,
            "--match-filter",
            "!is_live",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        if let Some(ffmpeg) = self.ffmpeg.as_ref().filter(|f| f.is_file()) {
            mode.push("--ffmpeg-location".into());
            mode.push(ffmpeg.into());
        }
        mode.push("-o".into());
        mode.push(output_template(stem));
        self.argv(&mode, url)
    }

    async fn download_to(
        &self,
        url: &str,
        stem: &Path,
        max_size: &str,
        timeout: Duration,
        max_bytes: Option<u64>,
    ) -> Result<PathBuf, ResolveError> {
        let args = self.download_argv(url, stem, max_size);
        let mut leftovers = Leftovers { stem, keep: None };
        let out = match max_bytes {
            None => self.runner.run(&self.path, &args, timeout).await,
            // Over the limit (or the time) yt-dlp and what it started are stopped and awaited
            // before their files are removed.
            Some(max) => {
                let stop = Box::pin(grown_beyond(stem.to_path_buf(), max));
                match self
                    .runner
                    .run_stoppable(&self.path, &args, timeout, stop)
                    .await
                {
                    Ok(Some(out)) => Ok(out),
                    Ok(None) => {
                        remove_leftovers(stem).await;
                        return Err(ResolveError::new(format!(
                            "File is larger than max-filesize ({max_size}). Aborting."
                        )));
                    }
                    Err(e) => {
                        remove_leftovers(stem).await;
                        Err(e)
                    }
                }
            }
        }
        .map_err(process_error)?;
        let film = finished_download(stem, &files_of(stem))
            .filter(|f| std::fs::metadata(f).is_ok_and(|m| m.len() > 0));
        match film {
            Some(film) if out.code == Some(0) => {
                leftovers.keep = Some(film.clone());
                Ok(film)
            }
            _ => Err(download_failure(&out)),
        }
    }
}

#[async_trait]
impl MediaResolver for YtDlpResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        Ok(parse_ytdlp_meta(
            &self.exec(&["-J", "--flat-playlist"], url).await?,
        ))
    }

    async fn fetch_meta_capped(
        &self,
        url: &str,
        max_entries: usize,
    ) -> Result<MediaMeta, ResolveError> {
        let end = max_entries.max(1).to_string();
        let mut meta = parse_ytdlp_meta(
            &self
                .exec(&["-J", "--flat-playlist", "--playlist-end", &end], url)
                .await?,
        );
        if let Some(entries) = meta.entries.as_mut() {
            entries.truncate(max_entries);
        }
        Ok(meta)
    }

    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        let json = self
            .exec(&["-j", "--no-playlist", "-f", FORMAT_SELECTOR], url)
            .await?;
        parse_ytdlp_resolve(&json)
    }

    async fn resolve_with_fallback_client(
        &self,
        url: &str,
    ) -> Option<Result<ResolvedStream, ResolveError>> {
        let client = format!("youtube:player_client={FALLBACK_YOUTUBE_CLIENT}");
        let mode = [
            "--extractor-args",
            client.as_str(),
            "-j",
            "--no-playlist",
            "-f",
            FORMAT_SELECTOR,
        ];
        Some(
            self.exec(&mode, url)
                .await
                .and_then(|json| parse_ytdlp_resolve(&json)),
        )
    }

    fn js_runtime_passed(&self, _url: &str) -> Option<bool> {
        Some(self.runtime().is_some())
    }

    async fn download(&self, url: &str, stem: &Path) -> Option<Result<PathBuf, ResolveError>> {
        Some(
            self.download_to(url, stem, MAX_DOWNLOAD_SIZE, DOWNLOAD_TIMEOUT, None)
                .await,
        )
    }

    async fn download_preview(
        &self,
        url: &str,
        stem: &Path,
    ) -> Option<Result<PathBuf, ResolveError>> {
        Some(
            self.download_to(
                url,
                stem,
                PREVIEW_MAX_DOWNLOAD_SIZE,
                PREVIEW_DOWNLOAD_TIMEOUT,
                Some(PREVIEW_MAX_DOWNLOAD_BYTES),
            )
            .await,
        )
    }
}

#[cfg(test)]
mod size_limit_tests {
    use super::*;
    /// "Downloads" by writing ever more bytes into `<stem>.mp4.part` and never ends.
    struct Endless;

    #[async_trait]
    impl ProcessRunner for Endless {
        async fn run(
            &self,
            _cmd: &Path,
            args: &[OsString],
            _timeout: Duration,
        ) -> Result<ProcessOutput, ProcessError> {
            let at = args.iter().position(|a| a == "-o").unwrap();
            let template = args[at + 1].to_string_lossy().into_owned();
            let file = template.replace("%(ext)s", "mp4.part");
            loop {
                let mut bytes = std::fs::read(&file).unwrap_or_default();
                bytes.extend(std::iter::repeat_n(0u8, 4096));
                std::fs::write(&file, bytes).unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    #[tokio::test]
    async fn a_download_growing_beyond_its_byte_limit_is_stopped_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("7-1");
        let mut y = YtDlpResolver::new(PathBuf::from("yt-dlp"), None, None);
        y.runner = Box::new(Endless);
        let started = std::time::Instant::now();
        let result = y
            .download_to(
                "https://x.example/v",
                &stem,
                "2G",
                Duration::from_secs(60),
                Some(10_000),
            )
            .await;
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(files_of(&stem).is_empty(), "the part file was removed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join(name);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn meta_of_a_single_video() {
        assert_eq!(
            parse_ytdlp_meta(&fixture("ytdlp-meta-video.json")),
            MediaMeta {
                title: "Me at the zoo".into(),
                duration_s: Some(19.0),
                is_live: false,
                is_playlist: false,
                entries: None,
            }
        );
    }

    #[test]
    fn meta_of_a_playlist_with_entries() {
        let m = parse_ytdlp_meta(&fixture("ytdlp-meta-playlist.json"));
        assert!(m.is_playlist);
        assert_eq!(m.title, "Popular Music Videos");
        let entries = m.entries.unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0],
            PlaylistEntry {
                url: "https://www.youtube.com/watch?v=fOT0BUpITw8".into(),
                title: "BELLAKEO (Video Oficial) - Peso Pluma, Anitta".into(),
                duration_s: Some(235.0),
                is_live: false,
            }
        );
    }

    #[test]
    fn meta_of_a_live_stream() {
        let m = parse_ytdlp_meta(
            &json!({"_type": "video", "title": "Live", "live_status": "is_live", "is_live": true}),
        );
        assert_eq!(
            m,
            MediaMeta {
                title: "Live".into(),
                duration_s: None,
                is_live: true,
                is_playlist: false,
                entries: None,
            }
        );
    }

    #[test]
    fn missing_title_gets_a_placeholder() {
        let m = parse_ytdlp_meta(&json!({"_type": "video", "duration": 5}));
        assert_eq!(m.title, "Bez tytułu");
        let m = parse_ytdlp_meta(&json!({"title": "   "}));
        assert_eq!(m.title, "Bez tytułu");
    }

    #[test]
    fn resolve_with_separate_video_and_audio_keeps_headers() {
        let r = parse_ytdlp_resolve(&fixture("ytdlp-resolve-split.json")).unwrap();
        assert_eq!(r.inputs.len(), 2);
        assert!(r.inputs[0].url.contains("itag=133"));
        assert!(r.inputs[1].url.contains("itag=251"));
        let ua = r.inputs[0]
            .headers
            .iter()
            .find(|(k, _)| k == "User-Agent")
            .map(|(_, v)| v.as_str());
        assert!(ua.is_some_and(|v| !v.is_empty()));
        assert_eq!(r.title, "Me at the zoo");
        assert_eq!(r.duration_s, Some(19.0));
        assert!(!r.is_live);
    }

    #[test]
    fn resolve_single_file_with_audio_and_video() {
        let r = parse_ytdlp_resolve(&json!({
            "title": "CDA", "duration": 100, "url": "https://cdn/x.mp4",
            "http_headers": {"Referer": "https://cda.pl"}
        }))
        .unwrap();
        assert_eq!(
            r.inputs,
            vec![StreamInput {
                url: "https://cdn/x.mp4".into(),
                headers: vec![("Referer".into(), "https://cda.pl".into())],
            }]
        );
    }

    #[test]
    fn resolve_without_url_is_an_error() {
        let e = parse_ytdlp_resolve(&json!({"title": "x"})).unwrap_err();
        assert_eq!(
            e.0.render(Lang::Pl),
            "yt-dlp nie zwrócił adresu strumienia."
        );
        let e = parse_ytdlp_resolve(&json!({"requested_formats": [{"format_id": "1"}]}));
        assert!(e.is_err());
    }

    #[test]
    fn error_message_extraction() {
        let polish = |stderr: &str| ytdlp_error_message(stderr).0.render(Lang::Pl);
        assert_eq!(
            polish("WARNING: a\nERROR: [youtube] abc: Video unavailable\n"),
            "[youtube] abc: Video unavailable"
        );
        assert_eq!(polish("cos innego"), "cos innego");
        assert_eq!(polish(""), "Nieznany błąd yt-dlp");
        assert_eq!(polish("ERROR: one\r\nERROR: two\r\n"), "two");
    }

    type Calls = Arc<Mutex<Vec<(PathBuf, Vec<String>, Duration)>>>;

    struct FakeRunner {
        calls: Calls,
        reply: Result<ProcessOutput, ProcessError>,
    }

    #[async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(
            &self,
            cmd: &Path,
            args: &[OsString],
            timeout: Duration,
        ) -> Result<ProcessOutput, ProcessError> {
            self.calls.lock().unwrap().push((
                cmd.to_path_buf(),
                args.iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
                timeout,
            ));
            match &self.reply {
                Ok(o) => Ok(o.clone()),
                Err(ProcessError::Timeout { secs, cmd }) => Err(ProcessError::Timeout {
                    secs: *secs,
                    cmd: cmd.clone(),
                }),
                Err(ProcessError::Io(e)) => Err(ProcessError::Io(std::io::Error::new(
                    e.kind(),
                    e.to_string(),
                ))),
            }
        }
    }

    fn ok(stdout: String) -> Result<ProcessOutput, ProcessError> {
        Ok(ProcessOutput {
            code: Some(0),
            stdout,
            stderr: String::new(),
        })
    }

    fn resolver(reply: Result<ProcessOutput, ProcessError>) -> (YtDlpResolver, Calls) {
        let calls = Calls::default();
        let runner = FakeRunner {
            calls: calls.clone(),
            reply,
        };
        let r = YtDlpResolver::new(
            PathBuf::from("yt-dlp"),
            Some("node:/node".into()),
            Some(PathBuf::from("c.txt")),
        )
        .with_runner(Box::new(runner));
        (r, calls)
    }

    #[tokio::test]
    async fn resolve_passes_runtime_cookies_and_format_selector() {
        let (r, calls) = resolver(ok(fixture("ytdlp-resolve-split.json").to_string()));
        r.resolve("https://youtu.be/x").await.unwrap();
        let calls = calls.lock().unwrap();
        let (cmd, args, timeout) = &calls[0];
        assert_eq!(cmd, Path::new("yt-dlp"));
        assert_eq!(*timeout, Duration::from_secs(60));
        let expected: Vec<&str> = vec![
            "--no-progress",
            "--no-warnings",
            "--socket-timeout",
            "20",
            "--js-runtimes",
            "node:/node",
            "--cookies",
            "c.txt",
            "-j",
            "--no-playlist",
            "-f",
            FORMAT_SELECTOR,
            "--",
            "https://youtu.be/x",
        ];
        assert_eq!(
            args.iter().map(String::as_str).collect::<Vec<_>>(),
            expected
        );
    }

    #[tokio::test]
    async fn fetch_meta_args_and_optional_flags_omitted() {
        let calls = Calls::default();
        let runner = FakeRunner {
            calls: calls.clone(),
            reply: ok(fixture("ytdlp-meta-video.json").to_string()),
        };
        let r =
            YtDlpResolver::new(PathBuf::from("yt-dlp"), None, None).with_runner(Box::new(runner));
        let meta = r.fetch_meta("https://youtu.be/x").await.unwrap();
        assert_eq!(meta.title, "Me at the zoo");
        let calls = calls.lock().unwrap();
        let args: Vec<&str> = calls[0].1.iter().map(String::as_str).collect();
        assert_eq!(
            args,
            vec![
                "--no-progress",
                "--no-warnings",
                "--socket-timeout",
                "20",
                "-J",
                "--flat-playlist",
                "--",
                "https://youtu.be/x"
            ]
        );
    }

    #[tokio::test]
    async fn a_capped_fetch_asks_ytdlp_for_the_first_entries_only() {
        let calls = Calls::default();
        let runner = FakeRunner {
            calls: calls.clone(),
            reply: ok(fixture("ytdlp-meta-playlist.json").to_string()),
        };
        let r =
            YtDlpResolver::new(PathBuf::from("yt-dlp"), None, None).with_runner(Box::new(runner));
        let meta = r
            .fetch_meta_capped("https://youtube.com/playlist?list=x", 2)
            .await
            .unwrap();
        assert_eq!(meta.entries.unwrap().len(), 2, "kept to the cap");
        let calls = calls.lock().unwrap();
        let args: Vec<&str> = calls[0].1.iter().map(String::as_str).collect();
        let at = args.iter().position(|a| *a == "--playlist-end").unwrap();
        assert_eq!(args[at + 1], "2");
        assert!(args.contains(&"--flat-playlist"));
        assert_eq!(
            args[args.len() - 2..],
            ["--", "https://youtube.com/playlist?list=x"]
        );
    }

    #[test]
    fn a_live_entry_of_a_flat_playlist_is_marked_live() {
        let m = parse_ytdlp_meta(&json!({"_type": "playlist", "title": "P", "entries": [
            {"url": "https://www.youtube.com/watch?v=a", "title": "A", "live_status": "is_live"},
            {"url": "https://www.youtube.com/watch?v=b", "title": "B"}
        ]}));
        let e = m.entries.unwrap();
        assert!(e[0].is_live);
        assert!(!e[1].is_live);
    }

    #[tokio::test]
    async fn ytdlp_failure_becomes_a_readable_error() {
        let (r, _) = resolver(Ok(ProcessOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: "ERROR: Unsupported URL: https://x".into(),
        }));
        let e = r.fetch_meta("https://x").await.unwrap_err();
        assert_eq!(e.0.render(Lang::Pl), "Ta strona nie jest obsługiwana.");
        assert_eq!(e.0.render(Lang::En), "This site is not supported.");
        assert_eq!(e.0.code(), Some("site_not_supported"));
        // Other messages of yt-dlp stay as they are.
        let e = ytdlp_error_message("ERROR: Video unavailable");
        assert_eq!(e.0.render(Lang::Pl), "Video unavailable");
    }

    #[tokio::test]
    async fn invalid_json_and_timeout_are_errors() {
        let (r, _) = resolver(ok("nie json".into()));
        let e = r.fetch_meta("https://x").await.unwrap_err();
        assert_eq!(e.0.render(Lang::Pl), "yt-dlp zwrócił niepoprawne dane.");

        let (r, _) = resolver(Err(ProcessError::Timeout {
            secs: 60,
            cmd: "yt-dlp".into(),
        }));
        let e = r.resolve("https://x").await.unwrap_err();
        assert_eq!(
            e.0.render(Lang::Pl),
            "Przekroczono limit czasu (60 s): yt-dlp"
        );
    }

    #[test]
    fn resolver_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let r = YtDlpResolver::new(PathBuf::from("yt-dlp"), None, None);
        assert_send(&r.fetch_meta("https://x"));
        assert_send(&r.resolve("https://x"));
    }

    #[tokio::test]
    async fn the_fallback_client_adds_only_the_extractor_args() {
        let (r, calls) = resolver(ok(fixture("ytdlp-resolve-split.json").to_string()));
        assert_eq!(r.js_runtime_passed("https://youtu.be/x"), Some(true));
        r.resolve("https://youtu.be/x").await.unwrap();
        r.resolve_with_fallback_client("https://youtu.be/x")
            .await
            .expect("yt-dlp has a fallback")
            .unwrap();
        let calls = calls.lock().unwrap();
        let normal = &calls[0].1;
        assert!(!normal.iter().any(|a| a == "--extractor-args"));
        let fallback = &calls[1].1;
        let i = fallback
            .iter()
            .position(|a| a == "--extractor-args")
            .unwrap();
        assert_eq!(fallback[i + 1], "youtube:player_client=visionos");
        let mut without: Vec<String> = fallback.clone();
        without.drain(i..i + 2);
        assert_eq!(
            &without, normal,
            "everything else as in a normal resolution"
        );
        assert_eq!(fallback.last().unwrap(), "https://youtu.be/x");

        let bare = YtDlpResolver::new(PathBuf::from("yt-dlp"), Some(String::new()), None);
        assert_eq!(bare.js_runtime_passed("https://youtu.be/x"), Some(false));
    }

    #[test]
    fn the_client_of_a_youtube_address() {
        assert_eq!(
            youtube_client(
                "https://rr7---sn-u2oxu.googlevideo.com/videoplayback?ip=1.2.3.4&c=TVHTML5&sig=x"
            )
            .as_deref(),
            Some("TVHTML5")
        );
        assert_eq!(
            youtube_client("https://rr1.GOOGLEVIDEO.com/videoplayback?itag=1").as_deref(),
            Some("?")
        );
        assert_eq!(youtube_client("https://cdn.example/v.mp4?c=WEB"), None);
        assert_eq!(youtube_client("https://evilgooglevideo.com/v?c=WEB"), None);
        assert_eq!(youtube_client("C:\\film.mp4"), None);
    }

    #[test]
    fn ytdlp_errors_lose_the_signed_parts_of_addresses() {
        let e = ytdlp_error_message(
            "ERROR: unable to download https://rr1.googlevideo.com/videoplayback?ip=188.47.115.50&sig=abc: HTTP Error 403\n",
        );
        assert_eq!(
            e.0.render(Lang::En),
            "unable to download https://rr1.googlevideo.com/videoplayback: HTTP Error 403"
        );
        let e = ytdlp_error_message("see https://x.example/a/b?token=1");
        assert_eq!(e.0.render(Lang::En), "see https://x.example/a");
    }

    #[test]
    fn the_output_template_names_the_stem_and_escapes_percent_signs() {
        assert_eq!(
            output_template(Path::new("data/temp/7-1")),
            OsString::from("data/temp/7-1.%(ext)s")
        );
        assert_eq!(
            output_template(Path::new("C:\\100%\\7-1")),
            OsString::from("C:\\100%%\\7-1.%(ext)s")
        );
    }

    #[test]
    fn only_the_finished_film_counts_as_the_download() {
        let stem = Path::new("t/7-1");
        let files = |names: &[&str]| -> Vec<PathBuf> {
            names.iter().map(|n| Path::new("t").join(n)).collect()
        };
        assert_eq!(
            finished_download(stem, &files(&["7-1.mp4.part", "7-1.mp4"])),
            Some(Path::new("t").join("7-1.mp4"))
        );
        for leftovers in [
            &["7-1.mp4.part"][..],
            &["7-1.f137.mp4", "7-1.f251.webm"],
            &["7-1.part", "7-1.ytdl", "7-1."],
            &["7-10.mp4", "17-1.mp4"],
        ] {
            assert_eq!(
                finished_download(stem, &files(leftovers)),
                None,
                "{leftovers:?}"
            );
        }
    }

    #[test]
    fn a_download_without_a_film_says_why() {
        let out = |code: i32, stdout: &str, stderr: &str| ProcessOutput {
            code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
        };
        let too_large = download_failure(&out(
            0,
            "[TikTok] 1: Downloading webpage\n[download] File is larger than max-filesize (9449815 bytes > 209715200 bytes). Aborting.\n",
            "",
        ));
        assert_eq!(
            too_large.0.render(Lang::En),
            "File is larger than max-filesize (9449815 bytes > 209715200 bytes). Aborting."
        );
        let refused = download_failure(&out(
            1,
            "",
            "ERROR: unable to download https://v16.tiktokcdn.com/a/b?sig=x: HTTP Error 403\n",
        ));
        assert_eq!(
            refused.0.render(Lang::En),
            "unable to download https://v16.tiktokcdn.com/a: HTTP Error 403"
        );
        // A live stream fails the `!is_live` filter: yt-dlp skips it and ends with 0.
        let skipped = download_failure(&out(0, "[download] x does not pass filter (!is_live)", ""));
        assert_eq!(skipped.0.code(), Some("ytdlp_no_stream"));
    }

    #[tokio::test]
    async fn a_download_keeps_the_film_and_removes_the_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("7-1");
        // What yt-dlp would have written (the fake runner writes nothing).
        std::fs::write(dir.path().join("7-1.mp4"), b"film").unwrap();
        std::fs::write(dir.path().join("7-1.f251.webm.part"), b"x").unwrap();
        std::fs::write(dir.path().join("8-1.mp4"), b"other").unwrap();
        let (r, calls) = resolver(ok(String::new()));
        let film = r
            .download("https://www.tiktok.com/@a/video/1", &stem)
            .await
            .expect("yt-dlp downloads")
            .unwrap();
        assert_eq!(film, dir.path().join("7-1.mp4"));
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["7-1.mp4", "8-1.mp4"]);

        let calls = calls.lock().unwrap();
        let (_, args, timeout) = &calls[0];
        assert_eq!(*timeout, DOWNLOAD_TIMEOUT);
        let after = |flag: &str| {
            let i = args.iter().position(|a| a == flag).unwrap();
            args[i + 1].clone()
        };
        assert_eq!(after("--max-filesize"), "200M");
        assert_eq!(after("--match-filter"), "!is_live");
        assert_eq!(after("-f"), FORMAT_SELECTOR);
        assert_eq!(after("--js-runtimes"), "node:/node");
        assert_eq!(after("--cookies"), "c.txt");
        assert_eq!(
            after("-o"),
            output_template(&stem).to_string_lossy().into_owned()
        );
        assert!(
            !args.iter().any(|a| a == "--ffmpeg-location"),
            "no such file"
        );
        assert_eq!(
            &args[args.len() - 2..],
            ["--", "https://www.tiktok.com/@a/video/1"]
        );
    }

    #[tokio::test]
    async fn a_failed_download_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("7-2");
        std::fs::write(dir.path().join("7-2.mp4.part"), b"x").unwrap();
        let (r, _) = resolver(Ok(ProcessOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: "ERROR: [TikTok] 1: Video not available".into(),
        }));
        let e = r.download("https://x", &stem).await.unwrap().unwrap_err();
        assert_eq!(e.0.render(Lang::En), "[TikTok] 1: Video not available");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        // An empty file is no film either.
        std::fs::write(dir.path().join("7-2.mp4"), b"").unwrap();
        let (r, _) = resolver(ok(String::new()));
        let e = r.download("https://x", &stem).await.unwrap().unwrap_err();
        assert_eq!(e.0.code(), Some("ytdlp_no_stream"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn live_cookies_are_read_at_each_call() {
        let cookies = crate::live::LiveValue::new(None);
        let (r, calls) = resolver(ok(fixture("ytdlp-resolve-split.json").to_string()));
        let r = r.with_cookies(cookies.clone());
        r.resolve("https://youtu.be/x").await.unwrap();
        cookies.set(Some(PathBuf::from("nowe.txt")));
        r.resolve("https://youtu.be/x").await.unwrap();
        let calls = calls.lock().unwrap();
        assert!(!calls[0].1.iter().any(|a| a == "--cookies"));
        let i = calls[1].1.iter().position(|a| a == "--cookies").unwrap();
        assert_eq!(calls[1].1[i + 1], "nowe.txt");
    }
}
