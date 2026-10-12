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

/// A text for a person, kept as its key and fields until the reader's language is known. It
/// has a stored form ([`Msg::to_stored`]): the log and the reasons kept with failed films hold
/// the key and the fields, never a rendered text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    body: Body,
    args: Vec<(String, Arg)>,
    count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    /// A key of the dictionaries (`errors.<code>` for an error code).
    Key(String),
    /// A text from outside (a system error, a tool's output): shown as it is in every language.
    Text(String),
    /// Messages joined as `a: b: c` (an error and its causes).
    Chain(Vec<Msg>),
}

/// The value of a field: plain text, or a message rendered in the same language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Text(String),
    Msg(Msg),
}

impl Msg {
    fn with_body(body: Body) -> Self {
        Self {
            body,
            args: Vec::new(),
            count: None,
        }
    }

    pub fn new(key: &'static str) -> Self {
        Self::with_body(Body::Key(key.to_string()))
    }

    /// The message `errors.<code>` of an error with a stable code (the `code` of API answers).
    pub fn error(code: &'static str) -> Self {
        Self::with_body(Body::Key(format!("{ERRORS}{code}")))
    }

    /// A text from outside the program, shown as it is (no key, no fields).
    pub fn text(text: impl fmt::Display) -> Self {
        Self::with_body(Body::Text(text.to_string()))
    }

    /// `a: b: c`, each part in the reader's language.
    pub fn chain(parts: Vec<Msg>) -> Self {
        Self::with_body(Body::Chain(parts))
    }

    pub fn arg(mut self, name: &'static str, value: impl fmt::Display) -> Self {
        self.args
            .push((name.to_string(), Arg::Text(value.to_string())));
        self
    }

    pub fn arg_msg(mut self, name: &'static str, value: Msg) -> Self {
        self.args.push((name.to_string(), Arg::Msg(value)));
        self
    }

    /// The number that picks the plural form (and fills `{count}`).
    pub fn count(mut self, n: i64) -> Self {
        self.count = Some(n);
        self
    }

    /// The key of the dictionaries (empty for a text from outside or a chain).
    pub fn key(&self) -> String {
        match &self.body {
            Body::Key(key) => key.clone(),
            _ => String::new(),
        }
    }

    /// The error code of an `errors.` message.
    pub fn code(&self) -> Option<&str> {
        match &self.body {
            Body::Key(key) => key.strip_prefix(ERRORS),
            _ => None,
        }
    }

    pub fn render(&self, lang: Lang) -> String {
        match &self.body {
            Body::Text(text) => return text.clone(),
            Body::Chain(parts) => {
                let parts: Vec<String> = parts.iter().map(|p| p.render(lang)).collect();
                return parts.join(": ");
            }
            Body::Key(_) => {}
        }
        let values: Vec<(&str, String)> = self
            .args
            .iter()
            .map(|(name, arg)| {
                let value = match arg {
                    Arg::Text(text) => text.clone(),
                    Arg::Msg(msg) => msg.render(lang),
                };
                (name.as_str(), value)
            })
            .collect();
        let args: Vec<(&str, &str)> = values.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let key = self.key();
        match self.count {
            Some(n) => tn(lang, &key, n, &args),
            None => t(lang, &key, &args),
        }
    }

    // ---------- Stored form ----------

    fn to_json(&self) -> serde_json::Value {
        use serde_json::{Map, Value, json};
        match &self.body {
            Body::Text(text) => json!({ "t": text }),
            Body::Chain(parts) => {
                json!({ "c": parts.iter().map(Msg::to_json).collect::<Vec<_>>() })
            }
            Body::Key(key) => {
                let mut map = Map::new();
                map.insert("k".into(), Value::from(key.as_str()));
                if !self.args.is_empty() {
                    let args: Map<String, Value> = self
                        .args
                        .iter()
                        .map(|(name, arg)| {
                            let value = match arg {
                                Arg::Text(text) => Value::from(text.as_str()),
                                Arg::Msg(msg) => msg.to_json(),
                            };
                            (name.clone(), value)
                        })
                        .collect();
                    map.insert("a".into(), Value::Object(args));
                }
                if let Some(n) = self.count {
                    map.insert("n".into(), Value::from(n));
                }
                Value::Object(map)
            }
        }
    }

