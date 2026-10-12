//! Info ticker burned into the stream (spec section 7): a half-transparent band with a scrolling
//! text (the film, who added it, the next film, the request page and the owner's messages), the
//! QR code of the request page and a progress line, drawn by ffmpeg while the clip is converted.
//!
//! ffmpeg reads the text from a file in the clip's folder (`reload=1`), so the text changes
//! without a restart: [`TickerText::write_atomic`] replaces the file whole. Everything the
//! filter reads (text, caption, font, code) sits in or next to ffmpeg's working directory and is
//! named relatively, so no Windows path (`C:\…`) has to be escaped inside a filter.
//!
//! Progress line: `drawbox` cannot follow time in this ffmpeg (its `t` is the thickness and its
//! expressions are evaluated once, when the filter is configured), so the line is the fallback
//! the spec names: a colour source slid in from the left by `overlay`, whose `x` is evaluated
//! for every frame.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::Serialize;

use crate::db::{AddedBy, QueueItem};
use crate::i18n::{self, Lang};

/// The ticker text, in the clip's folder (ffmpeg's working directory).
pub const TEXT_FILE: &str = "ticker.txt";
/// The caption under the QR code ("Add a video" in the slate language).
pub const CAPTION_FILE: &str = "qr.txt";
/// The QR code of the request page.
pub const QR_FILE: &str = "qr.png";
/// Folder next to the clip folders that holds one copy of each font the ticker uses.
pub const FONTS_DIR: &str = "_fonts";

/// Between the parts of the text.
const SEPARATOR: &str = "   ·   ";
/// The progress line (the accent of the player).
const ACCENT: &str = "0xf2b84b";

// Layout in a 1080p frame; other heights scale it (see `overlay_filter`).
const REF_HEIGHT: f64 = 1080.0;
const REF_WIDTH: f64 = 1920.0;
const FONT_PX: f64 = 36.0;
const BAND_H: f64 = 60.0;
/// The band's bottom edge is this far above the bottom of the frame.
const BAND_BOTTOM: f64 = 48.0;
const PROGRESS_H: f64 = 5.0;
/// Size of the QR code (modules are whole pixels, so the real size is a little less).
pub const QR_PX: f64 = 200.0;
const QR_RIGHT: f64 = 32.0;
const CAPTION_PX: f64 = 26.0;
const CAPTION_BORDER: f64 = 8.0;
/// Between the band and the caption, and between the caption and the code.
const GAP: f64 = 16.0;
const QR_CAPTION_GAP: f64 = 8.0;

/// Interval mode: the band stays hidden at least this long in every interval.
const WINDOW_SPARE_S: f64 = 8.0;
/// Interval mode: the band never shows longer than the interval minus this (a wider frame
/// scrolls a little longer than the 16:9 the window is measured for).
const WINDOW_GUARD_S: u32 = 4;
/// Interval mode: the window has room for a text this much longer than the longest one known
/// when the clip starts (a longer next title, the address switched on).
const HEADROOM_PX: f64 = 600.0;
/// Interval mode: how much wider than 16:9 a frame may be before the next appearance is
/// prepared too early (2.39:1 is 1.34 × 16:9).
const WIDE_FRAME: f64 = 1.4;

/// Interval lengths the owner can choose, in seconds.
pub const INTERVALS_S: [u32; 4] = [60, 120, 300, 600];
pub const MAX_MESSAGES: usize = 10;
pub const MAX_MESSAGE_CHARS: usize = 200;

/// A conversion slower than real time for this long is reported (the ticker forces it).
const SLOW_AFTER_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TickerMode {
    /// The band shows once every interval: the film, then the next message.
    #[default]
    Interval,
    /// The band is always there; the film and every message loop.
    Continuous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TickerSpeed {
    Slow,
    #[default]
    Normal,
    Fast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TickerQr {
    Off,
    /// Only while the band shows.
    #[default]
    WithTicker,
    Always,
}

impl TickerMode {
    pub const ALL: [Self; 2] = [Self::Interval, Self::Continuous];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interval => "interval",
            Self::Continuous => "continuous",
        }
    }
}

impl TickerSpeed {
    pub const ALL: [Self; 3] = [Self::Slow, Self::Normal, Self::Fast];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Normal => "normal",
            Self::Fast => "fast",
        }
    }

    /// Scrolling speed in a 1080p frame.
    pub fn px_per_s(self) -> f64 {
        match self {
            Self::Slow => 120.0,
            Self::Normal => 180.0,
            Self::Fast => 270.0,
        }
    }
}

impl TickerQr {
    pub const ALL: [Self; 3] = [Self::Off, Self::WithTicker, Self::Always];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::WithTicker => "with_ticker",
            Self::Always => "always",
        }
    }
}

/// The ticker settings of a channel (part of the channel settings, keys `ticker…`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TickerSettings {
    /// QR code and progress line belong to the ticker: off, nothing is drawn and films are
    /// converted only when they need it. Default: on when a GPU encoder was found (stored at
    /// the first detection, see `SettingsRepo::seed_ticker_enabled`).
    #[serde(rename = "tickerEnabled")]
    pub enabled: bool,
    #[serde(rename = "tickerMode")]
    pub mode: TickerMode,
    /// Interval mode: one of [`INTERVALS_S`].
    #[serde(rename = "tickerIntervalS")]
    pub interval_s: u32,
    #[serde(rename = "tickerSpeed")]
    pub speed: TickerSpeed,
    #[serde(rename = "tickerShowNext")]
    pub show_next: bool,
    /// The request page's address as text.
    #[serde(rename = "tickerShowAddress")]
    pub show_address: bool,
    #[serde(rename = "tickerQr")]
    pub qr: TickerQr,
    #[serde(rename = "tickerProgress")]
    pub progress: bool,
    /// The owner's messages (never translated), at most [`MAX_MESSAGES`] of at most
    /// [`MAX_MESSAGE_CHARS`] characters.
    #[serde(rename = "tickerMessages")]
    pub messages: Vec<String>,
}

impl Default for TickerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: TickerMode::Interval,
            interval_s: 60,
            speed: TickerSpeed::Normal,
            show_next: true,
            show_address: false,
            qr: TickerQr::WithTicker,
            progress: true,
            messages: Vec::new(),
        }
    }
}

