//! Per-channel settings (`settings` table: one JSON text value per camelCase key).

use rusqlite::{Connection, params};
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::i18n::{Lang, Msg};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSettings {
    pub allowed_domains: Vec<String>,
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
}

fn serialize_lang<S: Serializer>(lang: &Lang, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(lang.as_str())
}

impl Default for ChannelSettings {
    fn default() -> Self {
        Self {
            allowed_domains: ["youtube.com", "youtu.be", "pixeldrain.com", "cda.pl"]
                .map(String::from)
                .to_vec(),
            max_duration_s: 7200.0,
            request_limit_count: 3,
            request_limit_window_s: 600,
            max_active_per_requester: 5,
            moderation: false,
            force_transcode: false,
            discontinuity_tags: true,
            slate_language: Lang::En,
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
    pub allowed_domains: Option<Vec<String>>,
    pub max_duration_s: Option<f64>,
    pub request_limit_count: Option<u32>,
    pub request_limit_window_s: Option<u64>,
    pub max_active_per_requester: Option<u32>,
    pub moderation: Option<bool>,
    pub force_transcode: Option<bool>,
    pub discontinuity_tags: Option<bool>,
    pub slate_language: Option<Lang>,
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
            "allowedDomains" => out.allowed_domains = Some(expect_list(key, value)?),
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
            "forceTranscode" => out.force_transcode = Some(expect_bool(key, value)?),
            "discontinuityTags" => out.discontinuity_tags = Some(expect_bool(key, value)?),
            "slateLanguage" => out.slate_language = Some(expect_lang(key, value)?),
            _ => return Err(Msg::error("unknown_setting").arg("key", key)),
        }
    }
    Ok(out)
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
                "allowedDomains" => {
                    if let Ok(list) = serde_json::from_value::<Vec<String>>(value) {
                        s.allowed_domains = list;
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
                "forceTranscode" => {
                    s.force_transcode = value.as_bool().unwrap_or(s.force_transcode)
                }
                "discontinuityTags" => {
                    s.discontinuity_tags = value.as_bool().unwrap_or(s.discontinuity_tags);
                }
                "slateLanguage" => {
                    s.slate_language = lang_from_tag(&value).unwrap_or(s.slate_language)
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
        let entries: [(&str, Option<String>); 9] = [
            (
                "allowedDomains",
                patch.allowed_domains.as_deref().map(list_json),
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
}
