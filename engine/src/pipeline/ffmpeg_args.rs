//! ffmpeg argument construction. Port of `src/pipeline/ffmpegArgs.ts`.

use std::ffi::OsString;
use std::path::Path;

use crate::config::VideoEncoder;
use crate::pipeline::probe::{VideoInfo, VideoMode};
use crate::resolver::types::StreamInput;

pub const VIDEO_BITRATE: &str = "6M";
pub const MAX_HEIGHT: u32 = 1080;
pub const MAX_FPS: u32 = 30;

/// A portrait picture (already scaled to its output height) centred in a 16:9 frame of the same
/// height, black bars left and right (an even width; ffmpeg aligns `x` to the chroma grid).
pub const LETTERBOX: &str = "pad=w='trunc(ih*16/9/2)*2':h=ih:x='(ow-iw)/2':y=0:color=black";

pub const AUDIO_ARGS: [&str; 8] = ["-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-ac", "2"];

/// The pace a source is read at, relative to real time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadRate {
    pub rate: f64,
    /// Seconds of the source read at once before the pace applies.
    pub initial_burst_s: f64,
    /// The pace while the reading is behind its schedule (after a stall); `None`: ffmpeg's
    /// default (1.05×).
    pub catchup: Option<f64>,
}

/// The initial burst of a source read at a pace.
pub const INITIAL_BURST_S: f64 = 20.0;

