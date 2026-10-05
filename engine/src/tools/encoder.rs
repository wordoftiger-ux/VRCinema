//! Automatic video-encoder detection (spec 5.1): tries the hardware H.264 encoders in turn by
//! encoding one second of a test pattern to `-f null`; the first that exits with 0 wins.

use std::ffi::OsString;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

use crate::config::VideoEncoder;
use crate::process::run_process;

/// Hardware encoders in order of preference.
const CANDIDATES: [VideoEncoder; 3] = [
    VideoEncoder::H264Nvenc,
    VideoEncoder::H264Qsv,
    VideoEncoder::H264Amf,
];

const TEST_TIMEOUT: Duration = Duration::from_secs(15);

fn test_args(encoder: VideoEncoder) -> Vec<OsString> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1280x720:rate=30:duration=1",
        "-c:v",
        encoder.as_str(),
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(OsString::from)
    .collect()
}

/// Core of the detection with an injectable runner: `run` gets the ffmpeg arguments of one test
/// and returns the exit code (`None` = could not run / timed out).
pub async fn detect_encoder_with<F, Fut>(run: F) -> VideoEncoder
where
    F: Fn(Vec<OsString>) -> Fut,
    Fut: Future<Output = Option<i32>>,
{
    for encoder in CANDIDATES {
        if run(test_args(encoder)).await == Some(0) {
            return encoder;
        }
    }
    VideoEncoder::Libx264
}

/// Tests nvenc, qsv and amf (in that order) with the given ffmpeg; falls back to libx264.
pub async fn detect_encoder(ffmpeg: &Path) -> VideoEncoder {
    detect_encoder_with(|args| async move {
        run_process(ffmpeg, &args, TEST_TIMEOUT)
            .await
            .ok()
            .and_then(|out| out.code)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Fake runner: exit code per encoder name found in the arguments; records the order tried.
    async fn detect_with_codes(codes: &[(&str, Option<i32>)]) -> (VideoEncoder, Vec<String>) {
        let tried = Mutex::new(Vec::new());
        let result = detect_encoder_with(|args| {
            let name = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .find(|a| a.starts_with("h264_"))
                .expect("encoder in args");
            tried.lock().unwrap().push(name.clone());
            let code = codes.iter().find(|(n, _)| *n == name).and_then(|(_, c)| *c);
            async move { code }
        })
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
        let args: Vec<String> = test_args(VideoEncoder::H264Qsv)
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
    async fn missing_ffmpeg_falls_back_to_libx264() {
        let enc = detect_encoder(Path::new("definitely-not-a-real-ffmpeg-xyz")).await;
        assert_eq!(enc, VideoEncoder::Libx264);
    }

    #[tokio::test]
    async fn real_ffmpeg_returns_a_valid_encoder() {
        let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bin/ffmpeg.exe");
        if !ffmpeg.exists() {
            eprintln!("skip: bin/ffmpeg.exe missing");
            return;
        }
        let enc = detect_encoder(&ffmpeg).await;
        assert!(VideoEncoder::ALL.contains(&enc), "{enc:?}");
    }
}
