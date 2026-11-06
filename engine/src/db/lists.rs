//! Viewer lists (`request_lists`, `request_list_items`): which queue items one list request
//! added, so the panel can show them as one group and approve or reject them together. The
//! items themselves are ordinary rows of `queue_items`.

use rusqlite::{Connection, params};

use super::now_ms;

/// The tables, created by [`super::migrate`].
pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS request_lists (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel_id INTEGER NOT NULL,
  title TEXT,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS request_list_items (
  item_id INTEGER PRIMARY KEY,
  list_id INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS request_list_items_list ON request_list_items(list_id);
";

/// A list with items still waiting for approval.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingList {
    pub id: i64,
    /// The playlist's title when the list was one playlist.
    pub title: Option<String>,
    /// The waiting items, in queue order.
    pub item_ids: Vec<i64>,
}

pub struct ListRepo<'a> {
    conn: &'a Connection,
}

impl<'a> ListRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Stores a list of `item_ids` and returns its id.
    pub fn create(
        &self,
        channel_id: i64,
        title: Option<&str>,
        item_ids: &[i64],
    ) -> rusqlite::Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO request_lists (channel_id, title, created_at) VALUES (?, ?, ?)",
            params![channel_id, title, now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO request_list_items (item_id, list_id) VALUES (?, ?)",
            )?;
            for item in item_ids {
                stmt.execute(params![item, id])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }

    /// The channel's lists with at least `min_items` items waiting for approval, oldest first.
    pub fn pending(&self, channel_id: i64, min_items: usize) -> rusqlite::Result<Vec<PendingList>> {
        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.title, q.id FROM request_lists l
               JOIN request_list_items li ON li.list_id = l.id
               JOIN queue_items q ON q.id = li.item_id
             WHERE l.channel_id = ? AND q.channel_id = l.channel_id
               AND q.status = 'pending_approval'
             ORDER BY l.id, q.position, q.id",
        )?;
        let rows = stmt.query_map([channel_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let mut lists: Vec<PendingList> = Vec::new();
        for row in rows {
            let (id, title, item) = row?;
            match lists.last_mut() {
                Some(last) if last.id == id => last.item_ids.push(item),
                _ => lists.push(PendingList {
                    id,
                    title,
                    item_ids: vec![item],
                }),
            }
        }
        lists.retain(|l| l.item_ids.len() >= min_items);
        Ok(lists)
    }

    /// The items of list `id` of the channel still waiting for approval.
    pub fn pending_items(&self, channel_id: i64, id: i64) -> rusqlite::Result<Vec<i64>> {
        Ok(self
            .pending(channel_id, 1)?
            .into_iter()
            .find(|l| l.id == id)
            .map(|l| l.item_ids)
            .unwrap_or_default())
    }

    /// Whether list `id` belongs to the channel.
    pub fn exists(&self, channel_id: i64, id: i64) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM request_lists WHERE id = ? AND channel_id = ?)",
            params![id, channel_id],
            |r| r.get(0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};
    use std::path::Path;

    fn add(conn: &Connection, status: ItemStatus) -> i64 {
        QueueRepo::new(conn)
            .add(&NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url: "https://youtu.be/x".into(),
                title: "x".into(),
                duration_s: None,
                is_live: false,
                status,
                added_by: AddedBy::Public,
                requester_name: None,
                requester_ip: Some("1.1.1.1".into()),
                requester_key_hash: None,
            })
            .unwrap()
            .id
    }

    #[test]
    fn a_list_shows_only_while_enough_of_its_items_wait() {
        let conn = open_database(Path::new(":memory:")).unwrap();
        let a = add(&conn, ItemStatus::PendingApproval);
        let b = add(&conn, ItemStatus::PendingApproval);
        let c = add(&conn, ItemStatus::Queued);
        let repo = ListRepo::new(&conn);
        let id = repo
            .create(MAIN_CHANNEL_ID, Some("PL"), &[a, b, c])
            .unwrap();
        assert!(repo.exists(MAIN_CHANNEL_ID, id).unwrap());
        assert!(!repo.exists(MAIN_CHANNEL_ID + 1, id).unwrap());
        assert_eq!(
            repo.pending(MAIN_CHANNEL_ID, 2).unwrap(),
            vec![PendingList {
                id,
                title: Some("PL".into()),
                item_ids: vec![a, b]
            }]
        );
        QueueRepo::new(&conn)
            .set_status(a, ItemStatus::Queued, None)
            .unwrap();
        assert!(repo.pending(MAIN_CHANNEL_ID, 2).unwrap().is_empty());
        assert_eq!(repo.pending_items(MAIN_CHANNEL_ID, id).unwrap(), vec![b]);
    }
}
