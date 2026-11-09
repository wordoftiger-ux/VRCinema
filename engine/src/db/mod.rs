//! SQLite persistence: schema, queue and per-channel settings.
//!
//! The schema, pragmas and the seeded "main" channel are identical to the
//! TypeScript version (`src/db/database.ts`), so a database file created by
//! either implementation can be opened by the other.

pub mod journal;
pub mod lists;
pub mod queue;
pub mod settings;
pub mod thumbs;

use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub use lists::{ListRepo, PendingList};
pub use queue::{ACTIVE_STATUSES, AddedBy, ItemStatus, NewItem, QueueItem, QueueRepo, Requester};
pub use settings::{
    ChannelSettings, LinkMode, SettingsPatch, SettingsRepo, sanitize_settings_patch,
};
pub use thumbs::{is_public_thumbnail, set_thumbnail, thumbnails, youtube_thumbnail};

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
CREATE TABLE IF NOT EXISTS nick_keys (
  key_hash TEXT PRIMARY KEY,
  skeleton TEXT NOT NULL,
  added_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS nick_keys_skeleton ON nick_keys(skeleton);
";

/// Brings an opened database up to the current schema: new tables and columns only, so no
/// row is lost and the old server can still read the file.
pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(MIGRATIONS)?;
    conn.execute_batch(lists::SCHEMA)?;
    conn.execute_batch(journal::SCHEMA)?;
    conn.execute_batch(thumbs::SCHEMA)?;
    add_column_if_missing(conn, "queue_items", "requester_key_hash", "TEXT")?;
    // In and out points (film seconds) and the start time (Unix ms) of an item.
    add_column_if_missing(conn, "queue_items", "in_s", "REAL")?;
    add_column_if_missing(conn, "queue_items", "out_s", "REAL")?;
    add_column_if_missing(conn, "queue_items", "start_at", "INTEGER")?;
    // The nick a blocked device used last (the block outlives the nick).
    add_column_if_missing(conn, "blocked_devices", "nick", "TEXT")?;
    // A protected nick: Argon2 PHC of its PIN and SHA-256 of its recovery code.
    add_column_if_missing(conn, "viewer_nicks", "pin_hash", "TEXT")?;
    add_column_if_missing(conn, "viewer_nicks", "recovery_hash", "TEXT")
}

fn has_table(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
        [name],
        |r| r.get(0),
    )
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
    let existed = has_table(&conn, "channels")?;
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
    // A database from before the link modes keeps the rule it had (its allowed sites); a new
    // one lets viewers send any public site. Stored either way, so the next start leaves it.
    let mode = if existed { "allowlist" } else { "all" };
    conn.execute(
        "INSERT OR IGNORE INTO settings (channel_id, key, value) VALUES (?, 'linkMode', ?)",
        params![MAIN_CHANNEL_ID, format!("\"{mode}\"")],
    )?;
    Ok(conn)
}

#[cfg(test)]
mod tests;
