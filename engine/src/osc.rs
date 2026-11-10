//! The film on air in the owner's VRChat chatbox over OSC (Settings → Channel → VRChat).
//!
//! An observer follows the engine's snapshots (the film viewers see) and sends one UDP datagram
//! `/chatbox/input ,sTF <text>` when the film changes and, optionally, every N seconds — never
//! closer than [`MIN_GAP`]. The text comes from the owner's template; titles and nicks come from
//! viewers and are untrusted: control characters go, the length is capped at
//! [`CHATBOX_MAX_CHARS`]. OSC reaches VRChat only on the same PC (or the PC `oscHost` names)
//! with OSC enabled in VRChat's Action Menu.

use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::db::{AddedBy, SettingsRepo};
use crate::engine::Snapshot;
use crate::i18n::{self, Lang, Msg};

pub const DEFAULT_HOST: &str = "127.0.0.1";
/// VRChat's OSC input port.
pub const DEFAULT_PORT: u16 = 9000;
pub const DEFAULT_TEMPLATE: &str = "▶ {title} — {added}";
/// Longest template the owner can store.
pub const MAX_TEMPLATE_CHARS: usize = 200;
/// The chatbox shows at most this many characters.
pub const CHATBOX_MAX_CHARS: usize = 144;
/// VRChat takes about one chatbox message per 1.5 s.
pub const MIN_GAP: Duration = Duration::from_millis(1500);
/// `oscRepeatS`: 0 (only on a change) or within this range.
pub const REPEAT_MIN_S: u32 = 5;
pub const REPEAT_MAX_S: u32 = 600;
/// The OSC address of the chatbox's text.
pub const CHATBOX_ADDRESS: &str = "/chatbox/input";
/// How often the settings are read again.
const SETTINGS_EVERY: Duration = Duration::from_secs(2);
/// How often the observer looks again without a new snapshot (a waiting send, the repeat).
const TICK: Duration = Duration::from_millis(250);

// ---------- Settings ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OscSettings {
    pub osc_enabled: bool,
    pub osc_host: String,
    pub osc_port: u16,
    pub osc_template: String,
    /// 0: only when the film (or the text) changes.
    pub osc_repeat_s: u32,
}

impl Default for OscSettings {
    fn default() -> Self {
        Self {
            osc_enabled: false,
            osc_host: DEFAULT_HOST.to_string(),
            osc_port: DEFAULT_PORT,
            osc_template: DEFAULT_TEMPLATE.to_string(),
            osc_repeat_s: 0,
        }
    }
}

/// A validated partial update of [`OscSettings`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OscPatch {
    pub enabled: Option<bool>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub template: Option<String>,
    pub repeat_s: Option<u32>,
}

/// An IP address (IPv6 with or without brackets) or a host name: labels of letters, digits and
/// `-`, at most 253 characters.
pub fn valid_host(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if bare.parse::<IpAddr>().is_ok() {
        return true;
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// Validates one settings key; `None`: not an OSC key.
pub fn sanitize_key(key: &str, value: &Value, out: &mut OscPatch) -> Option<Result<(), Msg>> {
    let result = match key {
        "oscEnabled" => value
            .as_bool()
            .map(|b| out.enabled = Some(b))
            .ok_or_else(|| Msg::error("setting_not_boolean").arg("key", key)),
        "oscHost" => value
            .as_str()
            .map(str::trim)
            .filter(|h| valid_host(h))
            .map(|h| out.host = Some(h.to_string()))
            .ok_or_else(|| Msg::error("osc_host_invalid").arg("key", key)),
        "oscPort" => value
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p > 0)
            .map(|p| out.port = Some(p))
            .ok_or_else(|| {
                Msg::error("setting_out_of_range")
                    .arg("key", key)
                    .arg("min", 1)
                    .arg("max", 65535)
            }),
        "oscTemplate" => match value.as_str() {
            Some(text) if text.trim().chars().count() <= MAX_TEMPLATE_CHARS => {
                out.template = Some(one_line(text));
                Ok(())
            }
            Some(_) => Err(Msg::error("osc_template_too_long")
                .arg("key", key)
                .arg("max", MAX_TEMPLATE_CHARS)),
            None => Err(Msg::error("osc_template_too_long")
                .arg("key", key)
                .arg("max", MAX_TEMPLATE_CHARS)),
        },
        "oscRepeatS" => value
            .as_u64()
            .and_then(|s| u32::try_from(s).ok())
            .filter(|s| *s == 0 || (REPEAT_MIN_S..=REPEAT_MAX_S).contains(s))
            .map(|s| out.repeat_s = Some(s))
            .ok_or_else(|| {
                Msg::error("osc_repeat_invalid")
                    .arg("key", key)
                    .arg("min", REPEAT_MIN_S)
                    .arg("max", REPEAT_MAX_S)
            }),
        _ => return None,
    };
    Some(result)
}

impl OscPatch {
    /// The stored JSON texts of the keys present.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let text = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
        let mut out = Vec::new();
        if let Some(b) = self.enabled {
            out.push(("oscEnabled", b.to_string()));
        }
        if let Some(h) = &self.host {
            out.push(("oscHost", text(h)));
        }
        if let Some(p) = self.port {
            out.push(("oscPort", p.to_string()));
        }
        if let Some(t) = &self.template {
            out.push(("oscTemplate", text(t)));
        }
        if let Some(s) = self.repeat_s {
            out.push(("oscRepeatS", s.to_string()));
        }
        out
    }
}

