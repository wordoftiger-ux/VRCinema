//! The look of the slates: the last frame of the film behind the waiting slates (dimmed or
//! blurred) and the owner's own background (a picture, a GIF or a short video) behind the idle
//! slate and the countdown.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::i18n::Msg;
use crate::pipeline::slates::SlateKind;

// ---------- Settings ----------

/// How the last frame of the film shows behind the waiting slates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameStyle {
    /// Darker (the default).
    #[default]
    Dim,
    /// Blurred and a little darker.
    Blur,
    /// No frame: the plain slate.
    Off,
}

impl FrameStyle {
    pub const ALL: [FrameStyle; 3] = [Self::Dim, Self::Blur, Self::Off];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dim => "dim",
            Self::Blur => "blur",
            Self::Off => "off",
        }
    }
}

/// The channel's slate look settings (keys `slateFrameStyle`, `slateBackgroundAll`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlateLookSettings {
    pub slate_frame_style: FrameStyle,
    /// The owner's background also behind the pause, buffering, "technical problem" and "be
    /// right back" slates (when no last frame shows there).
    pub slate_background_all: bool,
}

/// A validated partial update of [`SlateLookSettings`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlateLookPatch {
    pub frame_style: Option<FrameStyle>,
    pub background_all: Option<bool>,
}

/// Validates one settings key; `None`: not a slate look key.
pub fn sanitize_key(key: &str, value: &Value, out: &mut SlateLookPatch) -> Option<Result<(), Msg>> {
    match key {
        "slateFrameStyle" => Some(
            value
                .as_str()
                .and_then(|v| FrameStyle::ALL.into_iter().find(|s| s.as_str() == v))
                .map(|s| out.frame_style = Some(s))
                .ok_or_else(|| {
                    let all: Vec<&str> = FrameStyle::ALL.iter().map(|s| s.as_str()).collect();
                    Msg::error("setting_not_one_of")
                        .arg("key", key)
                        .arg("values", all.join(", "))
                }),
        ),
        "slateBackgroundAll" => Some(
            value
                .as_bool()
                .map(|b| out.background_all = Some(b))
                .ok_or_else(|| Msg::error("setting_not_boolean").arg("key", key)),
        ),
        _ => None,
    }
}

impl SlateLookPatch {
    /// The stored JSON texts of the keys present.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(style) = self.frame_style {
            out.push(("slateFrameStyle", format!("\"{}\"", style.as_str())));
        }
        if let Some(all) = self.background_all {
            out.push(("slateBackgroundAll", all.to_string()));
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.frame_style.is_none() && self.background_all.is_none()
    }
}

impl SlateLookSettings {
    /// Applies a stored value; `false`: not a slate look key. A wrong value keeps the default.
    pub fn read_stored(&mut self, key: &str, value: &Value) -> bool {
        let mut patch = SlateLookPatch::default();
        match sanitize_key(key, value, &mut patch) {
            None => false,
            Some(Err(_)) => true,
            Some(Ok(())) => {
                self.apply(&patch);
                true
            }
        }
    }

    pub fn apply(&mut self, p: &SlateLookPatch) {
        if let Some(style) = p.frame_style {
            self.slate_frame_style = style;
        }
        if let Some(all) = p.background_all {
            self.slate_background_all = all;
        }
    }
}

// ---------- The owner's background ----------

/// The folder (in the data folder) of the owner's background.
pub const BACKGROUND_DIR: &str = "slate-background";
/// Largest upload.
pub const MAX_UPLOAD_BYTES: usize = 30 * 1024 * 1024;
/// Largest side of an uploaded picture or video (pixels).
pub const MAX_SIDE: u64 = 4096;
/// Longest GIF or video (seconds).
pub const MAX_MOTION_S: u32 = 30;
const META_FILE: &str = "meta.json";
const STILL_FILE: &str = "background.png";
const MOTION_FILE: &str = "background.mp4";
const UPLOAD_FILE: &str = "upload.tmp";
/// How long one ffprobe or ffmpeg run of the background may take.
const TOOL_TIMEOUT: Duration = Duration::from_secs(180);

