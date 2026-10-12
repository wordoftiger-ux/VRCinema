//! Idle, buffering and pause slates, pre-rendered in the background and swappable while the
//! channel airs. Port of `src/pipeline/slates.ts`, extended with the QR code of the request page.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::{AppConfig, VideoEncoder};
use crate::engine::Command;
use crate::i18n::{self, Lang, Msg};
use crate::pipeline::ffmpeg_args::{ClipArgsOptions, build_clip_args};
use crate::pipeline::probe::{VideoInfo, VideoMode};
use crate::pipeline::segmenter::{ClipSegmenter, SegmentInfo, SegmenterState};
use crate::resolver::types::StreamInput;

const SLATE_SECONDS: u32 = 8;
/// Width of the QR image in a 1080p frame (the largest whole-pixel module size that fits).
const QR_TARGET_PX: u32 = 400;
/// White border around the code, in modules (the QR standard asks for 4).
const QR_QUIET_MODULES: u32 = 4;
/// The code is placed in ffmpeg's working directory under this name.
const QR_FILE: &str = "qr.png";
/// How long the broadcaster keeps segments that left the playlist (`retain_s` in server.rs).
pub const RETAIN_S: u64 = 60;
/// Extra time before an old slate set is deleted.
const SWAP_MARGIN_S: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlateKind {
    Idle,
    Buffering,
    /// Shown while the channel is paused (tunnel being set up, no viewers, ...).
    Pause,
}

impl SlateKind {
    pub const ALL: [SlateKind; 3] = [SlateKind::Idle, SlateKind::Buffering, SlateKind::Pause];

