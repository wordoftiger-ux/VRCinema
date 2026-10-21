use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::*;

fn dicts_of(sources: &[(Lang, &str)]) -> Vec<Dict> {
    Lang::ALL
        .iter()
        .map(|lang| {
            sources
                .iter()
                .find(|(l, _)| l == lang)
                .map_or_else(Dict::new, |(_, json)| parse_dict(json).unwrap())
        })
        .collect()
}

// ---------- Languages ----------

#[test]
fn languages_parse_from_tags() {
    assert_eq!(Lang::parse("pl"), Some(Lang::Pl));
    assert_eq!(Lang::parse("PL"), Some(Lang::Pl));
    assert_eq!(Lang::parse("pl-PL"), Some(Lang::Pl));
    assert_eq!(Lang::parse(" de_AT "), Some(Lang::De));
    assert_eq!(Lang::parse("ja"), Some(Lang::Ja));
    for tag in ["", "*", "zh", "pol", "e"] {
        assert_eq!(Lang::parse(tag), None, "{tag:?}");
    }
    for lang in Lang::ALL {
        assert_eq!(Lang::parse(lang.as_str()), Some(lang));
    }
    assert_eq!(Lang::default(), Lang::En);
}

#[test]
fn accept_language_picks_the_most_wanted_supported_language() {
    assert_eq!(Lang::from_accept_language("de-DE,de;q=0.9"), Some(Lang::De));
    assert_eq!(Lang::from_accept_language("zh"), None);
    assert_eq!(Lang::from_accept_language("*"), None);
    assert_eq!(Lang::from_accept_language(""), None);
    assert_eq!(
        Lang::from_accept_language("zh-CN,zh;q=0.9,pl;q=0.8,en;q=0.7"),
        Some(Lang::Pl)
    );
    // The highest q wins wherever it is; on a tie the earlier one.
    assert_eq!(Lang::from_accept_language("en;q=0.5, fr"), Some(Lang::Fr));
    assert_eq!(
        Lang::from_accept_language("fr;q=0.8, es;q=0.8"),
        Some(Lang::Fr)
    );
    // q=0 means "not this one"; a broken q drops the entry.
    assert_eq!(Lang::from_accept_language("pl;q=0"), None);
    assert_eq!(
        Lang::from_accept_language("pl;q=x, ja;q=0.1"),
        Some(Lang::Ja)
    );
}

// ---------- Lookups ----------