impl TickerSettings {
    /// Whether `other` draws the same way: a change of the switch, mode, interval, speed, code
    /// or progress line needs a new ffmpeg (the current film starts again from where it is);
    /// the texts change through the text file.
    pub fn same_layout(&self, other: &Self) -> bool {
        (
            self.enabled,
            self.mode,
            self.interval_s,
            self.speed,
            self.qr,
            self.progress,
        ) == (
            other.enabled,
            other.mode,
            other.interval_s,
            other.speed,
            other.qr,
            other.progress,
        )
    }
}

/// Who added the film, as the ticker names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requester {
    /// The owner (a film added in the panel).
    Host,
    Nick(String),
    /// A viewer without a nick: the ticker shows only the title.
    Anonymous,
}

impl Requester {
    pub fn of(item: &QueueItem) -> Self {
        match item.added_by {
            AddedBy::Admin => Self::Host,
            AddedBy::Public => item
                .requester_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map_or(Self::Anonymous, |n| Self::Nick(n.to_string())),
        }
    }
}

/// What the ticker tells about the film on air.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowNext {
    pub title: String,
    pub requester: Requester,
    pub next_title: Option<String>,
    /// The request page's address; empty: no public address.
    pub address: String,
}

/// A part of the one-line text: line breaks and other control characters become spaces.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// The ticker text in `lang` (the slate language): "► Now: <title> · added by <nick | Host> ·
/// next: <title> · add a video: <address>", then the owner's messages: in interval mode the
/// `message_index`-th (cyclically), in continuous mode all of them.
pub fn compose_text(
    lang: Lang,
    now: &NowNext,
    settings: &TickerSettings,
    message_index: usize,
) -> String {
    let mut parts = vec![i18n::t(
        lang,
        "ticker.now",
        &[("title", &clean(&now.title))],
    )];
    match &now.requester {
        Requester::Host => parts.push(i18n::t(lang, "ticker.added_by_host", &[])),
        Requester::Nick(nick) => {
            parts.push(i18n::t(lang, "ticker.added_by", &[("nick", &clean(nick))]))
        }
        Requester::Anonymous => {}
    }
    if settings.show_next
        && let Some(next) = &now.next_title
    {
        parts.push(i18n::t(lang, "ticker.next", &[("title", &clean(next))]));
    }
    if settings.show_address && !now.address.is_empty() {
        parts.push(i18n::t(
            lang,
            "ticker.address",
            &[("address", &clean(&now.address))],
        ));
    }
    let messages: Vec<String> = settings
        .messages
        .iter()
        .map(|m| clean(m))
        .filter(|m| !m.is_empty())
        .collect();
    match settings.mode {
        TickerMode::Interval if !messages.is_empty() => {
            parts.push(messages[message_index % messages.len()].clone());
        }
        TickerMode::Interval => {}
        TickerMode::Continuous => parts.extend(messages),
    }
    parts
        .iter()
        .map(|p| clean(p))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

/// Width of `c` in ems, rounded up for the usual fonts (Arial, Yu Gothic), so a text estimated
/// to fit really fits.
fn char_em(c: char) -> f64 {
    let wide = matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F000..=0x1FAFF
        | 0x20000..=0x3FFFD);
    if wide {
        1.1
    } else if c == ' ' {
        0.3
    } else if c.is_uppercase() {
        0.75
    } else if c.is_ascii_digit() || c.is_alphabetic() {
        0.6
    } else if c.is_ascii_punctuation() || c == '·' {
        0.45
    } else {
        0.8
    }
}

/// Estimated width of `text` drawn by the ticker in a 1080p frame.
pub fn text_width_px(text: &str) -> f64 {
    text.chars().map(char_em).sum::<f64>() * FONT_PX
}

/// `text` cut (with "…") to at most `max_px` wide.
pub fn fit_text(text: &str, max_px: f64) -> String {
    if text_width_px(text) <= max_px {
        return text.to_string();
    }
    let room = max_px - text_width_px("…");
    let mut out = String::new();
    let mut width = 0.0;
    for c in text.chars() {
        let w = char_em(c) * FONT_PX;
        if width + w > room {
            break;
        }
        width += w;
        out.push(c);
    }
    format!("{}…", out.trim_end())
}

/// Interval mode: how wide a text the band's window has room for (1080p pixels): the longest of
/// `candidates` (the texts known when the clip starts) plus room for changes, but never so
/// much that the band would show longer than the interval minus [`WINDOW_SPARE_S`].
pub fn window_budget_px(settings: &TickerSettings, candidates: &[String]) -> f64 {
    let longest = candidates
        .iter()
        .map(|t| text_width_px(t))
        .fold(0.0, f64::max);
    let speed = settings.speed.px_per_s();
    let cap = (f64::from(settings.interval_s) - WINDOW_SPARE_S) * speed - REF_WIDTH;
    (longest + HEADROOM_PX).min(cap).max(0.0)
}

/// Interval mode: when the text for the next appearance may be written (the band of the
/// current one is gone, with room for frames wider than 16:9 and for ffmpeg's lead).
pub fn prepare_after_s(settings: &TickerSettings, budget_px: f64) -> f64 {
    let interval = f64::from(settings.interval_s);
    ((REF_WIDTH * WIDE_FRAME + budget_px) / settings.speed.px_per_s() + 1.0)
        .min(interval - f64::from(WINDOW_GUARD_S))
}

/// The QR code drawn above the band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QrOverlay<'a> {
    pub png: &'a str,
    /// Width of the PNG.
    pub size_px: u32,
    pub caption_file: &'a str,
}

/// What [`overlay_filter`] needs to know about the clip. File names are relative to ffmpeg's
/// working directory (the clip's folder).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayContext<'a> {
    /// Length of the film; `None` (live stream, unknown): no progress line.
    pub duration_s: Option<f64>,
    /// Film second the clip starts from (`t` = 0 in the filter).
    pub start_at_s: f64,
    pub text_file: &'a str,
    pub font: &'a str,
    /// `None`: no public address, no code.
    pub qr: Option<QrOverlay<'a>>,
    /// Height of the converted frame (the layout is made for 1080 and scaled).
    pub frame_height: u32,
    /// Interval mode: see [`window_budget_px`].
    pub text_budget_px: f64,
}