    pub fn as_str(self) -> &'static str {
        match self {
            SlateKind::Idle => "idle",
            SlateKind::Buffering => "buffering",
            SlateKind::Pause => "pause",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Slates {
    pub idle: Vec<SegmentInfo>,
    pub buffering: Vec<SegmentInfo>,
    pub pause: Vec<SegmentInfo>,
    /// Which set this is: 0 for the one made at start-up, then 1, 2, ... for every live
    /// replacement. It is part of the source keys, so the broadcaster sees a new set as a
    /// different source (a discontinuity) even when the same kind of slate airs on.
    pub generation: u32,
}

impl Slates {
    /// Source key of `kind` in this set (`idle`, then `idle-g1`, `idle-g2`, ...).
    pub fn source_key(&self, kind: SlateKind) -> String {
        match self.generation {
            0 => kind.as_str().to_string(),
            n => format!("{}-g{n}", kind.as_str()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SlateOptions {
    pub ffmpeg_path: PathBuf,
    pub font_file: PathBuf,
    pub dir: PathBuf,
    /// Address of the request page shown (as text and as a QR code) on the idle slate; empty:
    /// no address, no code.
    pub request_url: String,
    /// Without an address: the tunnel is connecting, the idle slate says so ("Connecting to
    /// the internet…") instead of the usual hint.
    pub connecting: bool,
    pub segment_seconds: u32,
    /// Number of the set being made (see [`Slates::generation`]).
    pub generation: u32,
    /// Language of the texts.
    pub lang: Lang,
}

/// What the idle slate is made for: the public address (QR code and text of the request page)
/// or none, and then whether the tunnel is still connecting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlateAddress {
    /// No trailing slash; empty: no public address.
    pub public_url: String,
    pub connecting: bool,
}

impl From<String> for SlateAddress {
    fn from(public_url: String) -> Self {
        Self {
            public_url,
            connecting: false,
        }
    }
}

impl From<&str> for SlateAddress {
    fn from(public_url: &str) -> Self {
        public_url.to_string().into()
    }
}

#[derive(Debug)]
pub enum SlateError {
    Failed { kind: &'static str, message: Msg },
    Empty(&'static str),
    Io(std::io::Error),
}

impl SlateError {
    /// Why, for a person.
    pub fn message(&self) -> Msg {
        match self {
            Self::Failed { kind, message } => Msg::new("log.slate_failed")
                .arg("kind", kind)
                .arg_msg("message", message.clone()),
            Self::Empty(kind) => Msg::new("log.slate_empty").arg("kind", kind),
            Self::Io(e) => Msg::text(e),
        }
    }
}

impl std::fmt::Display for SlateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.message(), f)
    }
}

impl std::error::Error for SlateError {}

impl From<std::io::Error> for SlateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Address of the request page for a public address (no trailing slash); empty stays empty.
pub fn request_url(public_url: &str) -> String {
    if public_url.is_empty() {
        String::new()
    } else {
        format!("{public_url}/")
    }
}

fn texts(lang: Lang, kind: SlateKind, request_url: &str, connecting: bool) -> (String, String) {
    match kind {
        SlateKind::Idle => (
            i18n::t(lang, "slates.idle_title", &[]),
            if !request_url.is_empty() {
                // Under the QR code: just the address.
                request_url.to_string()
            } else if connecting {
                i18n::t(lang, "slates.connecting", &[])
            } else {
                i18n::t(lang, "slates.idle_hint", &[])
            },
        ),
        SlateKind::Buffering => (
            i18n::t(lang, "slates.buffering_title", &[]),
            i18n::t(lang, "slates.buffering_subtitle", &[]),
        ),
        SlateKind::Pause => (i18n::t(lang, "slates.pause_title", &[]), String::new()),
    }
}

/// PNG of the QR code for `text`: error correction M, a quiet zone of [`QR_QUIET_MODULES`]
/// modules, whole-pixel modules, about [`QR_TARGET_PX`] px wide.
pub fn qr_png(text: &str) -> Result<Vec<u8>, SlateError> {
    use image::{ImageFormat, Luma};
    use qrcode::{Color, EcLevel, QrCode};

    let failed = |message: Msg| SlateError::Failed {
        kind: SlateKind::Idle.as_str(),
        message,
    };
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .map_err(|e| failed(Msg::new("log.slate_qr_failed").arg("error", e)))?;
    let modules = u32::try_from(code.width()).unwrap_or(u32::MAX);
    let total = modules + 2 * QR_QUIET_MODULES;
    let scale = (QR_TARGET_PX / total).max(1);
    let colors = code.to_colors();
    let image = image::ImageBuffer::from_fn(total * scale, total * scale, |x, y| {
        let (mx, my) = (x / scale, y / scale);
        let inside = |v: u32| (QR_QUIET_MODULES..QR_QUIET_MODULES + modules).contains(&v);
        let dark = inside(mx)
            && inside(my)
            && colors[((my - QR_QUIET_MODULES) * modules + (mx - QR_QUIET_MODULES)) as usize]
                == Color::Dark;
        Luma([if dark { 0u8 } else { 255u8 }])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|e| failed(Msg::new("log.slate_qr_save_failed").arg("error", e)))?;
    Ok(png.into_inner())
}

/// lavfi filter: dark 1080p30 background and two centred texts, with the QR code between them
/// when `with_qr`. Texts, font and code are read from files in ffmpeg's working directory,
/// which sidesteps path and character escaping.
pub fn slate_filter(with_text: bool, with_qr: bool, seconds: u32) -> String {
    let base = format!("color=c=0x14161c:s=1920x1080:r=30:d={seconds}");
    let (title_y, subtitle_y) = if with_qr {
        ("110", "700")
    } else {
        ("(h/2)-90", "(h/2)+30")
    };
    let mut chain = vec![base];
    if with_text {
        let common = "fontfile=font.ttf:x=(w-text_w)/2";
        chain.push(format!(
            "drawtext={common}:fontcolor=white:textfile=title.txt:fontsize=72:y={title_y}"
        ));
        chain.push(format!(
            "drawtext={common}:fontcolor=0xb8bcc8:textfile=subtitle.txt:fontsize=40:y={subtitle_y}"
        ));
    }
    let chain = chain.join(",");
    if with_qr {
        format!("{chain}[bg];movie={QR_FILE}[qr];[bg][qr]overlay=x=(W-w)/2:y=250")
    } else {
        chain
    }
}

/// Kills the segmenter's ffmpeg when dropped while it still runs ([`ClipSegmenter::kill`] does
/// nothing once it has ended).
struct StopOnDrop<'a>(&'a ClipSegmenter);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.kill();
    }
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
        let (title, subtitle) = texts(o.lang, kind, &o.request_url, o.connecting);
        std::fs::write(dir.join("title.txt"), title)?;
        std::fs::write(dir.join("subtitle.txt"), subtitle)?;
        with_text = true;
    } else {
        eprintln!(
            "Brak czcionki {} — plansze bez tekstu. Ustaw FONT_FILE w .env.",
            o.font_file.display()
        );
    }
    let with_qr = kind == SlateKind::Idle && !o.request_url.is_empty();
    if with_qr {
        std::fs::write(dir.join(QR_FILE), qr_png(&o.request_url)?)?;
    }
    let args = build_clip_args(&ClipArgsOptions {
        inputs: vec![StreamInput {
            url: slate_filter(with_text, with_qr, SLATE_SECONDS),
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
    // Dropped before the end (the generation aborted: a swap cancelled, the app quitting):
    // ffmpeg is stopped, not left making a set nobody waits for. A no-op once it ended.
    let _stop = StopOnDrop(&seg);
    if seg.wait().await != SegmenterState::Done {
        return Err(SlateError::Failed {
            kind: kind.as_str(),
            message: seg.error().unwrap_or_else(|| Msg::text("")),
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
        generation: o.generation,
    })
}

/// Where the set on air lives: the three kind folders of the start-up set sit directly in the
/// slates folder (`Root`), every later set has a folder of its own (`Own`).
#[derive(Debug, Clone)]
enum SetDir {
    Root(PathBuf),
    Own(PathBuf),
}

impl SetDir {
    fn remove(&self) {
        match self {
            SetDir::Root(root) => {
                for kind in SlateKind::ALL {
                    let _ = std::fs::remove_dir_all(root.join(kind.as_str()));
                }
            }
            SetDir::Own(dir) => {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
}

struct SwapState {
    /// What the set on air was made for.
    address: SlateAddress,
    generation: u32,
    dir: SetDir,
}

/// What [`SlateSwapper::swap`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapOutcome {
    /// The set on air already shows this address.
    Unchanged,
    /// A new set was made and handed to the channel loop.
    Replaced { generation: u32 },
    /// Nothing changed: the previous set stays on air.
    Failed(Msg),
}

/// Replaces the slate set of a running channel when the public address changes: the new set is
/// made in a folder of its own, handed to the channel loop ([`Command::ReplaceSlates`]) and the
/// old folder is deleted only after its segments left the playlist and the retention window.
pub struct SlateSwapper {
    ffmpeg_path: PathBuf,
    font_file: PathBuf,
    root: PathBuf,
    segment_seconds: u32,
    lang: Lang,
    /// How long the old set's folder is kept after the swap.
    hold: Duration,
    state: tokio::sync::Mutex<SwapState>,
    /// Pending deletions of old sets; dropped with the swapper (a restarted server starts a
    /// fresh swapper and wipes the old folders itself, and must not lose its own to these).
    removals: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Drop for SlateSwapper {
    fn drop(&mut self) {
        let removals = self
            .removals
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for task in removals.drain(..) {
            task.abort();
        }
    }
}

impl SlateSwapper {
    /// `root` is the slates folder, `initial` what the start-up set was made for.
    pub fn new(
        options: &SlateOptions,
        root: PathBuf,
        initial: impl Into<SlateAddress>,
        hold: Duration,
    ) -> Self {
        let mut initial: SlateAddress = initial.into();
        initial.connecting &= initial.public_url.is_empty();
        Self {
            ffmpeg_path: options.ffmpeg_path.clone(),
            font_file: options.font_file.clone(),
            segment_seconds: options.segment_seconds,
            lang: options.lang,
            state: tokio::sync::Mutex::new(SwapState {
                address: initial,
                generation: options.generation,
                dir: SetDir::Root(root.clone()),
            }),
            root,
            hold,
            removals: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The swapper for a channel started with `config` (start-up set: generation 0 in
    /// `<data>/slates`, made for `config.public_url` and `connecting`, see [`SlateOptions`]).
    pub fn for_config(config: &AppConfig, connecting: bool) -> Self {
        let root = config.data_dir.join("slates");
        // The old set stays until its segments left the playlist (`window_size` segments) and the
        // broadcaster's retention (`RETAIN_S`), plus a margin.
        let window = (config.window_size as u64) * u64::from(config.segment_seconds);
        let hold = Duration::from_secs(window + RETAIN_S + SWAP_MARGIN_S);
        let options = SlateOptions {
            ffmpeg_path: config.ffmpeg_path.clone(),
            font_file: config.font_file.clone(),
            dir: root.clone(),
            request_url: request_url(&config.public_url),
            connecting,
            segment_seconds: config.segment_seconds,
            generation: 0,
            lang: config.language,
        };
        let initial = SlateAddress {
            public_url: config.public_url.clone(),
            connecting,
        };
        Self::new(&options, root, initial, hold)
    }

    /// Makes the set for `public_url` and hands it to the loop behind `commands`. Calls are
    /// serialized; one for the address already on air does nothing.
    pub async fn swap(&self, public_url: &str, commands: &mpsc::Sender<Command>) -> SwapOutcome {
        self.swap_to(|| public_url, commands).await
    }

    /// Like [`Self::swap`], with the address read once the lock is held: of several racing
    /// swaps the one that runs last sees the newest address, so a stale one never wins.
    pub async fn swap_to<A: Into<SlateAddress>>(
        &self,
        current: impl FnOnce() -> A,
        commands: &mpsc::Sender<Command>,
    ) -> SwapOutcome {
        let mut state = self.state.lock().await;
        let mut address: SlateAddress = current().into();
        // "Connecting" only matters without an address.
        address.connecting &= address.public_url.is_empty();
        if state.address == address {
            return SwapOutcome::Unchanged;
        }
        let public_url = address.public_url.as_str();
        let generation = state.generation + 1;
        let dir = self.root.join(format!("g{generation}"));
        let made = generate_slates(&SlateOptions {
            ffmpeg_path: self.ffmpeg_path.clone(),
            font_file: self.font_file.clone(),
            dir: dir.clone(),
            request_url: request_url(public_url),
            connecting: address.connecting,
            segment_seconds: self.segment_seconds,
            generation,
            lang: self.lang,
        })
        .await;
        let slates = match made {
            Ok(slates) => slates,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return SwapOutcome::Failed(e.message());
            }
        };
        if commands.send(Command::ReplaceSlates(slates)).await.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return SwapOutcome::Failed(Msg::new("log.channel_loop_not_running"));
        }
        let old = std::mem::replace(&mut state.dir, SetDir::Own(dir));
        state.address = address;
        state.generation = generation;
        // The old segments may still be in the playlist and in the retention window.
        let hold = self.hold;
        let removal = tokio::spawn(async move {
            tokio::time::sleep(hold).await;
            old.remove();
        });
        let mut removals = self.removals.lock().unwrap_or_else(PoisonError::into_inner);
        removals.retain(|t| !t.is_finished());
        removals.push(removal);
        SwapOutcome::Replaced { generation }
    }
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

    fn options(ffmpeg: PathBuf, dir: &Path, url: &str, generation: u32) -> SlateOptions {
        SlateOptions {
            ffmpeg_path: ffmpeg,
            font_file: default_font(),
            dir: dir.to_path_buf(),
            request_url: url.into(),
            connecting: false,
            segment_seconds: 4,
            generation,
            lang: Lang::Pl,
        }
    }

    /// Decodes the QR codes found in a grey image.
    fn decode_qr(gray: &image::GrayImage) -> Vec<String> {
        let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(
            gray.width() as usize,
            gray.height() as usize,
            |x, y| gray.get_pixel(x as u32, y as u32).0[0],
        );
        prepared
            .detect_grids()
            .into_iter()
            .filter_map(|g| g.decode().ok().map(|(_, text)| text))
            .collect()
    }

    /// Frame at `at_s` seconds into a slate segment, decoded for QR codes.
    fn qr_in_segment(ffmpeg: &Path, segment: &Path, at_s: f64) -> Vec<String> {
        let frame = segment.with_extension(format!("{at_s}.png"));
        // Output seeking (`-ss` after `-i`): the segment's timestamps do not start at 0.
        let out = std::process::Command::new(ffmpeg)
            .args(["-v", "error", "-y", "-i"])
            .arg(segment)
            .args(["-ss", &at_s.to_string(), "-frames:v", "1"])
            .arg(&frame)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        decode_qr(&image::open(&frame).unwrap().to_luma8())
    }

    #[test]
    fn filter_with_font_draws_two_texts_from_files() {
        let f = slate_filter(true, false, 8);
        assert!(f.contains("textfile=title.txt"));
        assert!(f.contains("textfile=subtitle.txt"));
        assert!(f.contains("fontfile=font.ttf"));
        assert!(f.starts_with("color=c=0x14161c:s=1920x1080:r=30:d=8"));
        assert!(!f.contains("movie="), "no code without an address");
    }

    #[test]
    fn filter_without_font_is_background_only() {
        assert_eq!(
            slate_filter(false, false, 8),
            "color=c=0x14161c:s=1920x1080:r=30:d=8"
        );
    }

    #[test]
    fn filter_with_a_code_overlays_it_between_the_two_texts() {
        let f = slate_filter(true, true, 8);
        assert!(f.contains("movie=qr.png"), "{f}");
        assert!(f.contains("overlay=x=(W-w)/2:y=250"), "{f}");
        // The code is independent of the font: it still shows on a text-less slate.
        let bare = slate_filter(false, true, 8);
        assert!(
            bare.contains("movie=qr.png") && !bare.contains("drawtext"),
            "{bare}"
        );
    }

    #[test]
    fn slate_texts_match_the_original() {
        assert_eq!(
            texts(Lang::Pl, SlateKind::Idle, "https://tv.example.com/", false),
            (
                "Brak filmów w kolejce".to_string(),
                "https://tv.example.com/".to_string()
            )
        );
        assert_eq!(
            texts(Lang::Pl, SlateKind::Idle, "", false).1,
            "Dodaj film na stronie zgłoszeń"
        );
        assert_eq!(
            texts(Lang::Pl, SlateKind::Buffering, "x", false),
            (
                "Buforowanie…".to_string(),
                "Film zaraz się zacznie".to_string()
            )
        );
    }

    #[test]
    fn slate_texts_follow_the_language() {
        assert_eq!(
            texts(Lang::En, SlateKind::Idle, "", false),
            (
                "No videos in the queue".to_string(),
                "Add a video on the request page".to_string()
            )
        );
        assert_eq!(
            texts(Lang::En, SlateKind::Idle, "", true).1,
            "Connecting to the internet…"
        );
        assert_eq!(
            texts(Lang::En, SlateKind::Buffering, "", false),
            (
                "Buffering…".to_string(),
                "The video is about to start".to_string()
            )
        );
        assert_eq!(
            texts(Lang::En, SlateKind::Pause, "https://tv.example.com/", false),
            ("Paused — back in a moment".to_string(), String::new())
        );
    }

    #[test]
    fn pause_slate_text_is_polish_with_an_empty_subtitle() {
        assert_eq!(
            texts(Lang::Pl, SlateKind::Pause, "https://tv.example.com/", false),
            ("Pauza — wracamy za chwilę".to_string(), String::new())
        );
        assert_eq!(SlateKind::Pause.as_str(), "pause");
    }

    /// Spec section 6: while the tunnel connects the idle slate says so instead of the hint.
    #[test]
    fn while_the_tunnel_connects_the_idle_slate_says_so() {
        assert_eq!(
            texts(Lang::Pl, SlateKind::Idle, "", true),
            (
                "Brak filmów w kolejce".to_string(),
                "Łączenie z internetem…".to_string()
            )
        );
        // An address always wins.
        assert_eq!(
            texts(Lang::Pl, SlateKind::Idle, "https://tv.example.com/", true).1,
            "https://tv.example.com/"
        );
        assert_eq!(
            texts(Lang::Pl, SlateKind::Pause, "", true),
            ("Pauza — wracamy za chwilę".to_string(), String::new())
        );
    }

    #[test]
    fn a_slate_address_from_text_is_not_connecting() {
        assert_eq!(
            SlateAddress::from("https://a.example.com"),
            SlateAddress {
                public_url: "https://a.example.com".into(),
                connecting: false
            }
        );
        assert_eq!(SlateAddress::default(), SlateAddress::from(String::new()));
    }

    #[test]
    fn request_url_is_the_public_address_with_a_slash() {
        assert_eq!(request_url(""), "");
        assert_eq!(
            request_url("https://tv.example.com"),
            "https://tv.example.com/"
        );
    }

    #[test]
    fn source_keys_are_unique_per_generation() {
        let mut s = Slates::default();
        assert_eq!(s.source_key(SlateKind::Idle), "idle");
        assert_eq!(s.source_key(SlateKind::Pause), "pause");
        s.generation = 3;
        assert_eq!(s.source_key(SlateKind::Idle), "idle-g3");
        assert_eq!(s.source_key(SlateKind::Buffering), "buffering-g3");
    }

    #[test]
    fn qr_png_decodes_to_the_address_with_a_quiet_zone_and_about_400_px() {
        for url in [
            "https://tv.example.com/",
            "https://abc-def-ghi-jkl-mno.trycloudflare.com/",
            "http://192.168.0.12:8080/",
        ] {
            let png = qr_png(url).unwrap();
            let gray = image::load_from_memory(&png).unwrap().to_luma8();
            assert_eq!(gray.width(), gray.height());
            assert!((300..=400).contains(&gray.width()), "{}", gray.width());
            assert_eq!(decode_qr(&gray), vec![url.to_string()]);
            // Quiet zone: the first dark pixel is 4 whole modules from the edge, and the code
            // is symmetric (same border on the far side).
            let first_dark = gray
                .enumerate_pixels()
                .find(|(_, _, p)| p.0[0] == 0)
                .map(|(x, y, _)| x.min(y))
                .unwrap();
            let last_dark = gray
                .enumerate_pixels()
                .filter(|(_, _, p)| p.0[0] == 0)
                .map(|(x, y, _)| x.max(y))
                .max()
                .unwrap();
            assert!(first_dark % QR_QUIET_MODULES == 0 && first_dark >= QR_QUIET_MODULES);
            assert_eq!(gray.width() - 1 - last_dark, first_dark);
            // Error correction level M: the code carries format bits for M (decoded level).
            let code =
                qrcode::QrCode::with_error_correction_level(url, qrcode::EcLevel::M).unwrap();
            let module = first_dark / QR_QUIET_MODULES;
            assert_eq!(
                gray.width() / module,
                code.width() as u32 + 2 * QR_QUIET_MODULES
            );
        }
    }

    #[tokio::test]
    async fn broken_ffmpeg_reports_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = generate_slates(&SlateOptions {
            font_file: dir.path().join("brak.ttf"),
            ..options(
                "definitely-not-a-real-program-xyz".into(),
                dir.path(),
                "",
                0,
            )
        })
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("Could not generate the slate \"idle\": "),
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
        let slates = generate_slates(&options(ffmpeg, dir.path(), "https://tv.example.com/", 0))
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
            assert_eq!(sub, "https://tv.example.com/");
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
            font_file: dir.path().join("brak.ttf"),
            ..options(ffmpeg, dir.path(), "", 0)
        })
        .await
        .unwrap();
        assert!(
            !slates.idle.is_empty() && !slates.buffering.is_empty() && !slates.pause.is_empty()
        );
        assert!(!dir.path().join("idle").join("title.txt").exists());
    }

    /// Review Focus 3: the code on the airing slate is readable and leads to the request page.
    #[tokio::test]
    async fn the_idle_slate_shows_a_qr_code_that_decodes_to_the_request_page() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let url = "https://tv-przyklad.trycloudflare.com/";
        let slates = generate_slates(&options(ffmpeg.clone(), dir.path(), url, 0))
            .await
            .unwrap();
        let first = &slates.idle[0].path;
        let wanted = vec![url.to_string()];
        assert_eq!(qr_in_segment(&ffmpeg, first, 0.0), wanted, "first frame");
        assert_eq!(qr_in_segment(&ffmpeg, first, 3.0), wanted, "later frame");
        let last = &slates.idle.last().unwrap().path;
        assert_eq!(qr_in_segment(&ffmpeg, last, 1.0), wanted, "last segment");
        // Only the idle slate has a code.
        assert!(qr_in_segment(&ffmpeg, &slates.pause[0].path, 1.0).is_empty());
        assert!(qr_in_segment(&ffmpeg, &slates.buffering[0].path, 1.0).is_empty());
    }

    #[tokio::test]
    async fn without_an_address_the_idle_slate_has_no_code() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let slates = generate_slates(&options(ffmpeg.clone(), dir.path(), "", 0))
            .await
            .unwrap();
        assert!(!dir.path().join("idle").join(QR_FILE).exists());
        assert!(qr_in_segment(&ffmpeg, &slates.idle[0].path, 1.0).is_empty());
        if default_font().exists() {
            let sub =
                std::fs::read_to_string(dir.path().join("idle").join("subtitle.txt")).unwrap();
            assert_eq!(sub, "Dodaj film na stronie zgłoszeń");
        }
    }

    fn swapper(ffmpeg: PathBuf, root: &Path, url: &str, hold: Duration) -> SlateSwapper {
        let o = options(ffmpeg, root, "", 0);
        SlateSwapper::new(&o, root.to_path_buf(), url, hold)
    }

    #[tokio::test]
    async fn a_swap_makes_a_new_set_in_its_own_folder_that_decodes_to_the_new_address() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let first = generate_slates(&options(
            ffmpeg.clone(),
            root.path(),
            "https://stary.example.com/",
            0,
        ))
        .await
        .unwrap();
        let swapper = swapper(
            ffmpeg.clone(),
            root.path(),
            "https://stary.example.com",
            Duration::from_secs(3600),
        );
        let (tx, mut rx) = mpsc::channel(4);

        // Same address: nothing to do.
        assert_eq!(
            swapper.swap("https://stary.example.com", &tx).await,
            SwapOutcome::Unchanged
        );
        assert!(rx.try_recv().is_err());

        assert_eq!(
            swapper.swap("https://nowy.example.com", &tx).await,
            SwapOutcome::Replaced { generation: 1 }
        );
        let Ok(Command::ReplaceSlates(second)) = rx.try_recv() else {
            panic!("the new set was not handed over");
        };
        assert_eq!(second.generation, 1);
        assert_eq!(second.source_key(SlateKind::Idle), "idle-g1");
        assert!(second.idle[0].path.starts_with(root.path().join("g1")));
        assert_eq!(
            qr_in_segment(&ffmpeg, &second.idle[0].path, 1.0),
            vec!["https://nowy.example.com/".to_string()]
        );
        // The set on air keeps its files (the hold is an hour).
        assert!(first.idle[0].path.exists());

        // A second change makes generation 2 and clears the code with the address.
        assert_eq!(
            swapper.swap("", &tx).await,
            SwapOutcome::Replaced { generation: 2 }
        );
        let Ok(Command::ReplaceSlates(third)) = rx.try_recv() else {
            panic!("the third set was not handed over");
        };
        assert_eq!(third.generation, 2);
        assert!(qr_in_segment(&ffmpeg, &third.idle[0].path, 1.0).is_empty());
    }

    /// No address in both, but "connecting" is a different slate: it is swapped in, once.
    #[tokio::test]
    async fn a_connecting_tunnel_swaps_in_the_connecting_slate() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let swapper = swapper(ffmpeg.clone(), root.path(), "", Duration::from_secs(3600));
        let (tx, mut rx) = mpsc::channel(4);
        let connecting = || SlateAddress {
            public_url: String::new(),
            connecting: true,
        };
        assert_eq!(
            swapper.swap_to(connecting, &tx).await,
            SwapOutcome::Replaced { generation: 1 }
        );
        let Ok(Command::ReplaceSlates(set)) = rx.try_recv() else {
            panic!("the connecting set was not handed over");
        };
        assert!(qr_in_segment(&ffmpeg, &set.idle[0].path, 1.0).is_empty());
        if default_font().exists() {
            let sub =
                std::fs::read_to_string(root.path().join("g1").join("idle").join("subtitle.txt"))
                    .unwrap();
            assert_eq!(sub, "Łączenie z internetem…");
        }
        assert_eq!(
            swapper.swap_to(connecting, &tx).await,
            SwapOutcome::Unchanged
        );
    }

    #[test]
    fn the_swapper_for_a_connecting_start_knows_its_initial_slate() {
        let config = crate::config::AppConfig {
            public_url: String::new(),
            ..crate::app_settings::app_config(
                &crate::app_settings::AppSettings::default(),
                Path::new("H"),
            )
        };
        let swapper = SlateSwapper::for_config(&config, true);
        let state = swapper.state.try_lock().unwrap();
        assert_eq!(
            state.address,
            SlateAddress {
                public_url: String::new(),
                connecting: true
            }
        );
    }

    #[tokio::test]
    async fn the_old_set_is_deleted_only_after_the_hold() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let first = generate_slates(&options(ffmpeg.clone(), root.path(), "", 0))
            .await
            .unwrap();
        let hold = Duration::from_secs(4);
        let swapper = swapper(ffmpeg, root.path(), "", hold);
        let (tx, mut rx) = mpsc::channel(4);
        let started = std::time::Instant::now();
        assert!(matches!(
            swapper.swap("https://a.example.com", &tx).await,
            SwapOutcome::Replaced { .. }
        ));
        let Ok(Command::ReplaceSlates(second)) = rx.try_recv() else {
            panic!("no set");
        };
        // Start-up set: its three folders sit in the root; they stay while the hold runs.
        assert!(first.idle[0].path.exists());
        assert!(root.path().join("pause").is_dir());

        let gone = tokio::time::timeout(Duration::from_secs(30), async {
            while root.path().join("idle").exists() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(gone.is_ok(), "the old folders were never removed");
        assert!(started.elapsed() >= hold, "removed before the hold ended");
        for kind in SlateKind::ALL {
            assert!(!root.path().join(kind.as_str()).exists());
        }
        // The new set is untouched.
        assert!(second.idle[0].path.exists());

        // Swapping again removes the previous *own* folder (g1) after the hold.
        assert!(matches!(
            swapper.swap("https://b.example.com", &tx).await,
            SwapOutcome::Replaced { generation: 2 }
        ));
        assert!(root.path().join("g1").is_dir());
        let gone = tokio::time::timeout(Duration::from_secs(30), async {
            while root.path().join("g1").exists() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(gone.is_ok(), "g1 was never removed");
        assert!(root.path().join("g2").is_dir());
    }

    #[tokio::test]
    async fn a_failed_generation_keeps_the_previous_set_and_reports_why() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("idle")).unwrap();
        let swapper = swapper(
            "definitely-not-a-real-program-xyz".into(),
            root.path(),
            "https://stary.example.com",
            Duration::from_millis(10),
        );
        let (tx, mut rx) = mpsc::channel(4);
        let outcome = swapper.swap("https://nowy.example.com", &tx).await;
        let SwapOutcome::Failed(why) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(
            why.to_string().contains("Could not generate the slate"),
            "{why}"
        );
        assert!(rx.try_recv().is_err(), "nothing may reach the loop");
        // The old set and its folders are untouched even after the hold would have passed.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(root.path().join("idle").is_dir());
        assert!(
            !root.path().join("g1").exists(),
            "the half-made folder is cleaned up"
        );
        // The address on air did not change: the same request is tried again, not skipped.
        assert!(matches!(
            swapper.swap("https://nowy.example.com", &tx).await,
            SwapOutcome::Failed(_)
        ));
        assert_eq!(
            swapper.swap("https://stary.example.com", &tx).await,
            SwapOutcome::Unchanged
        );
    }

    #[tokio::test]
    async fn dropping_the_swapper_cancels_the_pending_deletion() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        generate_slates(&options(ffmpeg.clone(), root.path(), "", 0))
            .await
            .unwrap();
        let swapper = swapper(ffmpeg, root.path(), "", Duration::from_secs(1));
        let (tx, _rx) = mpsc::channel(4);
        assert!(matches!(
            swapper.swap("https://a.example.com", &tx).await,
            SwapOutcome::Replaced { .. }
        ));
        // A restarted server wipes the folders itself and must not lose its new ones to a
        // deletion the old server scheduled.
        drop(swapper);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(root.path().join("idle").is_dir());
    }

    /// Two swaps race while the address changes: the address is read under the swapper's lock,
    /// so the set that ends up on air is for the newest one (never N+1 then a stale N).
    #[tokio::test]
    async fn racing_swaps_end_on_the_latest_address() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let swapper = swapper(ffmpeg.clone(), root.path(), "", Duration::from_secs(3600));
        let (tx, mut rx) = mpsc::channel(4);
        let saved = std::sync::Mutex::new("https://a.example.com".to_string());
        let read = || saved.lock().unwrap().clone();
        let first = swapper.swap_to(read, &tx);
        let second = async {
            // Starts while the first one holds the lock (generating) and sees a newer address.
            tokio::time::sleep(Duration::from_millis(300)).await;
            *saved.lock().unwrap() = "https://b.example.com".to_string();
            swapper.swap_to(read, &tx).await
        };
        let (a, b) = tokio::join!(first, second);
        assert_eq!(a, SwapOutcome::Replaced { generation: 1 });
        assert_eq!(b, SwapOutcome::Replaced { generation: 2 });
        let Ok(Command::ReplaceSlates(one)) = rx.try_recv() else {
            panic!("no first set");
        };
        let Ok(Command::ReplaceSlates(two)) = rx.try_recv() else {
            panic!("no second set");
        };
        assert_eq!((one.generation, two.generation), (1, 2));
        assert_eq!(
            qr_in_segment(&ffmpeg, &two.idle[0].path, 1.0),
            vec!["https://b.example.com/".to_string()]
        );
        // Nothing stale follows.
        assert_eq!(swapper.swap_to(read, &tx).await, SwapOutcome::Unchanged);
    }

    #[tokio::test]
    async fn a_stopped_loop_fails_the_swap_and_keeps_the_old_folder() {
        let Some(ffmpeg) = ffmpeg_bin() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("idle")).unwrap();
        let swapper = swapper(ffmpeg, root.path(), "", Duration::from_millis(10));
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let outcome = swapper.swap("https://nowy.example.com", &tx).await;
        assert!(matches!(outcome, SwapOutcome::Failed(_)), "{outcome:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(root.path().join("idle").is_dir());
    }

    /// Final review M1: an aborted slate generation (a swap cancelled, the app quitting) stops
    /// its ffmpeg instead of leaving it running. The "ffmpeg" here is the fake cloudflared
    /// (`examples/fake_cloudflared.rs`, built by a plain `cargo test`) scripted to run until
    /// killed; a process whose working directory is a slate folder keeps that folder from
    /// being deleted on Windows.
    #[cfg(windows)]
    #[tokio::test]
    async fn an_aborted_generation_stops_its_ffmpeg() {
        let exe = std::env::current_exe().unwrap();
        let fake = exe
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join("fake_cloudflared.exe");
        assert!(
            fake.exists(),
            "{} is missing: run `cargo build --example fake_cloudflared`",
            fake.display()
        );
        let bin = tempfile::tempdir().unwrap();
        let ffmpeg = bin.path().join("ffmpeg.exe");
        if std::fs::hard_link(&fake, &ffmpeg).is_err() {
            std::fs::copy(&fake, &ffmpeg).unwrap();
        }
        std::fs::write(
            bin.path().join("fake_cloudflared.json"),
            r#"[{"serve_metrics": false}]"#,
        )
        .unwrap();
        let out = tempfile::tempdir().unwrap();
        let slates = out.path().join("slates");
        let options = options(ffmpeg, &slates, "https://tv.example.com/", 0);
        let generation = tokio::spawn(async move { generate_slates(&options).await });
        let log = bin.path().join("fake_cloudflared.log");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .count()
            < 3
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the three ffmpeg did not start"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        generation.abort();
        let _ = generation.await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while let Err(e) = std::fs::remove_dir_all(&slates) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the slate folder is still in use (ffmpeg left running): {e}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
