use super::queue::{AddedBy, ItemStatus, NewItem, QueueRepo};
use super::settings::{ChannelSettings, SettingsRepo, sanitize_settings_patch};
use super::{MAIN_CHANNEL_ID, open_database};
use rusqlite::{Connection, params};
use serde_json::json;

fn item(title: &str) -> NewItem {
    NewItem {
        channel_id: MAIN_CHANNEL_ID,
        source_url: format!("https://youtu.be/{title}"),
        title: title.to_string(),
        duration_s: Some(60.0),
        is_live: false,
        status: ItemStatus::Queued,
        added_by: AddedBy::Admin,
        requester_name: None,
        requester_ip: None,
        requester_key_hash: None,
    }
}

fn mem() -> Connection {
    open_database(std::path::Path::new(":memory:")).unwrap()
}

fn titles(items: Vec<super::queue::QueueItem>) -> Vec<String> {
    items.into_iter().map(|i| i.title).collect()
}

fn queued_titles(q: &QueueRepo) -> Vec<String> {
    titles(
        q.list_by_status(MAIN_CHANNEL_ID, &[ItemStatus::Queued])
            .unwrap(),
    )
}

#[test]
fn open_creates_main_channel() {
    let conn = mem();
    let (slug, name): (String, String) = conn
        .query_row("SELECT slug, name FROM channels WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(slug, "main");
    assert_eq!(name, "Kanał główny");
    assert_eq!(MAIN_CHANNEL_ID, 1);
}

#[test]
fn status_strings_round_trip() {
    let all = [
        (ItemStatus::PendingApproval, "pending_approval"),
        (ItemStatus::Queued, "queued"),
        (ItemStatus::Playing, "playing"),
        (ItemStatus::Played, "played"),
        (ItemStatus::Failed, "failed"),
        (ItemStatus::Removed, "removed"),
        (ItemStatus::Rejected, "rejected"),
    ];
    for (status, s) in all {
        assert_eq!(status.as_str(), s);
        assert_eq!(ItemStatus::parse(s), Some(status));
    }
    assert_eq!(ItemStatus::parse("nope"), None);
    assert_eq!(AddedBy::Admin.as_str(), "admin");
    assert_eq!(AddedBy::Public.as_str(), "public");
    assert_eq!(AddedBy::parse("public"), Some(AddedBy::Public));
    assert_eq!(AddedBy::parse("x"), None);
}

#[test]
fn adds_to_end_of_queue_and_lists_in_order() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    q.add(&item("a")).unwrap();
    q.add(&item("b")).unwrap();
    q.add(&item("c")).unwrap();
    assert_eq!(queued_titles(&q), ["a", "b", "c"]);
    assert_eq!(q.next_queued(MAIN_CHANNEL_ID).unwrap().unwrap().title, "a");
}

#[test]
fn stores_polish_characters_and_fields() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let mut new = item("Żółć");
    new.requester_name = Some("Łukasz".into());
    new.requester_ip = Some("1.2.3.4".into());
    new.added_by = AddedBy::Public;
    let added = q.add(&new).unwrap();
    let got = q.get(added.id).unwrap().unwrap();
    assert_eq!(got.title, "Żółć");
    assert_eq!(got.requester_name.as_deref(), Some("Łukasz"));
    assert_eq!(got.requester_ip.as_deref(), Some("1.2.3.4"));
    assert_eq!(got.added_by, AddedBy::Public);
    assert!(!got.is_live);
    assert_eq!(got.duration_s, Some(60.0));
    assert_eq!(got.error, None);
    assert_eq!(got.started_at, None);
    assert_eq!(got.finished_at, None);
    assert!(got.created_at > 1_600_000_000_000);
    assert!(q.get(9999).unwrap().is_none());
}

#[test]
fn live_item_and_null_duration() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let mut new = item("live");
    new.is_live = true;
    new.duration_s = None;
    let got = q.add(&new).unwrap();
    assert!(got.is_live);
    assert_eq!(got.duration_s, None);
}

#[test]
fn position_is_max_plus_one_over_channel() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let a = q.add(&item("a")).unwrap();
    let b = q.add(&item("b")).unwrap();
    assert_eq!((a.position, b.position), (1, 2));
    // Removing an item does not free its position: MAX over all statuses + 1.
    q.set_status(b.id, ItemStatus::Removed, None).unwrap();
    let c = q.add(&item("c")).unwrap();
    assert_eq!(c.position, 3);
}

#[test]
fn move_item_moves_to_index() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let a = q.add(&item("a")).unwrap();
    q.add(&item("b")).unwrap();
    let c = q.add(&item("c")).unwrap();
    q.move_item(c.id, 0).unwrap();
    assert_eq!(queued_titles(&q), ["c", "a", "b"]);
    q.move_item(a.id, 2).unwrap();
    assert_eq!(queued_titles(&q), ["c", "b", "a"]);
    // Index past the end clamps; positions are renumbered 1..n.
    q.move_item(c.id, 99).unwrap();
    assert_eq!(queued_titles(&q), ["b", "a", "c"]);
    let positions: Vec<i64> = q
        .list_by_status(MAIN_CHANNEL_ID, &[ItemStatus::Queued])
        .unwrap()
        .iter()
        .map(|i| i.position)
        .collect();
    assert_eq!(positions, [1, 2, 3]);
    // Unknown id is a no-op.
    q.move_item(9999, 0).unwrap();
    assert_eq!(queued_titles(&q), ["b", "a", "c"]);
}

#[test]
fn set_status_records_error_and_times() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let a = q.add(&item("a")).unwrap();
    q.set_status(a.id, ItemStatus::Playing, None).unwrap();
    let playing = q.get(a.id).unwrap().unwrap();
    assert!(playing.started_at.is_some());
    assert_eq!(playing.finished_at, None);
    q.set_status(a.id, ItemStatus::Failed, Some("zepsute"))
        .unwrap();
    let got = q.get(a.id).unwrap().unwrap();
    assert_eq!(got.status, ItemStatus::Failed);
    assert_eq!(got.error.as_deref(), Some("zepsute"));
    assert!(got.finished_at.is_some());
    // started_at is kept (COALESCE).
    assert_eq!(got.started_at, playing.started_at);
    assert!(q.next_queued(MAIN_CHANNEL_ID).unwrap().is_none());
}