/// The owner's background, normalised: a 1920×1080 PNG or a 1920×1080 30 fps H.264 video of
/// whole seconds, without sound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Background {
    #[serde(skip)]
    pub file: PathBuf,
    pub motion: bool,
    /// Length of the video (whole seconds); 0 for a picture.
    pub seconds: u32,
    /// When it was set (Unix ms): a new upload makes a new slate set.
    pub stamp: u64,
    /// What the owner gave: `image`, `gif` or `video`.
    pub source: String,
}

impl Background {
    /// The background stored in `data_dir`, if there is a usable one.
    pub fn load(data_dir: &Path) -> Option<Self> {
        let dir = data_dir.join(BACKGROUND_DIR);
        let meta: Value = serde_json::from_slice(&std::fs::read(dir.join(META_FILE)).ok()?).ok()?;
        let motion = meta["motion"].as_bool()?;
        let file = dir.join(if motion { MOTION_FILE } else { STILL_FILE });
        if !file.is_file() {
            return None;
        }
        Some(Self {
            file: std::path::absolute(&file).unwrap_or(file),
            motion,
            seconds: u32::try_from(meta["seconds"].as_u64().unwrap_or(0)).unwrap_or(0),
            stamp: meta["stamp"].as_u64().unwrap_or(0),
            source: meta["source"].as_str().unwrap_or("image").to_string(),
        })
    }

    /// The name of its copy in a slate set's folder.
    pub fn placed_name(&self) -> &'static str {
        if self.motion { "bg.mp4" } else { "bg.png" }
    }

    /// Puts a link to the background (a copy where linking fails) into `dir` under
    /// [`Self::placed_name`].
    pub fn place(&self, dir: &Path) -> std::io::Result<()> {
        let target = dir.join(self.placed_name());
        let _ = std::fs::remove_file(&target);
        if std::fs::hard_link(&self.file, &target).is_err() {
            std::fs::copy(&self.file, &target)?;
        }
        Ok(())
    }
}

