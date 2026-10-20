//! Keeping the ticker's texts up to date while ffmpeg runs.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::settings::{TickerMode, TickerSettings};
use super::text::{NowNext, Requester, compose_text, fit_text, for_font};
use super::text_file::TickerText;
use super::{CAPTION_FILE, TEXT_FILE};
use crate::db::QueueItem;
use crate::i18n::{self, Lang};

/// A conversion slower than real time for this long is reported (the ticker forces it).
const SLOW_AFTER_MS: u64 = 30_000;
/// A clip with the ticker is read at real time, so its speed sits around 1.0×; only below
/// this it falls behind.
const SLOW_BELOW: f64 = 0.95;

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
    /// The ticker is drawn with the Japanese font (see [`for_font`]).
    japanese_font: bool,
}

impl TickerWriter {
    pub fn new(
        dir: PathBuf,
        item: &QueueItem,
        schedule: Schedule,
        budget_px: Option<f64>,
        rotation: Arc<AtomicUsize>,
        japanese_font: bool,
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
            japanese_font,
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
        let text = for_font(
            compose_text(content.lang, &now, &content.settings, self.message_index),
            self.japanese_font,
        );
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
            TickerText::write_in_place(&self.dir.join(CAPTION_FILE), caption)?;
        }
        if self.written.as_ref().is_none_or(|(t, _)| t != text) {
            TickerText::write_in_place(&self.dir.join(TEXT_FILE), text)?;
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
        if speed < SLOW_BELOW {
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
