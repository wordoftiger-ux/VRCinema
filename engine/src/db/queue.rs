//! The channel queue (`queue_items` table).

use super::now_ms;
use rusqlite::types::{Type, Value};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    PendingApproval,
    Queued,
    Playing,
    Played,
    Failed,
    Removed,
    Rejected,
}

impl ItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingApproval => "pending_approval",
            Self::Queued => "queued",
            Self::Playing => "playing",
            Self::Played => "played",
            Self::Failed => "failed",
            Self::Removed => "removed",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending_approval" => Self::PendingApproval,
            "queued" => Self::Queued,
            "playing" => Self::Playing,
            "played" => Self::Played,
            "failed" => Self::Failed,
            "removed" => Self::Removed,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }

    /// Statuses that set `finished_at`.
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Played | Self::Failed | Self::Removed | Self::Rejected
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddedBy {
    Admin,
    Public,
}

impl AddedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Public => "public",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Self::Admin),
            "public" => Some(Self::Public),
            _ => None,
        }
    }
}

/// Statuses that count towards a requester's active items.
pub const ACTIVE_STATUSES: [ItemStatus; 3] = [
    ItemStatus::PendingApproval,
    ItemStatus::Queued,
    ItemStatus::Playing,
];
const FINISHED_STATUSES: [ItemStatus; 2] = [ItemStatus::Played, ItemStatus::Failed];

#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    pub id: i64,
    pub channel_id: i64,
    pub position: i64,
    pub source_url: String,
    pub title: String,
    pub duration_s: Option<f64>,
    pub is_live: bool,
    pub status: ItemStatus,
    pub added_by: AddedBy,
    pub requester_name: Option<String>,
    pub requester_ip: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewItem {
    pub channel_id: i64,
    pub source_url: String,
    pub title: String,
    pub duration_s: Option<f64>,
    pub is_live: bool,
    pub status: ItemStatus,
    pub added_by: AddedBy,
    pub requester_name: Option<String>,
    pub requester_ip: Option<String>,
}

fn bad_value(idx: usize, what: &str, got: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        idx,
        Type::Text,
        format!("nieznana wartość {what}: {got}").into(),
    )
}

fn to_item(r: &Row<'_>) -> rusqlite::Result<QueueItem> {
    let status: String = r.get("status")?;
    let added_by: String = r.get("added_by")?;
    Ok(QueueItem {
        id: r.get("id")?,
        channel_id: r.get("channel_id")?,
        position: r.get("position")?,
        source_url: r.get("source_url")?,
        title: r.get("title")?,
        duration_s: r.get("duration_s")?,
        is_live: r.get::<_, i64>("is_live")? == 1,
        status: ItemStatus::parse(&status).ok_or_else(|| {
            bad_value(
                r.as_ref().column_index("status").unwrap_or(0),
                "status",
                &status,
            )
        })?,
        added_by: AddedBy::parse(&added_by).ok_or_else(|| {
            bad_value(
                r.as_ref().column_index("added_by").unwrap_or(0),
                "added_by",
                &added_by,
            )
        })?,
        requester_name: r.get("requester_name")?,
        requester_ip: r.get("requester_ip")?,
        error: r.get("error")?,
        created_at: r.get("created_at")?,
        started_at: r.get("started_at")?,
        finished_at: r.get("finished_at")?,
    })
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

pub struct QueueRepo<'a> {
    conn: &'a Connection,
}

