//! Live capture of the computer the program runs on (spec `2026-11-08-capture-design.md`): a
//! window, the desktop, one screen or a region, a camera, or only a microphone, aired as a live
//! source of the queue.
//!
//! A capture's queue item has the source `capture:<token>`, a kind no link check accepts and no
//! resolver opens. What it captures lives only in this process ([`CaptureRegistry`]), bound to
//! the item it was made for: nothing in the database or the log names a window or a device, and
//! a copy of the item (history, a restart of the program) never captures again by itself.

pub mod list;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::resolver::StreamInput;

/// The source of a capture's queue item: `capture:` and a random token.
pub const SOURCE_PREFIX: &str = "capture:";
/// The frame rates the owner can pick (never above the channel's 30).
pub const FPS_CHOICES: [u32; 3] = [15, 25, 30];
/// The heights the owner can pick (the box is 16:9).
pub const HEIGHT_CHOICES: [u32; 3] = [480, 720, 1080];
/// How long a listing stays the one a new capture is checked against.
const CATALOG_KEEP: Duration = Duration::from_secs(30 * 60);
/// Capture descriptions kept at most (the oldest go first).
const MAX_ENTRIES: usize = 64;
/// The smallest region side.
const MIN_REGION: u32 = 64;
/// The largest region side.
const MAX_REGION: u32 = 16384;
/// The still picture of a capture with sound only.
const SOUND_ONLY_COLOR: &str = "0x0b1020";

/// A `capture:` source (any case of the prefix).
pub fn is_capture_source(url: &str) -> bool {
    url.get(..SOURCE_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(SOURCE_PREFIX))
}

/// The token of a `capture:` source.
pub fn token_of(url: &str) -> Option<&str> {
    is_capture_source(url)
        .then(|| &url[SOURCE_PREFIX.len()..])
        .filter(|t| !t.is_empty())
}

/// One screen of the desktop (its rectangle in desktop coordinates).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    pub index: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// A camera or a sound device: `id` is what ffmpeg opens, `name` what the owner reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// What can be captured on this computer right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    /// `windows`, `linux` or `other`.
    pub platform: &'static str,
    /// The whole desktop (all screens) can be captured.
    pub desktop: bool,
    /// A region of the desktop can be captured.
    pub region: bool,
    pub screens: Vec<Screen>,
    /// Titles of the windows that can be captured.
    pub windows: Vec<String>,
    pub cameras: Vec<Device>,
    pub audio: Vec<Device>,
    /// ffmpeg has `ddagrab` (Desktop Duplication): faster capture of one screen.
    pub fast: bool,
    /// Codes of notes for the owner (`capture.note_<code>`).
    pub notes: Vec<&'static str>,
}

/// Where a capture's ffmpeg runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    /// Linux with this X display (`$DISPLAY`).
    Linux,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

/// The picture of a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Video {
    /// Every screen.
    Desktop,
    Screen(Screen),
    Region {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    /// A window by its title.
    Window(String),
    Camera(Device),
    /// Sound only: a still dark picture.
    None,
}

impl Video {
    /// The kind as the API and the log name it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Desktop | Self::Screen(_) | Self::Region { .. } => "screen",
            Self::Window(_) => "window",
            Self::Camera(_) => "camera",
            Self::None => "none",
        }
    }

    /// The dictionary key naming the kind (the log and the window).
    pub fn kind_key(&self) -> &'static str {
        match self {
            Self::Desktop | Self::Screen(_) | Self::Region { .. } => "capture.kind_screen",
            Self::Window(_) => "capture.kind_window",
            Self::Camera(_) => "capture.kind_camera",
            Self::None => "capture.kind_none",
        }
    }
}

/// What one capture takes and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSpec {
    pub video: Video,
    pub audio: Option<Device>,
    pub fps: u32,
    /// The height of the 16:9 box the picture is fitted into.
    pub height: u32,
    /// One screen through `ddagrab` (Windows, when ffmpeg has it).
    pub fast: bool,
}

/// Why a capture request was refused (an `errors.` code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    /// Malformed request.
    Invalid,
    /// The window or device is not in the current listing (refresh it).
    UnknownSource,
}

impl SpecError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "capture_invalid",
            Self::UnknownSource => "capture_unknown_source",
        }
    }
}

