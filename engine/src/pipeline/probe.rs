//! ffprobe analysis of a source. Port of `src/pipeline/probe.ts`.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::i18n::Msg;
use crate::pipeline::ffmpeg_args::{MAX_FPS, input_options};
use crate::process::{ProcessError, run_process};
use crate::redact::redact_urls;
use crate::resolver::types::StreamInput;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoInfo {
    pub codec: String,
    pub height: u32,
    pub pix_fmt: String,
    pub fps: f64,
    pub has_audio: bool,
    pub duration_s: Option<f64>,
    /// Taller than wide as shown (rotation included), e.g. a 9:16 phone video: it is
    /// converted and letterboxed into a 16:9 picture.
    pub portrait: bool,
    /// The H.264 profile as ffprobe names it (`High`, `Main`, ...); empty: not known.
    pub profile: String,
    /// The codec level (H.264: 41 = 4.1); 0 or less: not known.
    pub level: i64,
    /// The frames are interlaced (fields): converted and deinterlaced.
    pub interlaced: bool,
    /// HDR (PQ or HLG transfer): converted and tonemapped to SDR.
    pub hdr: bool,
    /// The frame rate is not constant (average and real rates differ): converted.
    pub vfr: bool,
    /// The colour matrix (`bt709`, `smpte170m`, ...); empty: not tagged.
    pub colorspace: String,
    /// The longest stretch without a key frame in the first [`KEYFRAME_SAMPLE_S`] seconds of
    /// the video; `None`: not measured. A copied video can only be cut at its key frames.
    pub keyframe_gap_s: Option<f64>,
    /// An audio-only source (no video stream; an attached cover picture is not one): what its
    /// tags say. `None`: a video.
    pub audio: Option<AudioTags>,
}

/// The tags of an audio-only source (empty: not tagged).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The stream index of the attached cover picture, if the file has one.
    pub cover_stream: Option<i64>,
}

/// Seconds of the video whose packets are read to measure the key frame spacing.
pub const KEYFRAME_SAMPLE_S: u32 = 6;

/// The longest key frame gap a copied video may have: its segments are then about
/// [`crate::pipeline::SEGMENT_S`] long (25 % slack for 24/25/29.97 fps groups of pictures).
pub const MAX_COPY_KEYFRAME_GAP_S: f64 = crate::pipeline::SEGMENT_S as f64 * 1.25;

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

    /// The server refused the address (HTTP 403 or 401), as ffprobe reports it.
    pub fn access_denied(&self) -> bool {
        match self {
            Self::Failed(reason) => is_access_denied(reason),
            _ => false,
        }
    }
}

