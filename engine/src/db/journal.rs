//! The broadcast journal (Settings → Advanced): which film aired when. One row per stretch of a
//! film on air (a pause or a slate in between starts a new row). Written by the air observer
//! (`pipeline::air_watch`), read and exported by the panel.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::queue::QueueItem;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS air_journal (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel_id INTEGER NOT NULL,
  item_id INTEGER,
  title TEXT NOT NULL,
  source_url TEXT NOT NULL,
  requester_name TEXT,
  added_by TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  from_s REAL,
  to_s REAL
);
CREATE INDEX IF NOT EXISTS air_journal_channel ON air_journal(channel_id, id);
";

/// Rows kept per channel (the oldest go first).
pub const KEEP_ROWS: i64 = 5000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub id: i64,
    pub item_id: Option<i64>,
    pub title: String,
    pub source_url: String,
    pub requester_name: Option<String>,
    /// `admin` or `public`.
    pub added_by: String,
    /// Unix ms.
    pub started_at: i64,
    /// Unix ms; `None` while it airs (or after a crash).
    pub ended_at: Option<i64>,
    /// Film seconds aired: from, to.
    pub from_s: Option<f64>,
    pub to_s: Option<f64>,
}

pub struct JournalRepo<'a> {
    conn: &'a Connection,
}

impl<'a> JournalRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// A film started airing: a new row (the oldest beyond [`KEEP_ROWS`] go). Returns its id.
    pub fn open(
        &self,
        channel_id: i64,
        item: &QueueItem,
        started_at: i64,
        from_s: Option<f64>,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO air_journal (channel_id, item_id, title, source_url, requester_name,
               added_by, started_at, from_s) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                channel_id,
                item.id,
                item.title,
                item.source_url,
                item.requester_name,
                item.added_by.as_str(),
                started_at,
                from_s
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM air_journal WHERE channel_id = ? AND id <= ?",
            params![channel_id, id - KEEP_ROWS],
        )?;
        Ok(id)
    }

    /// The row's film stopped airing at `ended_at`, having reached film second `to_s`.
    pub fn close(&self, id: i64, ended_at: i64, to_s: Option<f64>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE air_journal SET ended_at = ?, to_s = ? WHERE id = ?",
            params![ended_at, to_s, id],
        )?;
        Ok(())
    }

    /// The newest `limit` rows, newest first.
    pub fn list(&self, channel_id: i64, limit: usize) -> rusqlite::Result<Vec<JournalEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, item_id, title, source_url, requester_name, added_by, started_at,
               ended_at, from_s, to_s FROM air_journal WHERE channel_id = ? ORDER BY id DESC LIMIT ?",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        stmt.query_map(params![channel_id, limit], |r| {
            Ok(JournalEntry {
                id: r.get(0)?,
                item_id: r.get(1)?,
                title: r.get(2)?,
                source_url: r.get(3)?,
                requester_name: r.get(4)?,
                added_by: r.get(5)?,
                started_at: r.get(6)?,
                ended_at: r.get(7)?,
                from_s: r.get(8)?,
                to_s: r.get(9)?,
            })
        })?
        .collect()
    }

    pub fn clear(&self, channel_id: i64) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM air_journal WHERE channel_id = ?", [channel_id])
    }

    /// The newest row without an end (airing now, or left open when the program stopped).
    pub fn newest_open(&self, channel_id: i64) -> rusqlite::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM air_journal WHERE channel_id = ? AND ended_at IS NULL
                 ORDER BY id DESC LIMIT 1",
                [channel_id],
                |r| r.get(0),
            )
            .optional()
    }
}

/// Local date and time of Unix ms, `2026-11-06 20:15:03` (empty for none).
fn local_time(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// A CSV field: quoted when needed; a leading `=`, `+`, `-`, `@` (a formula in a spreadsheet)
/// gets an apostrophe, since titles and nicks come from viewers.
fn csv_field(text: &str) -> String {
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_string()
    };
    if text.contains([',', '"', '\n', '\r', ';']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn seconds(v: Option<f64>) -> String {
    v.map(|s| format!("{s:.1}")).unwrap_or_default()
}

/// The journal as CSV (oldest first, local times, a header line).
pub fn journal_csv(entries: &[JournalEntry]) -> String {
    let mut out =
        String::from("started,ended,duration_s,title,link,requested_by,added_by,from_s,to_s\r\n");
    for e in entries.iter().rev() {
        let duration = e
            .ended_at
            .map(|end| format!("{:.0}", (end - e.started_at).max(0) as f64 / 1000.0))
            .unwrap_or_default();
        let fields = [
            local_time(Some(e.started_at)),
            local_time(e.ended_at),
            duration,
            csv_field(&e.title),
            csv_field(&e.source_url),
            csv_field(e.requester_name.as_deref().unwrap_or("")),
            e.added_by.clone(),
            seconds(e.from_s),
            seconds(e.to_s),
        ];
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

/// The journal as JSON (oldest first), with local times beside the Unix ms.
pub fn journal_json(entries: &[JournalEntry]) -> serde_json::Value {
    serde_json::Value::Array(
        entries
            .iter()
            .rev()
            .map(|e| {
                let mut v = serde_json::to_value(e).unwrap_or_default();
                v["started"] = local_time(Some(e.started_at)).into();
                v["ended"] = e.ended_at.map(|t| local_time(Some(t))).into();
                v
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};

    #[test]
    fn rows_open_close_list_and_export() {
        let conn = open_database(std::path::Path::new(":memory:")).unwrap();
        let item = QueueRepo::new(&conn)
            .add(&NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url: "https://youtu.be/x".into(),
                title: "=SUM(A1), \"quoted\"".into(),
                duration_s: Some(60.0),
                is_live: false,
                status: ItemStatus::Queued,
                added_by: AddedBy::Public,
                requester_name: Some("Kasia".into()),
                requester_ip: None,
                requester_key_hash: None,
            })
            .unwrap();
        let j = JournalRepo::new(&conn);
        let a = j
            .open(MAIN_CHANNEL_ID, &item, 1_000_000, Some(0.0))
            .unwrap();
        assert_eq!(j.newest_open(MAIN_CHANNEL_ID).unwrap(), Some(a));
        j.close(a, 1_061_000, Some(60.0)).unwrap();
        let b = j
            .open(MAIN_CHANNEL_ID, &item, 1_100_000, Some(10.0))
            .unwrap();
        let rows = j.list(MAIN_CHANNEL_ID, 10).unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [b, a]);
        assert_eq!(rows[1].ended_at, Some(1_061_000));
        assert_eq!(rows[1].added_by, "public");
        let csv = journal_csv(&rows);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(
            lines[1].contains(
                ",61,\"'=SUM(A1), \"\"quoted\"\"\",https://youtu.be/x,Kasia,public,0.0,60.0"
            ),
            "{}",
            lines[1]
        );
        assert!(lines[2].ends_with(",public,10.0,"), "{}", lines[2]);
        let json = journal_json(&rows);
        assert_eq!(json[0]["id"], a);
        assert_eq!(json[1]["ended"], serde_json::Value::Null);
        assert_eq!(j.clear(MAIN_CHANNEL_ID).unwrap(), 2);
    }
}