#[test]
fn set_status_finished_for_all_terminal_states_only() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    for st in [
        ItemStatus::Played,
        ItemStatus::Failed,
        ItemStatus::Removed,
        ItemStatus::Rejected,
    ] {
        let it = q.add(&item("x")).unwrap();
        q.set_status(it.id, st, None).unwrap();
        assert!(
            q.get(it.id).unwrap().unwrap().finished_at.is_some(),
            "{st:?}"
        );
    }
    let it = q.add(&item("y")).unwrap();
    q.set_status(it.id, ItemStatus::Queued, None).unwrap();
    assert_eq!(q.get(it.id).unwrap().unwrap().finished_at, None);
}

#[test]
fn start_playing_only_takes_a_queued_item() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let a = q.add(&item("a")).unwrap();
    assert!(q.start_playing(a.id).unwrap());
    let got = q.get(a.id).unwrap().unwrap();
    assert_eq!(got.status, ItemStatus::Playing);
    assert!(got.started_at.is_some());
    assert_eq!(got.error, None);
    // Already playing: no second transition.
    assert!(!q.start_playing(a.id).unwrap());

    // Removed (DELETE between reading the head and starting it) stays removed.
    let b = q.add(&item("b")).unwrap();
    q.set_status(b.id, ItemStatus::Removed, None).unwrap();
    assert!(!q.start_playing(b.id).unwrap());
    let got = q.get(b.id).unwrap().unwrap();
    assert_eq!(got.status, ItemStatus::Removed);
    assert_eq!(got.started_at, None);
    for st in [ItemStatus::PendingApproval, ItemStatus::Rejected] {
        let c = q.add(&item("c")).unwrap();
        q.set_status(c.id, st, None).unwrap();
        assert!(!q.start_playing(c.id).unwrap(), "{st:?}");
        assert_eq!(q.get(c.id).unwrap().unwrap().status, st);
    }
    assert!(!q.start_playing(9_999).unwrap());
}

#[test]
fn finish_does_not_overwrite_a_removed_item() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    for status in [ItemStatus::Played, ItemStatus::Failed] {
        let a = q.add(&item("a")).unwrap();
        q.set_status(a.id, ItemStatus::Playing, None).unwrap();
        q.set_status(a.id, ItemStatus::Removed, None).unwrap();
        assert!(!q.finish(a.id, status, Some("x")).unwrap(), "{status:?}");
        let got = q.get(a.id).unwrap().unwrap();
        assert_eq!(got.status, ItemStatus::Removed);
        assert_eq!(got.error, None);

        // From playing (and queued) it does finish, with error and finished_at.
        let b = q.add(&item("b")).unwrap();
        q.set_status(b.id, ItemStatus::Playing, None).unwrap();
        assert!(q.finish(b.id, status, Some("zepsute")).unwrap());
        let got = q.get(b.id).unwrap().unwrap();
        assert_eq!(got.status, status);
        assert_eq!(got.error.as_deref(), Some("zepsute"));
        assert!(got.finished_at.is_some());
        let c = q.add(&item("c")).unwrap();
        assert!(q.finish(c.id, status, None).unwrap());
        assert_eq!(q.get(c.id).unwrap().unwrap().status, status);
        // A finished item is not finished again.
        assert!(!q.finish(c.id, ItemStatus::Failed, Some("y")).unwrap());
    }
}

#[test]
fn reset_playing_puts_interrupted_item_first() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    q.add(&item("a")).unwrap();
    let b = q.add(&item("b")).unwrap();
    q.set_status(b.id, ItemStatus::Playing, None).unwrap();
    q.reset_playing(MAIN_CHANNEL_ID).unwrap();
    assert_eq!(queued_titles(&q), ["b", "a"]);
    // Nothing playing: no-op.
    q.reset_playing(MAIN_CHANNEL_ID).unwrap();
    assert_eq!(queued_titles(&q), ["b", "a"]);
}

#[test]
fn count_active_by_ip_counts_pending_and_queued() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let with_ip = |t: &str, ip: &str, st: ItemStatus| {
        let mut n = item(t);
        n.requester_ip = Some(ip.into());
        n.status = st;
        n
    };
    q.add(&with_ip("a", "1.1.1.1", ItemStatus::Queued)).unwrap();
    q.add(&with_ip("b", "1.1.1.1", ItemStatus::PendingApproval))
        .unwrap();
    let c = q.add(&with_ip("c", "1.1.1.1", ItemStatus::Queued)).unwrap();
    q.set_status(c.id, ItemStatus::Played, None).unwrap();
    q.add(&with_ip("d", "2.2.2.2", ItemStatus::Queued)).unwrap();
    assert_eq!(q.count_active_by_ip(MAIN_CHANNEL_ID, "1.1.1.1").unwrap(), 2);
    assert_eq!(q.count_active_by_ip(MAIN_CHANNEL_ID, "9.9.9.9").unwrap(), 0);
}

#[test]
fn history_returns_finished_newest_first() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let a = q.add(&item("a")).unwrap();
    let b = q.add(&item("b")).unwrap();
    let c = q.add(&item("c")).unwrap();
    q.set_status(a.id, ItemStatus::Played, None).unwrap();
    q.set_status(b.id, ItemStatus::Failed, Some("x")).unwrap();
    q.set_status(c.id, ItemStatus::Removed, None).unwrap();
    let h = q.history(MAIN_CHANNEL_ID, 50).unwrap();
    assert_eq!(titles(h), ["b", "a"]);
    let h = q.history(MAIN_CHANNEL_ID, 1).unwrap();
    assert_eq!(titles(h), ["b"]);
}

