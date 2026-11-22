//! Stremio-compatible addons the owner added (`addons`): the manifest as stored by
//! [`crate::addons::Manifest`] (normalised JSON), the order and whether each one is on.

use rusqlite::{Connection, OptionalExtension, params};

use super::now_ms;
use crate::addons::Manifest;

/// The tables, created by [`super::migrate`]. `addon_flags` remembers that Cinemeta was seeded,
/// so removing it sticks.
pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS addons (
  id INTEGER PRIMARY KEY,
  manifest_url TEXT UNIQUE NOT NULL,
  manifest_json TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  position INTEGER NOT NULL,
  added_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS addon_flags (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
";

/// The catalog addon every install starts with (the owner may switch it off or remove it).
pub const CINEMETA_URL: &str = "https://v3-cinemeta.strem.io/manifest.json";
const SEEDED_FLAG: &str = "addons_seeded";

#[derive(Debug, Clone, PartialEq)]
pub struct AddonRow {
    pub id: i64,
    pub manifest_url: String,
    pub manifest: Manifest,
    pub enabled: bool,
    pub position: i64,
}

/// Why [`AddonRepo::add`] failed.
#[derive(Debug)]
pub enum AddError {
    /// The manifest URL is already in the list.
    Exists,
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for AddError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

pub struct AddonRepo<'a> {
    conn: &'a Connection,
}

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<AddonRow> {
    let json: String = r.get(2)?;
    let manifest = serde_json::from_str(&json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(AddonRow {
        id: r.get(0)?,
        manifest_url: r.get(1)?,
        manifest,
        enabled: r.get::<_, i64>(3)? != 0,
        position: r.get(4)?,
    })
}

const COLUMNS: &str = "id, manifest_url, manifest_json, enabled, position";

impl<'a> AddonRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// All addons in the owner's order. A row whose stored manifest cannot be read is skipped.
    pub fn list(&self) -> rusqlite::Result<Vec<AddonRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM addons ORDER BY position, id"
        ))?;
        let rows = stmt.query_map([], |r| Ok(row_of(r).ok()))?;
        let mut out = Vec::new();
        for row in rows {
            match row? {
                Some(a) => out.push(a),
                None => eprintln!("addons: skipping a row with an unreadable manifest"),
            }
        }
        Ok(out)
    }

    /// The addons that are switched on, in order.
    pub fn enabled(&self) -> rusqlite::Result<Vec<AddonRow>> {
        Ok(self.list()?.into_iter().filter(|a| a.enabled).collect())
    }

    pub fn get(&self, id: i64) -> rusqlite::Result<Option<AddonRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM addons WHERE id = ?"),
                [id],
                row_of,
            )
            .optional()
    }

    pub fn exists(&self, manifest_url: &str) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM addons WHERE manifest_url = ?)",
            [manifest_url],
            |r| r.get(0),
        )
    }

    /// Appends an addon (on). A URL already there is [`AddError::Exists`].
    pub fn add(&self, manifest_url: &str, manifest_json: &str) -> Result<i64, AddError> {
        let tx = self.conn.unchecked_transaction()?;
        let inserted = tx.execute(
            "INSERT INTO addons (manifest_url, manifest_json, enabled, position, added_at)
             VALUES (?, ?, 1, (SELECT COALESCE(MAX(position), 0) + 1 FROM addons), ?)",
            params![manifest_url, manifest_json, now_ms()],
        );
        match inserted {
            Ok(_) => {
                let id = tx.last_insert_rowid();
                tx.commit()?;
                Ok(id)
            }
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(AddError::Exists)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_enabled(&self, id: i64, enabled: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE addons SET enabled = ? WHERE id = ?",
            params![enabled, id],
        )?;
        Ok(())
    }

    /// Swaps the addon with its neighbour above or below (nothing at the ends).
    pub fn move_(&self, id: i64, up: bool) -> rusqlite::Result<()> {
        let all = self.list()?;
        let Some(at) = all.iter().position(|a| a.id == id) else {
            return Ok(());
        };
        let other = if up {
            at.checked_sub(1)
        } else {
            Some(at + 1).filter(|n| *n < all.len())
        };
        let Some(other) = other else { return Ok(()) };
        let tx = self.conn.unchecked_transaction()?;
        for (row, position) in [
            (&all[at], all[other].position),
            (&all[other], all[at].position),
        ] {
            tx.execute(
                "UPDATE addons SET position = ? WHERE id = ?",
                params![position, row.id],
            )?;
        }
        tx.commit()
    }

    pub fn update_manifest(&self, id: i64, manifest_json: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE addons SET manifest_json = ? WHERE id = ?",
            params![manifest_json, id],
        )?;
        Ok(())
    }

    pub fn remove(&self, id: i64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM addons WHERE id = ?", [id])? > 0)
    }

    /// Whether Cinemeta was seeded already (even if the owner removed it since).
    pub fn seeded(&self) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM addon_flags WHERE key = ?)",
            [SEEDED_FLAG],
            |r| r.get(0),
        )
    }

    /// Stores Cinemeta and sets the flag, once: false when it was seeded before.
    pub fn seed_cinemeta_once(&self, manifest_json: &str) -> rusqlite::Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let flagged = tx.execute(
            "INSERT OR IGNORE INTO addon_flags (key, value) VALUES (?, '1')",
            [SEEDED_FLAG],
        )?;
        if flagged == 0 {
            return Ok(false);
        }
        tx.execute(
            "INSERT OR IGNORE INTO addons (manifest_url, manifest_json, enabled, position, added_at)
             VALUES (?, ?, 1, (SELECT COALESCE(MAX(position), 0) + 1 FROM addons), ?)",
            params![CINEMETA_URL, manifest_json, now_ms()],
        )?;
        tx.commit()?;
        Ok(true)
    }
}