/// Removes the owner's background (the default slates come back).
pub fn remove_background(data_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(data_dir.join(BACKGROUND_DIR)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What the first bytes say the file is: the demuxer to read it with, whether it moves and
/// the kind for the owner. Only these are accepted (never a playlist or a text format, which
/// could make ffmpeg open other files or addresses).
fn sniff(bytes: &[u8]) -> Option<(&'static str, bool, &'static str)> {
    let at = |offset: usize, magic: &[u8]| bytes.get(offset..offset + magic.len()) == Some(magic);
    if at(0, b"\x89PNG\r\n\x1a\n") {
        Some(("png_pipe", false, "image"))
    } else if at(0, b"\xff\xd8\xff") {
        Some(("jpeg_pipe", false, "image"))
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some(("webp_pipe", false, "image"))
    } else if at(0, b"BM") {
        Some(("bmp_pipe", false, "image"))
    } else if at(0, b"GIF87a") || at(0, b"GIF89a") {
        Some(("gif", true, "gif"))
    } else if at(4, b"ftyp") {
        Some(("mov", true, "video"))
    } else if at(0, b"\x1a\x45\xdf\xa3") {
        Some(("matroska", true, "video"))
    } else {
        None
    }
}

/// Runs `program` in `cwd` (when given) and returns its error output on failure.
pub(crate) async fn run_tool(
    program: &Path,
    args: Vec<OsString>,
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<Vec<u8>, Msg> {
    let mut cmd = crate::process::command(program);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let child = crate::process::spawn(&mut cmd).map_err(Msg::text)?;
    // A timeout drops the child: it is killed (`kill_on_drop`).
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| Msg::text(format!("{}: timeout", program.display())))?
        .map_err(Msg::text)?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        Err(Msg::text(last.trim()))
    }
}

/// The tools the background needs.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Where the owner's background lives and what makes it (the HTTP routes hold one).
#[derive(Debug, Clone)]
pub struct SlateMedia {
    pub data_dir: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// The slates' font (the preview draws the idle slate's texts).
    pub font_file: PathBuf,
}

impl SlateMedia {
    pub fn tools(&self) -> Tools {
        Tools {
            ffmpeg: self.ffmpeg.clone(),
            ffprobe: self.ffprobe.clone(),
        }
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// Checks `bytes` (an image, a GIF or a short video) with ffprobe, turns them into the
/// normalised background in `data_dir` and returns it. The previous background stays until
/// the new one is ready. Errors are for the owner.
pub async fn store_background(
    tools: &Tools,
    data_dir: &Path,
    bytes: &[u8],
) -> Result<Background, Msg> {
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(Msg::error("slate_background_too_big").arg("max", MAX_UPLOAD_BYTES >> 20));
    }
    let (format, motion, source) =
        sniff(bytes).ok_or_else(|| Msg::error("slate_background_unknown"))?;
    let dir = std::path::absolute(data_dir.join(BACKGROUND_DIR)).map_err(Msg::text)?;
    std::fs::create_dir_all(&dir).map_err(Msg::text)?;
    let upload = dir.join(UPLOAD_FILE);
    std::fs::write(&upload, bytes).map_err(Msg::text)?;
    let result = convert(tools, &dir, &upload, format, motion, source).await;
    let _ = std::fs::remove_file(&upload);
    result
}

async fn convert(
    tools: &Tools,
    dir: &Path,
    upload: &Path,
    format: &str,
    motion: bool,
    source: &str,
) -> Result<Background, Msg> {
    let input = |extra: &[&str]| {
        let mut args = os(&["-v", "error", "-protocol_whitelist", "file", "-f", format]);
        args.extend(os(extra));
        args.push("-i".into());
        args.push(upload.as_os_str().to_owned());
        args
    };
    let mut probe = input(&[]);
    probe.extend(os(&[
        "-show_entries",
        "format=duration:stream=codec_type,width,height",
        "-of",
        "json",
    ]));
    let unknown = || Msg::error("slate_background_unknown");
    let out = run_tool(&tools.ffprobe, probe, None, TOOL_TIMEOUT)
        .await
        .map_err(|_| unknown())?;
    let info: Value = serde_json::from_slice(&out).map_err(|_| unknown())?;
    let video = info["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["codec_type"] == "video"))
        .ok_or_else(unknown)?;
    let (w, h) = (
        video["width"].as_u64().unwrap_or(0),
        video["height"].as_u64().unwrap_or(0),
    );
    if w == 0 || h == 0 {
        return Err(unknown());
    }
    if w > MAX_SIDE || h > MAX_SIDE {
        return Err(Msg::error("slate_background_too_large").arg("max", MAX_SIDE));
    }
    let duration = info["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);
    if motion && duration.is_some_and(|d| d > f64::from(MAX_MOTION_S) + 0.5) {
        return Err(Msg::error("slate_background_too_long").arg("max", MAX_MOTION_S));
    }
    // Whole seconds (a loop of whole segments), at least one.
    let seconds = duration.map_or(1, |d| (d.floor() as u32).clamp(1, MAX_MOTION_S));
    let cover = "scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,setsar=1";
    let (file, mut args) = if motion {
        let mut args = input(&[]);
        let vf = format!("fps=30,{cover},format=yuv420p");
        let t = seconds.to_string();
        args.extend(os(&[
            "-t",
            &t,
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &vf,
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "20",
            "-g",
            "30",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
            "-y",
        ]));
        (MOTION_FILE, args)
    } else {
        let mut args = input(&[]);
        args.extend(os(&[
            "-frames:v",
            "1",
            "-vf",
            cover,
            "-f",
            "image2",
            "-c:v",
            "png",
            "-y",
        ]));
        (STILL_FILE, args)
    };
    let fresh = dir.join(format!("new-{file}"));
    args.push(fresh.as_os_str().to_owned());
    if let Err(e) = run_tool(&tools.ffmpeg, args, None, TOOL_TIMEOUT).await {
        let _ = std::fs::remove_file(&fresh);
        return Err(Msg::error("slate_background_failed").arg_msg("error", e));
    }
    // The new one replaces whatever was there (a picture replaces a video and back).
    for old in [STILL_FILE, MOTION_FILE] {
        let _ = std::fs::remove_file(dir.join(old));
    }
    std::fs::rename(&fresh, dir.join(file)).map_err(Msg::text)?;
    let stamp = u64::try_from(crate::db::now_ms()).unwrap_or_default();
    let meta = json!({
        "motion": motion,
        "seconds": if motion { seconds } else { 0 },
        "stamp": stamp,
        "source": source,
        "width": w,
        "height": h,
    });
    std::fs::write(dir.join(META_FILE), meta.to_string()).map_err(Msg::text)?;
    Background::load(dir.parent().unwrap_or(dir)).ok_or_else(unknown)
}

// ---------- What the slates are drawn with ----------

/// The look of one slate set: the frame style (for the waiting slates, see
/// [`crate::pipeline::slates::BackdropSpec`]) and the owner's background. The default is the
/// plain look of old (no frame, no background).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlateLook {
    pub frame_style: FrameStyle,
    pub background: Option<Background>,
    pub background_all: bool,
}

impl Default for SlateLook {
    fn default() -> Self {
        Self {
            frame_style: FrameStyle::Off,
            background: None,
            background_all: false,
        }
    }
}

impl SlateLook {
    /// The look the channel's settings and the stored background give.
    pub fn read(data_dir: &Path, settings: &SlateLookSettings) -> Self {
        Self {
            frame_style: settings.slate_frame_style,
            background: Background::load(data_dir),
            background_all: settings.slate_background_all,
        }
    }

    /// The owner's background behind a pre-made slate of `kind`: always behind the idle slate,
    /// behind the others when asked to.
    pub fn background_for(&self, kind: SlateKind) -> Option<&Background> {
        match kind {
            SlateKind::Idle => self.background.as_ref(),
            _ if self.background_all => self.background.as_ref(),
            _ => None,
        }
    }
}

/// How a picture behind the texts is toned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The owner's background: a little darker, so the texts read.
    Custom,
    /// The last frame, darker.
    Dim,
    /// The last frame, blurred.
    Blur,
}

impl Tone {
    /// The tone of the last frame in `style` (`None`: no frame).
    pub fn of_frame(style: FrameStyle) -> Option<Self> {
        match style {
            FrameStyle::Dim => Some(Self::Dim),
            FrameStyle::Blur => Some(Self::Blur),
            FrameStyle::Off => None,
        }
    }

    fn filters(self) -> &'static str {
        match self {
            Self::Custom => "eq=brightness=-0.10",
            Self::Dim => "eq=brightness=-0.28:saturation=0.75",
            Self::Blur => "gblur=sigma=28,eq=brightness=-0.15",
        }
    }
}

/// What a slate is drawn on. Names are files in ffmpeg's working directory (no escaping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop<'a> {
    /// The dark plain background.
    Plain,
    /// A picture, scaled to cover the frame, toned, held for the whole slate.
    Picture { name: &'a str, tone: Tone },
    /// A normalised video (1920×1080, 30 fps), looped.
    Motion { name: &'a str },
}

impl Backdrop<'_> {
    /// The texts get a shadow over a picture.
    pub fn is_plain(&self) -> bool {
        matches!(self, Self::Plain)
    }
}

/// The source of a slate's lavfi chain: 1920×1080, 30 fps, yuv420p, `duration` (ffmpeg's
/// number text) long, starting at 0. The texts are drawn onto it.
pub fn backdrop_source(backdrop: Backdrop<'_>, duration: &str) -> String {
    match backdrop {
        Backdrop::Plain => format!("color=c=0x14161c:s=1920x1080:r=30:d={duration}"),
        Backdrop::Picture { name, tone } => format!(
            "movie={name},scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,\
             setsar=1,{},format=yuv420p,loop=loop=-1:size=1,fps=30,trim=duration={duration},\
             setpts=PTS-STARTPTS",
            tone.filters()
        ),
        Backdrop::Motion { name } => format!(
            "movie={name}:loop=0,setpts=N/(30*TB),{},format=yuv420p,trim=duration={duration},\
             setpts=PTS-STARTPTS",
            Tone::Custom.filters()
        ),
    }
}

/// The drawtext options added over a picture (a shadow keeps the texts readable).
pub fn text_shadow(backdrop: Backdrop<'_>) -> &'static str {
    if backdrop.is_plain() {
        ""
    } else {
        ":shadowcolor=black@0.7:shadowx=3:shadowy=3"
    }
}

/// What the countdown is drawn on: the last frame of the film (the kept segment, its length
/// and the style) when it shows, else the owner's background, else the plain background.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CountdownBackdrop {
    pub frame: Option<(PathBuf, f64, FrameStyle)>,
    pub background: Option<Background>,
}

impl CountdownBackdrop {
    /// Puts what the countdown needs into its folder `dir` (ffmpeg's working directory) and
    /// returns the backdrop to draw on; whatever fails falls back to the next choice.
    pub async fn prepare(&self, ffmpeg: &Path, dir: &Path) -> Backdrop<'static> {
        if let Some((segment, duration, style)) = &self.frame
            && let Some(tone) = Tone::of_frame(*style)
            && extract_last_frame(ffmpeg, segment, *duration, &dir.join("frame.png"))
                .await
                .is_ok()
        {
            return Backdrop::Picture {
                name: "frame.png",
                tone,
            };
        }
        match &self.background {
            Some(bg) if bg.place(dir).is_ok() => {
                if bg.motion {
                    Backdrop::Motion { name: "bg.mp4" }
                } else {
                    Backdrop::Picture {
                        name: "bg.png",
                        tone: Tone::Custom,
                    }
                }
            }
            _ => Backdrop::Plain,
        }
    }
}

