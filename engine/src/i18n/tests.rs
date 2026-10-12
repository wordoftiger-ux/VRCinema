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
    // No Spanish dictionary yet: English.
    assert_eq!(
        t(Lang::Es, "errors.no_such_item", &[]),
        t(Lang::En, "errors.no_such_item", &[])
    );
    assert_eq!(t(Lang::Pl, "no.such.key", &[]), "no.such.key");
    assert_eq!(
        tn(Lang::En, "errors.too_many_active", 1, &[]),
        "You already have 1 video in the queue. Wait until it has played."
    );
    assert_eq!(
        tn(Lang::En, "errors.too_many_active", 3, &[]),
        "You already have 3 videos in the queue. Wait until they have played."
    );
    assert_eq!(
        tn(Lang::Pl, "errors.too_many_active", 3, &[]),
        "Masz już 3 filmów w kolejce. Poczekaj, aż się odtworzą."
    );
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

// ---------- Completeness (strict for en and pl; es, de, fr, ja from task 17) ----------

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
fn english_and_polish_have_the_same_keys_fields_and_plural_forms() {
    let en = &dicts()[index(Lang::En)];
    assert!(!en.is_empty());
    for lang in Lang::ALL {
        let dict = &dicts()[index(lang)];
        let strict = matches!(lang, Lang::En | Lang::Pl);
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
const NAMESPACES: [&str; 6] = [
    "errors",
    "item_errors",
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
