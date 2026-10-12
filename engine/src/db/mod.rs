//! SQLite persistence: schema, queue and per-channel settings.
//!
//! The schema, pragmas and the seeded "main" channel are identical to the
//! TypeScript version (`src/db/database.ts`), so a database file created by
//! either implementation can be opened by the other.

pub mod queue;
pub mod settings;

use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub use queue::{ACTIVE_STATUSES, AddedBy, ItemStatus, NewItem, QueueItem, QueueRepo};
pub use settings::{ChannelSettings, SettingsPatch, SettingsRepo, sanitize_settings_patch};

pub const MAIN_CHANNEL_ID: i64 = 1;
pub const MAIN_CHANNEL_SLUG: &str = "main";
const MAIN_CHANNEL_NAME: &str = "Kanał główny";

const SCHEMA: &str = "
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

/// Additive changes made after the schema was shared with the TypeScript version.
const MIGRATIONS: &str = "
CREATE TABLE IF NOT EXISTS viewer_nicks (
  skeleton TEXT PRIMARY KEY,
  display TEXT NOT NULL,
  key_hash TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS viewer_nicks_key ON viewer_nicks(key_hash);
CREATE TABLE IF NOT EXISTS blocked_devices (
  key_hash TEXT PRIMARY KEY,
  blocked_at INTEGER NOT NULL
);
";

/// Brings an opened database up to the current schema: new tables and columns only, so no
/// row is lost and the old server can still read the file.
pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(MIGRATIONS)?;
    add_column_if_missing(conn, "queue_items", "requester_key_hash", "TEXT")?;
    // The nick a blocked device used last (the block outlives the nick).
    add_column_if_missing(conn, "blocked_devices", "nick", "TEXT")
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    ty: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
    let mut found = false;
    for name in names {
        found |= name? == column;
    }
    drop(stmt);
    if found {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"))
}

/// Current time as Unix milliseconds (like JS `Date.now()`).
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Opens the database (`":memory:"` in tests), creates the schema and the
/// "main" channel (id 1).
pub fn open_database(path: &Path) -> rusqlite::Result<Connection> {
    let in_memory = path.as_os_str() == ":memory:";
    if !in_memory && let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(SCHEMA)?;
    migrate(&conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO channels (id, slug, name, created_at) VALUES (?, ?, ?, ?)",
        params![
            MAIN_CHANNEL_ID,
            MAIN_CHANNEL_SLUG,
            MAIN_CHANNEL_NAME,
            now_ms()
        ],
    )?;
    Ok(conn)
}

#[cfg(test)]
mod tests;