fn text<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

fn int(body: &Value, key: &str) -> Result<i64, SpecError> {
    body.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && v.fract() == 0.0 && v.abs() <= 1e6)
        .map(|v| v as i64)
        .ok_or(SpecError::Invalid)
}

fn device(list: &[Device], id: Option<&str>) -> Result<Device, SpecError> {
    let id = id.ok_or(SpecError::Invalid)?;
    list.iter()
        .find(|d| d.id == id)
        .cloned()
        .ok_or(SpecError::UnknownSource)
}

/// The capture the window asks for, checked against the last listing: only a window, screen or
/// device the listing named can be captured.
///
/// `{"kind": "desktop" | "screen" | "region" | "window" | "camera" | "none", "source": <screen
/// index, window title or camera id>, "region": {x, y, width, height}, "audio": <device id> |
/// null, "fps": 15 | 25 | 30, "height": 480 | 720 | 1080, "fast": bool}`
pub fn parse_spec(body: &Value, catalog: &Catalog) -> Result<CaptureSpec, SpecError> {
    if !body.is_object() {
        return Err(SpecError::Invalid);
    }
    let source = text(body, "source");
    let video = match text(body, "kind").ok_or(SpecError::Invalid)? {
        "desktop" if catalog.desktop => Video::Desktop,
        "screen" => {
            let index = source
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or(SpecError::Invalid)?;
            let screen = catalog.screens.iter().find(|s| s.index == index);
            Video::Screen(screen.cloned().ok_or(SpecError::UnknownSource)?)
        }
        "region" if catalog.region => {
            let r = body.get("region").ok_or(SpecError::Invalid)?;
            let side = |key| {
                u32::try_from(int(r, key)?)
                    .ok()
                    .filter(|v| (MIN_REGION..=MAX_REGION).contains(v))
                    .ok_or(SpecError::Invalid)
            };
            let at = |key| {
                i32::try_from(int(r, key)?)
                    .ok()
                    .filter(|v| v.unsigned_abs() <= MAX_REGION)
                    .ok_or(SpecError::Invalid)
            };
            Video::Region {
                x: at("x")?,
                y: at("y")?,
                width: side("width")?,
                height: side("height")?,
            }
        }
        "window" => {
            let title = source.ok_or(SpecError::Invalid)?;
            if !catalog.windows.iter().any(|w| w == title) {
                return Err(SpecError::UnknownSource);
            }
            Video::Window(title.to_string())
        }
        "camera" => Video::Camera(device(&catalog.cameras, source)?),
        "none" => Video::None,
        "desktop" | "region" => return Err(SpecError::UnknownSource),
        _ => return Err(SpecError::Invalid),
    };
    let audio = match body.get("audio") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(device(&catalog.audio, Some(id))?),
        Some(_) => return Err(SpecError::Invalid),
    };
    if video == Video::None && audio.is_none() {
        return Err(SpecError::Invalid);
    }
    let choice = |key, choices: &[u32], default| match body.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| choices.contains(v))
            .ok_or(SpecError::Invalid),
    };
    let fast = match body.get("fast") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(SpecError::Invalid),
    };
    Ok(CaptureSpec {
        fast: fast && catalog.fast && matches!(video, Video::Screen(_)),
        video,
        audio,
        fps: choice("fps", &FPS_CHOICES, 30)?,
        height: choice("height", &HEIGHT_CHOICES, 1080)?,
    })
}

/// The ffmpeg inputs of a capture: video first, then the sound when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureInputs {
    pub inputs: Vec<StreamInput>,
    /// The options of each input (before its `-i`).
    pub input_args: Vec<Vec<String>>,
    pub has_audio: bool,
}