/// A number for an ffmpeg expression (at most 3 decimals, no trailing zeros).
fn num(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// Names the filter reads must not need escaping in a filter graph.
fn filter_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// The filters drawing the ticker on the converted picture (one input, one output; chained
/// after scaling): the band and its text (in interval mode only for a window of every
/// interval, the text scrolling in from the right; in continuous mode always, the text
/// looping), the QR code with its caption (with the band or always) and the progress line at
/// the bottom edge (`(start_at_s + t) / duration` of the width; never for a live stream).
pub fn overlay_filter(s: &TickerSettings, c: &OverlayContext<'_>) -> String {
    debug_assert!(filter_safe(c.text_file) && filter_safe(c.font));
    let k = if c.frame_height == 0 {
        1.0
    } else {
        f64::from(c.frame_height) / REF_HEIGHT
    };
    let px = |v: f64| ((v * k).round() as i64).max(1);
    let speed = num(s.speed.px_per_s() * k);
    let interval = s.interval_s;
    let shown = match s.mode {
        TickerMode::Interval => Some(format!(
            "lt(mod(t,{interval}),min((w+{})/{speed},{}))",
            px(c.text_budget_px),
            interval.saturating_sub(WINDOW_GUARD_S)
        )),
        TickerMode::Continuous => None,
    };
    let enable =
        |cond: Option<&String>| cond.map_or_else(String::new, |e| format!(":enable='{e}'"));
    let band_top = px(BAND_BOTTOM + BAND_H);
    let band_h = px(BAND_H);
    let font_px = px(FONT_PX);
    let text_top = band_top - (band_h - font_px) / 2;
    let shadow = px(2.0);
    let x = match s.mode {
        TickerMode::Interval => format!("w-mod(t,{interval})*{speed}"),
        TickerMode::Continuous => format!("w-mod(t*{speed},w+tw)"),
    };
    let en = enable(shown.as_ref());
    let mut graph = format!(
        "drawbox=x=0:y=ih-{band_top}:w=iw:h={band_h}:color=black@0.55:t=fill{en},\
         drawtext=fontfile={font}:textfile={text}:reload=1:expansion=none:fontsize={font_px}:\
         fontcolor=white:shadowcolor=black@0.6:shadowx={shadow}:shadowy={shadow}:\
         x='{x}':y=h-{text_top}{en}",
        font = c.font,
        text = c.text_file,
    );
    if let Some(qr) = c.qr.filter(|_| s.qr != TickerQr::Off) {
        debug_assert!(filter_safe(qr.png) && filter_safe(qr.caption_file));
        let en = enable(match s.qr {
            TickerQr::WithTicker => shown.as_ref(),
            TickerQr::Off | TickerQr::Always => None,
        });
        let right = px(QR_RIGHT);
        let caption_px = px(CAPTION_PX);
        let border = px(CAPTION_BORDER);
        // Distances from the bottom of the frame.
        let caption_top = band_top + px(GAP) + border + caption_px;
        let qr_bottom = caption_top + border + px(QR_CAPTION_GAP);
        let centre = i64::from(qr.size_px / 2);
        graph = format!(
            "{graph}[tk_main];movie={png}[tk_qr];\
             [tk_main][tk_qr]overlay=x=W-w-{right}:y=H-h-{qr_bottom}{en},\
             drawtext=fontfile={font}:textfile={caption}:reload=1:expansion=none:\
             fontsize={caption_px}:fontcolor=white:box=1:boxcolor=black@0.55:\
             boxborderw={border}:x=w-{right}-{centre}-tw/2:y=h-{caption_top}{en}",
            png = qr.png,
            font = c.font,
            caption = qr.caption_file,
        );
    }
    let duration = c.duration_s.filter(|d| d.is_finite() && *d > 0.0);
    if let Some(duration) = duration.filter(|_| s.progress) {
        let h = px(PROGRESS_H);
        graph = format!(
            "{graph},drawbox=x=0:y=ih-{h}:w=iw:h={h}:color=black@0.3:t=fill[tk_bar];\
             color=c={ACCENT}@0.85:s=4096x{h}:r=30[tk_fill];\
             [tk_bar][tk_fill]overlay=x='W*min(1,({start}+t)/{duration})-w':y=H-{h}:shortest=1",
            start = num(c.start_at_s),
            duration = num(duration),
        );
    }
    graph
}

/// Interval mode: which appearance of the band the text file is for at clip time `t`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Schedule {
    pub mode: TickerMode,
    pub interval_s: f64,
    /// See [`prepare_after_s`].
    pub prepare_after_s: f64,
}

impl Schedule {
    /// Appearance `k` starts at `k × interval`; its text is written once appearance `k - 1`
    /// has scrolled away. Continuous mode has one endless appearance. `t`: ffmpeg's output time
    /// (`None` before its first progress report).
    pub fn appearance(&self, t: Option<f64>) -> u64 {
        if self.mode == TickerMode::Continuous || self.interval_s <= 0.0 {
            return 0;
        }
        let t = t.filter(|t| t.is_finite()).unwrap_or(0.0).max(0.0);
        let k = (t / self.interval_s).floor();
        let next = t - k * self.interval_s >= self.prepare_after_s;
        k as u64 + u64::from(next)
    }
}

/// What a running ticker re-reads (a change shows without a restart).
#[derive(Debug, Clone, PartialEq)]
pub struct TickerContent {
    /// The slate language (English when Japanese has no font).
    pub lang: Lang,
    pub settings: TickerSettings,
    pub next_title: Option<String>,
    /// The request page's address; empty: none.
    pub address: String,
}

/// Keeps the text files of one clip's ticker up to date.
pub struct TickerWriter {
    dir: PathBuf,
    title: String,
    requester: Requester,
    schedule: Schedule,
    /// Interval mode: the text is cut to this width (see [`window_budget_px`]).
    budget_px: Option<f64>,
    /// Which message comes next, shared by the clips of a channel.
    rotation: Arc<AtomicUsize>,
    appearance: Option<u64>,
    message_index: usize,
    written: Option<(String, String)>,
}

