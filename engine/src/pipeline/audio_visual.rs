//! Audio-only sources (mp3, flac, a SoundCloud link, ...) air as video with a picture made from
//! the sound: the cover art, a waveform, a spectrum or a plain background with the title and the
//! artist. Spec: `docs/superpowers/specs/2026-11-08-audio-design.md`.
//!
//! Per clip the still part (background, cover, texts) is drawn once into the clip folder
//! ([`STILL_FILE`]); the clip's graph ([`clip_graph`]) loops it at [`FPS`] and overlays the
//! visualiser (or a hidden clock) made from `[0:a:0]`, so the picture follows the sound's pace
//! and ends with it. Everything else (cap, ticker, fades, encoder) is the converted chain of
//! [`crate::pipeline::ffmpeg_args::build_clip_args_ext`].

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::i18n::Msg;
use crate::pipeline::ffmpeg_args::input_options;
use crate::pipeline::slate_look::run_tool;
use crate::resolver::types::StreamInput;

// ---------- Settings ----------

/// What an audio-only source shows (`audioVisual`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioLook {
    /// The cover art when there is one, else the waveform (the default).
    #[default]
    Auto,
    /// The cover art (without one: the plain background).
    Cover,
    Waveform,
    /// Spectrum bars.
    Spectrum,
    /// The plain background with the title and the artist.
    Off,
}

impl AudioLook {
    pub const ALL: [AudioLook; 5] = [
        Self::Auto,
        Self::Cover,
        Self::Waveform,
        Self::Spectrum,
        Self::Off,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cover => "cover",
            Self::Waveform => "waveform",
            Self::Spectrum => "spectrum",
            Self::Off => "off",
        }
    }
}

/// The channel's audio settings (key `audioVisual`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    pub audio_visual: AudioLook,
}

/// A validated partial update of [`AudioSettings`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioPatch {
    pub visual: Option<AudioLook>,
}

/// Validates one settings key; `None`: not an audio key.
pub fn sanitize_key(key: &str, value: &Value, out: &mut AudioPatch) -> Option<Result<(), Msg>> {
    match key {
        "audioVisual" => Some(
            value
                .as_str()
                .and_then(|v| AudioLook::ALL.into_iter().find(|l| l.as_str() == v))
                .map(|l| out.visual = Some(l))
                .ok_or_else(|| {
                    let all: Vec<&str> = AudioLook::ALL.iter().map(|l| l.as_str()).collect();
                    Msg::error("setting_not_one_of")
                        .arg("key", key)
                        .arg("values", all.join(", "))
                }),
        ),
        _ => None,
    }
}

impl AudioPatch {
    /// The stored JSON texts of the keys present.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        self.visual
            .map(|l| ("audioVisual", format!("\"{}\"", l.as_str())))
            .into_iter()
            .collect()
    }
}

impl AudioSettings {
    /// Applies a stored value; `false`: not an audio key. A wrong value keeps the default.
    pub fn read_stored(&mut self, key: &str, value: &Value) -> bool {
        let mut patch = AudioPatch::default();
        match sanitize_key(key, value, &mut patch) {
            None => false,
            Some(result) => {
                if result.is_ok()
                    && let Some(look) = patch.visual
                {
                    self.audio_visual = look;
                }
                true
            }
        }
    }
}

// ---------- The picture ----------

/// Frames per second of the picture.
pub const FPS: u32 = 30;
/// The cover picture extracted into the clip folder.
pub const COVER_FILE: &str = "cover.png";
/// The still part of the picture, drawn into the clip folder.
pub const STILL_FILE: &str = "bg.png";
/// The slates' background colour.
const BG: &str = "0x14161c";
/// The colour of the visualisers (the ticker's accent).
const ACCENT: &str = "0xf2b84b";
const TITLE_COLOR: &str = "white";
const ARTIST_COLOR: &str = "0xb8bcc8";
/// How long reading the cover and drawing the still picture may take.
const TOOL_TIMEOUT: Duration = Duration::from_secs(20);

/// What is drawn, once the choice met the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    Cover,
    Waveform,
    Spectrum,
    Plain,
}

impl Picture {
    /// A technical word for the log.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Waveform => "waveform",
            Self::Spectrum => "spectrum",
            Self::Plain => "plain",
        }
    }
}

