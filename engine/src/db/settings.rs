//! Per-channel settings (`settings` table: one JSON text value per camelCase key).

use rusqlite::{Connection, params};
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::i18n::{Lang, Msg};
use crate::pipeline::ticker::{
    INTERVALS_S, MAX_MESSAGE_CHARS, MAX_MESSAGES, TickerMode, TickerQr, TickerSettings, TickerSpeed,
};

/// Which links viewers may send (the owner's links are never limited).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkMode {
    /// Any site (the protection of the home network still applies).
    All,
    /// Only the sites of `allowedDomains`.
    Allowlist,
    /// Every site but those of `blockedDomains`.
    Blocklist,
}

impl LinkMode {
    pub const ALL: [LinkMode; 3] = [Self::All, Self::Allowlist, Self::Blocklist];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Allowlist => "allowlist",
            Self::Blocklist => "blocklist",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSettings {
    pub link_mode: LinkMode,
    pub allowed_domains: Vec<String>,
    /// Always kept, used in the `blocklist` mode.
    pub blocked_domains: Vec<String>,
    #[serde(serialize_with = "serialize_number")]
    pub max_duration_s: f64,
    pub request_limit_count: u32,
    pub request_limit_window_s: u64,
    pub max_active_per_requester: u32,
    pub moderation: bool,
    pub force_transcode: bool,
    pub discontinuity_tags: bool,
    /// Language of the slates and of the info ticker on the stream.
    #[serde(serialize_with = "serialize_lang")]
    pub slate_language: Lang,
    /// The info ticker (keys `tickerEnabled`, `tickerMode`, ...).
    #[serde(flatten)]
    pub ticker: TickerSettings,
    /// Viewers may send several links at once and playlists (the request page's list form).
    pub viewer_lists: bool,
    /// Most videos one viewer list adds (also the entries read from a playlist).
    pub viewer_list_max: u32,
    /// The owner's upload speed in Mbit/s (0: not given): viewers × the stream's bitrate is
    /// compared with it and the panel warns (nothing is ever refused).
    #[serde(serialize_with = "serialize_number")]
    pub upload_mbps: f64,
}

fn serialize_lang<S: Serializer>(lang: &Lang, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(lang.as_str())
}

impl Default for ChannelSettings {
    fn default() -> Self {
        Self {
            link_mode: LinkMode::All,
            allowed_domains: ["youtube.com", "youtu.be", "pixeldrain.com", "cda.pl"]
                .map(String::from)
                .to_vec(),
            blocked_domains: Vec::new(),
            max_duration_s: 7200.0,
            request_limit_count: 3,
            request_limit_window_s: 600,
            max_active_per_requester: 5,
            moderation: false,
            viewer_lists: false,
            viewer_list_max: 10,
            upload_mbps: 0.0,
            force_transcode: false,
            discontinuity_tags: true,
            slate_language: Lang::En,
            ticker: TickerSettings::default(),
        }
    }
}

/// Whole numbers as integers (like JS `JSON.stringify(7200)`), others as floats.
fn serialize_number<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    match whole_number(*v) {
        Some(i) => s.serialize_i64(i),
        None => s.serialize_f64(*v),
    }
}

fn whole_number(v: f64) -> Option<i64> {
    // 2^53: every integer up to here is exactly representable as f64.
    (v.is_finite() && v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_992.0).then_some(v as i64)
}