impl TickerWriter {
    pub fn new(
        dir: PathBuf,
        item: &QueueItem,
        schedule: Schedule,
        budget_px: Option<f64>,
        rotation: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            dir,
            title: item.title.clone(),
            requester: Requester::of(item),
            schedule,
            budget_px,
            rotation,
            appearance: None,
            message_index: 0,
            written: None,
        }
    }

    /// The ticker text and the code's caption at clip time `t`.
    pub fn texts(&mut self, t: Option<f64>, content: &TickerContent) -> (String, String) {
        let appearance = self.schedule.appearance(t);
        if self.appearance != Some(appearance) {
            self.appearance = Some(appearance);
            self.message_index = self.rotation.fetch_add(1, Ordering::Relaxed);
        }
        let now = NowNext {
            title: self.title.clone(),
            requester: self.requester.clone(),
            next_title: content.next_title.clone(),
            address: content.address.clone(),
        };
        let text = compose_text(content.lang, &now, &content.settings, self.message_index);
        let text = match self.budget_px {
            Some(budget) => fit_text(&text, budget),
            None => text,
        };
        (text, i18n::t(content.lang, "ticker.qr_caption", &[]))
    }

    /// Writes the texts for clip time `t` when they changed; `Ok(true)`: something was written.
    pub fn update(&mut self, t: Option<f64>, content: &TickerContent) -> io::Result<bool> {
        let texts = self.texts(t, content);
        if self.written.as_ref() == Some(&texts) {
            return Ok(false);
        }
        let (text, caption) = &texts;
        if self.written.as_ref().is_none_or(|(_, c)| c != caption) {
            TickerText::write_atomic(&self.dir.join(CAPTION_FILE), caption)?;
        }
        if self.written.as_ref().is_none_or(|(t, _)| t != text) {
            TickerText::write_atomic(&self.dir.join(TEXT_FILE), text)?;
        }
        self.written = Some(texts);
        Ok(true)
    }
}

/// Watches the conversion speed of a clip with the ticker: below real time for 30 s in a row
/// is reported once, catching up again clears it.
#[derive(Debug, Clone, Copy, Default)]
pub struct SlowWatch {
    below_since: Option<u64>,
    slow: bool,
}

impl SlowWatch {
    /// ffmpeg's `speed` at `now_ms`; `Some(true)`: it became too slow, `Some(false)`: it keeps
    /// up again. No report (`None`) changes nothing.
    pub fn observe(&mut self, now_ms: u64, speed: Option<f64>) -> Option<bool> {
        let speed = speed?;
        if speed < 1.0 {
            let since = *self.below_since.get_or_insert(now_ms);
            if !self.slow && now_ms.saturating_sub(since) >= SLOW_AFTER_MS {
                self.slow = true;
                return Some(true);
            }
            None
        } else {
            self.below_since = None;
            if self.slow {
                self.slow = false;
                return Some(false);
            }
            None
        }
    }

    pub fn is_slow(&self) -> bool {
        self.slow
    }
}

/// Size of a text file ffmpeg re-reads: the text, then NUL bytes (drawtext stops at the first
/// one). Room for the longest text: the film part and ten messages of 200 characters.
pub const TEXT_FILE_BYTES: usize = 16 * 1024;
/// How many times a write that another program blocks (an antivirus scan) is tried.
const WRITE_TRIES: u32 = 50;

/// Text files ffmpeg re-reads while it runs (`reload=1`: once per frame).
pub struct TickerText;

