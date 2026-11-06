//! The channel's advanced broadcast options (spec `2026-11-06-advanced-design.md`): transitions
//! between films, the bitrate peak limit, burned-in subtitles, loudness normalisation and the
//! black picture / silence alarms. Part of the channel settings (camelCase keys, flattened into
//! `ChannelSettings`); every default keeps the channel airing as before.

use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::i18n::Msg;

/// The loudness targets the owner can choose (LUFS).
pub const LOUDNESS_TARGETS: [i32; 4] = [-23, -18, -16, -14];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedSettings {
    /// Fade in at each clip start and out before a film's end (picture and sound).
    pub transitions_enabled: bool,
    /// Length of each fade (seconds).
    #[serde(serialize_with = "number")]
    pub transition_s: f64,
    /// The highest bitrate of a converted picture (Mbit/s); 0: automatic (6).
    #[serde(serialize_with = "number")]
    pub peak_mbps: f64,
    /// The encoder's buffer in seconds of the peak bitrate.
    #[serde(serialize_with = "number")]
    pub peak_buffer_s: f64,
    /// Burn the film's subtitles (yt-dlp) into the picture.
    pub subtitles_enabled: bool,
    /// Which subtitles (a language code: `en`, `pl`, `pt-BR`).
    pub subtitles_language: String,
    /// Normalise the loudness of the sound.
    pub loudness_enabled: bool,
    /// One of [`LOUDNESS_TARGETS`].
    pub loudness_target: i32,
    /// Watch the stream for a black picture and silence.
    pub alarms_enabled: bool,
    pub alarm_black_s: u32,
    pub alarm_silence_s: u32,
    /// Below this level (dBFS) the sound counts as silence.
    pub alarm_silence_db: i32,
}

fn number<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.fract() == 0.0 && v.abs() < 1e9 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            transitions_enabled: false,
            transition_s: 1.0,
            peak_mbps: 0.0,
            peak_buffer_s: 2.0,
            subtitles_enabled: false,
            subtitles_language: "en".into(),
            loudness_enabled: false,
            loudness_target: -16,
            alarms_enabled: false,
            alarm_black_s: 10,
            alarm_silence_s: 15,
            alarm_silence_db: -50,
        }
    }
}

/// A validated partial update of [`AdvancedSettings`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdvancedPatch {
    pub transitions_enabled: Option<bool>,
    pub transition_s: Option<f64>,
    pub peak_mbps: Option<f64>,
    pub peak_buffer_s: Option<f64>,
    pub subtitles_enabled: Option<bool>,
    pub subtitles_language: Option<String>,
    pub loudness_enabled: Option<bool>,
    pub loudness_target: Option<i32>,
    pub alarms_enabled: Option<bool>,
    pub alarm_black_s: Option<u32>,
    pub alarm_silence_s: Option<u32>,
    pub alarm_silence_db: Option<i32>,
}

fn bool_of(key: &str, value: &Value) -> Result<bool, Msg> {
    value
        .as_bool()
        .ok_or_else(|| Msg::error("setting_not_boolean").arg("key", key))
}

/// A number within `min..=max`, rounded to `step`.
fn ranged(key: &str, value: &Value, min: f64, max: f64, step: f64) -> Result<f64, Msg> {
    let out_of_range = || {
        Msg::error("setting_out_of_range")
            .arg("key", key)
            .arg("min", min)
            .arg("max", max)
    };
    let n = value
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(out_of_range)?;
    // To the step, without the float noise of the multiplication (4.6, not 4.6000000000000005).
    let n = ((n / step).round() * step * 1e6).round() / 1e6;
    if n < min || n > max {
        return Err(out_of_range());
    }
    Ok(n)
}

/// The peak limit: 0 (automatic) or 1–12 Mbit/s, to a tenth.
fn peak(key: &str, value: &Value) -> Result<f64, Msg> {
    if value.as_f64() == Some(0.0) {
        return Ok(0.0);
    }
    ranged(key, value, 1.0, 12.0, 0.1)
}

/// A language code as yt-dlp names subtitles: 2–3 letters, optionally `-` and 2–8 letters or
/// digits (`en`, `pl`, `pt-BR`, `zh-Hans`).
pub fn valid_language_code(code: &str) -> bool {
    let mut parts = code.splitn(2, '-');
    let main = parts.next().unwrap_or("");
    let main_ok = (2..=3).contains(&main.len()) && main.bytes().all(|b| b.is_ascii_alphabetic());
    let rest_ok = parts
        .next()
        .is_none_or(|r| (2..=8).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric()));
    main_ok && rest_ok
}