/// The picture of `look` for a source with or without a cover.
pub fn picture_for(look: AudioLook, has_cover: bool) -> Picture {
    match look {
        AudioLook::Auto | AudioLook::Cover if has_cover => Picture::Cover,
        AudioLook::Auto | AudioLook::Waveform => Picture::Waveform,
        AudioLook::Spectrum => Picture::Spectrum,
        AudioLook::Cover | AudioLook::Off => Picture::Plain,
    }
}

/// Whether `look` may show the cover (only then is it read).
pub fn wants_cover(look: AudioLook) -> bool {
    matches!(look, AudioLook::Auto | AudioLook::Cover)
}

/// The 16:9 frame of a picture `height` lines high (even sides, 144..=1080 lines).
pub fn canvas(height: u32) -> (u32, u32) {
    let h = height.clamp(144, 1080) & !1;
    ((h * 16 / 9 + 1) & !1, h)
}

fn even(v: f64) -> u32 {
    ((v.round() as u32) / 2 * 2).max(2)
}

/// Where the cover comes from: the file's attached picture, or the site's thumbnail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverSource {
    Embedded { input: StreamInput, stream: i64 },
    Thumbnail(StreamInput),
}

/// ffmpeg's arguments that write the cover (at most 1080 px a side) as [`COVER_FILE`] in its
/// working folder.
pub fn cover_args(source: &CoverSource) -> Vec<OsString> {
    let (input, map) = match source {
        CoverSource::Embedded { input, stream } => (input, format!("0:{stream}")),
        CoverSource::Thumbnail(input) => (input, "0:v:0".to_string()),
    };
    let mut args: Vec<OsString> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]
        .iter()
        .map(OsString::from)
        .collect();
    args.extend(input_options(input));
    args.push("-i".into());
    args.push((&input.url).into());
    for a in [
        "-map",
        &map,
        "-frames:v",
        "1",
        "-vf",
        "scale=w='min(1080,iw)':h='min(1080,ih)':force_original_aspect_ratio=decrease,setsar=1",
        "-update",
        "1",
        COVER_FILE,
    ] {
        args.push(a.into());
    }
    args
}

/// One line of text on the still picture, read by drawtext from `file` in the clip folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub file: String,
    pub text: String,
    pub size: u32,
    pub color: &'static str,
    /// `None`: centred.
    pub x: Option<u32>,
    pub y: u32,
}

/// Width of a character in ems (an estimate: wide East Asian characters are square).
fn char_em(c: char) -> f64 {
    let wide = matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD);
    if wide { 1.0 } else { 0.6 }
}

