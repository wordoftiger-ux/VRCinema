//! Translations (spec section 6). The dictionaries are `public/locales/<lang>.json`, the files
//! the interface loads, built into the binary. Keys are dotted (`errors.no_such_item`); a value
//! is a text with fields `{name}` or, for a number, an object of CLDR plural categories
//! (`one`, `few`, `many`, `other`). A key missing in a language falls back to English, one
//! missing in English to the key itself. `context.json` describes every key for translators.

mod plural;

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

pub use plural::{plural_category, required_categories};

/// A language of the program; English is the default and the pattern of the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    #[default]
    En,
    Pl,
    Es,
    De,
    Fr,
    Ja,
}

impl Lang {
    pub const ALL: [Lang; 6] = [Lang::En, Lang::Pl, Lang::Es, Lang::De, Lang::Fr, Lang::Ja];

    pub fn as_str(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Pl => "pl",
            Lang::Es => "es",
            Lang::De => "de",
            Lang::Fr => "fr",
            Lang::Ja => "ja",
        }
    }

    /// `pl`, `PL`, `pl-PL` or `pl_PL`: the language of the tag's first part, if supported.
    pub fn parse(tag: &str) -> Option<Lang> {
        let primary = tag.trim().split(['-', '_']).next()?.to_ascii_lowercase();
        Self::ALL.into_iter().find(|l| l.as_str() == primary)
    }

    /// The supported language an `Accept-Language` header prefers most (highest `q`, then the
    /// header's order); `None` when it names none (`*` names none).
    pub fn from_accept_language(header: &str) -> Option<Lang> {
        let mut best: Option<(f32, Lang)> = None;
        for item in header.split(',') {
            let mut parts = item.split(';');
            let tag = parts.next().unwrap_or("");
            let q = match parts.find_map(|p| {
                let p = p.trim();
                p.strip_prefix("q=").or_else(|| p.strip_prefix("Q="))
            }) {
                None => Some(1.0),
                Some(value) => value.trim().parse::<f32>().ok(),
            };
            let (Some(q), Some(lang)) = (q, Lang::parse(tag)) else {
                continue;
            };
            if q > 0.0 && best.is_none_or(|(top, _)| q > top) {
                best = Some((q, lang));
            }
        }
        best.map(|(_, lang)| lang)
    }
}

/// A dictionary value.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Text(String),
    /// Forms by CLDR plural category.
    Plural(HashMap<String, String>),
}

type Dict = HashMap<String, Entry>;

const CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

fn source(lang: Lang) -> &'static str {
    match lang {
        Lang::En => include_str!("../../../public/locales/en.json"),
        Lang::Pl => include_str!("../../../public/locales/pl.json"),
        Lang::Es => include_str!("../../../public/locales/es.json"),
        Lang::De => include_str!("../../../public/locales/de.json"),
        Lang::Fr => include_str!("../../../public/locales/fr.json"),
        Lang::Ja => include_str!("../../../public/locales/ja.json"),
    }
}

/// A dictionary file: an object of keys whose values are texts or objects of plural forms.
fn parse_dict(json: &str) -> Result<Dict, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let serde_json::Value::Object(map) = value else {
        return Err("not an object".to_string());
    };
    let mut dict = Dict::new();
    for (key, value) in map {
        let entry = match value {
            serde_json::Value::String(text) => Entry::Text(text),
            serde_json::Value::Object(forms) => {
                let mut out = HashMap::new();
                for (category, form) in forms {
                    let (true, serde_json::Value::String(form)) =
                        (CATEGORIES.contains(&category.as_str()), form)
                    else {
                        return Err(format!("{key}: `{category}` is not a plural form"));
                    };
                    out.insert(category, form);
                }
                if !out.contains_key("other") {
                    return Err(format!("{key}: no `other` form"));
                }
                Entry::Plural(out)
            }
            _ => return Err(format!("{key}: neither a text nor plural forms")),
        };
        dict.insert(key, entry);
    }
    Ok(dict)
}

/// The built-in dictionaries, in the order of [`Lang::ALL`].
fn dicts() -> &'static [Dict] {
    static DICTS: OnceLock<Vec<Dict>> = OnceLock::new();
    DICTS.get_or_init(|| {
        Lang::ALL
            .iter()
            .map(|&l| parse_dict(source(l)).unwrap_or_default())
            .collect()
    })
}

