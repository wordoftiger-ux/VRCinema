//! The ticker settings of a channel.

use serde::Serialize;

/// Interval lengths the owner can choose, in seconds.
pub const INTERVALS_S: [u32; 4] = [60, 120, 300, 600];
pub const MAX_MESSAGES: usize = 10;
pub const MAX_MESSAGE_CHARS: usize = 200;

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