/// `text` broken into at most `max_lines` lines of at most `max_px` pixels at `size` (words
/// kept whole when they fit; the last line ends with "…" when the text is longer).
pub fn wrap(text: &str, max_px: f64, size: u32, max_lines: usize) -> Vec<String> {
    let width = |s: &str| s.chars().map(char_em).sum::<f64>() * f64::from(size);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let joined = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if width(&joined) <= max_px {
            line = joined;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        // A word longer than a line (or a language without spaces): cut between characters.
        for c in word.chars() {
            let mut next = line.clone();
            next.push(c);
            if width(&next) > max_px && !line.is_empty() {
                lines.push(std::mem::replace(&mut line, c.to_string()));
            } else {
                line = next;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            while !last.is_empty() && width(&format!("{last}…")) > max_px {
                last.pop();
            }
            let kept = last.trim_end().to_string();
            *last = format!("{kept}…");
        }
    }
    lines
}

/// The texts of `picture` on a `size` frame: the title (up to 2 or 3 lines) and the artist.
pub fn layout(picture: Picture, size: (u32, u32), title: &str, artist: &str) -> Vec<Line> {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let k = h / 1080.0;
    let px = |v: f64| (v * k).round() as u32;
    let (title_px, artist_px, max_w, title_lines, x, top) = match picture {
        Picture::Plain => (px(76.0), px(46.0), w * 0.84, 2, None, None),
        Picture::Waveform | Picture::Spectrum => {
            (px(64.0), px(42.0), w * 0.84, 2, None, Some(px(110.0)))
        }
        Picture::Cover => {
            let (art_x, side) = cover_box(size);
            let col = art_x + side + even(w * 0.05);
            let col_w = (w - f64::from(col) - w * 0.06).max(w * 0.2);
            (px(60.0), px(40.0), col_w, 3, Some(col), None)
        }
    };
    let title_lines = wrap(title, max_w, title_px, title_lines);
    let artist_lines = wrap(artist, max_w, artist_px, 1);
    let line_h = |size: u32| (f64::from(size) * 1.25).round() as u32;
    let gap = if artist_lines.is_empty() { 0 } else { px(24.0) };
    let total = title_lines.len() as u32 * line_h(title_px)
        + gap
        + artist_lines.len() as u32 * line_h(artist_px);
    let mut y = top.unwrap_or_else(|| size.1.saturating_sub(total) / 2);
    let mut out = Vec::new();
    for (i, text) in title_lines.into_iter().enumerate() {
        out.push(Line {
            file: format!("av-title-{i}.txt"),
            text,
            size: title_px,
            color: TITLE_COLOR,
            x,
            y,
        });
        y += line_h(title_px);
    }
    y += gap;
    for (i, text) in artist_lines.into_iter().enumerate() {
        out.push(Line {
            file: format!("av-artist-{i}.txt"),
            text,
            size: artist_px,
            color: ARTIST_COLOR,
            x,
            y,
        });
        y += line_h(artist_px);
    }
    out
}

/// The cover's square on the frame: its left edge and side.
fn cover_box(size: (u32, u32)) -> (u32, u32) {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    (even(w * 0.09), even(h * 0.62))
}

/// The lavfi graph of the still picture (one frame): background (the blurred cover), the cover
/// and the texts of `lines` drawn with `font` (relative to the clip folder).
pub fn still_graph(picture: Picture, size: (u32, u32), font: &str, lines: &[Line]) -> String {
    let (w, h) = size;
    let k = f64::from(h) / 1080.0;
    let mut graph = if picture == Picture::Cover {
        let (art_x, side) = cover_box(size);
        let blur = (30.0 * k).round().max(2.0);
        format!(
            "movie={COVER_FILE},split[av_c1][av_c2];\
             [av_c1]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1,\
             gblur=sigma={blur},drawbox=x=0:y=0:w=iw:h=ih:color=black@0.55:t=fill[av_bg];\
             [av_c2]scale={side}:{side}:force_original_aspect_ratio=decrease,setsar=1[av_art];\
             [av_bg][av_art]overlay=x={art_x}+({side}-w)/2:y=(H-h)/2"
        )
    } else {
        format!("color=c={BG}:s={w}x{h}:d=1")
    };
    let shadow = if picture == Picture::Cover {
        let d = (3.0 * k).round().max(1.0);
        format!(":shadowcolor=black@0.7:shadowx={d}:shadowy={d}")
    } else {
        String::new()
    };
    for line in lines {
        let x = line
            .x
            .map_or_else(|| "(w-text_w)/2".to_string(), |x| x.to_string());
        graph.push_str(&format!(
            ",drawtext=fontfile={font}:textfile={file}:expansion=none:fontsize={size}:\
             fontcolor={color}{shadow}:x={x}:y={y}",
            file = line.file,
            size = line.size,
            color = line.color,
            y = line.y,
        ));
    }
    graph.push_str(",format=rgb24");
    graph
}

/// ffmpeg's arguments that draw `graph` into [`STILL_FILE`] (in its working folder).
pub fn still_args(graph: &str) -> Vec<OsString> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        graph,
        "-frames:v",
        "1",
        "-update",
        "1",
        STILL_FILE,
    ]
    .iter()
    .map(OsString::from)
    .collect()
}