/// A validated, normalised partial update of [`ChannelSettings`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsPatch {
    pub link_mode: Option<LinkMode>,
    pub allowed_domains: Option<Vec<String>>,
    pub blocked_domains: Option<Vec<String>>,
    pub max_duration_s: Option<f64>,
    pub request_limit_count: Option<u32>,
    pub request_limit_window_s: Option<u64>,
    pub max_active_per_requester: Option<u32>,
    pub moderation: Option<bool>,
    pub viewer_lists: Option<bool>,
    pub viewer_list_max: Option<u32>,
    pub upload_mbps: Option<f64>,
    pub force_transcode: Option<bool>,
    pub discontinuity_tags: Option<bool>,
    pub slate_language: Option<Lang>,
    pub ticker_enabled: Option<bool>,
    pub ticker_mode: Option<TickerMode>,
    pub ticker_interval_s: Option<u32>,
    pub ticker_speed: Option<TickerSpeed>,
    pub ticker_show_next: Option<bool>,
    pub ticker_show_address: Option<bool>,
    pub ticker_qr: Option<TickerQr>,
    pub ticker_progress: Option<bool>,
    pub ticker_messages: Option<Vec<String>>,
}

fn normalize_domain(v: &str) -> String {
    let s = v.trim().to_lowercase();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(&s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.split('/').next().unwrap_or("");
    s.to_string()
}

fn expect_list(key: &str, value: &Value) -> Result<Vec<String>, Msg> {
    let err = || Msg::error("setting_not_list").arg("key", key);
    let arr = value.as_array().ok_or_else(err)?;
    let mut out = Vec::new();
    for v in arr {
        let s = v.as_str().ok_or_else(err)?;
        let d = normalize_domain(s);
        if !d.is_empty() {
            out.push(d);
        }
    }
    Ok(out)
}

fn expect_number(key: &str, value: &Value) -> Result<f64, Msg> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(|| Msg::error("setting_not_number").arg("key", key))
}

fn expect_bool(key: &str, value: &Value) -> Result<bool, Msg> {
    value
        .as_bool()
        .ok_or_else(|| Msg::error("setting_not_boolean").arg("key", key))
}

/// A language by its exact tag (`en`, `pl`, ...).
fn lang_from_tag(value: &Value) -> Option<Lang> {
    let tag = value.as_str()?;
    Lang::ALL.into_iter().find(|l| l.as_str() == tag)
}

fn expect_lang(key: &str, value: &Value) -> Result<Lang, Msg> {
    lang_from_tag(value).ok_or_else(|| {
        let all = Lang::ALL.map(Lang::as_str).join(", ");
        Msg::error("setting_not_language")
            .arg("key", key)
            .arg("languages", all)
    })
}

/// One of `all` by its exact name.
fn one_of<T: Copy>(
    key: &str,
    value: &Value,
    all: &[T],
    name: fn(T) -> &'static str,
) -> Result<T, Msg> {
    value
        .as_str()
        .and_then(|v| all.iter().copied().find(|t| name(*t) == v))
        .ok_or_else(|| {
            let names: Vec<&str> = all.iter().map(|t| name(*t)).collect();
            Msg::error("setting_not_one_of")
                .arg("key", key)
                .arg("values", names.join(", "))
        })
}

fn expect_interval(key: &str, value: &Value) -> Result<u32, Msg> {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| INTERVALS_S.contains(v))
        .ok_or_else(|| {
            let all: Vec<String> = INTERVALS_S.iter().map(u32::to_string).collect();
            Msg::error("setting_not_one_of")
                .arg("key", key)
                .arg("values", all.join(", "))
        })
}

/// The owner's ticker messages: trimmed, empty ones dropped, at most [`MAX_MESSAGES`] of at
/// most [`MAX_MESSAGE_CHARS`] characters.
fn expect_messages(key: &str, value: &Value) -> Result<Vec<String>, Msg> {
    let not_list = || Msg::error("setting_not_list").arg("key", key);
    let list = value.as_array().ok_or_else(not_list)?;
    let mut out = Vec::new();
    for v in list {
        let text = v.as_str().ok_or_else(not_list)?.trim();
        if text.chars().count() > MAX_MESSAGE_CHARS {
            return Err(Msg::error("ticker_message_too_long")
                .arg("key", key)
                .arg("max", MAX_MESSAGE_CHARS));
        }
        if !text.is_empty() {
            out.push(text.to_string());
        }
    }
    if out.len() > MAX_MESSAGES {
        return Err(Msg::error("ticker_too_many_messages")
            .arg("key", key)
            .arg("max", MAX_MESSAGES));
    }
    Ok(out)
}