fn index(lang: Lang) -> usize {
    Lang::ALL.iter().position(|&l| l == lang).unwrap_or(0)
}

/// The text of `key` in `lang` from `dicts` (`count` picks the plural form), falling back to
/// English.
fn lookup<'a>(dicts: &'a [Dict], lang: Lang, key: &str, count: Option<i64>) -> Option<&'a str> {
    let in_lang = |lang: Lang| -> Option<&'a str> {
        match dicts.get(index(lang))?.get(key)? {
            Entry::Text(text) => Some(text),
            Entry::Plural(forms) => {
                let category = count.map_or("other", |n| plural_category(lang, n));
                forms
                    .get(category)
                    .or_else(|| forms.get("other"))
                    .map(String::as_str)
            }
        }
    };
    in_lang(lang).or_else(|| in_lang(Lang::En))
}

fn template(lang: Lang, key: &str, count: Option<i64>) -> Option<&'static str> {
    lookup(dicts(), lang, key, count)
}

/// `template` with each `{name}` that has a value in `args` replaced (once, left to right: a
/// value is never searched for fields itself); unknown fields stay as they are.
fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let value = after.find('}').and_then(|close| {
            let name = &after[..close];
            args.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (*v, close))
        });
        match value {
            Some((value, close)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text of `key` in `lang` with its fields filled from `args`.
pub fn t(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    fill(template(lang, key, None).unwrap_or(key), args)
}

/// Like [`t`] for a number: the plural form for `count`, which also fills `{count}`.
pub fn tn(lang: Lang, key: &str, count: i64, args: &[(&str, &str)]) -> String {
    let n = count.to_string();
    let mut all = Vec::with_capacity(args.len() + 1);
    all.extend_from_slice(args);
    all.push(("count", n.as_str()));
    fill(template(lang, key, Some(count)).unwrap_or(key), &all)
}

const ERRORS: &str = "errors.";

/// A text for a person, kept as its key and fields until the reader's language is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    /// The key is `prefix` + `name`; `errors.` makes `name` an error code.
    prefix: &'static str,
    name: &'static str,
    args: Vec<(&'static str, Arg)>,
    count: Option<i64>,
}

/// The value of a field: plain text, or a message rendered in the same language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Text(String),
    Msg(Msg),
}

impl Msg {
    pub fn new(key: &'static str) -> Self {
        Self {
            prefix: "",
            name: key,
            args: Vec::new(),
            count: None,
        }
    }

    /// The message `errors.<code>` of an error with a stable code (the `code` of API answers).
    pub fn error(code: &'static str) -> Self {
        Self {
            prefix: ERRORS,
            ..Self::new(code)
        }
    }

    pub fn arg(mut self, name: &'static str, value: impl fmt::Display) -> Self {
        self.args.push((name, Arg::Text(value.to_string())));
        self
    }

    pub fn arg_msg(mut self, name: &'static str, value: Msg) -> Self {
        self.args.push((name, Arg::Msg(value)));
        self
    }

    /// The number that picks the plural form (and fills `{count}`).
    pub fn count(mut self, n: i64) -> Self {
        self.count = Some(n);
        self
    }

    pub fn key(&self) -> String {
        format!("{}{}", self.prefix, self.name)
    }

    /// The error code of an `errors.` message.
    pub fn code(&self) -> Option<&'static str> {
        (self.prefix == ERRORS).then_some(self.name)
    }

    pub fn render(&self, lang: Lang) -> String {
        let values: Vec<(&str, String)> = self
            .args
            .iter()
            .map(|(name, arg)| {
                let value = match arg {
                    Arg::Text(text) => text.clone(),
                    Arg::Msg(msg) => msg.render(lang),
                };
                (*name, value)
            })
            .collect();
        let args: Vec<(&str, &str)> = values.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let key = self.key();
        match self.count {
            Some(n) => tn(lang, &key, n, &args),
            None => t(lang, &key, &args),
        }
    }
}

/// In English: the language of the log file (spec section 6).
impl fmt::Display for Msg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(Lang::En))
    }
}

#[cfg(test)]
mod tests;