fn input(url: String) -> StreamInput {
    StreamInput {
        url,
        headers: Vec::new(),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// The width of the 16:9 box of `height` (even).
pub fn box_width(height: u32) -> u32 {
    (height * 16).div_ceil(9).next_multiple_of(2)
}

/// The X display a Linux capture reads (`$DISPLAY`, else `:0`).
pub fn x_display() -> String {
    std::env::var("DISPLAY")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| ":0".to_string())
}

/// The ffmpeg inputs of `spec` on `platform`, the picture `height` high at most.
pub fn capture_inputs(spec: &CaptureSpec, platform: Platform, display: &str) -> CaptureInputs {
    let fps = spec.fps.to_string();
    let grab = |format: &str| strings(&["-f", format, "-framerate", &fps, "-draw_mouse", "1"]);
    let still = || {
        let height = spec.height;
        (
            strings(&["-f", "lavfi"]),
            format!(
                "color=c={SOUND_ONLY_COLOR}:s={}x{height}:r={fps}",
                box_width(height)
            ),
        )
    };
    let (video_args, video_url) = match (platform, &spec.video) {
        (_, Video::None) => still(),
        (Platform::Windows, Video::Desktop) => (grab("gdigrab"), "desktop".to_string()),
        (Platform::Windows, Video::Screen(s)) if spec.fast => (
            strings(&["-f", "lavfi"]),
            format!(
                "ddagrab=output_idx={}:framerate={fps}:draw_mouse=1,hwdownload,format=bgra",
                s.index
            ),
        ),
        (Platform::Windows, Video::Screen(_) | Video::Region { .. }) => {
            let (x, y, width, height) = match &spec.video {
                Video::Screen(s) => (s.x, s.y, s.width, s.height),
                Video::Region {
                    x,
                    y,
                    width,
                    height,
                } => (*x, *y, *width, *height),
                _ => (0, 0, 0, 0),
            };
            let mut args = grab("gdigrab");
            args.extend(strings(&[
                "-offset_x",
                &x.to_string(),
                "-offset_y",
                &y.to_string(),
                "-video_size",
                &format!("{width}x{height}"),
            ]));
            (args, "desktop".to_string())
        }
        (Platform::Windows, Video::Window(title)) => (grab("gdigrab"), format!("title={title}")),
        (Platform::Windows, Video::Camera(d)) => (
            strings(&["-f", "dshow", "-rtbufsize", "256M"]),
            format!("video={}", d.id),
        ),
        (
            _,
            Video::Region {
                x,
                y,
                width,
                height,
            },
        ) => {
            let mut args = grab("x11grab");
            args.extend(strings(&["-video_size", &format!("{width}x{height}")]));
            (args, format!("{display}+{x},{y}"))
        }
        (_, Video::Camera(d)) => (strings(&["-f", "v4l2"]), d.id.clone()),
        // Linux: the whole X screen (a screen of its own is not listed there).
        (_, Video::Desktop | Video::Screen(_) | Video::Window(_)) => {
            (grab("x11grab"), display.to_string())
        }
    };
    let mut inputs = vec![input(video_url)];
    let mut input_args = vec![video_args];
    if let Some(audio) = &spec.audio {
        let (args, url) = match platform {
            Platform::Windows => (strings(&["-f", "dshow"]), format!("audio={}", audio.id)),
            _ => match audio.id.split_once(':') {
                Some(("alsa", name)) => (strings(&["-f", "alsa"]), name.to_string()),
                Some((_, name)) => (strings(&["-f", "pulse"]), name.to_string()),
                None => (strings(&["-f", "pulse"]), audio.id.clone()),
            },
        };
        inputs.push(input(url));
        input_args.push(args);
    }
    CaptureInputs {
        inputs,
        input_args,
        has_audio: spec.audio.is_some(),
    }
}

/// The filters that fit any picture into the `height`-high 16:9 box (down or up, keeping its
/// shape) and pad it there: odd window sizes and ultra-wide desktops become one valid size.
pub fn fit_filter(height: u32) -> String {
    let width = box_width(height);
    format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease:force_divisible_by=2,\
         pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1"
    )
}

/// What of `spec` must never reach the log: the window title and the device names and ids.
pub fn sensitive_texts(spec: &CaptureSpec) -> Vec<String> {
    let mut out = Vec::new();
    match &spec.video {
        Video::Window(title) => out.push(title.clone()),
        Video::Camera(d) => out.extend([d.id.clone(), d.name.clone()]),
        _ => {}
    }
    if let Some(d) = &spec.audio {
        out.extend([d.id.clone(), d.name.clone()]);
    }
    out.retain(|s| s.chars().count() >= 2);
    // Longer first, so a name inside an id is not cut first.
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out
}