/// Validates and normalises a partial settings object (e.g. from the admin
/// panel). Mirrors the TS `sanitizeSettingsPatch`; errors are the messages
/// shown to the admin. Keys are processed in input order, so the first
/// invalid one is reported.
///
/// Count/window settings are stored as integers (`u32`/`u64`); a fractional
/// value is truncated toward zero.
pub fn sanitize_settings_patch(patch: &Value) -> Result<SettingsPatch, Msg> {
    let obj = patch
        .as_object()
        .ok_or_else(|| Msg::error("settings_not_object"))?;
    let mut out = SettingsPatch::default();
    for (key, value) in obj {
        match key.as_str() {
            "linkMode" => {
                out.link_mode = Some(one_of(key, value, &LinkMode::ALL, LinkMode::as_str)?);
            }
            "allowedDomains" => out.allowed_domains = Some(expect_list(key, value)?),
            "blockedDomains" => out.blocked_domains = Some(expect_list(key, value)?),
            "maxDurationS" => out.max_duration_s = Some(expect_number(key, value)?),
            // `as` casts from f64 are saturating and truncating.
            "requestLimitCount" => {
                out.request_limit_count = Some(expect_number(key, value)? as u32)
            }
            "requestLimitWindowS" => {
                out.request_limit_window_s = Some(expect_number(key, value)? as u64);
            }
            "maxActivePerRequester" => {
                out.max_active_per_requester = Some(expect_number(key, value)? as u32);
            }
            "moderation" => out.moderation = Some(expect_bool(key, value)?),
            "viewerLists" => out.viewer_lists = Some(expect_bool(key, value)?),
            "viewerListMax" => {
                out.viewer_list_max = Some(list_max(expect_number(key, value)?));
            }
            "uploadMbps" => {
                // Up to 100 Gbit/s, to a tenth.
                let mbps = expect_number(key, value)?.min(100_000.0);
                out.upload_mbps = Some((mbps * 10.0).round() / 10.0);
            }
            "forceTranscode" => out.force_transcode = Some(expect_bool(key, value)?),
            "discontinuityTags" => out.discontinuity_tags = Some(expect_bool(key, value)?),
            "slateLanguage" => out.slate_language = Some(expect_lang(key, value)?),
            "tickerEnabled" => out.ticker_enabled = Some(expect_bool(key, value)?),
            "tickerMode" => {
                out.ticker_mode = Some(one_of(key, value, &TickerMode::ALL, TickerMode::as_str)?);
            }
            "tickerIntervalS" => out.ticker_interval_s = Some(expect_interval(key, value)?),
            "tickerSpeed" => {
                out.ticker_speed =
                    Some(one_of(key, value, &TickerSpeed::ALL, TickerSpeed::as_str)?);
            }
            "tickerShowNext" => out.ticker_show_next = Some(expect_bool(key, value)?),
            "tickerShowAddress" => out.ticker_show_address = Some(expect_bool(key, value)?),
            "tickerQr" => {
                out.ticker_qr = Some(one_of(key, value, &TickerQr::ALL, TickerQr::as_str)?);
            }
            "tickerProgress" => out.ticker_progress = Some(expect_bool(key, value)?),
            "tickerMessages" => out.ticker_messages = Some(expect_messages(key, value)?),
            _ => return Err(Msg::error("unknown_setting").arg("key", key)),
        }
    }
    Ok(out)
}

/// The largest [`ChannelSettings::viewer_list_max`] the owner can set.
pub const VIEWER_LIST_MAX: u32 = 50;

/// A list size within 1..=[`VIEWER_LIST_MAX`] (a fraction truncated).
fn list_max(n: f64) -> u32 {
    (n as u32).clamp(1, VIEWER_LIST_MAX)
}