impl TickerText {
    /// Puts `text` into `path` so ffmpeg reading it every frame never fails and never sees an
    /// empty file: the file always has [`TEXT_FILE_BYTES`] bytes (the text, NUL after it) and
    /// is overwritten in place with one write, never truncated, never replaced.
    ///
    /// Not a temporary file renamed over it (the plan's first idea): on Windows a rename that
    /// replaces the file makes ffmpeg's open of it fail now and then ("Permission denied"),
    /// and drawtext then stops ffmpeg (Review Focus 2). Opening it for writing never conflicts
    /// with ffmpeg's read (both share reading and writing). A write is tried again for a short
    /// while when another program holds the file.
    pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
        let block = text_block(text);
        let mut attempt = 0;
        loop {
            match overwrite(path, &block) {
                Ok(()) => return Ok(()),
                Err(e) if attempt + 1 < WRITE_TRIES && is_locked(&e) => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(u64::from(attempt.min(10))));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// The text of a file written by [`Self::write_atomic`].
    pub fn read(path: &Path) -> io::Result<String> {
        let bytes = std::fs::read(path)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

/// `text` (cut at a character boundary when too long, no NUL inside) padded with NUL bytes to
/// [`TEXT_FILE_BYTES`].
fn text_block(text: &str) -> Vec<u8> {
    let mut block = Vec::with_capacity(TEXT_FILE_BYTES);
    for c in text.chars().filter(|&c| c != '\0') {
        let mut buf = [0u8; 4];
        let encoded = c.encode_utf8(&mut buf).as_bytes();
        if block.len() + encoded.len() >= TEXT_FILE_BYTES {
            break;
        }
        block.extend_from_slice(encoded);
    }
    block.resize(TEXT_FILE_BYTES, 0);
    block
}

fn overwrite(path: &Path, block: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.write_all(block)
}

/// Another program holds the file (access denied, sharing or lock violation).
fn is_locked(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33)) || e.kind() == io::ErrorKind::PermissionDenied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    fn settings() -> TickerSettings {
        TickerSettings {
            enabled: true,
            show_address: false,
            ..TickerSettings::default()
        }
    }

    fn now(requester: Requester, next: Option<&str>) -> NowNext {
        NowNext {
            title: "Never Gonna Give You Up".into(),
            requester,
            next_title: next.map(String::from),
            address: "https://kanal.example.com/".into(),
        }
    }

    #[test]
    fn the_text_names_the_film_who_added_it_and_the_next_one() {
        let text = compose_text(
            Lang::En,
            &now(Requester::Nick("Kasia".into()), Some("Darude — Sandstorm")),
            &settings(),
            0,
        );
        assert_eq!(
            text,
            "► Now: Never Gonna Give You Up   ·   added by Kasia   ·   next: Darude — Sandstorm"
        );
    }

    #[test]
    fn a_film_the_owner_added_is_from_the_host_in_the_slate_language() {
        let text = compose_text(Lang::Pl, &now(Requester::Host, None), &settings(), 0);
        assert_eq!(
            text,
            "► Teraz: Never Gonna Give You Up   ·   dodane przez Gospodarza"
        );
    }

    #[test]
    fn without_a_nick_only_the_title_is_shown() {
        let text = compose_text(Lang::En, &now(Requester::Anonymous, None), &settings(), 0);
        assert_eq!(text, "► Now: Never Gonna Give You Up");
    }

    #[test]
    fn next_and_address_can_be_switched_off_and_on() {
        let n = now(Requester::Anonymous, Some("Pies"));
        let mut s = settings();
        s.show_next = false;
        assert_eq!(
            compose_text(Lang::En, &n, &s, 0),
            "► Now: Never Gonna Give You Up"
        );
        s.show_address = true;
        assert_eq!(
            compose_text(Lang::En, &n, &s, 0),
            "► Now: Never Gonna Give You Up   ·   add a video: https://kanal.example.com/"
        );
        // No public address: nothing to show.
        let mut no_address = n.clone();
        no_address.address = String::new();
        assert_eq!(
            compose_text(Lang::En, &no_address, &s, 0),
            "► Now: Never Gonna Give You Up"
        );
    }

    #[test]
    fn interval_mode_shows_the_next_message_at_every_appearance() {
        let mut s = settings();
        s.messages = vec!["A".into(), "B".into(), "C".into()];
        let n = now(Requester::Anonymous, None);
        let shown: Vec<String> = (0..5)
            .map(|i| compose_text(Lang::En, &n, &s, i))
            .map(|t| t.rsplit("   ·   ").next().unwrap().to_string())
            .collect();
        assert_eq!(shown, ["A", "B", "C", "A", "B"]);
    }

    #[test]
    fn continuous_mode_loops_the_film_and_every_message() {
        let mut s = settings();
        s.mode = TickerMode::Continuous;
        s.messages = vec!["Witajcie!".into(), "Max 3 filmy".into()];
        let n = now(Requester::Anonymous, None);
        let text = compose_text(Lang::En, &n, &s, 7);
        assert_eq!(
            text,
            "► Now: Never Gonna Give You Up   ·   Witajcie!   ·   Max 3 filmy"
        );
        assert_eq!(compose_text(Lang::En, &n, &s, 0), text);
    }

    #[test]
    fn line_breaks_and_control_characters_never_reach_the_one_line_ticker() {
        let mut s = settings();
        s.messages = vec!["dwie\nlinie\tok".into()];
        let mut n = now(Requester::Nick("Ka\rsia".into()), None);
        n.title = "Tytuł\u{0}".into();
        assert_eq!(
            compose_text(Lang::En, &n, &s, 0),
            "► Now: Tytuł   ·   added by Ka sia   ·   dwie linie ok"
        );
    }

    #[test]
    fn the_requester_comes_from_the_queue_item() {
        use crate::db::{AddedBy, ItemStatus, QueueItem};
        let mut item = QueueItem {
            id: 1,
            channel_id: 1,
            position: 0,
            source_url: String::new(),
            title: "t".into(),
            duration_s: None,
            is_live: false,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: Some("Kasia".into()),
            requester_ip: None,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
        };
        assert_eq!(Requester::of(&item), Requester::Host);
        item.added_by = AddedBy::Public;
        assert_eq!(Requester::of(&item), Requester::Nick("Kasia".into()));
        item.requester_name = Some("  ".into());
        assert_eq!(Requester::of(&item), Requester::Anonymous);
        item.requester_name = None;
        assert_eq!(Requester::of(&item), Requester::Anonymous);
    }

    #[test]
    fn a_text_longer_than_the_window_is_cut_with_an_ellipsis() {
        let long = "x".repeat(400);
        let fitted = fit_text(&long, 1000.0);
        assert!(fitted.ends_with('…'), "{fitted}");
        assert!(text_width_px(&fitted) <= 1000.0);
        assert!(fitted.chars().count() > 20);
        assert_eq!(fit_text("krótki", 1000.0), "krótki");
        // Wide characters count more.
        assert!(text_width_px("日本語") > text_width_px("abc"));
    }

    #[test]
    fn the_interval_window_fits_the_longest_text_and_leaves_the_band_hidden_for_a_while() {
        let mut s = settings();
        s.interval_s = 60;
        s.speed = TickerSpeed::Normal;
        let short = window_budget_px(&s, &["abc".to_string()]);
        let long = window_budget_px(&s, &["abc".to_string(), "x".repeat(80)]);
        assert!(long > short);
        assert!(long >= text_width_px(&"x".repeat(80)));
        // Never longer than the interval minus the hidden part.
        let huge = window_budget_px(&s, &["x".repeat(5000)]);
        let visible_s = (1920.0 + huge) / TickerSpeed::Normal.px_per_s();
        assert!(visible_s <= 60.0 - 8.0 + 1e-9, "{visible_s}");
    }

    fn ctx(duration_s: Option<f64>) -> OverlayContext<'static> {
        OverlayContext {
            duration_s,
            start_at_s: 20.0,
            text_file: TEXT_FILE,
            font: "../_fonts/arial.ttf",
            qr: Some(QrOverlay {
                png: QR_FILE,
                size_px: 185,
                caption_file: CAPTION_FILE,
            }),
            frame_height: 1080,
            text_budget_px: 2000.0,
        }
    }

    #[test]
    fn the_filter_reads_its_text_font_and_code_by_relative_names_only() {
        let f = overlay_filter(&settings(), &ctx(Some(60.0)));
        assert!(
            f.contains("textfile=ticker.txt:reload=1:expansion=none"),
            "{f}"
        );
        assert!(f.contains("fontfile=../_fonts/arial.ttf"), "{f}");
        assert!(f.contains("movie=qr.png"), "{f}");
        assert!(f.contains("textfile=qr.txt"), "{f}");
        assert!(!f.contains('\\') && !f.contains("C:"), "{f}");
    }

    #[test]
    fn the_progress_line_follows_the_position_in_the_film_but_not_for_a_live_stream() {
        let f = overlay_filter(&settings(), &ctx(Some(60.0)));
        assert!(f.contains("color=c=0xf2b84b@0.85"), "{f}");
        assert!(f.contains("x='W*min(1,(20+t)/60)-w'"), "{f}");
        assert!(f.contains("shortest=1"), "{f}");
        let live = overlay_filter(&settings(), &ctx(None));
        assert!(!live.contains("0xf2b84b"), "{live}");
        let mut off = settings();
        off.progress = false;
        assert!(!overlay_filter(&off, &ctx(Some(60.0))).contains("0xf2b84b"));
    }