fn language(key: &str, value: &Value) -> Result<String, Msg> {
    value
        .as_str()
        .map(str::trim)
        .filter(|c| valid_language_code(c))
        .map(String::from)
        .ok_or_else(|| Msg::error("setting_not_language_code").arg("key", key))
}

fn loudness_target(key: &str, value: &Value) -> Result<i32, Msg> {
    value
        .as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .filter(|v| LOUDNESS_TARGETS.contains(v))
        .ok_or_else(|| {
            let all: Vec<String> = LOUDNESS_TARGETS.iter().map(i32::to_string).collect();
            Msg::error("setting_not_one_of")
                .arg("key", key)
                .arg("values", all.join(", "))
        })
}

/// One key of a settings patch: `None` when the key is not an advanced one.
pub fn sanitize_key(key: &str, value: &Value, out: &mut AdvancedPatch) -> Option<Result<(), Msg>> {
    // `Ok(false)`: not an advanced key.
    let result = (|| -> Result<bool, Msg> {
        match key {
            "transitionsEnabled" => out.transitions_enabled = Some(bool_of(key, value)?),
            "transitionS" => out.transition_s = Some(ranged(key, value, 0.5, 3.0, 0.5)?),
            "peakMbps" => out.peak_mbps = Some(peak(key, value)?),
            "peakBufferS" => out.peak_buffer_s = Some(ranged(key, value, 0.5, 4.0, 0.5)?),
            "subtitlesEnabled" => out.subtitles_enabled = Some(bool_of(key, value)?),
            "subtitlesLanguage" => out.subtitles_language = Some(language(key, value)?),
            "loudnessEnabled" => out.loudness_enabled = Some(bool_of(key, value)?),
            "loudnessTarget" => out.loudness_target = Some(loudness_target(key, value)?),
            "alarmsEnabled" => out.alarms_enabled = Some(bool_of(key, value)?),
            "alarmBlackS" => out.alarm_black_s = Some(ranged(key, value, 2.0, 300.0, 1.0)? as u32),
            "alarmSilenceS" => {
                out.alarm_silence_s = Some(ranged(key, value, 2.0, 300.0, 1.0)? as u32);
            }
            "alarmSilenceDb" => {
                out.alarm_silence_db = Some(ranged(key, value, -80.0, -20.0, 1.0)? as i32);
            }
            _ => return Ok(false),
        }
        Ok(true)
    })();
    match result {
        Ok(false) => None,
        Ok(true) => Some(Ok(())),
        Err(e) => Some(Err(e)),
    }
}

impl AdvancedSettings {
    /// Applies a stored key (a hand-edited or out-of-range value keeps the default); `false`
    /// when the key is not an advanced one.
    pub fn read_stored(&mut self, key: &str, value: &Value) -> bool {
        let mut patch = AdvancedPatch::default();
        match sanitize_key(key, value, &mut patch) {
            None => false,
            Some(Err(_)) => true,
            Some(Ok(())) => {
                self.apply(&patch);
                true
            }
        }
    }

    pub fn apply(&mut self, p: &AdvancedPatch) {
        macro_rules! set {
            ($($field:ident),*) => {$(
                if let Some(v) = &p.$field {
                    self.$field = v.clone();
                }
            )*};
        }
        set!(
            transitions_enabled,
            transition_s,
            peak_mbps,
            peak_buffer_s,
            subtitles_enabled,
            subtitles_language,
            loudness_enabled,
            loudness_target,
            alarms_enabled,
            alarm_black_s,
            alarm_silence_s,
            alarm_silence_db
        );
    }
}