#[test]
fn list_by_status_filters_multiple_statuses() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    q.add(&item("a")).unwrap();
    let mut p = item("b");
    p.status = ItemStatus::PendingApproval;
    q.add(&p).unwrap();
    let both = q
        .list_by_status(
            MAIN_CHANNEL_ID,
            &[ItemStatus::Queued, ItemStatus::PendingApproval],
        )
        .unwrap();
    assert_eq!(titles(both), ["a", "b"]);
    assert!(q.list_by_status(MAIN_CHANNEL_ID, &[]).unwrap().is_empty());
}

#[test]
fn settings_defaults_and_merge() {
    let conn = mem();
    let s = SettingsRepo::new(&conn);
    assert_eq!(s.get(MAIN_CHANNEL_ID).unwrap(), ChannelSettings::default());
    let patch = sanitize_settings_patch(&json!({"moderation": true, "maxDurationS": 600})).unwrap();
    let got = s.update(MAIN_CHANNEL_ID, &patch).unwrap();
    assert!(got.moderation);
    assert_eq!(got.max_duration_s, 600.0);
    assert_eq!(
        got.allowed_domains,
        ChannelSettings::default().allowed_domains
    );
    assert_eq!(s.get(MAIN_CHANNEL_ID).unwrap(), got);
    // A second update keeps earlier keys.
    let patch = sanitize_settings_patch(&json!({"forceTranscode": true})).unwrap();
    let got = s.update(MAIN_CHANNEL_ID, &patch).unwrap();
    assert!(got.moderation && got.force_transcode);
}

#[test]
fn default_settings_match_spec() {
    let d = ChannelSettings::default();
    assert_eq!(
        d.allowed_domains,
        ["youtube.com", "youtu.be", "pixeldrain.com", "cda.pl"]
    );
    assert_eq!(d.max_duration_s, 7200.0);
    assert_eq!(d.request_limit_count, 3);
    assert_eq!(d.request_limit_window_s, 600);
    assert_eq!(d.max_active_per_requester, 5);
    assert!(!d.moderation);
    assert!(!d.viewer_lists);
    assert_eq!(d.viewer_list_max, 10);
    assert!(!d.force_transcode);
    assert!(d.discontinuity_tags);
    assert_eq!(d.slate_language, crate::i18n::Lang::En);
}

#[test]
fn slate_language_is_validated_stored_and_read_back() {
    let conn = mem();
    let s = SettingsRepo::new(&conn);
    let patch = sanitize_settings_patch(&json!({"slateLanguage": "ja"})).unwrap();
    let got = s.update(MAIN_CHANNEL_ID, &patch).unwrap();
    assert_eq!(got.slate_language, crate::i18n::Lang::Ja);
    assert_eq!(
        serde_json::to_value(&got).unwrap()["slateLanguage"],
        json!("ja")
    );
    let stored: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'slateLanguage'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, "\"ja\"");
    // A hand-edited value that is not a language keeps the default.
    conn.execute(
        "UPDATE settings SET value = '\"xx\"' WHERE key = 'slateLanguage'",
        [],
    )
    .unwrap();
    assert_eq!(
        s.get(MAIN_CHANNEL_ID).unwrap().slate_language,
        crate::i18n::Lang::En
    );
    for bad in [json!("xx"), json!(1), json!(null), json!("PL")] {
        let e = sanitize_settings_patch(&json!({ "slateLanguage": bad })).unwrap_err();
        assert_eq!(
            e.render(crate::i18n::Lang::Pl),
            "slateLanguage: oczekiwano jednego z: en, pl, es, de, fr, ja"
        );
    }
}

#[test]
fn settings_serialize_with_camel_case_keys() {
    let v = serde_json::to_value(ChannelSettings::default()).unwrap();
    assert_eq!(
        v,
        json!({
            "linkMode": "all",
            "allowedDomains": ["youtube.com", "youtu.be", "pixeldrain.com", "cda.pl"],
            "blockedDomains": [],
            "maxDurationS": 7200,
            "requestLimitCount": 3,
            "requestLimitWindowS": 600,
            "maxActivePerRequester": 5,
            "moderation": false,
            "forceTranscode": false,
            "discontinuityTags": true,
            "slateLanguage": "en",
            "tickerEnabled": false,
            "tickerMode": "interval",
            "tickerIntervalS": 60,
            "tickerSpeed": "normal",
            "tickerShowNext": true,
            "tickerShowAddress": false,
            "tickerQr": "with_ticker",
            "tickerProgress": true,
            "tickerMessages": [],
            "viewerLists": false,
            "viewerListMax": 10,
            "uploadMbps": 0,
            "transitionsEnabled": false,
            "transitionS": 1,
            "peakMbps": 0,
            "peakBufferS": 2,
            "subtitlesEnabled": false,
            "subtitlesLanguage": "en",
            "loudnessEnabled": false,
            "loudnessTarget": -16,
            "alarmsEnabled": false,
            "alarmBlackS": 10,
            "alarmSilenceS": 15,
            "alarmSilenceDb": -50,
            "maxHeight": 0,
            "slateFrameStyle": "dim",
            "slateBackgroundAll": false,
        })
    );
    // Keys are emitted in the same order as the TS object.
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "linkMode",
            "allowedDomains",
            "blockedDomains",
            "maxDurationS",
            "requestLimitCount",
            "requestLimitWindowS",
            "maxActivePerRequester",
            "moderation",
            "forceTranscode",
            "discontinuityTags",
            "slateLanguage",
            "tickerEnabled",
            "tickerMode",
            "tickerIntervalS",
            "tickerSpeed",
            "tickerShowNext",
            "tickerShowAddress",
            "tickerQr",
            "tickerProgress",
            "tickerMessages",
            "viewerLists",
            "viewerListMax",
            "uploadMbps",
            "transitionsEnabled",
            "transitionS",
            "peakMbps",
            "peakBufferS",
            "subtitlesEnabled",
            "subtitlesLanguage",
            "loudnessEnabled",
            "loudnessTarget",
            "alarmsEnabled",
            "alarmBlackS",
            "alarmSilenceS",
            "alarmSilenceDb",
            "maxHeight",
            "slateFrameStyle",
            "slateBackgroundAll"
        ]
    );
}

