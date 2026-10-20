//! What the band says.

use super::filter::FONT_PX;
use crate::db::{AddedBy, QueueItem};
use crate::i18n::{self, Lang};

use super::settings::{TickerMode, TickerSettings};

/// Between the parts of the text.
const SEPARATOR: &str = "   ·   ";

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

/// `text` as the ticker's font can draw it: the Japanese font (Yu Gothic, chosen for Japanese
/// or for a Japanese title in any language) has no ►, only ▶.
pub fn for_font(text: String, japanese_font: bool) -> String {
    if japanese_font {
        text.replace('►', "▶")
    } else {
        text
    }
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