    fn from_json(value: &serde_json::Value, depth: u32) -> Option<Msg> {
        use serde_json::Value;
        if depth > 8 {
            return None;
        }
        let map = value.as_object()?;
        if let Some(text) = map.get("t") {
            return Some(Msg::text(text.as_str()?));
        }
        if let Some(parts) = map.get("c") {
            let parts = parts
                .as_array()?
                .iter()
                .map(|p| Msg::from_json(p, depth + 1))
                .collect::<Option<Vec<_>>>()?;
            return Some(Msg::chain(parts));
        }
        let key = map.get("k")?.as_str()?;
        let mut msg = Msg::with_body(Body::Key(key.to_string()));
        if let Some(args) = map.get("a") {
            for (name, value) in args.as_object()? {
                let arg = match value {
                    Value::String(text) => Arg::Text(text.clone()),
                    other => Arg::Msg(Msg::from_json(other, depth + 1)?),
                };
                msg.args.push((name.clone(), arg));
            }
        }
        if let Some(n) = map.get("n") {
            msg.count = Some(n.as_i64()?);
        }
        Some(msg)
    }

    /// The form kept in a database text column or a file: [`STORED_PREFIX`] and JSON.
    pub fn to_stored(&self) -> String {
        format!("{STORED_PREFIX}{}", self.to_json())
    }

    /// The message of a stored form; `None` for a text that is not one (older rows keep plain
    /// text, which stays readable as it is).
    pub fn from_stored(stored: &str) -> Option<Msg> {
        let json = stored.strip_prefix(STORED_PREFIX)?;
        Msg::from_json(&serde_json::from_str(json).ok()?, 0)
    }
}

/// Starts a stored message; it cannot start a text a program or a person would write.
pub const STORED_PREFIX: &str = "\u{1}msg\u{1}";

/// A stored text (see [`Msg::to_stored`]) in `lang`; a plain text is returned as it is.
pub fn stored_text(stored: &str, lang: Lang) -> String {
    match Msg::from_stored(stored) {
        Some(msg) => msg.render(lang),
        None => stored.to_string(),
    }
}

/// In English: the language of the log file (spec section 6).
impl fmt::Display for Msg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(Lang::En))
    }
}

// ---------- Errors that carry a message ----------

/// An error whose text is a message: it shows in English where only a string fits (`{e:#}`)
/// and in the reader's language through [`error_msg`].
#[derive(Debug)]
pub struct MsgError {
    msg: Msg,
    source: Option<anyhow::Error>,
}

impl MsgError {
    pub fn new(msg: Msg) -> Self {
        Self { msg, source: None }
    }
}

impl fmt::Display for MsgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.msg, f)
    }
}

impl std::error::Error for MsgError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

/// An `anyhow` error that is only this message.
pub fn msg_error(msg: Msg) -> anyhow::Error {
    anyhow::Error::new(MsgError::new(msg))
}

/// An `anyhow` error that is this message, caused by `source`.
pub fn msg_error_with(msg: Msg, source: impl Into<anyhow::Error>) -> anyhow::Error {
    anyhow::Error::new(MsgError {
        msg,
        source: Some(source.into()),
    })
}

/// `result.msg_context(msg)`: like `anyhow`'s `context`, with a message.
pub trait MsgContext<T> {
    fn msg_context(self, msg: Msg) -> anyhow::Result<T>;
}

impl<T, E> MsgContext<T> for Result<T, E>
where
    E: Into<anyhow::Error>,
{
    fn msg_context(self, msg: Msg) -> anyhow::Result<T> {
        self.map_err(|e| {
            anyhow::Error::new(MsgError {
                msg,
                source: Some(e.into()),
            })
        })
    }
}

/// The error and its causes as one message (`a: b: c`): the parts made with [`msg_error`] or
/// [`MsgContext`] in the reader's language, the others as their text.
pub fn error_msg(e: &anyhow::Error) -> Msg {
    let mut parts: Vec<Msg> = e
        .chain()
        .map(|cause| match cause.downcast_ref::<MsgError>() {
            Some(m) => m.msg.clone(),
            None => Msg::text(cause),
        })
        .collect();
    if parts.len() == 1 {
        parts.remove(0)
    } else {
        Msg::chain(parts)
    }
}

// ---------- The language of the system ----------

/// The supported language of a Windows language id (`LANGID`: the low 10 bits are the primary
/// language, e.g. `0x15` Polish); anything else, or no id, is English.
pub fn system_lang_from(source: impl FnOnce() -> Option<u16>) -> Lang {
    match source().map(|id| id & 0x3ff) {
        Some(0x15) => Lang::Pl,
        Some(0x0a) => Lang::Es,
        Some(0x07) => Lang::De,
        Some(0x0c) => Lang::Fr,
        Some(0x11) => Lang::Ja,
        _ => Lang::En,
    }
}

/// The language of the Windows interface of the current user (`GetUserDefaultUILanguage`).
pub fn system_lang() -> Lang {
    system_lang_from(user_ui_language)
}

#[cfg(windows)]
fn user_ui_language() -> Option<u16> {
    // SAFETY: no arguments, no pointers; returns a language id.
    Some(unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() })
}

#[cfg(not(windows))]
fn user_ui_language() -> Option<u16> {
    None
}

#[cfg(test)]
mod tests;