#[test]
fn advanced_settings_are_saved_and_read_back() {
    let conn = mem();
    let s = SettingsRepo::new(&conn);
    let patch = sanitize_settings_patch(&json!({
        "loudnessEnabled": true,
        "loudnessTarget": -23,
        "peakMbps": 4.5,
        "moderation": true,
    }))
    .unwrap();
    let saved = s.update(MAIN_CHANNEL_ID, &patch).unwrap();
    assert!(saved.moderation && saved.advanced.loudness_enabled);
    assert_eq!(saved.advanced.loudness_target, -23);
    assert_eq!(saved.advanced.peak_mbps, 4.5);
    assert_eq!(s.get(MAIN_CHANNEL_ID).unwrap(), saved);
    let err = sanitize_settings_patch(&json!({"peakMbps": 40})).unwrap_err();
    assert!(err.to_string().contains("peakMbps"), "{err}");
    assert!(sanitize_settings_patch(&json!({"nieznane": 1})).is_err());
}

#[test]
fn settings_are_stored_as_json_text_with_camel_case_keys() {
    let conn = mem();
    let s = SettingsRepo::new(&conn);
    let patch = sanitize_settings_patch(&json!({
        "allowedDomains": ["a.com", "b.com"],
        "maxDurationS": 600,
        "requestLimitCount": 7,
        "moderation": true,
    }))
    .unwrap();
    s.update(MAIN_CHANNEL_ID, &patch).unwrap();
    let get = |k: &str| -> String {
        conn.query_row(
            "SELECT value FROM settings WHERE channel_id = 1 AND key = ?1",
            [k],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(get("allowedDomains"), r#"["a.com","b.com"]"#);
    assert_eq!(get("maxDurationS"), "600");
    assert_eq!(get("requestLimitCount"), "7");
    assert_eq!(get("moderation"), "true");
}

#[test]
fn sanitize_normalises_domains() {
    let p = sanitize_settings_patch(&json!({
        "allowedDomains": [" HTTPS://WWW.YouTube.com/watch?v=1 ", "http://cda.pl", "www.example.org/", "", "   ", "/only/path"]
    }))
    .unwrap();
    let conn = mem();
    let got = SettingsRepo::new(&conn)
        .update(MAIN_CHANNEL_ID, &p)
        .unwrap();
    assert_eq!(
        got.allowed_domains,
        ["youtube.com", "cda.pl", "example.org"]
    );
}

#[test]
fn sanitize_rejects_bad_input() {
    let err = |v: serde_json::Value| {
        sanitize_settings_patch(&v)
            .err()
            .unwrap()
            .render(crate::i18n::Lang::Pl)
    };
    assert_eq!(err(json!({"nope": 1})), "Nieznane ustawienie: nope");
    assert_eq!(
        err(json!({"allowedDomains": "a.com"})),
        "allowedDomains: oczekiwano listy"
    );
    assert_eq!(
        err(json!({"allowedDomains": ["a.com", 3]})),
        "allowedDomains: oczekiwano listy"
    );
    assert_eq!(
        err(json!({"maxDurationS": "5"})),
        "maxDurationS: oczekiwano liczby ≥ 0"
    );
    assert_eq!(
        err(json!({"maxDurationS": -1})),
        "maxDurationS: oczekiwano liczby ≥ 0"
    );
    assert_eq!(
        err(json!({"requestLimitCount": null})),
        "requestLimitCount: oczekiwano liczby ≥ 0"
    );
    assert_eq!(
        err(json!({"moderation": 1})),
        "moderation: oczekiwano true/false"
    );
    assert_eq!(
        err(json!({"discontinuityTags": "true"})),
        "discontinuityTags: oczekiwano true/false"
    );
    // The first bad key (in input order) is reported.
    assert_eq!(
        err(json!({"moderation": 1, "zzz": 1})),
        "moderation: oczekiwano true/false"
    );
    assert_eq!(
        err(json!({"zzz": 1, "moderation": 1})),
        "Nieznane ustawienie: zzz"
    );
    // Non-object bodies are rejected rather than panicking.
    assert!(sanitize_settings_patch(&json!([1, 2])).is_err());
    assert!(sanitize_settings_patch(&json!("x")).is_err());
}

#[test]
fn sanitize_accepts_zero_and_empty_patch() {
    let p = sanitize_settings_patch(&json!({"maxDurationS": 0, "requestLimitCount": 0})).unwrap();
    let conn = mem();
    let got = SettingsRepo::new(&conn)
        .update(MAIN_CHANNEL_ID, &p)
        .unwrap();
    assert_eq!(got.max_duration_s, 0.0);
    assert_eq!(got.request_limit_count, 0);
    let empty = sanitize_settings_patch(&json!({})).unwrap();
    assert_eq!(
        SettingsRepo::new(&conn)
            .update(MAIN_CHANNEL_ID, &empty)
            .unwrap(),
        got
    );
}

#[test]
fn corrupt_setting_value_falls_back_to_default() {
    let conn = mem();
    conn.execute(
        "INSERT INTO settings (channel_id, key, value) VALUES (1, 'moderation', 'not json')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO settings (channel_id, key, value) VALUES (1, 'maxDurationS', '\"str\"')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO settings (channel_id, key, value) VALUES (1, 'unknownKey', '1')",
        [],
    )
    .unwrap();
    let got = SettingsRepo::new(&conn).get(MAIN_CHANNEL_ID).unwrap();
    assert_eq!(got, ChannelSettings::default());
}

// ---- Compatibility with a database file created by the TypeScript version ----

/// Verbatim from src/db/database.ts (SCHEMA).
const TS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS channels (
  id INTEGER PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
  channel_id INTEGER NOT NULL REFERENCES channels(id),
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (channel_id, key)
);
CREATE TABLE IF NOT EXISTS queue_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel_id INTEGER NOT NULL REFERENCES channels(id),
  position INTEGER NOT NULL,
  source_url TEXT NOT NULL,
  title TEXT NOT NULL,
  duration_s REAL,
  is_live INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL,
  added_by TEXT NOT NULL,
  requester_name TEXT,
  requester_ip TEXT,
  error TEXT,
  created_at INTEGER NOT NULL,
  started_at INTEGER,
  finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS queue_items_channel_status ON queue_items(channel_id, status, position);
";

fn schema_dump(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
/// The shared schema plus the additive migrations (the old server ignores the extras).
fn schema_is_identical_to_typescript_schema() {
    let ts = Connection::open_in_memory().unwrap();
    ts.execute_batch(TS_SCHEMA).unwrap();
    super::migrate(&ts).unwrap();
    assert_eq!(schema_dump(&mem()), schema_dump(&ts));
}

#[test]
fn open_enables_wal_and_foreign_keys_and_creates_parent_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("data").join("vrcinema.db");
    let conn = open_database(&path).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);
    // Foreign keys are enforced.
    let mut bad = item("x");
    bad.channel_id = 42;
    assert!(QueueRepo::new(&conn).add(&bad).is_err());
}

#[test]
fn reads_database_file_written_by_typescript_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vrcinema.db");
    {
        // Exactly what the TS version writes.
        let ts = Connection::open(&path).unwrap();
        ts.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .unwrap();
        ts.execute_batch(TS_SCHEMA).unwrap();
        ts.execute(
            "INSERT OR IGNORE INTO channels (id, slug, name, created_at) VALUES (?, ?, ?, ?)",
            params![1, "main", "Kanał główny", 1_700_000_000_000_i64],
        )
        .unwrap();
        let ins = "INSERT INTO queue_items (channel_id, position, source_url, title, duration_s, is_live, status,
               added_by, requester_name, requester_ip, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        // node:sqlite binds JS numbers 60 as integers; REAL affinity converts them.
        ts.execute(
            ins,
            params![
                1,
                1,
                "https://youtu.be/a",
                "Żółć",
                60,
                0,
                "queued",
                "public",
                "Łukasz",
                "1.2.3.4",
                1_700_000_000_100_i64
            ],
        )
        .unwrap();
        ts.execute(
            ins,
            params![
                1,
                2,
                "https://live.example/x",
                "Live",
                None::<f64>,
                1,
                "pending_approval",
                "admin",
                None::<String>,
                None::<String>,
                1_700_000_000_200_i64
            ],
        )
        .unwrap();
        ts.execute(
            ins,
            params![
                1,
                3,
                "https://youtu.be/c",
                "Done",
                12.5,
                0,
                "queued",
                "admin",
                None::<String>,
                None::<String>,
                1_700_000_000_300_i64
            ],
        )
        .unwrap();
        ts.execute(
            "UPDATE queue_items SET status = 'played', error = NULL, started_at = 1700000001000, finished_at = 1700000002000 WHERE id = 3",
            [],
        )
        .unwrap();
        let set = "INSERT INTO settings (channel_id, key, value) VALUES (?, ?, ?)
             ON CONFLICT(channel_id, key) DO UPDATE SET value = excluded.value";
        ts.execute(
            set,
            params![1, "allowedDomains", r#"["example.com","cda.pl"]"#],
        )
        .unwrap();
        ts.execute(set, params![1, "maxDurationS", "600"]).unwrap();
        ts.execute(set, params![1, "moderation", "true"]).unwrap();
        ts.execute(set, params![1, "requestLimitWindowS", "120"])
            .unwrap();
    }

    let conn = open_database(&path).unwrap();
    // The existing main channel is left untouched (INSERT OR IGNORE).
    let created: i64 = conn
        .query_row("SELECT created_at FROM channels WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(created, 1_700_000_000_000);

    let q = QueueRepo::new(&conn);
    let items = q
        .list_by_status(
            MAIN_CHANNEL_ID,
            &[ItemStatus::Queued, ItemStatus::PendingApproval],
        )
        .unwrap();
    assert_eq!(items.len(), 2);
    let a = &items[0];
    assert_eq!(a.title, "Żółć");
    assert_eq!(a.duration_s, Some(60.0));
    assert!(!a.is_live);
    assert_eq!(a.added_by, AddedBy::Public);
    assert_eq!(a.requester_name.as_deref(), Some("Łukasz"));
    assert_eq!(a.created_at, 1_700_000_000_100);
    let live = &items[1];
    assert!(live.is_live);
    assert_eq!(live.status, ItemStatus::PendingApproval);
    assert_eq!(live.duration_s, None);
    assert_eq!(live.requester_ip, None);

    let hist = q.history(MAIN_CHANNEL_ID, 10).unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].started_at, Some(1_700_000_001_000));
    assert_eq!(hist[0].finished_at, Some(1_700_000_002_000));
    assert_eq!(hist[0].duration_s, Some(12.5));

    // New items continue the TS numbering.
    assert_eq!(q.add(&item("n")).unwrap().position, 4);

    let s = SettingsRepo::new(&conn).get(MAIN_CHANNEL_ID).unwrap();
    assert_eq!(s.allowed_domains, ["example.com", "cda.pl"]);
    assert_eq!(s.max_duration_s, 600.0);
    assert!(s.moderation);
    assert_eq!(s.request_limit_window_s, 120);
    // Missing keys fall back to defaults.
    assert_eq!(s.request_limit_count, 3);
    assert_eq!(s.max_active_per_requester, 5);
    assert!(!s.force_transcode);
    assert!(s.discontinuity_tags);
}

#[test]
fn typescript_numeric_settings_edge_cases_are_readable() {
    // TS accepts any finite number >= 0, including fractions and floats like 7200.5.
    let conn = mem();
    for (k, v) in [
        ("maxDurationS", "7200.5"),
        ("requestLimitCount", "2.0"),
        ("maxActivePerRequester", "4"),
    ] {
        conn.execute(
            "INSERT INTO settings (channel_id, key, value) VALUES (1, ?1, ?2)",
            params![k, v],
        )
        .unwrap();
    }
    let s = SettingsRepo::new(&conn).get(MAIN_CHANNEL_ID).unwrap();
    assert_eq!(s.max_duration_s, 7200.5);
    assert_eq!(s.request_limit_count, 2);
    assert_eq!(s.max_active_per_requester, 4);
}

#[test]
fn add_first_puts_the_new_item_at_the_head_of_the_queue() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    q.add(&item("a")).unwrap();
    q.add(&item("b")).unwrap();
    let c = q.add_first(&item("c")).unwrap();
    assert_eq!(c.status, ItemStatus::Queued);
    assert_eq!(queued_titles(&q), ["c", "a", "b"]);
    assert_eq!(q.next_queued(MAIN_CHANNEL_ID).unwrap().unwrap().id, c.id);
}

