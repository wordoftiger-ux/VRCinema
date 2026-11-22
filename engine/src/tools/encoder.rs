//! Automatic video-encoder detection (spec 5.1): tries the hardware H.264 encoders in turn by
//! encoding one second of a test pattern to `-f null`; the first that exits with 0 wins.

use std::ffi::OsString;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

use crate::config::VideoEncoder;
use crate::pipeline::ffmpeg_args::{HdrPath, MAX_HEIGHT, VAAPI_UPLOAD};
use crate::process::run_process;

/// Hardware encoders in order of preference; VA-API (Linux only) last.
const CANDIDATES: [VideoEncoder; 4] = [
    VideoEncoder::H264Nvenc,
    VideoEncoder::H264Qsv,
    VideoEncoder::H264Amf,
    VideoEncoder::H264Vaapi,
];

const TEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The encoders tried on this platform: VA-API only on Linux.
fn candidates(linux: bool) -> Vec<VideoEncoder> {
    CANDIDATES
        .into_iter()
        .filter(|e| linux || *e != VideoEncoder::H264Vaapi)
        .collect()
}

/// The test of one encoder; VA-API opens `vaapi_device` and uploads the picture to it like a
/// real clip does.
fn test_args(encoder: VideoEncoder, vaapi_device: &str) -> Vec<OsString> {
    let mut args: Vec<&str> = vec!["-hide_banner", "-loglevel", "error"];
    if encoder == VideoEncoder::H264Vaapi {
        args.extend(["-vaapi_device", vaapi_device]);
    }
    args.extend([
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1280x720:rate=30:duration=1",
    ]);
    if encoder == VideoEncoder::H264Vaapi {
        args.extend(["-vf", VAAPI_UPLOAD]);
    }
    args.extend(["-c:v", encoder.as_str(), "-f", "null", "-"]);
    args.into_iter().map(OsString::from).collect()
}

/// Core of the detection with an injectable runner: `run` gets the ffmpeg arguments of one test
/// and returns the exit code (`None` = could not run / timed out).
pub async fn detect_encoder_with<F, Fut>(run: F, linux: bool, vaapi_device: &str) -> VideoEncoder
where
    F: Fn(Vec<OsString>) -> Fut,
    Fut: Future<Output = Option<i32>>,
{
    for encoder in candidates(linux) {
        if run(test_args(encoder, vaapi_device)).await == Some(0) {
            return encoder;
        }
    }
    VideoEncoder::Libx264
}

/// Tests nvenc, qsv, amf and (on Linux) VA-API on `vaapi_device`, in that order, with the
/// given ffmpeg; falls back to libx264.
pub async fn detect_encoder(ffmpeg: &Path, vaapi_device: &str) -> VideoEncoder {
    detect_encoder_with(
        |args| async move {
            run_process(ffmpeg, &args, TEST_TIMEOUT)
                .await
                .ok()
                .and_then(|out| out.code)
        },
        cfg!(target_os = "linux"),
        vaapi_device,
    )
    .await
}

/// The ffmpeg arguments that write the HDR test film to `sample`: one second of a 320×180
/// test pattern, 10-bit HEVC tagged BT.2020 PQ (libx265).
fn hdr_sample_args(sample: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=24:duration=1",
        "-pix_fmt",
        "yuv420p10le",
        "-c:v",
        "libx265",
        "-x265-params",
        "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:log-level=error",
        "-f",
        "matroska",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(sample.into());
    args
}

/// The test of one HDR path: the test film decoded and tonemapped like a real clip, to `-f null`.
fn hdr_test_args(path: HdrPath, sample: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-hide_banner", "-loglevel", "error"]
        .into_iter()
        .chain(path.input_args().iter().copied())
        .chain(["-i"])
        .map(OsString::from)
        .collect();
    args.push(sample.into());
    for a in [
        "-vf",
        &path.filter(MAX_HEIGHT).unwrap_or_default(),
        "-f",
        "null",
        "-",
    ] {
        args.push(a.into());
    }
    args
}

/// Core of the HDR test with an injectable runner (like [`detect_encoder_with`]): writes the
/// test film to `sample`, then tries Vulkan and CUDA + OpenCL in turn; the first that exits with
/// 0 wins. No test film (no libx265) or no working path: the processor.
pub async fn detect_hdr_path_with<F, Fut>(run: F, sample: &Path) -> HdrPath
where
    F: Fn(Vec<OsString>) -> Fut,
    Fut: Future<Output = Option<i32>>,
{
    if run(hdr_sample_args(sample)).await != Some(0) {
        return HdrPath::Cpu;
    }
    for path in HdrPath::GPU {
        if run(hdr_test_args(path, sample)).await == Some(0) {
            return path;
        }
    }
    HdrPath::Cpu
}