impl OscSettings {
    /// Applies a stored value; `false`: not an OSC key. A wrong value keeps the default.
    pub fn read_stored(&mut self, key: &str, value: &Value) -> bool {
        let mut patch = OscPatch::default();
        match sanitize_key(key, value, &mut patch) {
            None => false,
            Some(Err(_)) => true,
            Some(Ok(())) => {
                self.apply(&patch);
                true
            }
        }
    }

    pub fn apply(&mut self, p: &OscPatch) {
        if let Some(b) = p.enabled {
            self.osc_enabled = b;
        }
        if let Some(h) = &p.host {
            self.osc_host = h.clone();
        }
        if let Some(port) = p.port {
            self.osc_port = port;
        }
        if let Some(t) = &p.template {
            self.osc_template = t.clone();
        }
        if let Some(s) = p.repeat_s {
            self.osc_repeat_s = s;
        }
    }

    /// `host:port` as a socket address string (IPv6 in brackets).
    pub fn target(&self) -> String {
        let host = self.osc_host.trim_start_matches('[').trim_end_matches(']');
        if host.contains(':') {
            format!("[{host}]:{}", self.osc_port)
        } else {
            format!("{host}:{}", self.osc_port)
        }
    }
}

// ---------- Text ----------

/// Bidirectional overrides, isolates and zero-width characters: dropped (a title could reverse
/// or hide the rest of the line).
fn invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// One line: control characters (line breaks too) become spaces, invisible ones go, runs of
/// spaces become one, both ends trimmed.
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars().filter(|c| !invisible(*c)) {
        if c.is_control() || c.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

/// At most `max` characters; a longer text ends in `…`.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

/// What the chatbox tells about a film.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowOnAir {
    pub item_id: i64,
    pub title: String,
    /// The viewer's nick (`None`: the owner's video or an anonymous viewer).
    pub nick: Option<String>,
    pub by_host: bool,
}

/// The chatbox text: the template (the default when empty) with `{title}`, `{nick}` and
/// `{added}` filled in one pass (a title holding `{nick}` stays text), then one line, without a
/// dangling separator at the end, at most [`CHATBOX_MAX_CHARS`] characters.
pub fn render(template: &str, now: &NowOnAir, lang: Lang) -> String {
    let template = if template.trim().is_empty() {
        DEFAULT_TEMPLATE
    } else {
        template
    };
    let title = if now.title == crate::resolver::ytdlp::NO_TITLE || now.title.trim().is_empty() {
        i18n::t(lang, "player.untitled", &[])
    } else {
        one_line(&now.title)
    };
    let nick = now.nick.as_deref().map(one_line).unwrap_or_default();
    let added = if now.by_host {
        i18n::t(lang, "ticker.added_by_host", &[])
    } else if nick.is_empty() {
        String::new()
    } else {
        i18n::t(lang, "ticker.added_by", &[("nick", &nick)])
    };
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let field = [("{title}", &title), ("{nick}", &nick), ("{added}", &added)]
            .into_iter()
            .find(|(name, _)| after.starts_with(name));
        match field {
            Some((name, value)) => {
                out.push_str(value);
                rest = &after[name.len()..];
            }
            None => {
                out.push('{');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    truncate(&drop_dangling(one_line(&out)), CHATBOX_MAX_CHARS)
}

/// Without a separator left alone at the end ("▶ Song —" of an empty `{added}`).
fn drop_dangling(mut s: String) -> String {
    loop {
        s.truncate(s.trim_end().len());
        let mut back = s.chars().rev();
        match (back.next(), back.next()) {
            (Some(c), Some(before)) if "—–-·|:,;/".contains(c) && before.is_whitespace() => {
                s.pop();
            }
            _ => return s,
        }
    }
}

// ---------- OSC encoding ----------

/// An OSC string: UTF-8, a NUL, padded with NULs to a multiple of 4 bytes.
fn push_osc_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

/// `/chatbox/input` with the text, `T` (send at once, bypassing the keyboard — `F` would open
/// VRChat's keyboard with the text) and `F` (no notification sound).
pub fn chatbox_message(text: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + text.len());
    push_osc_string(&mut buf, CHATBOX_ADDRESS);
    push_osc_string(&mut buf, ",sTF");
    push_osc_string(&mut buf, text);
    buf
}

/// Sends one datagram to `target` (`host:port`) from an ephemeral port.
pub async fn send(target: &str, packet: &[u8]) -> std::io::Result<()> {
    let addr = tokio::net::lookup_host(target)
        .await?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, target.to_string()))?;
    let bind = if addr.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = tokio::net::UdpSocket::bind(bind).await?;
    socket.send_to(packet, addr).await?;
    Ok(())
}

