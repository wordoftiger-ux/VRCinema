//! ffprobe analysis of a source. Port of `src/pipeline/probe.ts`.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::i18n::Msg;
use crate::pipeline::ffmpeg_args::input_options;
use crate::process::{ProcessError, run_process};
use crate::resolver::types::StreamInput;

#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub codec: String,
    pub height: u32,
    pub pix_fmt: String,
    pub fps: f64,
    pub has_audio: bool,
    pub duration_s: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoMode {
    Copy,
    Transcode,
}

/// Why a source could not be analysed; [`ProbeError::message`] says it to a person (`Display`:
/// in English).
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("{}", Msg::new(NO_VIDEO))]
    NoVideoStream,
    /// ffprobe's last words (empty: it said nothing).
    #[error("{}", failed(.0))]
    Failed(String),
    #[error("{}", .0.message())]
    Process(#[from] ProcessError),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

const NO_VIDEO: &str = "item_errors.no_video";

fn failed(reason: &str) -> Msg {
    if reason.is_empty() {
        Msg::new("item_errors.probe_failed")
    } else {
        Msg::error("source_message").arg("detail", format!("ffprobe: {reason}"))
    }
}

impl ProbeError {
    /// Why, for a person (kept with a film that failed).
    pub fn message(&self) -> Msg {
        match self {
            Self::NoVideoStream => Msg::new(NO_VIDEO),
            Self::Failed(reason) => failed(reason),
            Self::Process(e) => e.message(),
            Self::Json(e) => Msg::error("source_message").arg("detail", e),
        }
    }
}

const COPYABLE_PIX_FMTS: [&str; 2] = ["yuv420p", "yuvj420p"];

const PROBE_TIMEOUT: Duration = Duration::from_secs(45);
const DURATION_TIMEOUT: Duration = Duration::from_secs(30);

pub fn decide_video_mode(info: &VideoInfo, force_transcode: bool) -> VideoMode {
    if force_transcode {
        return VideoMode::Transcode;
    }
    if info.codec == "h264"
        && info.height > 0
        && info.height <= 1080
        && COPYABLE_PIX_FMTS.contains(&info.pix_fmt.as_str())
    {
        VideoMode::Copy
    } else {
        VideoMode::Transcode
    }
}

/// JavaScript `Number(value)` for the JSON values ffprobe produces (NaN when not numeric).
fn js_number(value: Option<&Value>) -> f64 {
    match value {
        None | Some(Value::Null) => f64::NAN,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_str_number(s),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(_) => f64::NAN,
    }
}

/// JavaScript `Number(string)`: blank is 0, non-numeric is NaN.
fn js_str_number(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        0.0
    } else {
        s.parse::<f64>().unwrap_or(f64::NAN)
    }
}

/// `"30000/1001"` to fps rounded to 2 decimals; 0 when missing or invalid.
fn parse_rate(rate: Option<&Value>) -> f64 {
    let Some(Value::String(rate)) = rate else {
        return 0.0;
    };
    let mut parts = rate.split('/');
    let n = parts.next().map_or(f64::NAN, js_str_number);
    let d = parts.next().map_or(f64::NAN, js_str_number);
    // `!n || !d` in JS: zero and NaN both count as missing.
    if n.is_nan() || d.is_nan() || n == 0.0 || d == 0.0 {
        return 0.0;
    }
    let r = (n / d * 100.0).round() / 100.0;
    if r.is_finite() { r } else { 0.0 }
}

