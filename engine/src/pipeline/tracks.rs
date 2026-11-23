//! Which audio track and which subtitles a film airs with (spec
//! `2026-11-22-viewer-and-tracks-design.md` §4): the owner's choice for the item, else the
//! preferred languages of Settings → Advanced. Subtitles are always burned in (VRChat players
//! cannot show selectable ones).

use crate::pipeline::probe::{AudioTrack, SubtitleKind, SubtitleTrack};

/// `subtitle_track` of an item: the owner turned the subtitles off.
pub const SUBTITLES_OFF: i64 = -1;

/// ISO 639-2 codes (bibliographic and terminology forms) of the common languages and their
/// two-letter ISO 639-1 code, so `pol` matches `pl` and `ger`, `deu` match `de`.
const THREE_TO_TWO: [(&str, &str); 44] = [
    ("pol", "pl"),
    ("eng", "en"),
    ("ger", "de"),
    ("deu", "de"),
    ("fre", "fr"),
    ("fra", "fr"),
    ("spa", "es"),
    ("ita", "it"),
    ("por", "pt"),
    ("rus", "ru"),
    ("ukr", "uk"),
    ("jpn", "ja"),
    ("chi", "zh"),
    ("zho", "zh"),
    ("kor", "ko"),
    ("dut", "nl"),
    ("nld", "nl"),
    ("swe", "sv"),
    ("nor", "no"),
    ("nob", "nb"),
    ("dan", "da"),
    ("fin", "fi"),
    ("cze", "cs"),
    ("ces", "cs"),
    ("slo", "sk"),
    ("slk", "sk"),
    ("hun", "hu"),
    ("tur", "tr"),
    ("ara", "ar"),
    ("heb", "he"),
    ("gre", "el"),
    ("ell", "el"),
    ("rum", "ro"),
    ("ron", "ro"),
    ("bul", "bg"),
    ("hrv", "hr"),
    ("srp", "sr"),
    ("slv", "sl"),
    ("lit", "lt"),
    ("lav", "lv"),
    ("est", "et"),
    ("hin", "hi"),
    ("tha", "th"),
    ("vie", "vi"),
];

/// A language tag in one form: lower case, the main part only (`pt-BR` → `pt`), a known
/// three-letter code as its two letters; empty for none (`und`, `mul`, ...).
pub fn normalize_lang(tag: &str) -> String {
    let main = tag
        .trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(main.as_str(), "" | "und" | "mul" | "zxx" | "mis") {
        return String::new();
    }
    THREE_TO_TWO
        .iter()
        .find(|(three, _)| *three == main)
        .map_or(main, |(_, two)| (*two).to_string())
}

/// Two tags name the same language (`pl` and `pol`); an untagged stream matches nothing.
pub fn same_lang(a: &str, b: &str) -> bool {
    let a = normalize_lang(a);
    !a.is_empty() && a == normalize_lang(b)
}

/// A comma list of language codes (`"pl, en"`) as its codes, in order, empty ones dropped.
pub fn lang_list(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(String::from)
        .collect()
}

/// The audio stream chosen without the owner: the first stream in the first preferred
/// language that has one, else the file's default, else the first. `None`: the file lists no
/// audio stream.
pub fn auto_audio(tracks: &[AudioTrack], prefs: &[String]) -> Option<u32> {
    prefs
        .iter()
        .find_map(|p| tracks.iter().find(|t| same_lang(&t.lang, p)))
        .or_else(|| tracks.iter().find(|t| t.default))
        .or_else(|| tracks.first())
        .map(|t| t.index)
}

/// The audio stream a film airs with: the owner's `choice` when the file has it (one it no
/// longer has: the automatic choice), else [`auto_audio`].
pub fn choose_audio(tracks: &[AudioTrack], prefs: &[String], choice: Option<i64>) -> Option<u32> {
    choice
        .and_then(|c| tracks.iter().find(|t| i64::from(t.index) == c))
        .map(|t| t.index)
        .or_else(|| auto_audio(tracks, prefs))
}

/// The subtitles chosen without the owner: in the first preferred language that has any,
/// a full (not forced) stream before a forced one, text before a picture; none without a
/// match.
pub fn auto_subtitle(tracks: &[SubtitleTrack], prefs: &[String]) -> Option<u32> {
    prefs.iter().find_map(|p| {
        let matching: Vec<&SubtitleTrack> =
            tracks.iter().filter(|t| same_lang(&t.lang, p)).collect();
        matching
            .iter()
            .min_by_key(|t| (t.forced, t.kind == SubtitleKind::Image))
            .map(|t| t.index)
    })
}