// ---------- When to send ----------

/// Decides what to send and when: on a new film or text, again after `repeat_s`, never closer
/// than [`MIN_GAP`] (a change inside the gap waits; the newest text goes).
#[derive(Debug, Default)]
pub struct Planner {
    last_text: Option<String>,
    last_at: Option<Instant>,
}

impl Planner {
    /// The text to send now, if any. `text`: the chatbox text of the film on air (`None`: no
    /// film, or the feature is off — nothing is sent and the next film goes out at once).
    pub fn next(&mut self, now: Instant, text: Option<&str>, repeat_s: u32) -> Option<String> {
        let Some(text) = text else {
            self.last_text = None;
            return None;
        };
        let since = self.last_at.map(|at| now.saturating_duration_since(at));
        let changed = self.last_text.as_deref() != Some(text);
        let repeat_due =
            repeat_s > 0 && since.is_some_and(|s| s >= Duration::from_secs(u64::from(repeat_s)));
        if !(changed || repeat_due) {
            return None;
        }
        if since.is_some_and(|s| s < MIN_GAP) {
            return None;
        }
        self.last_text = Some(text.to_string());
        self.last_at = Some(now);
        Some(text.to_string())
    }
}

// ---------- The observer ----------

pub type SharedLog = Arc<dyn Fn(Msg) + Send + Sync>;

pub struct OscWatchOptions {
    pub db: Arc<Mutex<Connection>>,
    pub channel_id: i64,
    pub log: Option<SharedLog>,
}

/// The film viewers see now, for the chatbox.
pub fn now_on_air(snapshot: &Snapshot) -> Option<NowOnAir> {
    let c = snapshot.state.current.as_ref()?;
    let by_host = c.added_by == AddedBy::Admin;
    Some(NowOnAir {
        item_id: c.item_id,
        title: c.title.clone(),
        nick: (!by_host)
            .then(|| c.requester_name.clone())
            .flatten()
            .filter(|n| !n.trim().is_empty()),
        by_host,
    })
}

/// Starts the observer; it ends when the engine's snapshots end (or when aborted).
pub fn spawn_osc_watch(
    snapshots: watch::Receiver<Arc<Snapshot>>,
    o: OscWatchOptions,
) -> JoinHandle<()> {
    tokio::spawn(run(snapshots, o))
}