/// Stage 4 fix round 1 (6): adding and moving to the front is one transaction: when the
/// renumbering fails, the new entry is not left at the end of the queue.
#[test]
fn add_first_leaves_nothing_behind_when_it_fails() {
    let conn = mem();
    let q = QueueRepo::new(&conn);
    q.add(&item("a")).unwrap();
    conn.execute_batch(
        "CREATE TEMP TRIGGER no_moves BEFORE UPDATE OF position ON queue_items
         BEGIN SELECT RAISE(ABORT, 'zablokowane'); END;",
    )
    .unwrap();
    assert!(q.add_first(&item("c")).is_err());
    let all: i64 = conn
        .query_row("SELECT COUNT(*) FROM queue_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(all, 1);
    assert_eq!(queued_titles(&q), ["a"]);
}

#[test]
fn ticker_settings_are_validated_stored_and_read_back() {
    use crate::pipeline::ticker::{TickerMode, TickerQr, TickerSpeed};
    let conn = mem();
    let s = SettingsRepo::new(&conn);
    let patch = sanitize_settings_patch(&json!({
        "tickerEnabled": true,
        "tickerMode": "continuous",
        "tickerIntervalS": 300,
        "tickerSpeed": "fast",
        "tickerShowNext": false,
        "tickerShowAddress": true,
        "tickerQr": "always",
        "tickerProgress": false,
        "tickerMessages": ["  Witajcie!  ", "", "Max 3 filmy"],
    }))
    .unwrap();
    let got = s.update(MAIN_CHANNEL_ID, &patch).unwrap();
    let t = &got.ticker;
    assert!(t.enabled);
    assert_eq!(t.mode, TickerMode::Continuous);
    assert_eq!(t.interval_s, 300);
    assert_eq!(t.speed, TickerSpeed::Fast);
    assert!(!t.show_next);
    assert!(t.show_address);
    assert_eq!(t.qr, TickerQr::Always);
    assert!(!t.progress);
    // Trimmed, empty ones dropped.
    assert_eq!(t.messages, ["Witajcie!", "Max 3 filmy"]);
    assert_eq!(s.get(MAIN_CHANNEL_ID).unwrap(), got);
    let v = serde_json::to_value(&got).unwrap();
    assert_eq!(v["tickerMode"], "continuous");
    assert_eq!(v["tickerQr"], "always");
    assert_eq!(v["tickerMessages"], json!(["Witajcie!", "Max 3 filmy"]));
    // Hand-edited nonsense keeps the defaults.
    for key in [
        "tickerMode",
        "tickerIntervalS",
        "tickerSpeed",
        "tickerQr",
        "tickerMessages",
    ] {
        conn.execute("UPDATE settings SET value = '\"xx\"' WHERE key = ?1", [key])
            .unwrap();
    }
    let t = s.get(MAIN_CHANNEL_ID).unwrap().ticker;
    let d = ChannelSettings::default().ticker;
    assert_eq!(
        (t.mode, t.interval_s, t.speed, t.qr, t.messages),
        (d.mode, d.interval_s, d.speed, d.qr, d.messages)
    );
}

#[test]
fn ticker_settings_refuse_too_many_or_too_long_messages_and_odd_values() {
    let err = |v: serde_json::Value| {
        sanitize_settings_patch(&v)
            .err()
            .unwrap()
            .render(crate::i18n::Lang::En)
    };
    let ten: Vec<String> = (0..10).map(|i| format!("m{i}")).collect();
    assert!(sanitize_settings_patch(&json!({ "tickerMessages": ten })).is_ok());
    let eleven: Vec<String> = (0..11).map(|i| format!("m{i}")).collect();
    assert_eq!(
        err(json!({ "tickerMessages": eleven })),
        "tickerMessages: at most 10 messages"
    );
    let max = "ż".repeat(200);
    assert!(sanitize_settings_patch(&json!({ "tickerMessages": [max] })).is_ok());
    assert_eq!(
        err(json!({ "tickerMessages": ["ż".repeat(201)] })),
        "tickerMessages: a message can have at most 200 characters"
    );
    assert_eq!(
        err(json!({ "tickerMessages": "tekst" })),
        "tickerMessages: expected a list"
    );
    assert_eq!(
        err(json!({ "tickerMessages": [1] })),
        "tickerMessages: expected a list"
    );
    for ok in [60, 120, 300, 600] {
        assert!(sanitize_settings_patch(&json!({ "tickerIntervalS": ok })).is_ok());
    }
    for bad in [
        json!(0),
        json!(30),
        json!(90),
        json!(60.5),
        json!(1200),
        json!("60"),
    ] {
        assert_eq!(
            err(json!({ "tickerIntervalS": bad })),
            "tickerIntervalS: expected one of: 60, 120, 300, 600"
        );
    }
    assert_eq!(
        err(json!({ "tickerMode": "loop" })),
        "tickerMode: expected one of: interval, continuous"
    );
    assert_eq!(
        err(json!({ "tickerSpeed": "Normal" })),
        "tickerSpeed: expected one of: slow, normal, fast"
    );
    assert_eq!(
        err(json!({ "tickerQr": true })),
        "tickerQr: expected one of: off, with_ticker, always"
    );
    assert_eq!(
        err(json!({ "tickerEnabled": "true" })),
        "tickerEnabled: expected true/false"
    );
}

#[test]
fn the_ticker_default_is_stored_once_and_the_owner_choice_wins() {
    let conn = mem();
    let s = SettingsRepo::new(&conn);
    assert!(!s.get(MAIN_CHANNEL_ID).unwrap().ticker.enabled);
    // First detection of a GPU encoder: on.
    assert!(s.seed_ticker_enabled(MAIN_CHANNEL_ID, true).unwrap());
    assert!(s.get(MAIN_CHANNEL_ID).unwrap().ticker.enabled);
    // Later detections do not change it.
    assert!(!s.seed_ticker_enabled(MAIN_CHANNEL_ID, false).unwrap());
    assert!(s.get(MAIN_CHANNEL_ID).unwrap().ticker.enabled);
    // Nor do they override the owner.
    let off = sanitize_settings_patch(&json!({"tickerEnabled": false})).unwrap();
    s.update(MAIN_CHANNEL_ID, &off).unwrap();
    assert!(!s.seed_ticker_enabled(MAIN_CHANNEL_ID, true).unwrap());
    assert!(!s.get(MAIN_CHANNEL_ID).unwrap().ticker.enabled);
}

#[test]
fn only_layout_changes_of_the_ticker_restart_the_film() {
    let base = ChannelSettings::default().ticker;
    let mut content = base.clone();
    content.messages = vec!["nowa".into()];
    content.show_next = !content.show_next;
    content.show_address = !content.show_address;
    assert!(base.same_layout(&content));
    use crate::pipeline::ticker::{TickerMode, TickerQr, TickerSettings, TickerSpeed};
    let changes: [fn(&mut TickerSettings); 6] = [
        |t| t.enabled = !t.enabled,
        |t| t.mode = TickerMode::Continuous,
        |t| t.interval_s = 600,
        |t| t.speed = TickerSpeed::Slow,
        |t| t.qr = TickerQr::Off,
        |t| t.progress = !t.progress,
    ];
    for change in changes {
        let mut other = base.clone();
        change(&mut other);
        assert!(!base.same_layout(&other), "{other:?}");
    }
}

#[test]
fn migration_keeps_every_item_of_a_stage_3_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vrcinema.db");
    {
        let old = Connection::open(&path).unwrap();
        old.execute_batch(TS_SCHEMA).unwrap();
        old.execute(
            "INSERT INTO channels (id, slug, name, created_at) VALUES (1, 'main', 'Kanał główny', 1)",
            [],
        )
        .unwrap();
        for (pos, title) in [(1, "Pierwszy"), (2, "Drugi")] {
            old.execute(
                "INSERT INTO queue_items (channel_id, position, source_url, title, is_live, status,
                   added_by, requester_name, requester_ip, created_at)
                 VALUES (1, ?, 'https://youtu.be/x', ?, 0, 'queued', 'public', 'Ola', '1.2.3.4', 5)",
                params![pos, title],
            )
            .unwrap();
        }
    }
    for _ in 0..2 {
        // Opening again changes nothing.
        let conn = open_database(&path).unwrap();
        let items = QueueRepo::new(&conn)
            .list_by_status(MAIN_CHANNEL_ID, &[ItemStatus::Queued])
            .unwrap();
        assert_eq!(titles(items.clone()), ["Pierwszy", "Drugi"]);
        assert!(items.iter().all(|i| i.requester_key_hash.is_none()));
        assert_eq!(items[0].requester_ip.as_deref(), Some("1.2.3.4"));
    }
    let conn = open_database(&path).unwrap();
    let mut with_key = item("z");
    with_key.requester_key_hash = Some("ab".repeat(32));
    let added = QueueRepo::new(&conn).add(&with_key).unwrap();
    assert_eq!(added.position, 3);
    assert_eq!(added.requester_key_hash.as_deref(), Some(&*"ab".repeat(32)));
    for table in ["viewer_nicks", "blocked_devices"] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}