/// The clip's picture from `[0:a:0]` (its last output unlabelled, see
/// `ClipExtras::audio_visual`): the still picture looped (`still`; else the plain colour) under
/// the visualiser, or a hidden 2×2 clock that makes the picture end with the sound.
pub fn clip_graph(picture: Picture, size: (u32, u32), still: bool) -> String {
    let (w, h) = size;
    let bg = if still {
        format!("movie={STILL_FILE},format=yuv420p,loop=loop=-1:size=1,fps={FPS}")
    } else {
        format!("color=c={BG}:s={w}x{h}:r={FPS},format=yuv420p")
    };
    let (vh, vy) = (even(f64::from(h) * 0.38), even(f64::from(h) * 0.5));
    let (visual, at) = match picture {
        Picture::Waveform => (
            format!(
                "aformat=channel_layouts=mono,showwaves=s={w}x{vh}:mode=cline:rate={FPS}:\
                 colors={ACCENT}:scale=sqrt:draw=full"
            ),
            format!("x=0:y={vy}"),
        ),
        Picture::Spectrum => (
            format!(
                "aformat=channel_layouts=mono,showfreqs=s={sw}x{vh}:mode=bar:fscale=log:\
                 rate={FPS}:colors={ACCENT}",
                sw = even(f64::from(w) * 0.84)
            ),
            format!("x=(W-w)/2:y={vy}"),
        ),
        Picture::Cover | Picture::Plain => {
            (format!("showwaves=s=2x2:rate={FPS}"), "x=W:y=H".into())
        }
    };
    format!("{bg}[av_bg];[0:a:0]{visual}[av_v];[av_bg][av_v]overlay={at}:shortest=1,format=yuv420p")
}

/// What [`prepare`] needs.
pub struct VisualJob<'a> {
    pub ffmpeg: &'a Path,
    /// The clip folder (ffmpeg's working folder for the clip too).
    pub out_dir: &'a Path,
    pub look: AudioLook,
    /// The height of the converted picture.
    pub height: u32,
    pub title: &'a str,
    pub artist: &'a str,
    /// The font as ffmpeg sees it from the clip folder; `None`: no texts.
    pub font: Option<&'a str>,
    pub cover: Option<CoverSource>,
}

/// The prepared picture of an audio-only clip.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedVisual {
    pub picture: Picture,
    /// `ClipExtras::audio_visual` of the clip.
    pub graph: String,
    /// The same picture without any file of the clip folder (the plain restart runs elsewhere).
    pub plain_graph: String,
    /// What went wrong (a log key and the error); the picture is simpler then, never missing.
    pub problems: Vec<(&'static str, Msg)>,
}

/// Reads the cover (when the look may show it), draws the still picture into the clip folder
/// and returns the clip's graph. A failure falls back to a simpler picture.
pub async fn prepare(job: VisualJob<'_>) -> PreparedVisual {
    let size = canvas(job.height);
    let mut problems = Vec::new();
    let mut has_cover = false;
    if let Some(source) = job.cover.as_ref().filter(|_| wants_cover(job.look)) {
        let read = match std::fs::create_dir_all(job.out_dir) {
            Ok(()) => run_tool(
                job.ffmpeg,
                cover_args(source),
                Some(job.out_dir),
                TOOL_TIMEOUT,
            )
            .await
            .map(drop),
            Err(e) => Err(Msg::text(e)),
        };
        match read {
            Ok(()) => has_cover = job.out_dir.join(COVER_FILE).is_file(),
            Err(e) => problems.push(("log.audio_cover_failed", e)),
        }
    }
    let picture = picture_for(job.look, has_cover);
    let lines = job
        .font
        .map(|_| layout(picture, size, job.title, job.artist))
        .unwrap_or_default();
    let drawn = draw_still(&job, picture, size, &lines).await;
    let (picture, still) = match drawn {
        Ok(()) => (picture, true),
        Err(e) => {
            problems.push(("log.audio_picture_failed", e));
            // Something that still moves with the sound, on the plain colour.
            let simpler = match picture {
                Picture::Cover | Picture::Plain => Picture::Waveform,
                other => other,
            };
            (simpler, false)
        }
    };
    PreparedVisual {
        picture,
        graph: clip_graph(picture, size, still),
        plain_graph: clip_graph(picture, size, false),
        problems,
    }
}

async fn draw_still(
    job: &VisualJob<'_>,
    picture: Picture,
    size: (u32, u32),
    lines: &[Line],
) -> Result<(), Msg> {
    std::fs::create_dir_all(job.out_dir).map_err(Msg::text)?;
    for line in lines {
        std::fs::write(job.out_dir.join(&line.file), &line.text).map_err(Msg::text)?;
    }
    let graph = still_graph(picture, size, job.font.unwrap_or_default(), lines);
    run_tool(
        job.ffmpeg,
        still_args(&graph),
        Some(job.out_dir),
        TOOL_TIMEOUT,
    )
    .await?;
    if job.out_dir.join(STILL_FILE).is_file() {
        Ok(())
    } else {
        Err(Msg::text("ffmpeg: no picture"))
    }
}

#[cfg(test)]
mod tests;
