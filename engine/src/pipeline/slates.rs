//! Idle and buffering slates, pre-rendered once at start-up. Port of `src/pipeline/slates.ts`.

use std::path::{Path, PathBuf};

use crate::config::VideoEncoder;
use crate::pipeline::ffmpeg_args::{ClipArgsOptions, build_clip_args};
use crate::pipeline::probe::{VideoInfo, VideoMode};
use crate::pipeline::segmenter::{ClipSegmenter, SegmentInfo, SegmenterState};
use crate::resolver::types::StreamInput;

const SLATE_SECONDS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlateKind {
    Idle,
    Buffering,
    /// Shown while the channel is paused (tunnel being set up, no viewers, ...).
    Pause,
}

impl SlateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SlateKind::Idle => "idle",
            SlateKind::Buffering => "buffering",
            SlateKind::Pause => "pause",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Slates {
    pub idle: Vec<SegmentInfo>,
    pub buffering: Vec<SegmentInfo>,
    pub pause: Vec<SegmentInfo>,
}

#[derive(Debug, Clone)]
pub struct SlateOptions {
    pub ffmpeg_path: PathBuf,
    pub font_file: PathBuf,
    pub dir: PathBuf,
    /// Address of the request page shown on the idle slate.
    pub request_url: String,
    pub segment_seconds: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SlateError {
    #[error("Nie udało się wygenerować planszy \"{kind}\": {message}")]
    Failed { kind: &'static str, message: String },
    #[error("Plansza \"{0}\" nie ma fragmentów.")]
    Empty(&'static str),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

fn texts(kind: SlateKind, request_url: &str) -> (String, String) {
    match kind {
        SlateKind::Idle => (
            "Brak filmów w kolejce".to_string(),
            if request_url.is_empty() {
                "Dodaj film na stronie zgłoszeń".to_string()
            } else {
                format!("Dodaj film: {request_url}")
            },
        ),
        SlateKind::Buffering => (
            "Buforowanie…".to_string(),
            "Film zaraz się zacznie".to_string(),
        ),
        SlateKind::Pause => ("Pauza — wracamy za chwilę".to_string(), String::new()),
    }
}

/// lavfi filter: dark 1080p30 background and two centred texts. Texts and font are read from
/// files in ffmpeg's working directory, which sidesteps path and character escaping.
pub fn slate_filter(with_text: bool, seconds: u32) -> String {
    let base = format!("color=c=0x14161c:s=1920x1080:r=30:d={seconds}");
    if !with_text {
        return base;
    }
    let common = "fontfile=font.ttf:x=(w-text_w)/2";
    [
        base,
        format!("drawtext={common}:fontcolor=white:textfile=title.txt:fontsize=72:y=(h/2)-90"),
        format!(
            "drawtext={common}:fontcolor=0xb8bcc8:textfile=subtitle.txt:fontsize=40:y=(h/2)+30"
        ),
    ]
    .join(",")
}

async fn generate_one(kind: SlateKind, o: &SlateOptions) -> Result<Vec<SegmentInfo>, SlateError> {
    // ffmpeg runs with cwd = the slate dir, so the dir handed to it must not be relative.
    let dir = std::path::absolute(o.dir.join(kind.as_str()))?;
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    std::fs::create_dir_all(&dir)?;
    let mut with_text = false;
    if Path::new(&o.font_file).exists() {
        std::fs::copy(&o.font_file, dir.join("font.ttf"))?;
        let (title, subtitle) = texts(kind, &o.request_url);
        std::fs::write(dir.join("title.txt"), title)?;
        std::fs::write(dir.join("subtitle.txt"), subtitle)?;
        with_text = true;
    } else {
        eprintln!(
            "Brak czcionki {} — plansze bez tekstu. Ustaw FONT_FILE w .env.",
            o.font_file.display()
        );
    }
    let args = build_clip_args(&ClipArgsOptions {
        inputs: vec![StreamInput {
            url: slate_filter(with_text, SLATE_SECONDS),
            headers: vec![],
        }],
        input_format: Some("lavfi".into()),
        mode: VideoMode::Transcode,
        info: VideoInfo {
            codec: "rawvideo".into(),
            height: 1080,
            pix_fmt: "yuv420p".into(),
            fps: 30.0,
            has_audio: false,
            duration_s: Some(f64::from(SLATE_SECONDS)),
        },
        encoder: VideoEncoder::Libx264,
        segment_seconds: o.segment_seconds,
        out_dir: dir.clone(),
        start_at_s: 0.0,
        readrate: None,
    });
    let seg = ClipSegmenter::start(&o.ffmpeg_path, args, dir.clone(), Some(dir));
    if seg.wait().await != SegmenterState::Done {
        return Err(SlateError::Failed {
            kind: kind.as_str(),
            message: seg.error().unwrap_or_default(),
        });
    }
    let segments = seg.poll();
    if segments.is_empty() {
        return Err(SlateError::Empty(kind.as_str()));
    }
    Ok(segments)
}

pub async fn generate_slates(o: &SlateOptions) -> Result<Slates, SlateError> {
    let (idle, buffering, pause) = tokio::join!(
        generate_one(SlateKind::Idle, o),
        generate_one(SlateKind::Buffering, o),
        generate_one(SlateKind::Pause, o)
    );
    Ok(Slates {
        idle: idle?,
        buffering: buffering?,
        pause: pause?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tsrewrite::min_dts;

    fn ffmpeg_bin() -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
        ["ffmpeg.exe", "ffmpeg"]
            .iter()
            .map(|name| root.join("bin").join(name))
            .find(|p| p.exists())
    }

    fn default_font() -> PathBuf {
        if cfg!(windows) {
            "C:/Windows/Fonts/arial.ttf".into()
        } else {
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into()
        }
    }

    #[test]
    fn filter_with_font_draws_two_texts_from_files() {
        let f = slate_filter(true, 8);
        assert!(f.contains("textfile=title.txt"));
        assert!(f.contains("textfile=subtitle.txt"));
        assert!(f.contains("fontfile=font.ttf"));
        assert!(f.starts_with("color=c=0x14161c:s=1920x1080:r=30:d=8"));
    }

    #[test]
    fn filter_without_font_is_background_only() {
        assert_eq!(
            slate_filter(false, 8),
            "color=c=0x14161c:s=1920x1080:r=30:d=8"
        );
    }

    #[test]
    fn slate_texts_match_the_original() {
        assert_eq!(
            texts(SlateKind::Idle, "https://tv.example.com/"),
            (
                "Brak filmów w kolejce".to_string(),
                "Dodaj film: https://tv.example.com/".to_string()
            )
        );
        assert_eq!(
            texts(SlateKind::Idle, "").1,
            "Dodaj film na stronie zgłoszeń"
        );
        assert_eq!(
            texts(SlateKind::Buffering, "x"),
            (
                "Buforowanie…".to_string(),
                "Film zaraz się zacznie".to_string()
            )
        );
    }

    #[test]
    fn pause_slate_text_is_polish_with_an_empty_subtitle() {
        assert_eq!(
            texts(SlateKind::Pause, "https://tv.example.com/"),
            ("Pauza — wracamy za chwilę".to_string(), String::new())
        );
        assert_eq!(SlateKind::Pause.as_str(), "pause");
    }

    #[tokio::test]
    async fn broken_ffmpeg_reports_the_polish_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = generate_slates(&SlateOptions {
            ffmpeg_path: "definitely-not-a-real-program-xyz".into(),
            font_file: dir.path().join("brak.ttf"),
            dir: dir.path().to_path_buf(),
            request_url: String::new(),
            segment_seconds: 4,
        })
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("Nie udało się wygenerować planszy \"idle\": "),
            "{err}"
        );
    }

    #[tokio::test]
    async fn generates_idle_and_buffering_slates_with_polish_text() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let font = default_font();
        let slates = generate_slates(&SlateOptions {
            ffmpeg_path: ffmpeg,
            font_file: font.clone(),
            dir: dir.path().to_path_buf(),
            request_url: "https://tv.example.com/".into(),
            segment_seconds: 4,
        })
        .await
        .unwrap();
        for segments in [&slates.idle, &slates.buffering, &slates.pause] {
            assert!(!segments.is_empty());
            let total: f64 = segments.iter().map(|s| s.duration).sum();
            assert!(total > 7.5, "{total}");
            let bytes = std::fs::read(&segments[0].path).unwrap();
            assert!(min_dts(&bytes).is_some());
        }
        if font.exists() {
            let title = std::fs::read_to_string(dir.path().join("idle").join("title.txt")).unwrap();
            assert_eq!(title, "Brak filmów w kolejce");
            let sub =
                std::fs::read_to_string(dir.path().join("idle").join("subtitle.txt")).unwrap();
            assert_eq!(sub, "Dodaj film: https://tv.example.com/");
            let pause =
                std::fs::read_to_string(dir.path().join("pause").join("title.txt")).unwrap();
            assert_eq!(pause, "Pauza — wracamy za chwilę");
        }
    }

    #[tokio::test]
    async fn slates_without_font_are_text_less_but_still_generated() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let slates = generate_slates(&SlateOptions {
            ffmpeg_path: ffmpeg,
            font_file: dir.path().join("brak.ttf"),
            dir: dir.path().to_path_buf(),
            request_url: String::new(),
            segment_seconds: 4,
        })
        .await
        .unwrap();
        assert!(
            !slates.idle.is_empty() && !slates.buffering.is_empty() && !slates.pause.is_empty()
        );
        assert!(!dir.path().join("idle").join("title.txt").exists());
    }
}