// ---------- link modes ----------

fn link_mode_of(conn: &Connection) -> super::settings::LinkMode {
    SettingsRepo::new(conn)
        .get(MAIN_CHANNEL_ID)
        .unwrap()
        .link_mode
}

#[test]
fn a_new_database_lets_viewers_send_any_site() {
    let conn = mem();
    let s = SettingsRepo::new(&conn).get(MAIN_CHANNEL_ID).unwrap();
    assert_eq!(s.link_mode, super::settings::LinkMode::All);
    assert!(s.blocked_domains.is_empty());
}

#[test]
fn a_database_from_before_the_link_modes_becomes_an_allowlist_with_its_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite");
    {
        // What an older version left: the channel and its allowed sites, no `linkMode`.
        let old = open_database(&path).unwrap();
        old.execute("DELETE FROM settings WHERE key = 'linkMode'", [])
            .unwrap();
        old.execute(
            "INSERT INTO settings (channel_id, key, value) VALUES (1, 'allowedDomains', ?)",
            [r#"["example.com","cda.pl"]"#],
        )
        .unwrap();
    }
    let conn = open_database(&path).unwrap();
    let s = SettingsRepo::new(&conn).get(MAIN_CHANNEL_ID).unwrap();
    assert_eq!(s.link_mode, super::settings::LinkMode::Allowlist);
    assert_eq!(s.allowed_domains, ["example.com", "cda.pl"]);
    assert!(s.blocked_domains.is_empty());
}

