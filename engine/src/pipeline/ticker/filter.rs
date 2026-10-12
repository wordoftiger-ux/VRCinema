//! How ffmpeg draws the ticker.

use super::settings::{TickerMode, TickerQr, TickerSettings};
use super::text::text_width_px;

/// The progress line (the accent of the player).
const ACCENT: &str = "0xf2b84b";

// Layout in a 1080p frame; other heights scale it (see `overlay_filter`).
const REF_HEIGHT: f64 = 1080.0;
const REF_WIDTH: f64 = 1920.0;
pub(super) const FONT_PX: f64 = 36.0;
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
