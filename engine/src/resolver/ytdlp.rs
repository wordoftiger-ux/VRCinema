//! yt-dlp resolver (port of `src/resolver/ytdlp.ts`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use super::{MediaMeta, MediaResolver, PlaylistEntry, ResolveError, ResolvedStream, StreamInput};
use crate::i18n::Msg;
use crate::live::LiveValue;
use crate::process::{ProcessError, ProcessOutput, run_process};
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
}

impl YtDlpResolver {
    pub fn new(path: PathBuf, js_runtime: Option<String>, cookies: Option<PathBuf>) -> Self {
        Self {
            path,
            js_runtime,
            cookies: LiveValue::new(cookies),
            timeout: DEFAULT_TIMEOUT,
            runner: Box::new(RealRunner),
        }
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
    fn argv(&self, mode: &[&str], url: &str) -> Vec<OsString> {
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
        args.extend(mode.iter().map(OsString::from));
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
}

#[async_trait]
impl MediaResolver for YtDlpResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        Ok(parse_ytdlp_meta(
            &self.exec(&["-J", "--flat-playlist"], url).await?,
        ))
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