#[test]
fn an_old_database_without_a_stored_list_keeps_the_default_list_as_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite");
    {
        let old = Connection::open(&path).unwrap();
        old.execute_batch(
            "CREATE TABLE channels (id INTEGER PRIMARY KEY, slug TEXT NOT NULL UNIQUE,
               name TEXT NOT NULL, created_at INTEGER NOT NULL);
             INSERT INTO channels VALUES (1, 'main', 'Kanał główny', 0);",
        )
        .unwrap();
    }
    let conn = open_database(&path).unwrap();
    let s = SettingsRepo::new(&conn).get(MAIN_CHANNEL_ID).unwrap();
    assert_eq!(s.link_mode, super::settings::LinkMode::Allowlist);
    assert_eq!(
        s.allowed_domains,
        ChannelSettings::default().allowed_domains
    );
}

#[test]
fn the_link_mode_is_decided_once_and_the_owners_choice_survives_a_restart() {
    use super::settings::LinkMode;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.sqlite");
    assert_eq!(link_mode_of(&open_database(&path).unwrap()), LinkMode::All);
    // A second start of a database made now is not "old": it stays `all`.
    let conn = open_database(&path).unwrap();
    assert_eq!(link_mode_of(&conn), LinkMode::All);
    let patch = sanitize_settings_patch(&json!({"linkMode": "blocklist"})).unwrap();
    SettingsRepo::new(&conn)
        .update(MAIN_CHANNEL_ID, &patch)
        .unwrap();
    drop(conn);
    assert_eq!(
        link_mode_of(&open_database(&path).unwrap()),
        LinkMode::Blocklist
    );
}