pub fn parse_probe_output(j: &Value) -> Result<VideoInfo, ProbeError> {
    let streams: &[Value] = j
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let is_type = |s: &Value, t: &str| s.get("codec_type").and_then(Value::as_str) == Some(t);
    let video = streams
        .iter()
        .find(|s| {
            is_type(s, "video")
                && !s
                    .get("disposition")
                    .and_then(|d| d.get("attached_pic"))
                    .is_some_and(js_truthy)
        })
        .ok_or(ProbeError::NoVideoStream)?;

    let duration = js_number(j.get("format").and_then(|f| f.get("duration")));
    let height = js_number(video.get("height"));
    let text = |key: &str| {
        video
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let avg = parse_rate(video.get("avg_frame_rate"));
    Ok(VideoInfo {
        codec: text("codec_name"),
        // Saturating float-to-int cast: NaN and negatives become 0.
        height: height as u32,
        pix_fmt: text("pix_fmt"),
        fps: if avg != 0.0 {
            avg
        } else {
            parse_rate(video.get("r_frame_rate"))
        },
        has_audio: streams.iter().any(|s| is_type(s, "audio")),
        duration_s: (duration.is_finite() && duration > 0.0).then_some(duration),
    })
}

/// JavaScript truthiness of a JSON value.
fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

async fn probe_with_timeout(
    ffprobe: &Path,
    inputs: &[StreamInput],
    timeout: Duration,
) -> Result<VideoInfo, ProbeError> {
    let video = inputs
        .first()
        .ok_or_else(|| ProbeError::Failed(String::new()))?;
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_streams",
        "-show_format",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.extend(input_options(video));
    args.push("-i".into());
    args.push((&video.url).into());

    let res = run_process(ffprobe, &args, timeout).await?;
    if res.code != Some(0) {
        let last = res
            .stderr
            .trim()
            .lines()
            .next_back()
            .filter(|l| !l.is_empty())
            .unwrap_or_default();
        return Err(ProbeError::Failed(last.to_string()));
    }
    let info = parse_probe_output(&serde_json::from_str(&res.stdout)?)?;
    Ok(if inputs.len() > 1 {
        VideoInfo {
            has_audio: true,
            ..info
        }
    } else {
        info
    })
}

/// Analyses the source with ffprobe. When audio arrives as a separate input, `has_audio` = true.
pub async fn probe_video(ffprobe: &Path, inputs: &[StreamInput]) -> Result<VideoInfo, ProbeError> {
    probe_with_timeout(ffprobe, inputs, PROBE_TIMEOUT).await
}

/// File duration in seconds, `None` on any error.
pub async fn probe_duration(
    ffprobe: &Path,
    url: &str,
    headers: &[(String, String)],
) -> Option<f64> {
    let input = StreamInput {
        url: url.to_string(),
        headers: headers.to_vec(),
    };
    probe_with_timeout(ffprobe, &[input], DURATION_TIMEOUT)
        .await
        .ok()?
        .duration_s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn h264() -> VideoInfo {
        VideoInfo {
            codec: "h264".into(),
            height: 1080,
            pix_fmt: "yuv420p".into(),
            fps: 30.0,
            has_audio: true,
            duration_s: Some(60.0),
        }
    }

    #[test]
    fn remuxes_h264_up_to_1080p_yuv420p() {
        assert_eq!(decide_video_mode(&h264(), false), VideoMode::Copy);
        let mut full_range = h264();
        full_range.pix_fmt = "yuvj420p".into();
        assert_eq!(decide_video_mode(&full_range, false), VideoMode::Copy);
    }

    #[test]
    fn transcodes_other_codecs_resolutions_and_pixel_formats() {
        let mut a = h264();
        a.codec = "vp9".into();
        assert_eq!(decide_video_mode(&a, false), VideoMode::Transcode);
        let mut b = h264();
        b.height = 1440;
        assert_eq!(decide_video_mode(&b, false), VideoMode::Transcode);
        let mut c = h264();
        c.pix_fmt = "yuv420p10le".into();
        assert_eq!(decide_video_mode(&c, false), VideoMode::Transcode);
        let mut d = h264();
        d.height = 0;
        assert_eq!(decide_video_mode(&d, false), VideoMode::Transcode);
    }

    #[test]
    fn force_transcode_forces() {
        assert_eq!(decide_video_mode(&h264(), true), VideoMode::Transcode);
    }

    #[test]
    fn parses_video_audio_fps_and_duration() {
        let info = parse_probe_output(&json!({
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "height": 720, "pix_fmt": "yuv420p", "avg_frame_rate": "30000/1001"},
                {"codec_type": "audio", "codec_name": "aac"}
            ],
            "format": {"duration": "123.45"}
        }))
        .unwrap();
        assert_eq!(
            info,
            VideoInfo {
                codec: "h264".into(),
                height: 720,
                pix_fmt: "yuv420p".into(),
                fps: 29.97,
                has_audio: true,
                duration_s: Some(123.45),
            }
        );
    }

    #[test]
    fn errors_when_there_is_no_video() {
        let err = parse_probe_output(&json!({"streams": [{"codec_type": "audio"}], "format": {}}))
            .unwrap_err();
        assert_eq!(
            err.message().render(crate::i18n::Lang::Pl),
            "Źródło nie zawiera ścieżki wideo."
        );
        assert_eq!(err.to_string(), "The source has no video track.");
    }

    #[test]
    fn failures_say_why_in_the_readers_language() {
        use crate::i18n::Lang;
        let silent = ProbeError::Failed(String::new());
        assert_eq!(
            silent.message().render(Lang::Pl),
            "ffprobe: nie udało się odczytać źródła"
        );
        assert_eq!(silent.to_string(), "ffprobe: could not read the source");
        let said = ProbeError::Failed("Invalid data found when processing input".into());
        assert_eq!(
            said.message().render(Lang::Pl),
            "ffprobe: Invalid data found when processing input"
        );
        let slow = ProbeError::Process(ProcessError::Timeout {
            secs: 45,
            cmd: "ffprobe".into(),
        });
        assert_eq!(
            slow.message().render(Lang::Pl),
            "Przekroczono limit czasu (45 s): ffprobe"
        );
        assert_eq!(slow.to_string(), "Time limit exceeded (45 s): ffprobe");
    }

    #[test]
    fn ignores_attached_pictures() {
        let err = parse_probe_output(&json!({
            "streams": [{"codec_type": "video", "codec_name": "mjpeg", "disposition": {"attached_pic": 1}}]
        }))
        .unwrap_err();
        assert!(matches!(err, ProbeError::NoVideoStream));

        let info = parse_probe_output(&json!({
            "streams": [
                {"codec_type": "video", "codec_name": "mjpeg", "disposition": {"attached_pic": 1}},
                {"codec_type": "video", "codec_name": "h264", "height": 480, "disposition": {"attached_pic": 0}}
            ]
        }))
        .unwrap();
        assert_eq!(info.codec, "h264");
        assert_eq!(info.height, 480);
    }

    #[test]
    fn fps_falls_back_to_r_frame_rate_and_zero_rates_are_ignored() {
        let info = parse_probe_output(&json!({
            "streams": [{"codec_type": "video", "avg_frame_rate": "0/0", "r_frame_rate": "25/1"}]
        }))
        .unwrap();
        assert_eq!(info.fps, 25.0);
        let none = parse_probe_output(&json!({
            "streams": [{"codec_type": "video"}]
        }))
        .unwrap();
        assert_eq!(none.fps, 0.0);
        assert_eq!(none.codec, "");
        assert_eq!(none.height, 0);
        assert_eq!(none.pix_fmt, "");
        assert!(!none.has_audio);
        assert_eq!(none.duration_s, None);
    }

    #[test]
    fn duration_must_be_positive_and_finite() {
        let mk = |d: Value| {
            parse_probe_output(&json!({
                "streams": [{"codec_type": "video"}],
                "format": {"duration": d}
            }))
            .unwrap()
            .duration_s
        };
        assert_eq!(mk(json!("0")), None);
        assert_eq!(mk(json!("N/A")), None);
        assert_eq!(mk(json!(12.5)), Some(12.5));
        assert_eq!(mk(Value::Null), None);
    }

    // ---- real ffmpeg / ffprobe ----

    fn bin(name: &str) -> Option<std::path::PathBuf> {
        let exe = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        };
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("bin")
            .join(exe);
        p.exists().then_some(p)
    }

    #[tokio::test]
    async fn probes_a_real_generated_clip() {
        let (Some(ffmpeg), Some(ffprobe)) = (bin("ffmpeg"), bin("ffprobe")) else {
            eprintln!("skipping: bin/ffmpeg or bin/ffprobe missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("zażółć.mp4");
        let gen_args: Vec<std::ffi::OsString> = [
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x120:rate=25:duration=1",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=channel_layout=stereo:sample_rate=48000",
            "-t",
            "1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-y",
        ]
        .iter()
        .map(Into::into)
        .chain([file.clone().into_os_string()])
        .collect();
        let out = run_process(&ffmpeg, &gen_args, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(out.code, Some(0), "{}", out.stderr);

        let input = StreamInput {
            url: file.to_string_lossy().into_owned(),
            headers: vec![],
        };
        let info = probe_video(&ffprobe, std::slice::from_ref(&input))
            .await
            .unwrap();
        assert_eq!(info.codec, "h264");
        assert_eq!(info.height, 120);
        assert_eq!(info.pix_fmt, "yuv420p");
        assert_eq!(info.fps, 25.0);
        assert!(info.has_audio);
        assert!(info.duration_s.unwrap() > 0.5);

        // separate audio input forces has_audio
        let two = [input.clone(), input.clone()];
        assert!(probe_video(&ffprobe, &two).await.unwrap().has_audio);

        let d = probe_duration(&ffprobe, &input.url, &[]).await.unwrap();
        assert!(d > 0.5 && d < 2.0, "{d}");
    }

    #[tokio::test]
    async fn failing_source_reports_last_stderr_line_and_duration_is_none() {
        let Some(ffprobe) = bin("ffprobe") else {
            eprintln!("skipping: bin/ffprobe missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nie-ma.mp4");
        let input = StreamInput {
            url: missing.to_string_lossy().into_owned(),
            headers: vec![],
        };
        let err = probe_video(&ffprobe, std::slice::from_ref(&input))
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("ffprobe: "), "{err}");
        assert!(matches!(err, ProbeError::Failed(_)));
        assert_eq!(probe_duration(&ffprobe, &input.url, &[]).await, None);
    }

    #[tokio::test]
    async fn missing_ffprobe_binary_gives_none_for_duration() {
        let p = Path::new("definitely-not-ffprobe-xyz");
        assert_eq!(probe_duration(p, "x", &[]).await, None);
        assert!(probe_video(p, &[]).await.is_err());
    }
}