/// ffmpeg's and ffprobe's words for an HTTP 403 or 401 answer.
pub fn is_access_denied(text: &str) -> bool {
    [
        "403 Forbidden",
        "401 Unauthorized",
        "HTTP error 403",
        "HTTP error 401",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

const COPYABLE_PIX_FMTS: [&str; 2] = ["yuv420p", "yuvj420p"];

const PROBE_TIMEOUT: Duration = Duration::from_secs(45);
const DURATION_TIMEOUT: Duration = Duration::from_secs(30);

/// H.264 profiles every player decodes; another one (High 10, 4:2:2, ...) is converted.
const COPYABLE_PROFILES: [&str; 4] = ["Baseline", "Constrained Baseline", "Main", "High"];
/// Highest H.264 level copied (4.2: 1080p at up to 60 frames per second).
const MAX_COPY_LEVEL: i64 = 42;
/// Highest frame rate copied (the converted picture has at most [`MAX_FPS`] frames a second).
const MAX_COPY_FPS: f64 = MAX_FPS as f64 + 0.5;

/// Copy only a source that already is what a converted film would be (one look across films):
/// H.264 in a common profile and level, 8-bit 4:2:0, at most 1080 lines and 30 frames a second
/// at a constant rate, progressive, SDR, BT.709 or untagged. Anything else is converted.
pub fn decide_video_mode(info: &VideoInfo, force_transcode: bool) -> VideoMode {
    if force_transcode || info.portrait {
        return VideoMode::Transcode;
    }
    let uniform = info.codec == "h264"
        && info.height > 0
        && info.height <= 1080
        && COPYABLE_PIX_FMTS.contains(&info.pix_fmt.as_str())
        && info.fps <= MAX_COPY_FPS
        && !info.vfr
        && !info.interlaced
        && !info.hdr
        && (info.profile.is_empty() || COPYABLE_PROFILES.contains(&info.profile.as_str()))
        && info.level <= MAX_COPY_LEVEL
        && matches!(info.colorspace.as_str(), "" | "unknown" | "bt709");
    // Key frames far apart (or not measured) would cut segments longer than one.
    let cuttable = info
        .keyframe_gap_s
        .is_some_and(|gap| gap <= MAX_COPY_KEYFRAME_GAP_S);
    if uniform && cuttable {
        VideoMode::Copy
    } else {
        VideoMode::Transcode
    }
}

/// ffprobe's field order of an interlaced stream.
fn is_interlaced(field_order: &str) -> bool {
    matches!(field_order, "tt" | "bb" | "tb" | "bt")
}

/// ffprobe's transfer of an HDR stream (PQ, HLG).
fn is_hdr(color_transfer: &str) -> bool {
    matches!(color_transfer, "smpte2084" | "arib-std-b67")
}

/// The average and the real frame rate differ by more than 1 %: a variable frame rate. Either
/// unknown: taken as constant.
fn is_vfr(avg: f64, real: f64) -> bool {
    avg > 0.0 && real > 0.0 && (avg - real).abs() / real > 0.01
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
    let is_cover = |s: &Value| {
        s.get("disposition")
            .and_then(|d| d.get("attached_pic"))
            .is_some_and(js_truthy)
    };
    let Some(video) = streams.iter().find(|s| is_type(s, "video") && !is_cover(s)) else {
        return audio_only(j, streams);
    };

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
    let real = parse_rate(video.get("r_frame_rate"));
    let width = js_number(video.get("width"));
    let level = js_number(video.get("level"));
    Ok(VideoInfo {
        profile: text("profile"),
        level: if level.is_finite() { level as i64 } else { 0 },
        interlaced: is_interlaced(&text("field_order")),
        hdr: is_hdr(&text("color_transfer")),
        vfr: is_vfr(avg, real),
        colorspace: text("color_space"),
        portrait: is_portrait(width, height, rotation(video)),
        codec: text("codec_name"),
        // Saturating float-to-int cast: NaN and negatives become 0.
        height: height as u32,
        pix_fmt: text("pix_fmt"),
        fps: if avg != 0.0 { avg } else { real },
        has_audio: streams.iter().any(|s| is_type(s, "audio")),
        duration_s: (duration.is_finite() && duration > 0.0).then_some(duration),
        keyframe_gap_s: keyframe_gap(j, video),
        audio: None,
    })
}

/// A source without a video stream: audio-only when it has an audio stream (its picture is
/// made by the clip factory), else [`ProbeError::NoVideoStream`].
fn audio_only(j: &Value, streams: &[Value]) -> Result<VideoInfo, ProbeError> {
    let is_type = |s: &Value, t: &str| s.get("codec_type").and_then(Value::as_str) == Some(t);
    let audio = streams
        .iter()
        .find(|s| is_type(s, "audio"))
        .ok_or(ProbeError::NoVideoStream)?;
    let format = j.get("format");
    // Tags of the container (mp3, m4a, flac) or of the audio stream (ogg, opus), any case.
    let tag = |names: &[&str]| -> String {
        [format, Some(audio)]
            .into_iter()
            .flatten()
            .filter_map(|v| v.get("tags").and_then(Value::as_object))
            .find_map(|tags| {
                names.iter().find_map(|name| {
                    tags.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .and_then(|(_, v)| v.as_str())
                        .map(|s| s.trim().replace(['\r', '\n'], " "))
                        .filter(|s| !s.is_empty())
                })
            })
            .unwrap_or_default()
    };
    let cover_stream = streams
        .iter()
        .find(|s| {
            is_type(s, "video")
                && s.get("disposition")
                    .and_then(|d| d.get("attached_pic"))
                    .is_some_and(js_truthy)
        })
        .and_then(|s| s.get("index").and_then(Value::as_i64));
    let positive = |v: f64| (v.is_finite() && v > 0.0).then_some(v);
    let duration = positive(js_number(format.and_then(|f| f.get("duration"))))
        .or_else(|| positive(js_number(audio.get("duration"))));
    Ok(VideoInfo {
        has_audio: true,
        duration_s: duration,
        audio: Some(AudioTags {
            title: tag(&["title"]),
            artist: tag(&["artist", "album_artist", "albumartist", "composer"]),
            album: tag(&["album"]),
            cover_stream,
        }),
        ..VideoInfo::default()
    })
}

/// The longest gap between key frames of `video` in the packets ffprobe read (`packets`, see
/// [`KEYFRAME_SAMPLE_S`]), the stretch after the last key frame included; `None` without
/// packets of that stream.
fn keyframe_gap(j: &Value, video: &Value) -> Option<f64> {
    let packets = j.get("packets")?.as_array()?;
    let index = video.get("index").and_then(Value::as_i64);
    let mut keys = Vec::new();
    let (mut first, mut last) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in packets {
        if index.is_some() && p.get("stream_index").and_then(Value::as_i64) != index {
            continue;
        }
        let pts = js_number(p.get("pts_time"));
        let t = if pts.is_finite() {
            pts
        } else {
            js_number(p.get("dts_time"))
        };
        if !t.is_finite() {
            continue;
        }
        first = first.min(t);
        last = last.max(t);
        if p.get("flags")
            .and_then(Value::as_str)
            .is_some_and(|f| f.starts_with('K'))
        {
            keys.push(t);
        }
    }
    if !first.is_finite() {
        return None;
    }
    if keys.is_empty() {
        return Some(last - first);
    }
    keys.sort_by(f64::total_cmp);
    let between = keys.windows(2).map(|w| w[1] - w[0]);
    let tail = last - keys[keys.len() - 1];
    Some(between.chain([tail]).fold(0.0, f64::max))
}

/// The rotation of a phone video, in degrees (ffprobe's display matrix side data; 0 when none).
fn rotation(video: &Value) -> f64 {
    video
        .get("side_data_list")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find_map(|d| d.get("rotation")))
        .map(|r| js_number(Some(r)))
        .filter(|r| r.is_finite())
        .unwrap_or(0.0)
}

/// Taller than wide as shown: a quarter turn swaps the stored sides. Unknown sides: not.
fn is_portrait(width: f64, height: f64, rotation_deg: f64) -> bool {
    if !(width > 0.0 && height > 0.0) {
        return false;
    }
    let quarter_turn = (rotation_deg.round() as i64).rem_euclid(180) == 90;
    if quarter_turn {
        width > height
    } else {
        height > width
    }
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
    keyframes: bool,
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
    if keyframes {
        // The packets of the first seconds (no decoding): where the key frames are.
        for arg in [
            "-show_entries",
            "packet=stream_index,pts_time,dts_time,flags",
            "-read_intervals",
            &format!("%+{KEYFRAME_SAMPLE_S}"),
        ] {
            args.push(arg.into());
        }
    }
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
        return Err(ProbeError::Failed(redact_urls(last)));
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
    probe_with_timeout(ffprobe, inputs, PROBE_TIMEOUT, true).await
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
    probe_with_timeout(ffprobe, &[input], DURATION_TIMEOUT, false)
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
            portrait: false,
            keyframe_gap_s: Some(1.0),
            ..VideoInfo::default()
        }
    }

    #[test]
    fn a_portrait_video_is_detected_and_always_converted() {
        let probe = |video: serde_json::Value| {
            parse_probe_output(&json!({"streams": [video], "format": {}}))
                .unwrap()
                .portrait
        };
        // The TikTok film: H.265 1080x1920.
        assert!(probe(
            json!({"codec_type": "video", "codec_name": "hevc", "width": 1080, "height": 1920})
        ));
        assert!(!probe(
            json!({"codec_type": "video", "width": 1920, "height": 1080})
        ));
        assert!(!probe(json!({"codec_type": "video", "height": 1080})));
        // A phone film stored wide but shown upright, and the other way round.
        assert!(probe(
            json!({"codec_type": "video", "width": 1920, "height": 1080,
            "side_data_list": [{"side_data_type": "Display Matrix", "rotation": -90}]})
        ));
        assert!(!probe(
            json!({"codec_type": "video", "width": 1080, "height": 1920,
            "side_data_list": [{"rotation": 90}]})
        ));
        assert!(probe(
            json!({"codec_type": "video", "width": 1080, "height": 1920,
            "side_data_list": [{"rotation": 180}]})
        ));

        let mut upright = h264();
        upright.portrait = true;
        assert_eq!(decide_video_mode(&upright, false), VideoMode::Transcode);
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
                portrait: false,
                keyframe_gap_s: None,
                ..VideoInfo::default()
            }
        );
    }

    #[test]
    fn parses_profile_level_fields_hdr_frame_rate_and_matrix() {
        let info = parse_probe_output(&json!({
            "streams": [{"codec_type": "video", "codec_name": "h264", "height": 1080,
                "pix_fmt": "yuv420p", "profile": "High", "level": 40, "field_order": "tt",
                "color_transfer": "smpte2084", "color_space": "bt2020nc",
                "avg_frame_rate": "24000/1001", "r_frame_rate": "30/1"}],
            "format": {}
        }))
        .unwrap();
        assert_eq!(info.profile, "High");
        assert_eq!(info.level, 40);
        assert!(info.interlaced && info.hdr && info.vfr);
        assert_eq!(info.colorspace, "bt2020nc");
        let plain = parse_probe_output(&json!({
            "streams": [{"codec_type": "video", "field_order": "progressive",
                "color_transfer": "bt709", "avg_frame_rate": "30/1", "r_frame_rate": "30/1"}],
            "format": {}
        }))
        .unwrap();
        assert!(!plain.interlaced && !plain.hdr && !plain.vfr);
        assert_eq!(plain.level, 0);
    }

    #[test]
    fn only_a_uniform_source_is_copied() {
        let mode = |change: fn(&mut VideoInfo)| {
            let mut info = h264();
            info.profile = "High".into();
            info.level = 41;
            info.colorspace = "bt709".into();
            change(&mut info);
            decide_video_mode(&info, false)
        };
        assert_eq!(mode(|_| {}), VideoMode::Copy);
        assert_eq!(mode(|i| i.colorspace.clear()), VideoMode::Copy);
        assert_eq!(mode(|i| i.profile = "Main".into()), VideoMode::Copy);
        let converted: [fn(&mut VideoInfo); 7] = [
            |i| i.fps = 60.0,
            |i| i.vfr = true,
            |i| i.interlaced = true,
            |i| i.hdr = true,
            |i| i.profile = "High 10".into(),
            |i| i.level = 51,
            |i| i.colorspace = "smpte170m".into(),
        ];
        for change in converted {
            assert_eq!(mode(change), VideoMode::Transcode);
        }
        // Uniform but with key frames too far apart (or not measured) to cut 1 s segments.
        assert_eq!(mode(|i| i.keyframe_gap_s = Some(2.0)), VideoMode::Transcode);
        assert_eq!(mode(|i| i.keyframe_gap_s = None), VideoMode::Transcode);
    }

    #[test]
    fn the_keyframe_gap_comes_from_the_video_packets_tail_included() {
        let gap = |packets: serde_json::Value| {
            parse_probe_output(&json!({
                "streams": [
                    {"index": 0, "codec_type": "audio"},
                    {"index": 1, "codec_type": "video", "codec_name": "h264", "height": 720}
                ],
                "format": {},
                "packets": packets
            }))
            .unwrap()
            .keyframe_gap_s
        };
        let every = |step: f64, until: f64| {
            let mut v = Vec::new();
            let mut t = 0.0;
            while t <= until {
                let key = (t / step - (t / step).round()).abs() < 1e-9;
                v.push(json!({"stream_index": 1, "pts_time": format!("{t:.6}"), "flags": if key { "K__" } else { "___" }}));
                // Audio key frames everywhere must not count.
                v.push(json!({"stream_index": 0, "pts_time": format!("{t:.6}"), "flags": "K__"}));
                t += 0.5;
            }
            serde_json::Value::Array(v)
        };
        assert_eq!(gap(every(1.0, 6.0)), Some(1.0));
        assert_eq!(gap(every(2.0, 6.0)), Some(2.0));
        // One key frame, then nothing for 5.5 s.
        assert_eq!(gap(every(100.0, 5.5)), Some(5.5));
        assert_eq!(gap(json!([])), None);

        let mut copy = h264();
        copy.keyframe_gap_s = Some(1.04);
        assert_eq!(decide_video_mode(&copy, false), VideoMode::Copy);
        copy.keyframe_gap_s = Some(2.0);
        assert_eq!(decide_video_mode(&copy, false), VideoMode::Transcode);
        copy.keyframe_gap_s = None;
        assert_eq!(decide_video_mode(&copy, false), VideoMode::Transcode);
    }

    #[test]
    fn audio_only_sources_are_analysed_with_their_tags() {
        // mp3: tags of the container, an attached cover picture.
        let mp3 = parse_probe_output(&json!({
            "streams": [
                {"index": 0, "codec_type": "audio", "codec_name": "mp3"},
                {"index": 1, "codec_type": "video", "codec_name": "mjpeg", "disposition": {"attached_pic": 1}}
            ],
            "format": {"duration": "183.5", "tags": {"title": " Tytuł\n", "ARTIST": "Zespół", "album": "Płyta"}}
        }))
        .unwrap();
        assert_eq!(mp3.duration_s, Some(183.5));
        assert!(mp3.has_audio);
        assert_eq!(mp3.height, 0);
        assert_eq!(
            mp3.audio,
            Some(AudioTags {
                title: "Tytuł".into(),
                artist: "Zespół".into(),
                album: "Płyta".into(),
                cover_stream: Some(1),
            })
        );
        // Always converted: there is no picture to copy.
        assert_eq!(decide_video_mode(&mp3, false), VideoMode::Transcode);

        // opus: tags and duration of the stream, the album artist when there is no artist.
        let opus = parse_probe_output(&json!({
            "streams": [{"index": 0, "codec_type": "audio", "duration": "6.5",
                "tags": {"TITLE": "Ogg", "ALBUM_ARTIST": "Ktoś"}}],
            "format": {"duration": "N/A"}
        }))
        .unwrap();
        assert_eq!(opus.duration_s, Some(6.5));
        let tags = opus.audio.unwrap();
        assert_eq!((tags.title.as_str(), tags.artist.as_str()), ("Ogg", "Ktoś"));
        assert_eq!(tags.cover_stream, None);
        // A video stays a video.
        let video = parse_probe_output(&json!({"streams": [{"codec_type": "video"}]})).unwrap();
        assert_eq!(video.audio, None);
    }

    #[test]
    fn errors_when_there_is_no_video() {
        let err = parse_probe_output(&json!({"streams": [{"codec_type": "data"}], "format": {}}))
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
    fn http_403_and_401_count_as_access_denied() {
        let denied = |s: &str| ProbeError::Failed(s.into()).access_denied();
        assert!(denied(
            "https://rr7.googlevideo.com/videoplayback: Server returned 403 Forbidden (access denied)"
        ));
        assert!(denied(
            "https://x/a: Server returned 401 Unauthorized (authorization failed)"
        ));
        assert!(denied("HTTP error 403 Forbidden"));
        assert!(!denied("Server returned 404 Not Found"));
        assert!(!denied("Invalid data found when processing input"));
        assert!(!ProbeError::NoVideoStream.access_denied());
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
        // libx264's default group of pictures (250 frames) is longer than the 1 s clip.
        assert!(
            info.keyframe_gap_s.is_some_and(|g| g > 0.5 && g < 1.5),
            "{info:?}"
        );

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