/// Decodes the last frame of `segment` (`duration_s` long; 0: unknown) into the PNG `out`,
/// fitted into 1920×1080. Only the end of the segment is converted to a picture.
pub async fn extract_last_frame(
    ffmpeg: &Path,
    segment: &Path,
    duration_s: f64,
    out: &Path,
) -> Result<(), Msg> {
    let fit = "scale=1920:1080:force_original_aspect_ratio=decrease,\
               pad=1920:1080:(ow-iw)/2:(oh-ih)/2,setsar=1";
    let vf = if duration_s > 0.3 {
        format!(
            "setpts=PTS-STARTPTS,select=gte(t\\,{:.3}),{fit}",
            duration_s - 0.25
        )
    } else {
        fit.to_string()
    };
    let mut args = os(&["-v", "error", "-y", "-i"]);
    args.push(segment.as_os_str().to_owned());
    args.extend(os(&[
        "-an",
        "-sn",
        "-dn",
        "-vf",
        &vf,
        "-c:v",
        "png",
        "-compression_level",
        "1",
        "-update",
        "1",
        "-f",
        "image2",
    ]));
    args.push(out.as_os_str().to_owned());
    let _ = std::fs::remove_file(out);
    run_tool(ffmpeg, args, None, Duration::from_secs(20)).await?;
    if out.is_file() {
        Ok(())
    } else {
        Err(Msg::text("no frame"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(name: &str) -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
        [format!("{name}.exe"), name.to_string()]
            .iter()
            .map(|n| root.join("bin").join(n))
            .find(|p| p.exists())
    }

    pub(crate) fn tools() -> Option<Tools> {
        Some(Tools {
            ffmpeg: bin("ffmpeg")?,
            ffprobe: bin("ffprobe")?,
        })
    }

    /// Makes a test file with ffmpeg from a lavfi source.
    pub(crate) fn make(ffmpeg: &Path, source: &str, args: &[&str], out: &Path) {
        let status = std::process::Command::new(ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", source])
            .args(args)
            .arg(out)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn settings_keys_are_validated_and_read_back() {
        let mut patch = SlateLookPatch::default();
        assert!(
            sanitize_key("slateFrameStyle", &json!("blur"), &mut patch).is_some_and(|r| r.is_ok())
        );
        assert!(
            sanitize_key("slateBackgroundAll", &json!(true), &mut patch).is_some_and(|r| r.is_ok())
        );
        assert!(
            sanitize_key("slateFrameStyle", &json!("sepia"), &mut patch)
                .is_some_and(|r| r.is_err())
        );
        assert!(
            sanitize_key("slateBackgroundAll", &json!(1), &mut patch).is_some_and(|r| r.is_err())
        );
        assert!(sanitize_key("other", &json!(1), &mut patch).is_none());
        assert_eq!(
            patch.entries(),
            vec![
                ("slateFrameStyle", "\"blur\"".to_string()),
                ("slateBackgroundAll", "true".to_string())
            ]
        );
        let mut s = SlateLookSettings::default();
        assert_eq!(s.slate_frame_style, FrameStyle::Dim);
        assert!(s.read_stored("slateFrameStyle", &json!("off")));
        assert!(s.read_stored("slateFrameStyle", &json!("nonsense")));
        assert!(!s.read_stored("tickerEnabled", &json!(true)));
        assert_eq!(s.slate_frame_style, FrameStyle::Off);
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            json!({"slateFrameStyle": "off", "slateBackgroundAll": false})
        );
    }

    #[test]
    fn only_known_pictures_and_videos_are_sniffed() {
        assert_eq!(
            sniff(b"\x89PNG\r\n\x1a\nxxxx").map(|s| s.0),
            Some("png_pipe")
        );
        assert_eq!(sniff(b"GIF89a....").map(|s| s.1), Some(true));
        assert_eq!(sniff(b"\0\0\0\x20ftypisom").map(|s| s.0), Some("mov"));
        assert_eq!(
            sniff(b"RIFF\0\0\0\0WEBPVP8 ").map(|s| s.0),
            Some("webp_pipe")
        );
        // A playlist or any text is refused before ffprobe sees it.
        assert_eq!(sniff(b"#EXTM3U\nhttp://192.168.1.1/x.ts"), None);
        assert_eq!(sniff(b"ffconcat version 1.0"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn the_background_shows_behind_idle_and_on_request_behind_the_others() {
        let bg = Background {
            file: "x".into(),
            motion: false,
            seconds: 0,
            stamp: 1,
            source: "image".into(),
        };
        let mut look = SlateLook {
            background: Some(bg),
            ..SlateLook::default()
        };
        assert!(look.background_for(SlateKind::Idle).is_some());
        assert!(look.background_for(SlateKind::Pause).is_none());
        look.background_all = true;
        assert!(look.background_for(SlateKind::Brb).is_some());
        assert_eq!(SlateLook::default().frame_style, FrameStyle::Off);
    }

    #[test]
    fn backdrop_sources_keep_the_plain_slate_and_loop_pictures() {
        assert_eq!(
            backdrop_source(Backdrop::Plain, "8"),
            "color=c=0x14161c:s=1920x1080:r=30:d=8"
        );
        let p = backdrop_source(
            Backdrop::Picture {
                name: "frame.jpg",
                tone: Tone::Blur,
            },
            "2",
        );
        assert!(
            p.starts_with("movie=frame.jpg,")
                && p.contains("gblur")
                && p.contains("loop=loop=-1:size=1"),
            "{p}"
        );
        assert!(p.ends_with("trim=duration=2,setpts=PTS-STARTPTS"), "{p}");
        let m = backdrop_source(Backdrop::Motion { name: "../bg.mp4" }, "5");
        assert!(m.starts_with("movie=../bg.mp4:loop=0,"), "{m}");
        assert_eq!(text_shadow(Backdrop::Plain), "");
    }

    #[tokio::test]
    async fn uploads_are_checked_and_normalised() {
        let Some(tools) = tools() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        // A picture: 800×600 PNG → 1920×1080 PNG.
        let png = src.path().join("a.png");
        make(
            &tools.ffmpeg,
            "testsrc2=s=800x600",
            &["-frames:v", "1"],
            &png,
        );
        let bg = store_background(&tools, data.path(), &std::fs::read(&png).unwrap())
            .await
            .unwrap();
        assert!(!bg.motion && bg.source == "image");
        let img = image::open(&bg.file).unwrap();
        assert_eq!((img.width(), img.height()), (1920, 1080));
        assert_eq!(Background::load(data.path()), Some(bg.clone()));
        // A GIF: a looping video of whole seconds replaces it.
        let gif = src.path().join("a.gif");
        make(&tools.ffmpeg, "testsrc2=s=320x240:r=10:d=2.4", &[], &gif);
        let moving = store_background(&tools, data.path(), &std::fs::read(&gif).unwrap())
            .await
            .unwrap();
        assert!(moving.motion && moving.seconds == 2 && moving.source == "gif");
        assert!(!data.path().join(BACKGROUND_DIR).join(STILL_FILE).exists());
        assert!(!data.path().join(BACKGROUND_DIR).join(UPLOAD_FILE).exists());
        // Too long, too big, not a picture: refused, the background stays.
        let long = src.path().join("long.mp4");
        make(
            &tools.ffmpeg,
            "testsrc2=s=160x120:r=5:d=40",
            &["-c:v", "libx264", "-pix_fmt", "yuv420p"],
            &long,
        );
        let err = store_background(&tools, data.path(), &std::fs::read(&long).unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.key(), "errors.slate_background_too_long");
        let huge = src.path().join("huge.png");
        make(
            &tools.ffmpeg,
            "color=s=4100x200",
            &["-frames:v", "1"],
            &huge,
        );
        let err = store_background(&tools, data.path(), &std::fs::read(&huge).unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.key(), "errors.slate_background_too_large");
        let err = store_background(&tools, data.path(), b"#EXTM3U\nfile.ts")
            .await
            .unwrap_err();
        assert_eq!(err.key(), "errors.slate_background_unknown");
        // A PNG header with garbage after it: ffprobe refuses it.
        let err = store_background(&tools, data.path(), b"\x89PNG\r\n\x1a\nnot really")
            .await
            .unwrap_err();
        assert_eq!(err.key(), "errors.slate_background_unknown");
        let err = store_background(&tools, data.path(), &vec![0u8; MAX_UPLOAD_BYTES + 1])
            .await
            .unwrap_err();
        assert_eq!(err.key(), "errors.slate_background_too_big");
        assert_eq!(Background::load(data.path()), Some(moving));
        remove_background(data.path()).unwrap();
        assert_eq!(Background::load(data.path()), None);
        remove_background(data.path()).unwrap();
    }

    #[tokio::test]
    async fn the_last_frame_of_a_segment_is_decoded() {
        let Some(tools) = tools() else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let seg = dir.path().join("seg.ts");
        // Red for a second then green: the last frame is green.
        make(
            &tools.ffmpeg,
            "color=c=red:s=1280x720:r=30:d=1[a];color=c=lime:s=1280x720:r=30:d=1[b];[a][b]concat=n=2:v=1:a=0",
            &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "mpegts"],
            &seg,
        );
        let out = dir.path().join("frame.png");
        extract_last_frame(&tools.ffmpeg, &seg, 2.0, &out)
            .await
            .unwrap();
        let img = image::open(&out).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (1920, 1080));
        let p = img.get_pixel(960, 540).0;
        assert!(p[1] > 200 && p[0] < 60, "{p:?}");
        assert!(
            extract_last_frame(&tools.ffmpeg, &dir.path().join("none.ts"), 1.0, &out)
                .await
                .is_err()
        );
    }
}