#[test]
fn texts_fill_their_fields_and_fall_back_to_english() {
    let dicts = dicts_of(&[
        (
            Lang::En,
            r#"{"a.hello": "Hello, {name}!", "a.only_en": "English only"}"#,
        ),
        (Lang::Pl, r#"{"a.hello": "Cześć, {name}!"}"#),
    ]);
    let get = |lang, key| lookup(&dicts, lang, key, None);
    assert_eq!(get(Lang::Pl, "a.hello"), Some("Cześć, {name}!"));
    assert_eq!(get(Lang::Pl, "a.only_en"), Some("English only"));
    assert_eq!(get(Lang::De, "a.hello"), Some("Hello, {name}!"));
    assert_eq!(get(Lang::Pl, "a.missing"), None);
    assert_eq!(fill("Cześć, {name}!", &[("name", "Ala")]), "Cześć, Ala!");
}

#[test]
fn fields_are_filled_once_and_unknown_ones_stay() {
    assert_eq!(fill("{a} {b}", &[("a", "{b}"), ("b", "x")]), "{b} x");
    assert_eq!(fill("{a} {zzz} {", &[("a", "1")]), "1 {zzz} {");
    assert_eq!(fill("{a}{a}", &[("a", "ż")]), "żż");
    assert_eq!(fill("no fields", &[("a", "1")]), "no fields");
}

#[test]
fn plural_forms_follow_the_language() {
    let dicts = dicts_of(&[
        (
            Lang::En,
            r#"{"a.files": {"one": "{count} file", "other": "{count} files"}}"#,
        ),
        (
            Lang::Pl,
            r#"{"a.files": {"one": "{count} plik", "few": "{count} pliki", "many": "{count} plików", "other": "{count} pliku"}}"#,
        ),
    ]);
    let get = |lang, n| lookup(&dicts, lang, "a.files", Some(n));
    assert_eq!(get(Lang::Pl, 1), Some("{count} plik"));
    assert_eq!(get(Lang::Pl, 22), Some("{count} pliki"));
    assert_eq!(get(Lang::Pl, 25), Some("{count} plików"));
    assert_eq!(get(Lang::En, 1), Some("{count} file"));
    assert_eq!(get(Lang::En, 2), Some("{count} files"));
    // A language without the key: the English form for the English category.
    assert_eq!(get(Lang::Fr, 0), Some("{count} files"));
    // A plural entry read without a number: its `other` form.
    assert_eq!(
        lookup(&dicts, Lang::Pl, "a.files", None),
        Some("{count} pliku")
    );
}

#[test]
fn t_and_tn_use_the_built_in_dictionaries() {
    assert_eq!(
        t(
            Lang::Pl,
            "errors.domain_not_allowed",
            &[("domains", "youtube.com")]
        ),
        "Ten serwis nie jest dozwolony. Dozwolone: youtube.com."
    );
    assert_eq!(
        t(
            Lang::En,
            "errors.domain_not_allowed",
            &[("domains", "youtube.com")]
        ),
        "This site is not allowed. Allowed: youtube.com."
    );
    // The built-in Spanish dictionary (falling back to English is covered above).
    assert_eq!(
        t(Lang::Es, "errors.no_such_item", &[]),
        "No existe ese elemento."
    );
    assert_eq!(
        tn(Lang::Es, "errors.too_many_active", 2, &[]),
        "Ya tienes 2 videos en la cola. Espera a que se reproduzcan."
    );
    // French: 0 and 1 take the `one` form.
    assert_eq!(
        t(Lang::Fr, "errors.no_such_item", &[]),
        "Cet élément n’existe pas."
    );
    assert_eq!(
        tn(Lang::Fr, "errors.too_many_active", 1, &[]),
        "Tu as déjà 1 vidéo dans la file d’attente. Attends qu’elle soit lue."
    );
    assert_eq!(
        tn(Lang::Fr, "errors.too_many_active", 2, &[]),
        "Tu as déjà 2 vidéos dans la file d’attente. Attends qu’elles soient lues."
    );
    // Japanese: one plural form (`other`) for every number; the host's word matches nicks.rs.
    assert_eq!(
        t(Lang::Ja, "errors.no_such_item", &[]),
        "その項目は存在しません。"
    );
    for n in [1, 2] {
        assert_eq!(
            tn(Lang::Ja, "errors.too_many_active", n, &[]),
            format!("すでに{n}本の動画をキューに追加しています。再生が終わるまでお待ちください。")
        );
    }
    assert_eq!(t(Lang::Ja, "ticker.added_by_host", &[]), "追加：ホスト");
    // The Japanese ticker is drawn with Yu Gothic, which has ▶ but not the ► of Arial.
    assert_eq!(t(Lang::Ja, "ticker.now", &[("title", "x")]), "▶ 再生中：x");
    assert_eq!(t(Lang::Pl, "no.such.key", &[]), "no.such.key");
    assert_eq!(
        tn(Lang::En, "errors.too_many_active", 1, &[]),
        "You already have 1 video in the queue. Wait until it has played."
    );
    assert_eq!(
        tn(Lang::En, "errors.too_many_active", 3, &[]),
        "You already have 3 videos in the queue. Wait until they have played."
    );
    for (n, text) in [
        (1, "Masz już 1 film w kolejce. Poczekaj, aż się odtworzy."),
        (2, "Masz już 2 filmy w kolejce. Poczekaj, aż się odtworzą."),
        (3, "Masz już 3 filmy w kolejce. Poczekaj, aż się odtworzą."),
        (5, "Masz już 5 filmów w kolejce. Poczekaj, aż się odtworzą."),
        (
            12,
            "Masz już 12 filmów w kolejce. Poczekaj, aż się odtworzą.",
        ),
        (
            22,
            "Masz już 22 filmy w kolejce. Poczekaj, aż się odtworzą.",
        ),
    ] {
        assert_eq!(tn(Lang::Pl, "errors.too_many_active", n, &[]), text);
    }
}

#[test]
fn messages_render_nested_fields_in_the_readers_language() {
    let msg = Msg::error("fetch_failed").arg_msg(
        "reason",
        Msg::error("source_message").arg("detail", "Video unavailable"),
    );
    assert_eq!(msg.code(), Some("fetch_failed"));
    assert_eq!(msg.key(), "errors.fetch_failed");
    assert_eq!(
        msg.render(Lang::Pl),
        "Nie udało się odczytać filmu: Video unavailable"
    );
    assert_eq!(
        msg.to_string(),
        "Could not read the video: Video unavailable"
    );
    assert_eq!(Msg::new("slates.buffering_title").code(), None);
    assert_eq!(
        Msg::error("too_many_active").count(2).render(Lang::En),
        "You already have 2 videos in the queue. Wait until they have played."
    );
}

#[test]
fn a_message_survives_its_stored_form_with_nested_fields_and_a_count() {
    let msg = Msg::error("fetch_failed")
        .arg_msg(
            "reason",
            Msg::error("source_message").arg("detail", "a \"b\" ł"),
        )
        .count(3);
    let stored = msg.to_stored();
    assert!(stored.starts_with(STORED_PREFIX), "{stored}");
    let back = Msg::from_stored(&stored).unwrap();
    assert_eq!(back, msg);
    assert_eq!(back.render(Lang::Pl), msg.render(Lang::Pl));
    // Text that was stored before messages had a stored form stays readable as it is.
    assert_eq!(Msg::from_stored("Pominięty przez właściciela"), None);
    assert_eq!(
        stored_text("Pominięty przez właściciela", Lang::En),
        "Pominięty przez właściciela"
    );
    assert_eq!(
        stored_text(&Msg::error("busy").to_stored(), Lang::Pl),
        Msg::error("busy").render(Lang::Pl)
    );
    // Broken stored forms are shown as they are, never a panic.
    let broken = format!("{STORED_PREFIX}{{oops");
    assert_eq!(stored_text(&broken, Lang::En), broken);
}

#[test]
fn plain_text_and_chains_render_without_a_key() {
    let msg = Msg::chain(vec![Msg::error("busy"), Msg::text("os error 5")]);
    assert_eq!(
        msg.render(Lang::En),
        format!("{}: os error 5", Msg::error("busy").render(Lang::En))
    );
    assert_eq!(Msg::from_stored(&msg.to_stored()).unwrap(), msg);
    assert_eq!(Msg::text("x {y}").arg("y", "1").render(Lang::Pl), "x {y}");
}

#[test]
fn an_error_chain_becomes_a_message_in_the_readers_language() {
    let result: anyhow::Result<()> = Err(anyhow::anyhow!("os error 5"));
    let e = result
        .msg_context(Msg::error("busy"))
        .msg_context(Msg::error("cancelled"))
        .unwrap_err();
    let pl = |k| Msg::error(k).render(Lang::Pl);
    assert_eq!(
        error_msg(&e).render(Lang::Pl),
        format!("{}: {}: os error 5", pl("cancelled"), pl("busy"))
    );
    // `{e:#}` (the English text) is what tests and the log file see.
    assert_eq!(
        format!("{e:#}"),
        format!(
            "{}: {}: os error 5",
            Msg::error("cancelled"),
            Msg::error("busy")
        )
    );
    // A plain anyhow error is only its text.
    let plain = anyhow::anyhow!("x").context("y");
    assert_eq!(error_msg(&plain).render(Lang::Pl), "y: x");
}

#[test]
fn the_default_language_follows_the_windows_ui_language() {
    // Language ids: pl-PL 0x0415, de-AT 0x0C07, es-MX 0x080A, fr-CA 0x0C0C, ja-JP 0x0411,
    // en-GB 0x0809, ko-KR 0x0412.
    let from = |id: u16| system_lang_from(|| Some(id));
    assert_eq!(from(0x0415), Lang::Pl);
    assert_eq!(from(0x0C07), Lang::De);
    assert_eq!(from(0x080A), Lang::Es);
    assert_eq!(from(0x0C0C), Lang::Fr);
    assert_eq!(from(0x0411), Lang::Ja);
    assert_eq!(from(0x0809), Lang::En);
    assert_eq!(from(0x0412), Lang::En);
    assert_eq!(system_lang_from(|| None), Lang::En);
}

#[cfg(windows)]
#[test]
fn the_real_windows_ui_language_is_a_supported_language_or_english() {
    // Calls GetUserDefaultUILanguage for real: whatever it says maps to one of the six.
    let id = user_ui_language().expect("Windows knows its UI language");
    assert_ne!(id, 0);
    assert!(Lang::ALL.contains(&system_lang()));
    assert_eq!(system_lang(), system_lang_from(|| Some(id)));
}

// ---------- Completeness (strict for the languages in STRICT; the others may miss keys) ----------

/// Languages whose dictionary must be complete: every key of en.json, and every text within the
/// length limits of context.json. Task 17 adds each language here once it is fully translated.
const STRICT: [Lang; 6] = [Lang::En, Lang::Pl, Lang::De, Lang::Es, Lang::Fr, Lang::Ja];

/// The `{…}` fields of a text.
fn fields(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                out.insert(after[..close].to_string());
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

fn entry_fields(entry: &Entry) -> BTreeSet<String> {
    match entry {
        Entry::Text(text) => fields(text),
        Entry::Plural(forms) => forms.values().flat_map(|f| fields(f)).collect(),
    }
}

fn locales_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../public/locales")
}

#[test]
fn the_built_in_dictionaries_are_the_files_and_parse() {
    for lang in Lang::ALL {
        let file = locales_dir().join(format!("{}.json", lang.as_str()));
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(source(lang), text, "{}", file.display());
        parse_dict(source(lang)).unwrap_or_else(|e| panic!("{}: {e}", lang.as_str()));
    }
    assert!(parse_dict(r#"{"a": 1}"#).is_err());
    assert!(parse_dict(r#"{"a": {"several": "x"}}"#).is_err());
    assert!(parse_dict(r#"{"a": {"one": "x"}}"#).is_err(), "no `other`");
    assert!(parse_dict("[]").is_err());
}

#[test]
fn the_strict_languages_have_every_key_and_all_have_the_same_fields_and_plural_forms() {
    let en = &dicts()[index(Lang::En)];
    assert!(!en.is_empty());
    for lang in Lang::ALL {
        let dict = &dicts()[index(lang)];
        let strict = STRICT.contains(&lang);
        let keys: BTreeSet<&String> = dict.keys().collect();
        let en_keys: BTreeSet<&String> = en.keys().collect();
        let unknown: Vec<_> = keys.difference(&en_keys).collect();
        assert!(
            unknown.is_empty(),
            "{}: keys not in en: {unknown:?}",
            lang.as_str()
        );
        if strict {
            let missing: Vec<_> = en_keys.difference(&keys).collect();
            assert!(
                missing.is_empty(),
                "{}: missing keys: {missing:?}",
                lang.as_str()
            );
        }
        for (key, entry) in dict {
            let pattern = &en[key];
            assert_eq!(
                entry_fields(entry),
                entry_fields(pattern),
                "{}: fields of {key}",
                lang.as_str()
            );
            assert_eq!(
                matches!(entry, Entry::Plural(_)),
                matches!(pattern, Entry::Plural(_)),
                "{}: {key} is plural in one language only",
                lang.as_str()
            );
            if let Entry::Plural(forms) = entry {
                for category in required_categories(lang) {
                    assert!(
                        forms.contains_key(*category),
                        "{}: {key} has no `{category}` form",
                        lang.as_str()
                    );
                }
            }
        }
    }
}

/// The length limit a context note gives: "Max ~12 characters", "max ~30 characters",
/// "Max about 35 characters" (not "maximum 24 characters", which is the limit of a field).
fn length_limit(note: &str) -> Option<usize> {
    let lower = note.to_lowercase();
    for lead in ["max ~", "max about "] {
        if let Some(at) = lower.find(lead) {
            let rest = &lower[at + lead.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if rest[digits.len()..].starts_with(" characters") {
                return digits.parse().ok();
            }
        }
    }
    None
}

#[test]
fn the_length_limit_is_read_from_the_note() {
    assert_eq!(
        length_limit("Left rail. Max ~12 characters (72 px)."),
        Some(12)
    );
    assert_eq!(
        length_limit("Impersonal, short (max ~30 characters)."),
        Some(30)
    );
    assert_eq!(
        length_limit("Menu entry. Max about 35 characters."),
        Some(35)
    );
    assert_eq!(
        length_limit("Label of the nick field (maximum 24 characters)."),
        None
    );
    assert_eq!(length_limit("Keep it short."), None);
}

#[test]
fn the_strict_languages_fit_the_length_limits_of_the_context_notes() {
    let text = std::fs::read_to_string(locales_dir().join("context.json")).unwrap();
    let context: BTreeMap<String, serde_json::Value> = serde_json::from_str(&text).unwrap();
    let limits: Vec<(&String, usize)> = context
        .iter()
        .filter_map(|(key, note)| Some((key, length_limit(note.as_str()?)?)))
        .collect();
    assert!(limits.len() > 20, "only {} limits found", limits.len());
    for lang in STRICT {
        let dict = &dicts()[index(lang)];
        for (key, limit) in &limits {
            let entry = dict
                .get(*key)
                .unwrap_or_else(|| panic!("{}: no key {key}", lang.as_str()));
            let texts: Vec<&String> = match entry {
                Entry::Text(text) => vec![text],
                Entry::Plural(forms) => forms.values().collect(),
            };
            for text in texts {
                // A soft hyphen only allows a break; it is not shown.
                let length = text.chars().filter(|&c| c != '\u{ad}').count();
                assert!(
                    length <= *limit,
                    "{}: {key} has {length} characters, the limit is {limit}: {text:?}",
                    lang.as_str()
                );
            }
        }
    }
}

#[test]
fn every_english_key_has_a_context_note() {
    let text = std::fs::read_to_string(locales_dir().join("context.json")).unwrap();
    let context: BTreeMap<String, serde_json::Value> = serde_json::from_str(&text).unwrap();
    let en = &dicts()[index(Lang::En)];
    for key in en.keys() {
        let note = context.get(key).and_then(|v| v.as_str()).unwrap_or("");
        assert!(!note.trim().is_empty(), "context.json: no note for {key}");
    }
    for key in context.keys() {
        assert!(
            en.contains_key(key),
            "context.json: {key} is not a key of en.json"
        );
    }
}

/// Final wave I4: the VRChat caveat about Public and Group Public instances is on the request
/// page in every language (not English text in a fallback), with what to try instead.
#[test]
fn the_vrchat_instance_caveat_is_in_every_language() {
    let en = t(Lang::En, "viewer.howto_untrusted", &[]);
    for lang in Lang::ALL {
        let text = t(lang, "viewer.howto_untrusted", &[]);
        for part in ["Allow Untrusted URLs", "Group Public", "Friends", "Invite"] {
            assert!(
                text.contains(part),
                "{}: no {part} in {text}",
                lang.as_str()
            );
        }
        assert!(
            lang == Lang::En || text != en,
            "{}: the English text",
            lang.as_str()
        );
    }
}

// ---------- Keys used by the code ----------

/// The engine's own sources (not tests, not this module).
fn rust_sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && name != "i18n" {
                rust_sources(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" {
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n");
            let production = match text.find("#[cfg(test)]\nmod tests") {
                Some(end) => text[..end].to_string(),
                None => text,
            };
            out.push((path, production));
        }
    }
}

/// Namespaces of the dictionaries' keys: a literal `"<namespace>.…"` in the code is a key.
const NAMESPACES: [&str; 7] = [
    "errors",
    "item_errors",
    "log",
    "named",
    "slates",
    "tunnel",
    "units",
];

/// Keys named by literals: in calls (`Msg::error("code")`, `ApiError::coded(status, "code", …)`,
/// `bad_request("code")`, `with_code("code")` and the error helpers `err("code")`,
/// `failed("code", …)`, `failure("code", …)` — all `errors.<code>` —, `Msg::new("key")`,
/// `i18n::t(lang, "key", …)`) and any literal of a key namespace (constants).
fn keys_in(text: &str) -> Vec<String> {
    let calls = [
        ("Msg::error(", "errors."),
        ("coded(", "errors."),
        ("bad_request(", "errors."),
        ("with_code(", "errors."),
        ("Msg::new(", ""),
        ("i18n::t(", ""),
        ("i18n::tn(", ""),
    ];
    let mut keys = Vec::new();
    // The helpers: only with the code right after the parenthesis.
    for (call, prefix) in [
        ("err(\"", "errors."),
        ("failed(\"", "errors."),
        ("failure(\"", "errors."),
    ] {
        for (at, _) in text.match_indices(call) {
            let literal = &text[at + call.len()..];
            if let Some(end) = literal.find('"') {
                keys.push(format!("{prefix}{}", &literal[..end]));
            }
        }
    }
    for namespace in NAMESPACES {
        let start = format!("\"{namespace}.");
        for (at, _) in text.match_indices(&start) {
            let literal = &text[at + 1..];
            let Some(end) = literal.find('"') else {
                continue;
            };
            let key = &literal[..end];
            let is_key = key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.');
            if is_key && !key.ends_with('.') {
                keys.push(key.to_string());
            }
        }
    }
    for (call, prefix) in calls {
        for (at, _) in text.match_indices(call) {
            let rest = &text[at + call.len()..];
            let Some(quote) = rest.find('"') else {
                continue;
            };
            // Only a literal among the first arguments (not one inside a later expression).
            if rest[..quote].contains(['(', ')', ';', '{', '}']) {
                continue;
            }
            let literal = &rest[quote + 1..];
            if let Some(end) = literal.find('"') {
                keys.push(format!("{prefix}{}", &literal[..end]));
            }
        }
    }
    keys
}

#[test]
fn the_key_scanner_finds_literal_keys_only() {
    let code = r#"
        ApiError::coded(
            StatusCode::CONFLICT,
            "live_item",
            &[],
        );
        ApiError::bad_request("invalid_link");
        control_bad_request("seek_target").with_code("bad_request");
        Msg::error("source_message").arg("detail", text);
        Msg::new("slates.idle_title");
        i18n::t(lang, "named.done", &[]);
        ApiError::coded(status, code, &[("x", "y")]);
        return Err(err("no_such_root"));
        const STALLED: &str = "item_errors.stalled";
        i18n::t(self.lang(), STALLED, &[]);
        format!("errors.{code}");
    "#;
    let found: BTreeSet<String> = keys_in(code).into_iter().collect();
    assert_eq!(
        found,
        [
            "errors.bad_request",
            "errors.invalid_link",
            "errors.live_item",
            "errors.no_such_root",
            "errors.seek_target",
            "errors.source_message",
            "item_errors.stalled",
            "named.done",
            "slates.idle_title",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    );
}

#[test]
fn every_key_and_error_code_the_engine_uses_is_in_english_and_polish() {
    let mut sources = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    let mut used = BTreeSet::new();
    for (path, text) in &sources {
        for key in keys_in(text) {
            used.insert((key, path.display().to_string()));
        }
    }
    assert!(
        used.len() > 50,
        "the scanner found only {} keys",
        used.len()
    );
    for lang in [Lang::En, Lang::Pl] {
        let dict = &dicts()[index(lang)];
        for (key, path) in &used {
            assert!(
                dict.contains_key(key),
                "{}.json: no key {key} (used in {path})",
                lang.as_str()
            );
        }
    }
}

#[test]
fn every_code_of_the_permanent_link_wizard_has_a_message() {
    use crate::tunnel::named::ErrorCode;
    for code in ErrorCode::ALL {
        let key = format!("errors.{}", code.as_str());
        assert!(dicts()[index(Lang::En)].contains_key(&key), "{key}");
    }
}

// ---------- Keys used by the new interface (public/app) ----------

/// Namespaces of the interface's own keys: a literal `"<namespace>.…"` in its code is a key.
const UI_NAMESPACES: [&str; 18] = [
    "banner",
    "drop",
    "history",
    "library",
    "login",
    "monitor",
    "nav",
    "net",
    "permalink",
    "player",
    "queue",
    "settings",
    "share",
    "shortcuts",
    "titlebar",
    "ui",
    "viewer",
    "wizard",
];

/// The string literals of a JavaScript source (quotes, apostrophes and backticks), comments
/// skipped.
fn js_literals(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if matches!(c, '\'' | '"' | '`') {
            let mut literal = String::new();
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' {
                    i += 1;
                }
                if let Some(&ch) = chars.get(i) {
                    literal.push(ch);
                }
                i += 1;
            }
            out.push(literal);
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

fn is_key_text(s: &str) -> bool {
    s.contains('.')
        && !s.starts_with('.')
        && !s.ends_with('.')
        && !s.contains("..")
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

/// Keys named in the interface's code: literals of a UI namespace, and the first argument of
/// `t('…'` / `tn('…'` whatever its namespace (`errors.…` too).
fn ui_keys_in(text: &str) -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = js_literals(text)
        .into_iter()
        .filter(|s| is_key_text(s))
        .filter(|s| {
            UI_NAMESPACES
                .iter()
                .any(|ns| s.starts_with(&format!("{ns}.")))
        })
        .collect();
    for call in ["t(", "tn("] {
        for (at, _) in text.match_indices(call) {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                continue; // `split(`, `obj.t(`
            }
            let rest = &text[at + call.len()..];
            let Some(quote) = rest.chars().next().filter(|q| matches!(q, '\'' | '"')) else {
                continue;
            };
            if let Some(end) = rest[1..].find(quote) {
                let key = &rest[1..1 + end];
                if is_key_text(key) {
                    keys.insert(key.to_string());
                }
            }
        }
    }
    keys
}

fn js_sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            js_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "js") {
            out.push((path.clone(), std::fs::read_to_string(&path).unwrap()));
        }
    }
}

#[test]
fn the_interface_key_scanner_finds_keys_in_code_but_not_in_comments() {
    let code = r#"
        // 'player.in_a_comment' is not used
        /* "nav.neither" */
        const items = [{ label: 'player.restart' }, { label: "nav.history" }];
        html`<b>${t('errors.live_item')}</b> ${tn("ui.videos", 3)}`;
        const s = `titlebar.close`;
        const notKey = 'player.';
        const other = 'tunnel.online';
        path.split('.');
        const msg = "it's fine";
        const url = 'app.html';
    "#;
    assert_eq!(
        ui_keys_in(code),
        [
            "errors.live_item",
            "nav.history",
            "player.restart",
            "titlebar.close",
            "ui.videos",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    );
}

#[test]
fn every_key_the_new_interface_uses_is_in_english_and_polish() {
    let mut sources = Vec::new();
    js_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../public/app"),
        &mut sources,
    );
    let mut used = BTreeSet::new();
    for (path, text) in &sources {
        for key in ui_keys_in(text) {
            used.insert((key, path.display().to_string()));
        }
    }
    assert!(
        used.len() > 20,
        "the scanner found only {} keys",
        used.len()
    );
    for lang in [Lang::En, Lang::Pl] {
        let dict = &dicts()[index(lang)];
        for (key, path) in &used {
            assert!(
                dict.contains_key(key),
                "{}.json: no key {key} (used in {path})",
                lang.as_str()
            );
        }
    }
}