/// How the given ffmpeg tonemaps HDR films on this computer (the test film lives in the
/// temporary folder while the test runs).
pub async fn detect_hdr_path(ffmpeg: &Path) -> HdrPath {
    let sample = std::env::temp_dir().join(format!("vrcinema-hdr-test-{}.mkv", std::process::id()));
    let found = detect_hdr_path_with(
        |args| async move {
            run_process(ffmpeg, &args, TEST_TIMEOUT)
                .await
                .ok()
                .and_then(|out| out.code)
        },
        &sample,
    )
    .await;
    let _ = std::fs::remove_file(&sample);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Fake runner: exit code per encoder name found in the arguments; records the order tried.
    async fn detect_with_codes(codes: &[(&str, Option<i32>)]) -> (VideoEncoder, Vec<String>) {
        detect_on(false, codes).await
    }

    async fn detect_on(linux: bool, codes: &[(&str, Option<i32>)]) -> (VideoEncoder, Vec<String>) {
        let tried = Mutex::new(Vec::new());
        let result = detect_encoder_with(
            |args| {
                let name = args
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .find(|a| a.starts_with("h264_"))
                    .expect("encoder in args");
                tried.lock().unwrap().push(name.clone());
                let code = codes.iter().find(|(n, _)| *n == name).and_then(|(_, c)| *c);
                async move { code }
            },
            linux,
            "/dev/dri/renderD128",
        )
        .await;
        (result, tried.into_inner().unwrap())
    }

    #[tokio::test]
    async fn nvenc_wins_first_and_stops() {
        let (enc, tried) =
            detect_with_codes(&[("h264_nvenc", Some(0)), ("h264_qsv", Some(0))]).await;
        assert_eq!(enc, VideoEncoder::H264Nvenc);
        assert_eq!(tried, ["h264_nvenc"]);
    }

    #[tokio::test]
    async fn qsv_when_nvenc_fails() {
        let (enc, tried) = detect_with_codes(&[
            ("h264_nvenc", Some(1)),
            ("h264_qsv", Some(0)),
            ("h264_amf", Some(0)),
        ])
        .await;
        assert_eq!(enc, VideoEncoder::H264Qsv);
        assert_eq!(tried, ["h264_nvenc", "h264_qsv"]);
    }

    #[tokio::test]
    async fn amf_when_others_fail_or_cannot_run() {
        let (enc, tried) = detect_with_codes(&[
            ("h264_nvenc", None),
            ("h264_qsv", Some(69)),
            ("h264_amf", Some(0)),
        ])
        .await;
        assert_eq!(enc, VideoEncoder::H264Amf);
        assert_eq!(tried, ["h264_nvenc", "h264_qsv", "h264_amf"]);
    }

    #[tokio::test]
    async fn libx264_when_all_fail() {
        let (enc, tried) = detect_with_codes(&[
            ("h264_nvenc", Some(1)),
            ("h264_qsv", Some(1)),
            ("h264_amf", None),
        ])
        .await;
        assert_eq!(enc, VideoEncoder::Libx264);
        assert_eq!(tried.len(), 3);
    }

    #[test]
    fn test_command_line_matches_spec() {
        let args: Vec<String> = test_args(VideoEncoder::H264Qsv, "/dev/dri/renderD128")
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.join(" "),
            "-hide_banner -loglevel error -f lavfi -i testsrc2=size=1280x720:rate=30:duration=1 \
             -c:v h264_qsv -f null -"
        );
    }

    #[tokio::test]
    async fn linux_tries_vaapi_last_and_falls_back_to_libx264() {
        let (enc, tried) = detect_on(
            true,
            &[
                ("h264_nvenc", Some(1)),
                ("h264_qsv", Some(1)),
                ("h264_amf", None),
                ("h264_vaapi", Some(0)),
            ],
        )
        .await;
        assert_eq!(enc, VideoEncoder::H264Vaapi);
        assert_eq!(tried, ["h264_nvenc", "h264_qsv", "h264_amf", "h264_vaapi"]);
        let (enc, tried) = detect_on(true, &[("h264_vaapi", Some(1))]).await;
        assert_eq!(enc, VideoEncoder::Libx264, "a failed probe encode");
        assert_eq!(tried.len(), 4);
    }

    #[test]
    fn the_vaapi_test_opens_the_device_and_uploads() {
        let args: Vec<String> = test_args(VideoEncoder::H264Vaapi, "/dev/dri/renderD129")
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.join(" "),
            "-hide_banner -loglevel error -vaapi_device /dev/dri/renderD129 -f lavfi \
             -i testsrc2=size=1280x720:rate=30:duration=1 -vf format=nv12,hwupload \
             -c:v h264_vaapi -f null -"
        );
    }

    /// Fake HDR runner: exit codes of the sample write and of each path (by a filter name in
    /// the arguments); records what was tried.
    async fn hdr_with(
        sample: Option<i32>,
        vulkan: Option<i32>,
        cuda: Option<i32>,
    ) -> (HdrPath, Vec<String>) {
        let tried = Mutex::new(Vec::new());
        let found = detect_hdr_path_with(
            |args| {
                let line = args
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" ");
                let (name, code) = if line.contains("libx265") {
                    ("sample", sample)
                } else if line.contains("libplacebo") {
                    ("vulkan", vulkan)
                } else {
                    assert!(line.contains("tonemap_opencl"), "{line}");
                    ("cuda", cuda)
                };
                tried.lock().unwrap().push(name.to_string());
                async move { code }
            },
            Path::new("hdr.mkv"),
        )
        .await;
        (found, tried.into_inner().unwrap())
    }

    #[tokio::test]
    async fn hdr_paths_are_tried_vulkan_then_cuda_then_the_processor() {
        assert_eq!(
            hdr_with(Some(0), Some(0), Some(0)).await,
            (HdrPath::Vulkan, vec!["sample".into(), "vulkan".into()])
        );
        assert_eq!(
            hdr_with(Some(0), Some(1), Some(0)).await,
            (
                HdrPath::CudaOpencl,
                vec!["sample".into(), "vulkan".into(), "cuda".into()]
            )
        );
        assert_eq!(hdr_with(Some(0), None, Some(1)).await.0, HdrPath::Cpu);
        // No test film (no libx265): nothing else is tried.
        assert_eq!(
            hdr_with(Some(1), Some(0), Some(0)).await,
            (HdrPath::Cpu, vec!["sample".into()])
        );
    }

    #[test]
    fn the_hdr_test_decodes_on_the_card_before_its_input() {
        let args: Vec<String> = hdr_test_args(HdrPath::Vulkan, Path::new("t.mkv"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert!(args[..i].contains(&"-hwaccel".to_string()));
        assert_eq!(args[i + 1], "t.mkv");
        assert!(args[i + 3].starts_with("libplacebo="));
        assert_eq!(&args[args.len() - 3..], ["-f", "null", "-"]);
    }

    /// The HDR test with the real ffmpeg (`VRC_FFMPEG`); prints the path found.
    #[tokio::test]
    #[ignore]
    async fn real_hdr_path() {
        let ffmpeg = std::env::var("VRC_FFMPEG").expect("VRC_FFMPEG");
        let started = std::time::Instant::now();
        let path = detect_hdr_path(Path::new(&ffmpeg)).await;
        eprintln!(
            "HDR path: {path:?} in {:.2} s",
            started.elapsed().as_secs_f64()
        );
    }

    #[tokio::test]
    async fn missing_ffmpeg_gives_the_processor_hdr_path() {
        let path = detect_hdr_path(Path::new("definitely-not-a-real-ffmpeg-xyz")).await;
        assert_eq!(path, HdrPath::Cpu);
    }

    #[tokio::test]
    async fn missing_ffmpeg_falls_back_to_libx264() {
        let enc = detect_encoder(
            Path::new("definitely-not-a-real-ffmpeg-xyz"),
            "/dev/dri/renderD128",
        )
        .await;
        assert_eq!(enc, VideoEncoder::Libx264);
    }

    #[tokio::test]
    async fn real_ffmpeg_returns_a_valid_encoder() {
        let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bin/ffmpeg.exe");
        if !ffmpeg.exists() {
            eprintln!("skip: bin/ffmpeg.exe missing");
            return;
        }
        let enc = detect_encoder(&ffmpeg, "/dev/dri/renderD128").await;
        assert!(VideoEncoder::ALL.contains(&enc), "{enc:?}");
    }
}