/// `text` without any of `sensitive` (each replaced with `…`).
pub fn redact(text: &str, sensitive: &[String]) -> String {
    sensitive
        .iter()
        .fold(text.to_string(), |acc, s| acc.replace(s.as_str(), "…"))
}

/// A random token for a new capture (128 bits, hex).
fn new_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system random number generator");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Entry {
    spec: CaptureSpec,
    /// The queue item this capture is for (set right after the item is added).
    item_id: Option<i64>,
    added: u64,
}

#[derive(Default)]
struct Inner {
    catalog: Option<(Instant, Catalog)>,
    entries: HashMap<String, Entry>,
    /// Captures whose ffmpeg runs now: run id → item id.
    running: HashMap<u64, i64>,
    counter: u64,
}

/// The captures of this server run, in memory only. Cheap to share (`Arc`).
pub struct CaptureRegistry {
    ffmpeg: PathBuf,
    inner: Mutex<Inner>,
}

impl CaptureRegistry {
    pub fn new(ffmpeg: PathBuf) -> Self {
        Self {
            ffmpeg,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }

    /// Keeps `catalog` as the listing new captures are checked against.
    pub fn set_catalog(&self, catalog: Catalog) {
        self.lock().catalog = Some((Instant::now(), catalog));
    }

    /// The last listing, while it is recent.
    pub fn catalog(&self) -> Option<Catalog> {
        let inner = self.lock();
        let (at, catalog) = inner.catalog.as_ref()?;
        (at.elapsed() < CATALOG_KEEP).then(|| catalog.clone())
    }

    /// Keeps `spec` for a new capture; its token.
    pub fn register(&self, spec: CaptureSpec) -> String {
        let token = new_token();
        let mut inner = self.lock();
        inner.counter += 1;
        let added = inner.counter;
        if inner.entries.len() >= MAX_ENTRIES
            && let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, e)| e.added)
                .map(|(t, _)| t.clone())
        {
            inner.entries.remove(&oldest);
        }
        inner.entries.insert(
            token.clone(),
            Entry {
                spec,
                item_id: None,
                added,
            },
        );
        token
    }

    /// Ties the capture `token` to its queue item.
    pub fn bind(&self, token: &str, item_id: i64) {
        if let Some(e) = self.lock().entries.get_mut(token) {
            e.item_id = Some(item_id);
        }
    }

    /// Drops the capture `token` (its item could not be added).
    pub fn forget(&self, token: &str) {
        self.lock().entries.remove(token);
    }

    /// The capture of the item `item_id` with the source `capture:<token>`: only for the item it
    /// was made for.
    pub fn lookup(&self, token: &str, item_id: i64) -> Option<CaptureSpec> {
        self.lock()
            .entries
            .get(token)
            .filter(|e| e.item_id == Some(item_id))
            .map(|e| e.spec.clone())
    }

    /// The capture `token` still exists (not stopped).
    pub fn is_active(&self, token: &str) -> bool {
        self.lock().entries.contains_key(token)
    }

    /// The items of the captures kept now.
    pub fn bound_items(&self) -> Vec<i64> {
        self.lock()
            .entries
            .values()
            .filter_map(|e| e.item_id)
            .collect()
    }

    /// Stop: every capture is dropped (running ones end at their next poll); how many there were.
    pub fn stop_all(&self) -> usize {
        let mut inner = self.lock();
        let count = inner.entries.len();
        inner.entries.clear();
        count
    }

    /// A capture's ffmpeg starts for `item_id`; it counts as running until the guard drops.
    pub fn start_run(self: &Arc<Self>, item_id: i64) -> RunGuard {
        let mut inner = self.lock();
        inner.counter += 1;
        let id = inner.counter;
        inner.running.insert(id, item_id);
        RunGuard {
            registry: Arc::clone(self),
            id,
        }
    }

    /// The items whose capture runs now.
    pub fn running(&self) -> Vec<i64> {
        let mut items: Vec<i64> = self.lock().running.values().copied().collect();
        items.sort_unstable();
        items.dedup();
        items
    }
}

/// Held while a capture's ffmpeg runs (see [`CaptureRegistry::start_run`]).
pub struct RunGuard {
    registry: Arc<CaptureRegistry>,
    id: u64,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.registry.lock().running.remove(&self.id);
    }
}

#[cfg(test)]
mod tests;