/// The subtitles a film airs with: off when the owner turned them off ([`SUBTITLES_OFF`]),
/// the owner's stream when the file has it (one it no longer has: the automatic choice),
/// else [`auto_subtitle`].
pub fn choose_subtitle(
    tracks: &[SubtitleTrack],
    prefs: &[String],
    choice: Option<i64>,
) -> Option<u32> {
    if choice == Some(SUBTITLES_OFF) {
        return None;
    }
    choice
        .and_then(|c| tracks.iter().find(|t| i64::from(t.index) == c))
        .map(|t| t.index)
        .or_else(|| auto_subtitle(tracks, prefs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(index: u32, lang: &str, default: bool) -> AudioTrack {
        AudioTrack {
            index,
            lang: lang.into(),
            default,
            ..AudioTrack::default()
        }
    }

    fn sub(index: u32, lang: &str, kind: SubtitleKind, forced: bool) -> SubtitleTrack {
        SubtitleTrack {
            index,
            lang: lang.into(),
            kind,
            forced,
            ..SubtitleTrack::default()
        }
    }

    fn prefs(list: &str) -> Vec<String> {
        lang_list(list)
    }

    #[test]
    fn tracks_languages_match_in_two_and_three_letters() {
        assert!(same_lang("pol", "pl"));
        assert!(same_lang("PL", "pol"));
        assert!(same_lang("ger", "deu"));
        assert!(same_lang("de", "ger"));
        assert!(same_lang("pt-BR", "por"));
        assert!(same_lang("en", "eng"));
        assert!(!same_lang("en", "pl"));
        assert!(!same_lang("und", "und"));
        assert!(!same_lang("", ""));
        assert_eq!(lang_list(" pl, en ,, de"), ["pl", "en", "de"]);
    }

    #[test]
    fn tracks_audio_follows_the_preference_order_then_the_default_then_the_first() {
        let t = [
            audio(0, "eng", false),
            audio(1, "pol", true),
            audio(2, "ger", false),
        ];
        assert_eq!(auto_audio(&t, &prefs("pl, en")), Some(1));
        assert_eq!(auto_audio(&t, &prefs("de, pl")), Some(2));
        assert_eq!(auto_audio(&t, &prefs("fr, en")), Some(0));
        // No preferred language there: the file's default.
        assert_eq!(auto_audio(&t, &prefs("fr")), Some(1));
        assert_eq!(auto_audio(&t, &[]), Some(1));
        let no_default = [audio(0, "", false), audio(1, "", false)];
        assert_eq!(auto_audio(&no_default, &prefs("pl")), Some(0));
        assert_eq!(auto_audio(&[], &prefs("pl")), None);
    }

    #[test]
    fn tracks_an_override_wins_and_a_missing_one_falls_back_to_auto() {
        let t = [audio(0, "eng", true), audio(1, "pol", false)];
        assert_eq!(choose_audio(&t, &prefs("en"), Some(1)), Some(1));
        assert_eq!(choose_audio(&t, &prefs("pl"), Some(7)), Some(1));
        assert_eq!(choose_audio(&t, &prefs("pl"), Some(-3)), Some(1));
        assert_eq!(choose_audio(&t, &prefs("pl"), None), Some(1));
        let s = [
            sub(0, "eng", SubtitleKind::Text, false),
            sub(3, "pol", SubtitleKind::Text, false),
        ];
        assert_eq!(choose_subtitle(&s, &prefs("pl"), Some(0)), Some(0));
        // Stream 1 is not in the list (the file changed): automatic.
        assert_eq!(choose_subtitle(&s, &prefs("pl"), Some(1)), Some(3));
        assert_eq!(choose_subtitle(&s, &prefs("pl"), Some(SUBTITLES_OFF)), None);
        assert_eq!(choose_subtitle(&s, &[], Some(9)), None);
    }

    #[test]
    fn tracks_subtitles_prefer_full_text_streams_and_none_without_a_match() {
        use SubtitleKind::{Image, Text};
        let s = [
            sub(0, "pol", Image, false),
            sub(1, "pol", Text, true),
            sub(2, "pol", Text, false),
            sub(3, "eng", Image, false),
        ];
        assert_eq!(auto_subtitle(&s, &prefs("pl")), Some(2));
        assert_eq!(auto_subtitle(&s, &prefs("de, en")), Some(3));
        assert_eq!(auto_subtitle(&s, &prefs("de")), None);
        // Empty preference: no subtitles by default.
        assert_eq!(auto_subtitle(&s, &[]), None);
        // A picture stream before a forced text one.
        let s = [sub(0, "pol", Text, true), sub(1, "pol", Image, false)];
        assert_eq!(auto_subtitle(&s, &prefs("pol")), Some(1));
        // Only forced ones: the forced text one.
        let s = [sub(4, "pl", Image, true), sub(5, "pl", Text, true)];
        assert_eq!(auto_subtitle(&s, &prefs("pl")), Some(5));
        // An untagged stream is never chosen by language.
        let s = [sub(0, "", Text, false)];
        assert_eq!(auto_subtitle(&s, &prefs("pl")), None);
    }
}