#[test]
fn link_mode_and_blocked_domains_are_validated_normalised_and_stored() {
    use super::settings::LinkMode;
    let conn = mem();
    let patch = sanitize_settings_patch(&json!({
        "linkMode": "blocklist",
        "blockedDomains": [" HTTPS://www.Bad.Example/x ", "", "tiktok.com"],
    }))
    .unwrap();
    let got = SettingsRepo::new(&conn)
        .update(MAIN_CHANNEL_ID, &patch)
        .unwrap();
    assert_eq!(got.link_mode, LinkMode::Blocklist);
    assert_eq!(got.blocked_domains, ["bad.example", "tiktok.com"]);
    // Both lists are always kept.
    assert_eq!(
        got.allowed_domains,
        ChannelSettings::default().allowed_domains
    );
    let err = |v| {
        sanitize_settings_patch(&v)
            .unwrap_err()
            .render(crate::i18n::Lang::Pl)
    };
    assert_eq!(
        err(json!({"linkMode": "everything"})),
        "linkMode: oczekiwano jednego z: all, allowlist, blocklist"
    );
    assert_eq!(
        err(json!({"blockedDomains": "a.com"})),
        "blockedDomains: oczekiwano listy"
    );
}

fn viewer(title: &str, name: Option<&str>, key: Option<&str>, status: ItemStatus) -> NewItem {
    NewItem {
        added_by: AddedBy::Public,
        requester_name: name.map(str::to_string),
        requester_key_hash: key.map(str::to_string),
        status,
        ..item(title)
    }
}

#[test]
fn remove_where_takes_one_viewer_or_everything_and_restore_puts_it_back() {
    use super::queue::Requester;
    let conn = mem();
    let q = QueueRepo::new(&conn);
    let on_air = q
        .add(&viewer(
            "on-air",
            Some("Ola"),
            Some("k1"),
            ItemStatus::Queued,
        ))
        .unwrap();
    q.start_playing(on_air.id).unwrap();
    q.add(&viewer("a", Some("Ola"), Some("k1"), ItemStatus::Queued))
        .unwrap();
    q.add(&item("host")).unwrap();
    // Same device under another nick: still that viewer.
    q.add(&viewer(
        "b",
        Some("Ola2"),
        Some("k1"),
        ItemStatus::PendingApproval,
    ))
    .unwrap();
    q.add(&viewer("c", Some("Ola"), None, ItemStatus::Queued))
        .unwrap();
    q.add(&viewer("d", Some("Tomek"), Some("k2"), ItemStatus::Queued))
        .unwrap();

    let device = Requester::Device("k1".into());
    let removed = q
        .remove_where(
            MAIN_CHANNEL_ID,
            &[ItemStatus::Queued, ItemStatus::PendingApproval],
            Some(&device),
        )
        .unwrap();
    assert_eq!(removed.len(), 2);
    assert_eq!(queued_titles(&q), ["host", "c", "d"]);
    assert_eq!(
        q.get(on_air.id).unwrap().unwrap().status,
        ItemStatus::Playing
    );

    // A nick without a device matches only items without a device key.
    let nick = Requester::Nick("Ola".into());
    let by_nick = q
        .remove_where(MAIN_CHANNEL_ID, &[ItemStatus::Queued], Some(&nick))
        .unwrap();
    assert_eq!(by_nick.len(), 1);
    assert_eq!(queued_titles(&q), ["host", "d"]);

    assert_eq!(q.restore_removed(&removed).unwrap(), 2);
    assert_eq!(queued_titles(&q), ["a", "host", "d"]);
    let b = q
        .list_by_status(MAIN_CHANNEL_ID, &[ItemStatus::PendingApproval])
        .unwrap();
    assert_eq!(titles(b), ["b"]);
    assert!(q.get(removed[0].0).unwrap().unwrap().finished_at.is_none());
    // A second restore changes nothing.
    assert_eq!(q.restore_removed(&removed).unwrap(), 0);

    // Everything, the film on air too; it comes back first in the queue.
    let all = q
        .remove_where(
            MAIN_CHANNEL_ID,
            &[ItemStatus::Queued, ItemStatus::Playing],
            None,
        )
        .unwrap();
    assert_eq!(all.len(), 4);
    assert!(queued_titles(&q).is_empty());
    assert_eq!(q.restore_removed(&all).unwrap(), 4);
    assert_eq!(queued_titles(&q), ["on-air", "a", "host", "d"]);
    assert_eq!(
        Requester::of(&q.get(on_air.id).unwrap().unwrap()),
        Some(device)
    );
    assert_eq!(Requester::of(&q.add(&item("x")).unwrap()), None);
}

#[test]
fn viewer_list_settings_are_stored_and_the_size_is_kept_within_1_to_50() {
    let conn = mem();
    let s = SettingsRepo::new(&conn);
    let patch =
        sanitize_settings_patch(&json!({"viewerLists": true, "viewerListMax": 99})).unwrap();
    let got = s.update(MAIN_CHANNEL_ID, &patch).unwrap();
    assert!(got.viewer_lists);
    assert_eq!(got.viewer_list_max, 50);
    let patch = sanitize_settings_patch(&json!({"viewerListMax": 0})).unwrap();
    assert_eq!(
        s.update(MAIN_CHANNEL_ID, &patch).unwrap().viewer_list_max,
        1
    );
    assert!(sanitize_settings_patch(&json!({"viewerLists": "yes"})).is_err());
    assert!(sanitize_settings_patch(&json!({"viewerListMax": -1})).is_err());
}