impl<'a> QueueRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Appends the item at the end of the channel's queue (MAX(position) + 1).
    pub fn add(&self, item: &NewItem) -> rusqlite::Result<QueueItem> {
        let tx = self.conn.unchecked_transaction()?;
        let max: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), 0) AS m FROM queue_items WHERE channel_id = ?",
            [item.channel_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO queue_items (channel_id, position, source_url, title, duration_s, is_live, status,
               added_by, requester_name, requester_ip, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                item.channel_id,
                max + 1,
                item.source_url,
                item.title,
                item.duration_s,
                i64::from(item.is_live),
                item.status.as_str(),
                item.added_by.as_str(),
                item.requester_name,
                item.requester_ip,
                now_ms(),
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.get(id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    /// Adds the item at the front of the channel's queue, in one transaction (nothing is left
    /// behind when it fails).
    pub fn add_first(&self, item: &NewItem) -> rusqlite::Result<QueueItem> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO queue_items (channel_id, position, source_url, title, duration_s, is_live, status,
               added_by, requester_name, requester_ip, created_at)
             VALUES (?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                item.channel_id,
                item.source_url,
                item.title,
                item.duration_s,
                i64::from(item.is_live),
                item.status.as_str(),
                item.added_by.as_str(),
                item.requester_name,
                item.requester_ip,
                now_ms(),
            ],
        )?;
        let id = tx.last_insert_rowid();
        let others = QueueRepo::new(&tx).list_by_status(item.channel_id, &[ItemStatus::Queued])?;
        let ids: Vec<i64> = std::iter::once(id)
            .chain(others.iter().map(|i| i.id).filter(|&i| i != id))
            .collect();
        renumber_in(&tx, &ids)?;
        tx.commit()?;
        self.get(id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn get(&self, id: i64) -> rusqlite::Result<Option<QueueItem>> {
        self.conn
            .query_row("SELECT * FROM queue_items WHERE id = ?", [id], to_item)
            .optional()
    }

    pub fn list_by_status(
        &self,
        channel_id: i64,
        statuses: &[ItemStatus],
    ) -> rusqlite::Result<Vec<QueueItem>> {
        let sql = format!(
            "SELECT * FROM queue_items WHERE channel_id = ? AND status IN ({})
             ORDER BY position, id",
            placeholders(statuses.len())
        );
        let args = std::iter::once(Value::Integer(channel_id))
            .chain(statuses.iter().map(|s| Value::Text(s.as_str().to_string())));
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map(params_from_iter(args), to_item)?.collect()
    }

    pub fn next_queued(&self, channel_id: i64) -> rusqlite::Result<Option<QueueItem>> {
        Ok(self
            .list_by_status(channel_id, &[ItemStatus::Queued])?
            .into_iter()
            .next())
    }

    /// Sets the status and error; `started_at` is set (kept if already set) for
    /// `playing`, `finished_at` for played/failed/removed/rejected.
    pub fn set_status(
        &self,
        id: i64,
        status: ItemStatus,
        error: Option<&str>,
    ) -> rusqlite::Result<()> {
        let now = now_ms();
        let started = (status == ItemStatus::Playing).then_some(now);
        let finished = status.is_terminal().then_some(now);
        self.conn.execute(
            "UPDATE queue_items SET status = ?, error = ?,
               started_at = COALESCE(?, started_at), finished_at = COALESCE(?, finished_at)
             WHERE id = ?",
            params![status.as_str(), error, started, finished, id],
        )?;
        Ok(())
    }

    /// The channel starts airing `id`: `queued` → `playing` only. Returns `false` (nothing
    /// changed) when the item is no longer queued, e.g. removed after it was read as the head.
    pub fn start_playing(&self, id: i64) -> rusqlite::Result<bool> {
        let changed = self.conn.execute(
            "UPDATE queue_items SET status = 'playing', error = NULL,
               started_at = COALESCE(started_at, ?)
             WHERE id = ? AND status = 'queued'",
            params![now_ms(), id],
        )?;
        Ok(changed > 0)
    }

    /// The channel ends `id` with `status` (played/failed). Only an item still `playing` or
    /// `queued` changes, so an owner's removal is never overwritten. Returns whether it changed.
    pub fn finish(
        &self,
        id: i64,
        status: ItemStatus,
        error: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let finished = status.is_terminal().then(now_ms);
        let changed = self.conn.execute(
            "UPDATE queue_items SET status = ?, error = ?, finished_at = COALESCE(?, finished_at)
             WHERE id = ? AND status IN ('playing', 'queued')",
            params![status.as_str(), error, finished, id],
        )?;
        Ok(changed > 0)
    }

    /// Moves an item within the queued list to `to_index` (clamped) and
    /// renumbers queued items 1..n. Unknown ids are ignored.
    pub fn move_item(&self, id: i64, to_index: usize) -> rusqlite::Result<()> {
        let Some(item) = self.get(id)? else {
            return Ok(());
        };
        let mut ids: Vec<i64> = self
            .list_by_status(item.channel_id, &[ItemStatus::Queued])?
            .into_iter()
            .map(|i| i.id)
            .filter(|&i| i != id)
            .collect();
        ids.insert(to_index.min(ids.len()), id);
        self.renumber(&ids)
    }

    /// An item left "playing" by a restart goes back to the front of the queue.
    pub fn reset_playing(&self, channel_id: i64) -> rusqlite::Result<()> {
        let playing = self.list_by_status(channel_id, &[ItemStatus::Playing])?;
        if playing.is_empty() {
            return Ok(());
        }
        let queued = self.list_by_status(channel_id, &[ItemStatus::Queued])?;
        let tx = self.conn.unchecked_transaction()?;
        for p in &playing {
            tx.execute(
                "UPDATE queue_items SET status = 'queued' WHERE id = ?",
                [p.id],
            )?;
        }
        let ids: Vec<i64> = playing.iter().chain(queued.iter()).map(|i| i.id).collect();
        renumber_in(&tx, &ids)?;
        tx.commit()
    }

    pub fn count_active_by_ip(&self, channel_id: i64, ip: &str) -> rusqlite::Result<i64> {
        let sql = format!(
            "SELECT COUNT(*) AS n FROM queue_items WHERE channel_id = ? AND requester_ip = ?
             AND status IN ({})",
            placeholders(ACTIVE_STATUSES.len())
        );
        let args = [Value::Integer(channel_id), Value::Text(ip.to_string())]
            .into_iter()
            .chain(
                ACTIVE_STATUSES
                    .iter()
                    .map(|s| Value::Text(s.as_str().to_string())),
            );
        self.conn
            .query_row(&sql, params_from_iter(args), |r| r.get(0))
    }

    /// Finished (played + failed) items, newest first.
    pub fn history(&self, channel_id: i64, limit: u32) -> rusqlite::Result<Vec<QueueItem>> {
        let sql = format!(
            "SELECT * FROM queue_items WHERE channel_id = ? AND status IN ({})
             ORDER BY finished_at DESC, id DESC LIMIT ?",
            placeholders(FINISHED_STATUSES.len())
        );
        let args = std::iter::once(Value::Integer(channel_id))
            .chain(
                FINISHED_STATUSES
                    .iter()
                    .map(|s| Value::Text(s.as_str().to_string())),
            )
            .chain(std::iter::once(Value::Integer(i64::from(limit))));
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map(params_from_iter(args), to_item)?.collect()
    }

    fn renumber(&self, ids: &[i64]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        renumber_in(&tx, ids)?;
        tx.commit()
    }
}

fn renumber_in(conn: &Connection, ids: &[i64]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("UPDATE queue_items SET position = ? WHERE id = ?")?;
    for (i, id) in ids.iter().enumerate() {
        // `i` is bounded by the number of rows; the cast cannot overflow in practice.
        stmt.execute(params![i64::try_from(i + 1).unwrap_or(i64::MAX), id])?;
    }
    Ok(())
}