async fn run(mut snapshots: watch::Receiver<Arc<Snapshot>>, o: OscWatchOptions) {
    let log = |m: Msg| {
        if let Some(log) = &o.log {
            log(m);
        }
    };
    let mut planner = Planner::default();
    let mut settings: Option<(OscSettings, Lang)> = None;
    let mut settings_at: Option<Instant> = None;
    // The target announced last, and the last failure (each logged once).
    let mut announced: Option<String> = None;
    let mut failed: Option<String> = None;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            r = snapshots.changed() => if r.is_err() { break },
            _ = tick.tick() => {}
        }
        if settings_at.is_none_or(|at| at.elapsed() >= SETTINGS_EVERY) {
            let conn = o.db.lock().unwrap_or_else(PoisonError::into_inner);
            if let Ok(s) = SettingsRepo::new(&conn).get(o.channel_id) {
                settings = Some((s.osc, s.slate_language));
            }
            settings_at = Some(Instant::now());
        }
        let Some((s, lang)) = settings.as_ref().filter(|(s, _)| s.osc_enabled) else {
            planner.next(Instant::now(), None, 0);
            announced = None;
            continue;
        };
        let target = s.target();
        if announced.as_deref() != Some(&target) {
            log(Msg::new("log.osc_sending").arg("target", &target));
            announced = Some(target.clone());
            failed = None;
        }
        let snap = Arc::clone(&snapshots.borrow_and_update());
        let text = now_on_air(&snap).map(|now| render(&s.osc_template, &now, *lang));
        let Some(text) = planner.next(Instant::now(), text.as_deref(), s.osc_repeat_s) else {
            continue;
        };
        match send(&target, &chatbox_message(&text)).await {
            Ok(()) => failed = None,
            Err(e) => {
                let e = e.to_string();
                if failed.as_deref() != Some(&e) {
                    log(Msg::new("log.osc_send_failed")
                        .arg("target", &target)
                        .arg("error", &e));
                    failed = Some(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now(title: &str, nick: Option<&str>, by_host: bool) -> NowOnAir {
        NowOnAir {
            item_id: 1,
            title: title.into(),
            nick: nick.map(String::from),
            by_host,
        }
    }

    #[test]
    fn settings_keys_are_checked_and_stored() {
        let mut p = OscPatch::default();
        for (k, v) in [
            ("oscEnabled", json!(true)),
            ("oscHost", json!(" 192.168.1.20 ")),
            ("oscPort", json!(9001)),
            ("oscTemplate", json!("Now:\n{title}")),
            ("oscRepeatS", json!(30)),
        ] {
            sanitize_key(k, &v, &mut p).unwrap().unwrap();
        }
        assert_eq!(p.host.as_deref(), Some("192.168.1.20"));
        assert_eq!(p.template.as_deref(), Some("Now: {title}"));
        let mut s = OscSettings::default();
        for (k, text) in p.entries() {
            assert!(s.read_stored(k, &serde_json::from_str(&text).unwrap()));
        }
        assert!(s.osc_enabled && s.osc_port == 9001 && s.osc_repeat_s == 30);
        assert_eq!(s.target(), "192.168.1.20:9001");
        for (k, v) in [
            ("oscEnabled", json!("yes")),
            ("oscHost", json!("bad host!")),
            ("oscHost", json!("")),
            ("oscPort", json!(0)),
            ("oscPort", json!(70000)),
            ("oscTemplate", json!("x".repeat(201))),
            ("oscTemplate", json!(5)),
            ("oscRepeatS", json!(3)),
            ("oscRepeatS", json!(601)),
        ] {
            assert!(
                sanitize_key(k, &v, &mut OscPatch::default())
                    .unwrap()
                    .is_err(),
                "{k} {v}"
            );
        }
        assert!(sanitize_key("moderation", &json!(true), &mut p).is_none());
        for ok in ["localhost", "my-pc.lan", "::1", "[fe80::1]", "10.0.0.2"] {
            assert!(valid_host(ok), "{ok}");
        }
        let v6 = OscSettings {
            osc_host: "::1".into(),
            ..Default::default()
        };
        assert_eq!(v6.target(), "[::1]:9000");
    }

    #[test]
    fn the_text_fills_the_template_once_and_is_cleaned() {
        let en = Lang::En;
        assert_eq!(
            render(DEFAULT_TEMPLATE, &now("Song", Some("Ala"), false), en),
            "▶ Song — added by Ala"
        );
        assert_eq!(
            render("", &now("Song", None, true), en),
            "▶ Song — added by Host"
        );
        // Anonymous: no dangling separator.
        assert_eq!(
            render(DEFAULT_TEMPLATE, &now("Song", None, false), en),
            "▶ Song"
        );
        // A title holding a field stays text; line breaks and bidi controls go.
        assert_eq!(
            render(
                "{title} / {nick}",
                &now("A {nick}\nB\u{202E}C", Some("N\r\nX"), false),
                en
            ),
            "A {nick} BC / N X"
        );
        assert_eq!(
            render("{unknown} {title}", &now("T", None, false), en),
            "{unknown} T"
        );
        let long = render("{title}", &now(&"é".repeat(300), None, false), en);
        assert_eq!(long.chars().count(), CHATBOX_MAX_CHARS);
        assert!(long.ends_with('…'));
        assert_eq!(
            render(
                "{title}",
                &now(crate::resolver::ytdlp::NO_TITLE, None, false),
                en
            ),
            i18n::t(en, "player.untitled", &[])
        );
        assert_eq!(
            render(DEFAULT_TEMPLATE, &now("Film", Some("Ola"), false), Lang::Pl),
            "▶ Film — dodane przez Ola"
        );
    }

    #[test]
    fn osc_strings_are_padded_to_four_bytes() {
        let m = chatbox_message("hi");
        assert_eq!(m.len() % 4, 0);
        assert_eq!(&m[..16], b"/chatbox/input\0\0");
        assert_eq!(&m[16..24], b",sTF\0\0\0\0");
        assert_eq!(&m[24..], b"hi\0\0");
        // A string of 4 bytes still gets its NUL (and three more).
        assert_eq!(chatbox_message("abcd").len(), 24 + 8);
    }

    #[test]
    fn the_planner_sends_on_change_repeats_and_keeps_the_gap() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut p = Planner::default();
        assert_eq!(p.next(at(0), Some("A"), 0).as_deref(), Some("A"));
        assert_eq!(p.next(at(100), Some("A"), 0), None);
        // A change inside the gap waits, then the newest text goes.
        assert_eq!(p.next(at(500), Some("B"), 0), None);
        assert_eq!(p.next(at(1400), Some("C"), 0), None);
        assert_eq!(p.next(at(1500), Some("C"), 0).as_deref(), Some("C"));
        // Without a repeat nothing more; with one, again after it.
        assert_eq!(p.next(at(60_000), Some("C"), 0), None);
        assert_eq!(p.next(at(60_000), Some("C"), 30).as_deref(), Some("C"));
        assert_eq!(p.next(at(80_000), Some("C"), 30), None);
        assert_eq!(p.next(at(90_000), Some("C"), 30).as_deref(), Some("C"));
        // No film: nothing, and the same film afterwards goes out again.
        assert_eq!(p.next(at(95_000), None, 30), None);
        assert_eq!(p.next(at(95_100), Some("C"), 0).as_deref(), Some("C"));
    }

    /// Decodes `/chatbox/input ,sTF <text>` as VRChat would.
    fn decode(packet: &[u8]) -> (String, String, String) {
        let mut parts = Vec::new();
        let mut at = 0;
        while at < packet.len() && parts.len() < 3 {
            let end = at + packet[at..].iter().position(|b| *b == 0).unwrap();
            parts.push(String::from_utf8(packet[at..end].to_vec()).unwrap());
            at = (end + 4) & !3;
        }
        assert_eq!(at, packet.len(), "padding");
        (parts[0].clone(), parts[1].clone(), parts[2].clone())
    }

    #[tokio::test]
    async fn a_datagram_reaches_a_udp_listener() {
        let listener = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let text = render(
            DEFAULT_TEMPLATE,
            &now("Zażółć", Some("Ala"), false),
            Lang::En,
        );
        send(&target, &chatbox_message(&text)).await.unwrap();
        let mut buf = [0u8; 1024];
        let n = tokio::time::timeout(Duration::from_secs(5), listener.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let (address, tags, got) = decode(&buf[..n]);
        assert_eq!(address, "/chatbox/input");
        assert_eq!(tags, ",sTF");
        assert_eq!(got, "▶ Zażółć — added by Ala");
    }

    #[tokio::test]
    async fn the_observer_sends_the_film_on_air() {
        use crate::db::{MAIN_CHANNEL_ID, SettingsPatch, open_database};
        use crate::pipeline::channel::{ChannelState, ChannelStatus, NowPlaying};

        let listener = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let db = Arc::new(Mutex::new(
            open_database(std::path::Path::new(":memory:")).unwrap(),
        ));
        {
            let conn = db.lock().unwrap();
            let patch = SettingsPatch {
                osc: OscPatch {
                    enabled: Some(true),
                    port: Some(port),
                    template: Some("{title} | {nick}".into()),
                    ..Default::default()
                },
                ..Default::default()
            };
            SettingsRepo::new(&conn)
                .update(MAIN_CHANNEL_ID, &patch)
                .unwrap();
        }
        let state = ChannelState {
            status: ChannelStatus::Playing,
            current: Some(NowPlaying {
                item_id: 7,
                title: "Film\n1".into(),
                duration_s: Some(60.0),
                position_s: 0,
                requester_name: Some("Ala".into()),
                added_by: AddedBy::Public,
                requester_key_hash: None,
                is_live: false,
            }),
            paused: false,
            pause_reasons: Vec::new(),
            control: Default::default(),
            obs: Default::default(),
        };
        let snapshot = Snapshot {
            playlist: String::new(),
            segments: Default::default(),
            live_dir: std::path::PathBuf::new(),
            state,
            prefetched_item_id: None,
            stats: Default::default(),
            monitor: Default::default(),
            air: Default::default(),
        };
        let (tx, rx) = watch::channel(Arc::new(snapshot));
        let task = spawn_osc_watch(
            rx,
            OscWatchOptions {
                db,
                channel_id: MAIN_CHANNEL_ID,
                log: None,
            },
        );
        let mut buf = [0u8; 1024];
        let n = tokio::time::timeout(Duration::from_secs(5), listener.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(decode(&buf[..n]).2, "Film 1 | Ala");
        drop(tx);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }
}