impl ReadRate {
    /// `rate` with the usual initial burst and no catch-up of its own.
    pub const fn new(rate: f64) -> Self {
        Self {
            rate,
            initial_burst_s: INITIAL_BURST_S,
            catchup: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClipArgsOptions {
    pub inputs: Vec<StreamInput>,
    /// Input format, e.g. "lavfi" for synthetic sources.
    pub input_format: Option<String>,
    pub mode: VideoMode,
    pub info: VideoInfo,
    pub encoder: VideoEncoder,
    pub segment_seconds: u32,
    pub out_dir: std::path::PathBuf,
    /// Resume from this second (0 = from the start).
    pub start_at_s: f64,
    /// The pace the source is read at; `None` = unlimited.
    pub readrate: Option<ReadRate>,
    /// Filters drawn on the converted picture after scaling (the info ticker, see
    /// `ticker::overlay_filter`); ignored when the video is copied.
    pub overlay: Option<String>,
}

fn is_network(url: &str) -> bool {
    let lower = url.as_bytes();
    let starts = |p: &[u8]| lower.len() >= p.len() && lower[..p.len()].eq_ignore_ascii_case(p);
    starts(b"http://") || starts(b"https://")
}

/// Appends plain string arguments.
fn push_all(args: &mut Vec<OsString>, items: &[&str]) {
    args.extend(items.iter().map(OsString::from));
}

/// Input options shared by ffmpeg and ffprobe: reconnecting and HTTP headers.
pub fn input_options(input: &StreamInput) -> Vec<OsString> {
    let mut args = Vec::new();
    if is_network(&input.url) {
        push_all(
            &mut args,
            &[
                "-reconnect",
                "1",
                "-reconnect_streamed",
                "1",
                "-reconnect_on_network_error",
                "1",
                "-reconnect_delay_max",
                "5",
                "-rw_timeout",
                "20000000",
            ],
        );
    }
    if !input.headers.is_empty() {
        let lines: String = input
            .headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        args.push("-headers".into());
        args.push(lines.into());
    }
    args
}

/// The encoder and its rate control. The GPU encoders turn the forced key frames (every
/// segment, see [`build_clip_args`]) into IDR frames: the segmenter cuts only at those, and
/// without it the segments follow the encoder's own GOP (10 s with h264_nvenc).
pub fn encoder_args(encoder: VideoEncoder) -> Vec<OsString> {
    let mut args = Vec::new();
    match encoder {
        VideoEncoder::H264Nvenc => push_all(
            &mut args,
            &[
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p4",
                "-profile:v",
                "high",
                "-rc",
                "cbr",
                "-forced-idr",
                "1",
            ],
        ),
        VideoEncoder::H264Qsv => push_all(
            &mut args,
            &[
                "-c:v",
                "h264_qsv",
                "-preset",
                "faster",
                "-profile:v",
                "high",
                "-forced_idr",
                "1",
            ],
        ),
        VideoEncoder::H264Amf => push_all(
            &mut args,
            &[
                "-c:v",
                "h264_amf",
                "-quality",
                "speed",
                "-rc",
                "cbr",
                "-forced_idr",
                "1",
            ],
        ),
        VideoEncoder::Libx264 => push_all(
            &mut args,
            &[
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-profile:v",
                "high",
                "-level:v",
                "4.1",
                "-sc_threshold",
                "0",
            ],
        ),
    }
    push_all(
        &mut args,
        &[
            "-b:v",
            VIDEO_BITRATE,
            "-maxrate",
            VIDEO_BITRATE,
            "-bufsize",
            "12M",
        ],
    );
    args
}

pub fn segment_output_args(segment_seconds: u32, out_dir: &Path) -> Vec<OsString> {
    let mut args = Vec::new();
    push_all(
        &mut args,
        &[
            "-max_muxing_queue_size",
            "1024",
            "-f",
            "segment",
            "-segment_time",
            &segment_seconds.to_string(),
            "-segment_time_delta",
            "0.05",
            "-segment_format",
            "mpegts",
            // Each audio frame in its own PES: the last packet's timestamp = the real end of audio.
            "-segment_format_options",
            "pes_payload_size=0",
            "-segment_list",
        ],
    );
    args.push(out_dir.join("list.csv").into_os_string());
    push_all(
        &mut args,
        &["-segment_list_type", "csv", "-reset_timestamps", "0"],
    );
    args.push(out_dir.join("seg_%05d.ts").into_os_string());
    args
}

pub fn build_clip_args(o: &ClipArgsOptions) -> Vec<OsString> {
    let mut args = Vec::new();
    push_all(
        &mut args,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            // Progress on stdout (`speed=` for the stats); stderr keeps only errors.
            "-progress",
            "pipe:1",
            "-nostats",
        ],
    );
    for input in &o.inputs {
        args.extend(input_options(input));
        if o.start_at_s > 0.0 {
            push_all(&mut args, &["-ss", &o.start_at_s.to_string()]);
        }
        if let Some(rate) = o.readrate {
            push_all(
                &mut args,
                &[
                    "-readrate",
                    &rate.rate.to_string(),
                    "-readrate_initial_burst",
                    &rate.initial_burst_s.to_string(),
                ],
            );
            if let Some(catchup) = rate.catchup {
                push_all(&mut args, &["-readrate_catchup", &catchup.to_string()]);
            }
        }
        if let Some(format) = &o.input_format {
            push_all(&mut args, &["-f", format]);
        }
        args.push("-i".into());
        args.push((&input.url).into());
    }

    let add_silence = o.inputs.len() == 1 && !o.info.has_audio;
    let audio_map = if o.inputs.len() > 1 {
        "1:a:0".to_string()
    } else if !add_silence {
        "0:a:0".to_string()
    } else {
        push_all(
            &mut args,
            &[
                "-f",
                "lavfi",
                "-i",
                "anullsrc=channel_layout=stereo:sample_rate=48000",
            ],
        );
        format!("{}:a:0", o.inputs.len())
    };
    push_all(&mut args, &["-map", "0:v:0", "-map", &audio_map]);
    if add_silence {
        push_all(&mut args, &["-shortest"]);
    }

    match o.mode {
        VideoMode::Copy => push_all(&mut args, &["-c:v", "copy"]),
        VideoMode::Transcode => {
            let mut filters = vec![format!("scale=-2:'min({MAX_HEIGHT},ih)'")];
            if o.info.fps > f64::from(MAX_FPS) + 0.5 {
                filters.push(format!("fps={MAX_FPS}"));
            }
            filters.push("format=yuv420p".to_string());
            if o.info.portrait {
                filters.push(LETTERBOX.to_string());
            }
            if let Some(overlay) = &o.overlay {
                filters.push(overlay.clone());
                filters.push("format=yuv420p".to_string());
            }
            push_all(&mut args, &["-vf", &filters.join(",")]);
            args.extend(encoder_args(o.encoder));
            push_all(
                &mut args,
                &[
                    "-force_key_frames",
                    &format!("expr:gte(t,n_forced*{})", o.segment_seconds),
                ],
            );
        }
    }
    push_all(&mut args, &AUDIO_ARGS);
    args.extend(segment_output_args(o.segment_seconds, &o.out_dir));
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn h264() -> VideoInfo {
        VideoInfo {
            codec: "h264".into(),
            height: 1080,
            pix_fmt: "yuv420p".into(),
            fps: 30.0,
            has_audio: true,
            duration_s: Some(60.0),
            portrait: false,
        }
    }

    fn input(url: &str, headers: &[(&str, &str)]) -> StreamInput {
        StreamInput {
            url: url.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn opts() -> ClipArgsOptions {
        ClipArgsOptions {
            inputs: vec![input(
                "https://cdn/v.mp4",
                &[("User-Agent", "UA"), ("Referer", "https://x")],
            )],
            input_format: None,
            mode: VideoMode::Copy,
            info: h264(),
            encoder: VideoEncoder::Libx264,
            segment_seconds: 4,
            out_dir: PathBuf::from("/tmp/clip"),
            start_at_s: 0.0,
            readrate: Some(ReadRate::new(1.5)),
            overlay: None,
        }
    }

    fn strs(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn build(o: &ClipArgsOptions) -> Vec<String> {
        strs(&build_clip_args(o))
    }

    fn value_after(args: &[String], flag: &str, nth: usize) -> Option<String> {
        let mut seen = 0;
        for (i, a) in args.iter().enumerate() {
            if a == flag {
                if seen == nth {
                    return args.get(i + 1).cloned();
                }
                seen += 1;
            }
        }
        None
    }

    fn pos(args: &[String], flag: &str) -> usize {
        args.iter().position(|a| a == flag).unwrap()
    }

    fn count(args: &[String], flag: &str) -> usize {
        args.iter().filter(|a| *a == flag).count()
    }

    #[test]
    fn remux_copies_video_converts_audio_segments_to_csv() {
        let args = build(&opts());
        assert_eq!(value_after(&args, "-c:v", 0).as_deref(), Some("copy"));
        assert_eq!(value_after(&args, "-c:a", 0).as_deref(), Some("aac"));
        assert_eq!(value_after(&args, "-ar", 0).as_deref(), Some("48000"));
        assert_eq!(value_after(&args, "-ac", 0).as_deref(), Some("2"));
        assert_eq!(value_after(&args, "-b:a", 0).as_deref(), Some("160k"));
        assert_eq!(value_after(&args, "-f", 0).as_deref(), Some("segment"));
        assert_eq!(value_after(&args, "-segment_time", 0).as_deref(), Some("4"));
        assert_eq!(
            value_after(&args, "-segment_list_type", 0).as_deref(),
            Some("csv")
        );
        assert_eq!(
            value_after(&args, "-segment_format_options", 0).as_deref(),
            Some("pes_payload_size=0")
        );
        assert_eq!(
            value_after(&args, "-segment_time_delta", 0).as_deref(),
            Some("0.05")
        );
        assert_eq!(
            value_after(&args, "-segment_list", 0),
            Some(
                PathBuf::from("/tmp/clip")
                    .join("list.csv")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(
            args.last().cloned(),
            Some(
                PathBuf::from("/tmp/clip")
                    .join("seg_%05d.ts")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(value_after(&args, "-map", 0).as_deref(), Some("0:v:0"));
        assert_eq!(value_after(&args, "-map", 1).as_deref(), Some("0:a:0"));
    }

    #[test]
    fn progress_goes_to_stdout_so_stderr_keeps_only_errors() {
        let args = build(&opts());
        assert_eq!(
            value_after(&args, "-progress", 0).as_deref(),
            Some("pipe:1")
        );
        assert!(pos(&args, "-progress") < pos(&args, "-i"));
        assert!(args.iter().any(|a| a == "-nostats"));
    }

    #[test]
    fn headers_and_readrate_precede_input() {
        let args = build(&opts());
        let i = pos(&args, "-i");
        let h = pos(&args, "-headers");
        assert!(h < i);
        assert_eq!(args[h + 1], "User-Agent: UA\r\nReferer: https://x\r\n");
        assert!(pos(&args, "-readrate") < i);
        assert_eq!(value_after(&args, "-readrate", 0).as_deref(), Some("1.5"));
        assert_eq!(
            value_after(&args, "-readrate_initial_burst", 0).as_deref(),
            Some("20")
        );
    }

    #[test]
    fn network_inputs_get_reconnect_options() {
        let args = build(&opts());
        assert_eq!(value_after(&args, "-reconnect", 0).as_deref(), Some("1"));
        assert_eq!(
            value_after(&args, "-rw_timeout", 0).as_deref(),
            Some("20000000")
        );
    }

    #[test]
    fn separate_video_and_audio_inputs_map_from_two_files() {
        let mut o = opts();
        o.inputs = vec![input("https://v", &[]), input("https://a", &[])];
        let args = build(&o);
        assert_eq!(count(&args, "-i"), 2);
        assert_eq!(value_after(&args, "-map", 1).as_deref(), Some("1:a:0"));
        assert!(!args.contains(&"-headers".to_string()));
    }

    #[test]
    fn missing_audio_adds_anullsrc_silence() {
        let mut o = opts();
        o.info.has_audio = false;
        let args = build(&o);
        assert!(args.contains(&"anullsrc=channel_layout=stereo:sample_rate=48000".to_string()));
        assert_eq!(value_after(&args, "-map", 1).as_deref(), Some("1:a:0"));
        assert!(args.contains(&"-shortest".to_string()));
    }

    #[test]
    fn resume_puts_ss_before_every_input() {
        let mut o = opts();
        o.start_at_s = 75.5;
        o.inputs = vec![input("https://v", &[]), input("https://a", &[])];
        let args = build(&o);
        assert_eq!(count(&args, "-ss"), 2);
        assert_eq!(value_after(&args, "-ss", 0).as_deref(), Some("75.5"));
        assert!(pos(&args, "-ss") < pos(&args, "-i"));
        // the second -ss precedes the second -i
        let second_ss = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "-ss")
            .nth(1)
            .unwrap()
            .0;
        let second_i = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "-i")
            .nth(1)
            .unwrap()
            .0;
        assert!(second_ss < second_i);
    }

    #[test]
    fn whole_number_start_formats_without_trailing_zero() {
        let mut o = opts();
        o.start_at_s = 20.0;
        let args = build(&o);
        assert_eq!(value_after(&args, "-ss", 0).as_deref(), Some("20"));
    }

    #[test]
    fn libx264_transcode_scales_to_1080_keyframes_every_segment_no_fps_at_30() {
        let mut o = opts();
        o.mode = VideoMode::Transcode;
        let args = build(&o);
        assert_eq!(value_after(&args, "-c:v", 0).as_deref(), Some("libx264"));
        assert_eq!(
            value_after(&args, "-vf", 0).as_deref(),
            Some("scale=-2:'min(1080,ih)',format=yuv420p")
        );
        assert_eq!(
            value_after(&args, "-force_key_frames", 0).as_deref(),
            Some("expr:gte(t,n_forced*4)")
        );
        assert_eq!(value_after(&args, "-b:v", 0).as_deref(), Some("6M"));
    }

    #[test]
    fn the_overlay_is_drawn_after_scaling_and_only_when_converting() {
        let mut o = opts();
        o.mode = VideoMode::Transcode;
        o.info.fps = 60.0;
        o.overlay = Some("drawbox=x=0[a];movie=qr.png[b];[a][b]overlay".into());
        assert_eq!(
            value_after(&build(&o), "-vf", 0).as_deref(),
            Some(
                "scale=-2:'min(1080,ih)',fps=30,format=yuv420p,\
                 drawbox=x=0[a];movie=qr.png[b];[a][b]overlay,format=yuv420p"
            )
        );
        o.mode = VideoMode::Copy;
        assert!(!build(&o).contains(&"-vf".to_string()));
    }

    #[test]
    fn a_portrait_film_is_letterboxed_into_16_9_under_the_ticker() {
        let mut o = opts();
        o.mode = VideoMode::Transcode;
        o.info.portrait = true;
        o.overlay = Some("drawbox=x=0".into());
        assert_eq!(
            value_after(&build(&o), "-vf", 0).as_deref(),
            Some(
                "scale=-2:'min(1080,ih)',format=yuv420p,\
                 pad=w='trunc(ih*16/9/2)*2':h=ih:x='(ow-iw)/2':y=0:color=black,\
                 drawbox=x=0,format=yuv420p"
            )
        );
    }

    #[test]
    fn fps_filter_only_above_30_5() {
        let mut o = opts();
        o.mode = VideoMode::Transcode;
        o.info.fps = 59.94;
        assert_eq!(
            value_after(&build(&o), "-vf", 0).as_deref(),
            Some("scale=-2:'min(1080,ih)',fps=30,format=yuv420p")
        );
        o.info.fps = 30.5;
        assert_eq!(
            value_after(&build(&o), "-vf", 0).as_deref(),
            Some("scale=-2:'min(1080,ih)',format=yuv420p")
        );
        o.info.fps = 30.51;
        assert_eq!(
            value_after(&build(&o), "-vf", 0).as_deref(),
            Some("scale=-2:'min(1080,ih)',fps=30,format=yuv420p")
        );
    }

    #[test]
    fn nvidia_hardware_encoder() {
        let mut o = opts();
        o.mode = VideoMode::Transcode;
        o.encoder = VideoEncoder::H264Nvenc;
        let args = build(&o);
        assert_eq!(value_after(&args, "-c:v", 0).as_deref(), Some("h264_nvenc"));
        assert_eq!(value_after(&args, "-rc", 0).as_deref(), Some("cbr"));
        assert_eq!(value_after(&args, "-preset", 0).as_deref(), Some("p4"));
    }

    /// The forced key frames of a GPU encoder are plain I frames unless asked for IDR frames;
    /// the segmenter cuts only at IDR frames, so the segments grew to the encoder's own GOP
    /// (10 s with h264_nvenc) instead of 4 s (seen in the stage 4 live run).
    #[test]
    fn gpu_encoders_make_the_forced_key_frames_idr_frames() {
        let nvenc = strs(&encoder_args(VideoEncoder::H264Nvenc));
        assert_eq!(value_after(&nvenc, "-forced-idr", 0).as_deref(), Some("1"));
        for encoder in [VideoEncoder::H264Qsv, VideoEncoder::H264Amf] {
            let args = strs(&encoder_args(encoder));
            assert_eq!(
                value_after(&args, "-forced_idr", 0).as_deref(),
                Some("1"),
                "{encoder:?}"
            );
        }
        let mut o = opts();
        o.mode = VideoMode::Transcode;
        o.encoder = VideoEncoder::H264Nvenc;
        let args = build(&o);
        assert_eq!(value_after(&args, "-forced-idr", 0).as_deref(), Some("1"));
        assert_eq!(
            value_after(&args, "-force_key_frames", 0).as_deref(),
            Some("expr:gte(t,n_forced*4)")
        );
    }

    #[test]
    fn qsv_and_amf_encoders() {
        let qsv = strs(&encoder_args(VideoEncoder::H264Qsv));
        assert_eq!(
            qsv,
            [
                "-c:v",
                "h264_qsv",
                "-preset",
                "faster",
                "-profile:v",
                "high",
                "-forced_idr",
                "1",
                "-b:v",
                "6M",
                "-maxrate",
                "6M",
                "-bufsize",
                "12M"
            ]
        );
        let amf = strs(&encoder_args(VideoEncoder::H264Amf));
        assert_eq!(
            amf,
            [
                "-c:v",
                "h264_amf",
                "-quality",
                "speed",
                "-rc",
                "cbr",
                "-forced_idr",
                "1",
                "-b:v",
                "6M",
                "-maxrate",
                "6M",
                "-bufsize",
                "12M"
            ]
        );
        let x264 = strs(&encoder_args(VideoEncoder::Libx264));
        assert_eq!(
            x264,
            [
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-profile:v",
                "high",
                "-level:v",
                "4.1",
                "-sc_threshold",
                "0",
                "-b:v",
                "6M",
                "-maxrate",
                "6M",
                "-bufsize",
                "12M"
            ]
        );
    }

    /// A clip read at real time (the ticker) catches up at 1.5× after a stall; a clip
    /// prepared ahead of its turn has no initial burst.
    #[test]
    fn a_slower_read_rate_catches_up_and_the_burst_can_be_left_out() {
        let mut o = opts();
        o.readrate = Some(ReadRate {
            rate: 1.0,
            initial_burst_s: 0.0,
            catchup: Some(1.5),
        });
        let args = build(&o);
        assert_eq!(value_after(&args, "-readrate", 0).as_deref(), Some("1"));
        assert_eq!(
            value_after(&args, "-readrate_initial_burst", 0).as_deref(),
            Some("0")
        );
        assert_eq!(
            value_after(&args, "-readrate_catchup", 0).as_deref(),
            Some("1.5")
        );
        assert!(pos(&args, "-readrate_catchup") < pos(&args, "-i"));
        // The usual read rate has no catch-up of its own.
        assert!(!build(&opts()).contains(&"-readrate_catchup".to_string()));
    }

    #[test]
    fn no_readrate_for_live() {
        let mut o = opts();
        o.readrate = None;
        assert!(!build(&o).contains(&"-readrate".to_string()));
    }

    #[test]
    fn local_files_get_no_reconnect_options() {
        let mut o = opts();
        o.inputs = vec![input("/tmp/a.mp4", &[])];
        assert!(!build(&o).contains(&"-reconnect".to_string()));
    }

    #[test]
    fn http_scheme_is_case_insensitive() {
        let opts = strs(&input_options(&input("HTTPS://x/y", &[])));
        assert!(opts.contains(&"-reconnect".to_string()));
    }

    #[test]
    fn input_format_goes_before_input() {
        let mut o = opts();
        o.input_format = Some("lavfi".into());
        o.inputs = vec![input("color=c=black:s=64x64", &[])];
        o.readrate = None;
        let args = build(&o);
        let f = pos(&args, "-f");
        assert_eq!(args[f + 1], "lavfi");
        assert_eq!(args[f + 2], "-i");
        assert_eq!(args[f + 3], "color=c=black:s=64x64");
    }

    #[test]
    fn global_flags_come_first() {
        let args = build(&opts());
        assert_eq!(
            &args[..5],
            ["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]
        );
    }

    #[test]
    fn non_ascii_paths_survive() {
        let mut o = opts();
        o.out_dir = PathBuf::from("C:\\dane\\żółć");
        let raw = build_clip_args(&o);
        let last = raw.last().unwrap();
        assert!(last.to_string_lossy().contains("żółć"));
        assert!(last.to_string_lossy().ends_with("seg_%05d.ts"));
    }
}