    #[test]
    fn interval_mode_shows_the_band_for_a_window_every_interval_continuous_always() {
        let mut s = settings();
        s.interval_s = 120;
        let f = overlay_filter(&s, &ctx(Some(60.0)));
        assert!(
            f.contains("enable='lt(mod(t,120),min((w+2000)/180,116))'"),
            "{f}"
        );
        assert!(f.contains("x='w-mod(t,120)*180'"), "{f}");
        s.mode = TickerMode::Continuous;
        let f = overlay_filter(&s, &ctx(Some(60.0)));
        assert!(!f.contains("enable="), "{f}");
        assert!(f.contains("x='w-mod(t*180,w+tw)'"), "{f}");
    }

    #[test]
    fn the_code_shows_with_the_ticker_always_or_never() {
        let mut s = settings();
        s.qr = TickerQr::WithTicker;
        let f = overlay_filter(&s, &ctx(Some(60.0)));
        let qr_part = &f[f.find("movie=").unwrap()..];
        assert!(qr_part.contains("enable='lt(mod(t,60)"), "{f}");
        s.qr = TickerQr::Always;
        let f = overlay_filter(&s, &ctx(Some(60.0)));
        let qr_part = &f[f.find("movie=").unwrap()..];
        assert!(!qr_part.contains("enable="), "{f}");
        s.qr = TickerQr::Off;
        assert!(!overlay_filter(&s, &ctx(Some(60.0))).contains("movie="));
        // No public address, no code.
        let mut c = ctx(Some(60.0));
        c.qr = None;
        s.qr = TickerQr::Always;
        assert!(!overlay_filter(&s, &c).contains("movie="));
    }

    #[test]
    fn sizes_follow_the_frame_height() {
        let mut c = ctx(Some(60.0));
        c.frame_height = 720;
        let f = overlay_filter(&settings(), &c);
        assert!(f.contains("fontsize=24:"), "{f}");
        assert!(f.contains("x='w-mod(t,60)*120'"), "{f}");
    }

    #[test]
    fn an_appearance_is_prepared_once_the_previous_one_has_scrolled_away() {
        let schedule = Schedule {
            mode: TickerMode::Interval,
            interval_s: 60.0,
            prepare_after_s: 20.0,
        };
        assert_eq!(schedule.appearance(None), 0);
        assert_eq!(schedule.appearance(Some(0.0)), 0);
        assert_eq!(schedule.appearance(Some(19.9)), 0);
        assert_eq!(schedule.appearance(Some(20.0)), 1);
        assert_eq!(schedule.appearance(Some(61.0)), 1);
        assert_eq!(schedule.appearance(Some(85.0)), 2);
        let continuous = Schedule {
            mode: TickerMode::Continuous,
            ..schedule
        };
        assert_eq!(continuous.appearance(Some(500.0)), 0);
    }

    #[test]
    fn a_slow_conversion_warns_after_30_s_and_clears_when_it_keeps_up() {
        let mut w = SlowWatch::default();
        assert_eq!(w.observe(0, Some(0.9)), None);
        assert_eq!(w.observe(29_999, Some(0.95)), None);
        assert_eq!(w.observe(30_000, Some(0.95)), Some(true));
        assert!(w.is_slow());
        assert_eq!(w.observe(31_000, Some(0.9)), None);
        assert_eq!(w.observe(32_000, None), None, "no report: no change");
        assert_eq!(w.observe(33_000, Some(1.2)), Some(false));
        // A short dip does not count.
        assert_eq!(w.observe(40_000, Some(0.5)), None);
        assert_eq!(w.observe(50_000, Some(1.5)), None);
        assert_eq!(w.observe(75_000, Some(0.5)), None);
        assert_eq!(w.observe(104_999, Some(0.5)), None);
        assert_eq!(w.observe(105_000, Some(0.5)), Some(true));
    }