/// JSON text of a number as JS would write it (`600`, not `600.0`).
fn number_json(v: f64) -> String {
    match whole_number(v) {
        Some(i) => i.to_string(),
        None => serde_json::to_string(&v).unwrap_or_else(|_| "0".to_string()),
    }
}

fn list_json(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}

pub struct SettingsRepo<'a> {
    conn: &'a Connection,
}

impl<'a> SettingsRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Stored settings merged over the defaults. A value that is not valid JSON
    /// or has the wrong type is ignored (the default stays), so the channel
    /// keeps working with a hand-edited database.
    pub fn get(&self, channel_id: i64) -> rusqlite::Result<ChannelSettings> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM settings WHERE channel_id = ?")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([channel_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut s = ChannelSettings::default();
        for (key, text) in rows {
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let num = value.as_f64().filter(|n| n.is_finite() && *n >= 0.0);
            match key.as_str() {
                "linkMode" => {
                    s.link_mode = one_of(&key, &value, &LinkMode::ALL, LinkMode::as_str)
                        .unwrap_or(s.link_mode);
                }
                "allowedDomains" => {
                    if let Ok(list) = serde_json::from_value::<Vec<String>>(value) {
                        s.allowed_domains = list;
                    }
                }
                "blockedDomains" => {
                    if let Ok(list) = serde_json::from_value::<Vec<String>>(value) {
                        s.blocked_domains = list;
                    }
                }
                "maxDurationS" => s.max_duration_s = num.unwrap_or(s.max_duration_s),
                "requestLimitCount" => {
                    s.request_limit_count = num.map_or(s.request_limit_count, |n| n as u32);
                }
                "requestLimitWindowS" => {
                    s.request_limit_window_s = num.map_or(s.request_limit_window_s, |n| n as u64);
                }
                "maxActivePerRequester" => {
                    s.max_active_per_requester =
                        num.map_or(s.max_active_per_requester, |n| n as u32);
                }
                "moderation" => s.moderation = value.as_bool().unwrap_or(s.moderation),
                "viewerLists" => s.viewer_lists = value.as_bool().unwrap_or(s.viewer_lists),
                "viewerListMax" => s.viewer_list_max = num.map_or(s.viewer_list_max, list_max),
                "uploadMbps" => s.upload_mbps = num.unwrap_or(s.upload_mbps),
                "forceTranscode" => {
                    s.force_transcode = value.as_bool().unwrap_or(s.force_transcode)
                }
                "discontinuityTags" => {
                    s.discontinuity_tags = value.as_bool().unwrap_or(s.discontinuity_tags);
                }
                "slateLanguage" => {
                    s.slate_language = lang_from_tag(&value).unwrap_or(s.slate_language)
                }
                "tickerEnabled" => s.ticker.enabled = value.as_bool().unwrap_or(s.ticker.enabled),
                "tickerMode" => {
                    s.ticker.mode = one_of(&key, &value, &TickerMode::ALL, TickerMode::as_str)
                        .unwrap_or(s.ticker.mode);
                }
                "tickerIntervalS" => {
                    s.ticker.interval_s =
                        expect_interval(&key, &value).unwrap_or(s.ticker.interval_s);
                }
                "tickerSpeed" => {
                    s.ticker.speed = one_of(&key, &value, &TickerSpeed::ALL, TickerSpeed::as_str)
                        .unwrap_or(s.ticker.speed);
                }
                "tickerShowNext" => {
                    s.ticker.show_next = value.as_bool().unwrap_or(s.ticker.show_next);
                }
                "tickerShowAddress" => {
                    s.ticker.show_address = value.as_bool().unwrap_or(s.ticker.show_address);
                }
                "tickerQr" => {
                    s.ticker.qr = one_of(&key, &value, &TickerQr::ALL, TickerQr::as_str)
                        .unwrap_or(s.ticker.qr);
                }
                "tickerProgress" => {
                    s.ticker.progress = value.as_bool().unwrap_or(s.ticker.progress);
                }
                "tickerMessages" => {
                    if let Ok(messages) = expect_messages(&key, &value) {
                        s.ticker.messages = messages;
                    }
                }
                _ => {}
            }
        }
        Ok(s)
    }

    /// Upserts every key present in the patch and returns the merged settings.
    pub fn update(
        &self,
        channel_id: i64,
        patch: &SettingsPatch,
    ) -> rusqlite::Result<ChannelSettings> {
        let quoted = |name: &str| format!("\"{name}\"");
        let entries: [(&str, Option<String>); 23] = [
            ("linkMode", patch.link_mode.map(|m| quoted(m.as_str()))),
            (
                "allowedDomains",
                patch.allowed_domains.as_deref().map(list_json),
            ),
            (
                "blockedDomains",
                patch.blocked_domains.as_deref().map(list_json),
            ),
            ("maxDurationS", patch.max_duration_s.map(number_json)),
            (
                "requestLimitCount",
                patch.request_limit_count.map(|n| n.to_string()),
            ),
            (
                "requestLimitWindowS",
                patch.request_limit_window_s.map(|n| n.to_string()),
            ),
            (
                "maxActivePerRequester",
                patch.max_active_per_requester.map(|n| n.to_string()),
            ),
            ("moderation", patch.moderation.map(|b| b.to_string())),
            ("viewerLists", patch.viewer_lists.map(|b| b.to_string())),
            (
                "viewerListMax",
                patch.viewer_list_max.map(|n| n.to_string()),
            ),
            ("uploadMbps", patch.upload_mbps.map(number_json)),
            (
                "forceTranscode",
                patch.force_transcode.map(|b| b.to_string()),
            ),
            (
                "discontinuityTags",
                patch.discontinuity_tags.map(|b| b.to_string()),
            ),
            (
                "slateLanguage",
                patch.slate_language.map(|l| format!("\"{}\"", l.as_str())),
            ),
            ("tickerEnabled", patch.ticker_enabled.map(|b| b.to_string())),
            ("tickerMode", patch.ticker_mode.map(|m| quoted(m.as_str()))),
            (
                "tickerIntervalS",
                patch.ticker_interval_s.map(|n| n.to_string()),
            ),
            (
                "tickerSpeed",
                patch.ticker_speed.map(|v| quoted(v.as_str())),
            ),
            (
                "tickerShowNext",
                patch.ticker_show_next.map(|b| b.to_string()),
            ),
            (
                "tickerShowAddress",
                patch.ticker_show_address.map(|b| b.to_string()),
            ),
            ("tickerQr", patch.ticker_qr.map(|q| quoted(q.as_str()))),
            (
                "tickerProgress",
                patch.ticker_progress.map(|b| b.to_string()),
            ),
            (
                "tickerMessages",
                patch.ticker_messages.as_deref().map(list_json),
            ),
        ];
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO settings (channel_id, key, value) VALUES (?, ?, ?)
                 ON CONFLICT(channel_id, key) DO UPDATE SET value = excluded.value",
            )?;
            for (key, value) in &entries {
                if let Some(value) = value {
                    stmt.execute(params![channel_id, key, value])?;
                }
            }
        }
        tx.commit()?;
        self.get(channel_id)
    }

    /// Stores the default of `tickerEnabled` (on for a GPU encoder) unless a value is stored
    /// already: the first encoder detection decides, later only the owner. Returns whether it
    /// was stored now.
    pub fn seed_ticker_enabled(&self, channel_id: i64, gpu: bool) -> rusqlite::Result<bool> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO settings (channel_id, key, value) VALUES (?, 'tickerEnabled', ?)",
            params![channel_id, gpu.to_string()],
        )?;
        Ok(changed > 0)
    }
}