impl AdvancedPatch {
    /// The keys to store and their JSON text.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let mut changed = AdvancedSettings::default();
        changed.apply(self);
        let json = serde_json::to_value(&changed).unwrap_or(Value::Null);
        let present = [
            ("transitionsEnabled", self.transitions_enabled.is_some()),
            ("transitionS", self.transition_s.is_some()),
            ("peakMbps", self.peak_mbps.is_some()),
            ("peakBufferS", self.peak_buffer_s.is_some()),
            ("subtitlesEnabled", self.subtitles_enabled.is_some()),
            ("subtitlesLanguage", self.subtitles_language.is_some()),
            ("loudnessEnabled", self.loudness_enabled.is_some()),
            ("loudnessTarget", self.loudness_target.is_some()),
            ("alarmsEnabled", self.alarms_enabled.is_some()),
            ("alarmBlackS", self.alarm_black_s.is_some()),
            ("alarmSilenceS", self.alarm_silence_s.is_some()),
            ("alarmSilenceDb", self.alarm_silence_db.is_some()),
        ];
        present
            .into_iter()
            .filter(|(_, set)| *set)
            .map(|(key, _)| (key, json[key].to_string()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn patch(v: Value) -> Result<AdvancedPatch, Msg> {
        let mut out = AdvancedPatch::default();
        for (k, v) in v.as_object().unwrap() {
            sanitize_key(k, v, &mut out).expect("advanced key")?;
        }
        Ok(out)
    }

    #[test]
    fn the_defaults_keep_todays_behaviour() {
        let d = AdvancedSettings::default();
        assert!(!d.transitions_enabled && !d.subtitles_enabled && !d.loudness_enabled);
        assert!(!d.alarms_enabled);
        assert_eq!((d.peak_mbps, d.peak_buffer_s), (0.0, 2.0));
        assert_eq!(
            serde_json::to_value(&d).unwrap(),
            json!({
                "transitionsEnabled": false, "transitionS": 1, "peakMbps": 0, "peakBufferS": 2,
                "subtitlesEnabled": false, "subtitlesLanguage": "en", "loudnessEnabled": false,
                "loudnessTarget": -16, "alarmsEnabled": false, "alarmBlackS": 10,
                "alarmSilenceS": 15, "alarmSilenceDb": -50
            })
        );
    }

    #[test]
    fn values_are_checked_and_rounded() {
        let p = patch(
            json!({"transitionS": 1.3, "peakMbps": 4.56, "alarmSilenceDb": -60.4,
            "subtitlesLanguage": " pt-BR ", "loudnessTarget": -14}),
        )
        .unwrap();
        assert_eq!(p.transition_s, Some(1.5));
        assert_eq!(p.peak_mbps, Some(4.6));
        assert_eq!(p.alarm_silence_db, Some(-60));
        assert_eq!(p.subtitles_language.as_deref(), Some("pt-BR"));
        assert_eq!(patch(json!({"peakMbps": 0})).unwrap().peak_mbps, Some(0.0));
        for bad in [
            json!({"transitionS": 9}),
            json!({"peakMbps": 0.5}),
            json!({"alarmSilenceDb": -5}),
            json!({"loudnessTarget": -20}),
            json!({"subtitlesLanguage": "polski!"}),
            json!({"alarmsEnabled": "yes"}),
        ] {
            assert!(patch(bad.clone()).is_err(), "{bad}");
        }
        let mut out = AdvancedPatch::default();
        assert!(sanitize_key("moderation", &json!(true), &mut out).is_none());
    }

    #[test]
    fn stored_entries_round_trip() {
        let p =
            patch(json!({"loudnessEnabled": true, "transitionS": 2.5, "subtitlesLanguage": "pl"}))
                .unwrap();
        let entries = p.entries();
        assert_eq!(
            entries,
            [
                ("transitionS", "2.5".to_string()),
                ("subtitlesLanguage", "\"pl\"".to_string()),
                ("loudnessEnabled", "true".to_string()),
            ]
        );
        let mut s = AdvancedSettings::default();
        for (k, text) in &entries {
            assert!(s.read_stored(k, &serde_json::from_str(text).unwrap()));
        }
        assert!(s.loudness_enabled && s.transition_s == 2.5 && s.subtitles_language == "pl");
        // A bad stored value keeps the default.
        assert!(s.read_stored("alarmBlackS", &json!(-3)));
        assert_eq!(s.alarm_black_s, 10);
        assert!(!s.read_stored("tickerMode", &json!("x")));
    }

    #[test]
    fn language_codes() {
        for ok in ["en", "pl", "pt-BR", "zh-Hans", "fil"] {
            assert!(valid_language_code(ok), "{ok}");
        }
        for bad in ["", "e", "english", "en-", "en-B", "pl.*", "en,pl"] {
            assert!(!valid_language_code(bad), "{bad}");
        }
    }
}