    #[test]
    fn the_text_file_keeps_its_size_and_a_shorter_text_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TEXT_FILE);
        TickerText::write_atomic(&path, "drugi, dłuższy tekst").unwrap();
        assert_eq!(TickerText::read(&path).unwrap(), "drugi, dłuższy tekst");
        TickerText::write_atomic(&path, "krótki").unwrap();
        assert_eq!(TickerText::read(&path).unwrap(), "krótki");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), TEXT_FILE_BYTES);
        assert!(bytes["krótki".len()..].iter().all(|&b| b == 0));
        // Nothing else in the folder (no temporary file).
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [TEXT_FILE]);
    }

    #[test]
    fn a_text_too_long_for_the_file_is_cut_at_a_character() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TEXT_FILE);
        let long = "ż".repeat(TEXT_FILE_BYTES);
        TickerText::write_atomic(&path, &long).unwrap();
        let read = TickerText::read(&path).unwrap();
        assert!(read.len() < TEXT_FILE_BYTES);
        assert!(read.chars().all(|c| c == 'ż'));
        assert_eq!(std::fs::read(&path).unwrap().len(), TEXT_FILE_BYTES);
        // A NUL inside the text would end it early for ffmpeg: it is left out.
        TickerText::write_atomic(&path, "a\0b").unwrap();
        assert_eq!(TickerText::read(&path).unwrap(), "ab");
    }

    /// ffmpeg opens the file with read and write sharing (`_wopen`) and maps it; a new text is
    /// written while it holds it, without waiting.
    #[cfg(windows)]
    #[test]
    fn a_reader_holding_the_file_does_not_hold_up_a_new_text() {
        use std::io::Read;
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TEXT_FILE);
        TickerText::write_atomic(&path, "stary").unwrap();
        let mut reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&path)
            .unwrap();
        TickerText::write_atomic(&path, "nowy").unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert!(bytes.starts_with(b"nowy\0"));
        drop(reader);
        assert_eq!(TickerText::read(&path).unwrap(), "nowy");
    }

    // ---------- With the real ffmpeg (skipped when bin/ffmpeg or the font is missing) ----------

    mod drawn {
        use super::super::*;
        use crate::config::VideoEncoder;
        use crate::pipeline::ffmpeg_args::{ClipArgsOptions, build_clip_args};
        use crate::pipeline::probe::{VideoInfo, VideoMode};
        use crate::pipeline::segmenter::{ClipSegmenter, SegmenterState};
        use crate::pipeline::slates::qr_image;
        use crate::resolver::types::StreamInput;
        use std::path::{Path, PathBuf};
        use std::time::{Duration, Instant};

        const W: usize = 1920;
        const H: usize = 1080;
        const ADDRESS: &str = "https://kanal.example.com/";
        /// Rows of the band (1080p: 972..1032), a little inside its edges.
        const BAND_ROWS: std::ops::Range<usize> = 976..1028;

        fn ffmpeg() -> Option<PathBuf> {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
            ["ffmpeg.exe", "ffmpeg"]
                .iter()
                .map(|name| root.join("bin").join(name))
                .find(|p| p.exists())
        }

        fn font() -> PathBuf {
            if cfg!(windows) {
                "C:/Windows/Fonts/arial.ttf".into()
            } else {
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into()
            }
        }

        fn tools() -> Option<PathBuf> {
            let ffmpeg = ffmpeg()?;
            if !font().is_file() {
                return None;
            }
            Some(ffmpeg)
        }

        /// A clip folder with the ticker's files, as the clip factory makes it.
        struct Clip {
            _dir: tempfile::TempDir,
            dir: PathBuf,
        }

        fn clip(text: &str, with_qr: bool) -> (Clip, Option<u32>) {
            let root = tempfile::tempdir().unwrap();
            let fonts = root.path().join(FONTS_DIR);
            std::fs::create_dir_all(&fonts).unwrap();
            std::fs::copy(font(), fonts.join("arial.ttf")).unwrap();
            let dir = root.path().join("clip");
            std::fs::create_dir_all(&dir).unwrap();
            TickerText::write_atomic(&dir.join(TEXT_FILE), text).unwrap();
            TickerText::write_atomic(&dir.join(CAPTION_FILE), "Add a video").unwrap();
            let qr = with_qr.then(|| {
                let (png, size) = qr_image(ADDRESS, QR_PX as u32).unwrap();
                std::fs::write(dir.join(QR_FILE), png).unwrap();
                size
            });
            (Clip { _dir: root, dir }, qr)
        }

        /// Starts ffmpeg drawing the ticker on `seconds` of a mid-grey 1080p30 picture
        /// (`realtime`: at the pace it plays, as a converted film is read).
        fn start(
            c: &Clip,
            settings: &TickerSettings,
            qr: Option<u32>,
            duration_s: Option<f64>,
            start_at_s: f64,
            seconds: u32,
            realtime: bool,
        ) -> ClipSegmenter {
            let overlay = overlay_filter(
                settings,
                &OverlayContext {
                    duration_s,
                    start_at_s,
                    text_file: TEXT_FILE,
                    font: "../_fonts/arial.ttf",
                    qr: qr.map(|size_px| QrOverlay {
                        png: QR_FILE,
                        size_px,
                        caption_file: CAPTION_FILE,
                    }),
                    frame_height: 1080,
                    text_budget_px: window_budget_px(
                        settings,
                        &[TickerText::read(&c.dir.join(TEXT_FILE)).unwrap()],
                    ),
                },
            );
            let pace = if realtime { ",realtime" } else { "" };
            let args = build_clip_args(&ClipArgsOptions {
                inputs: vec![StreamInput {
                    url: format!("color=c=gray:s=1920x1080:r=30:d={seconds}{pace}"),
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
                    duration_s: Some(f64::from(seconds)),
                },
                encoder: VideoEncoder::Libx264,
                segment_seconds: 4,
                out_dir: c.dir.clone(),
                start_at_s: 0.0,
                readrate: None,
                overlay: Some(overlay),
            });
            ClipSegmenter::start(&ffmpeg().unwrap(), args, c.dir.clone(), Some(c.dir.clone()))
        }

        /// Waits for ffmpeg and joins its segments into one file.
        async fn finish(c: &Clip, seg: &ClipSegmenter) -> PathBuf {
            assert_eq!(seg.wait().await, SegmenterState::Done, "{:?}", seg.error());
            let joined = c.dir.join("joined.ts");
            let mut bytes = Vec::new();
            for s in seg.poll() {
                bytes.extend(std::fs::read(&s.path).unwrap());
            }
            assert!(!bytes.is_empty());
            std::fs::write(&joined, bytes).unwrap();
            joined
        }

        /// RGB frame at `at_s` seconds.
        fn frame(joined: &Path, at_s: f64) -> Vec<u8> {
            let out = std::process::Command::new(ffmpeg().unwrap())
                .args(["-v", "error", "-i"])
                .arg(joined)
                .args(["-ss", &at_s.to_string(), "-frames:v", "1"])
                .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
                .output()
                .unwrap();
            assert!(
                out.status.success() && out.stdout.len() == W * H * 3,
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        }

        fn rgb(f: &[u8], x: usize, y: usize) -> (i32, i32, i32) {
            let i = (y * W + x) * 3;
            (i32::from(f[i]), i32::from(f[i + 1]), i32::from(f[i + 2]))
        }

        fn luma(f: &[u8], x: usize, y: usize) -> i32 {
            let (r, g, b) = rgb(f, x, y);
            (r * 299 + g * 587 + b * 114) / 1000
        }

        /// The band darkens the grey picture (black at 55 %).
        fn band_visible(f: &[u8]) -> bool {
            let mut dark = 0;
            let mut all = 0;
            for y in BAND_ROWS.step_by(4) {
                for x in (0..W).step_by(8) {
                    all += 1;
                    if luma(f, x, y) < 90 {
                        dark += 1;
                    }
                }
            }
            dark * 2 > all
        }

        /// Last column of the bottom row in the accent colour (`None`: no progress line).
        fn progress_end(f: &[u8]) -> Option<usize> {
            (0..W).rev().find(|&x| {
                let (r, _, b) = rgb(f, x, H - 3);
                r - b > 80
            })
        }

        fn qr_codes(f: &[u8]) -> Vec<String> {
            // The bottom-right corner above the band.
            let (x0, y0, w, h) = (W - 420, 560, 420, 420);
            let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
                luma(f, x0 + x, y0 + y).clamp(0, 255) as u8
            });
            prepared
                .detect_grids()
                .into_iter()
                .filter_map(|g| g.decode().ok().map(|(_, text)| text))
                .collect()
        }

        /// Bright (text) pixels of each frame's band, for every frame of `joined`.
        fn band_text_per_frame(joined: &Path) -> Vec<usize> {
            let rows = BAND_ROWS.len();
            let out = std::process::Command::new(ffmpeg().unwrap())
                .args(["-v", "error", "-i"])
                .arg(joined)
                .args(["-vf", &format!("crop={W}:{rows}:0:{}", BAND_ROWS.start)])
                .args(["-pix_fmt", "gray", "-f", "rawvideo", "-"])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
                .chunks(W * rows)
                .map(|f| f.iter().filter(|&&v| v > 200).count())
                .collect()
        }

        /// Interval mode: the band (with the code) shows at the start of the interval and is
        /// gone half a minute later; the progress line ends at `(start + t) / duration` of the
        /// width.
        #[tokio::test(flavor = "multi_thread")]
        async fn interval_mode_shows_the_band_code_and_progress_where_expected() {
            let Some(_) = tools() else {
                eprintln!("skip: no ffmpeg or font");
                return;
            };
            let settings = TickerSettings {
                enabled: true,
                ..TickerSettings::default()
            };
            let (c, qr) = clip("► Now: Test   ·   added by Host", true);
            let seg = start(&c, &settings, qr, Some(60.0), 20.0, 31, false);
            let joined = finish(&c, &seg).await;

            let early = frame(&joined, 1.0);
            assert!(band_visible(&early));
            assert_eq!(qr_codes(&early), [ADDRESS]);
            let end = progress_end(&early).expect("progress line");
            let expected = (20.0 + 1.0) / 60.0 * W as f64;
            assert!((end as f64 - expected).abs() <= 4.0, "{end} vs {expected}");

            let late = frame(&joined, 30.0);
            assert!(!band_visible(&late));
            assert!(
                qr_codes(&late).is_empty(),
                "the code shows only with the band"
            );
            let end = progress_end(&late).expect("progress line");
            let expected = (20.0 + 30.0) / 60.0 * W as f64;
            assert!((end as f64 - expected).abs() <= 4.0, "{end} vs {expected}");
        }

        /// Continuous mode: the band is always there; a live stream has no progress line; the
        /// code set to "always" shows too.
        #[tokio::test(flavor = "multi_thread")]
        async fn continuous_mode_keeps_the_band_and_a_live_stream_has_no_progress_line() {
            let Some(_) = tools() else {
                eprintln!("skip: no ffmpeg or font");
                return;
            };
            let settings = TickerSettings {
                enabled: true,
                mode: TickerMode::Continuous,
                qr: TickerQr::Always,
                ..TickerSettings::default()
            };
            let (c, qr) = clip("► Now: Live   ·   added by Host", true);
            let seg = start(&c, &settings, qr, None, 0.0, 11, false);
            let joined = finish(&c, &seg).await;
            for at in [1.0, 5.0, 10.0] {
                let f = frame(&joined, at);
                assert!(band_visible(&f), "{at}");
                assert_eq!(progress_end(&f), None, "{at}");
                assert_eq!(qr_codes(&f), [ADDRESS], "{at}");
            }
        }

        /// A new text written while ffmpeg runs shows in a later frame (`reload=1`).
        #[tokio::test(flavor = "multi_thread")]
        async fn a_new_text_shows_in_a_later_frame() {
            let Some(_) = tools() else {
                eprintln!("skip: no ffmpeg or font");
                return;
            };
            let settings = TickerSettings {
                enabled: true,
                mode: TickerMode::Continuous,
                speed: TickerSpeed::Fast,
                qr: TickerQr::Off,
                ..TickerSettings::default()
            };
            // Dense letters, then a single dot: the band's bright pixels drop.
            let dense = "W".repeat(120);
            let sparse = ".".to_string();
            let (c, _) = clip(&dense, false);
            let seg = start(&c, &settings, None, None, 0.0, 9, true);
            // Written once ffmpeg's output is past 3 s (its filters are ahead of that) and
            // well before the end (it runs at the pace the picture plays).
            let deadline = Instant::now() + Duration::from_secs(60);
            while seg.out_time_s().is_none_or(|t| t < 3.0) {
                assert!(Instant::now() < deadline, "{:?}", seg.error());
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let written_at = seg.out_time_s().unwrap();
            TickerText::write_atomic(&c.dir.join(TEXT_FILE), &sparse).unwrap();
            let joined = finish(&c, &seg).await;
            assert!(written_at < 7.0, "the change came too late: {written_at}");
            let bright = band_text_per_frame(&joined);
            // 30 frames per second; the text has crossed the whole width after 2 s.
            let before = bright[60..90].iter().min().copied().unwrap();
            let after = bright[bright.len() - 30..].iter().max().copied().unwrap();
            assert!(before > 4 * after.max(1), "before {before}, after {after}");
        }

        /// Review Focus 2: the text file replaced 100 times while ffmpeg reads it every frame:
        /// ffmpeg never fails and no frame has an empty band.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_hundred_rewrites_while_ffmpeg_reads_never_leave_an_empty_band() {
            let Some(_) = tools() else {
                eprintln!("skip: no ffmpeg or font");
                return;
            };
            let settings = TickerSettings {
                enabled: true,
                mode: TickerMode::Continuous,
                speed: TickerSpeed::Fast,
                qr: TickerQr::Off,
                ..TickerSettings::default()
            };
            let a = format!("► Now: {}", "Ala ma kota ".repeat(20));
            let b = format!("► Teraz: {}", "Kot ma Alę ".repeat(20));
            let (c, _) = clip(&a, false);
            let seg = start(&c, &settings, None, None, 0.0, 10, true);
            let path = c.dir.join(TEXT_FILE);
            let writer = std::thread::spawn(move || {
                for i in 0..100 {
                    let text = if i % 2 == 0 { &b } else { &a };
                    TickerText::write_atomic(&path, text).unwrap();
                    std::thread::sleep(Duration::from_millis(70));
                }
            });
            let joined = finish(&c, &seg).await;
            writer.join().unwrap();
            let bright = band_text_per_frame(&joined);
            assert!(bright.len() >= 290, "{} frames", bright.len());
            // From 0.5 s on the text has entered the frame.
            let empty: Vec<usize> = (15..bright.len()).filter(|&i| bright[i] < 30).collect();
            assert!(empty.is_empty(), "frames with an empty band: {empty:?}");
        }
    }
}
