//! The audio and subtitle streams of each item (kept when the film is analysed, so the panel
//! lists them before and while it plays) and the owner's choice among them.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::pipeline::probe::StreamTracks;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS item_tracks (
  item_id INTEGER PRIMARY KEY,
  streams TEXT,
  audio_track INTEGER,
  subtitle_track INTEGER
);
";

/// What is kept for one item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemTracks {
    /// The streams of the last analysis (`None`: not analysed yet).
    pub streams: Option<StreamTracks>,
    /// The owner's audio stream (`None`: automatic).
    pub audio_track: Option<i64>,
    /// The owner's subtitle stream (`-1`: off; `None`: automatic).
    pub subtitle_track: Option<i64>,
}

/// Keeps the streams of the item's latest analysis (the owner's choice stays).
pub fn save_streams(
    conn: &Connection,
    item_id: i64,
    streams: &StreamTracks,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(streams).unwrap_or_default();
    conn.execute(
        "INSERT INTO item_tracks (item_id, streams) VALUES (?1, ?2)
         ON CONFLICT(item_id) DO UPDATE SET streams = excluded.streams",
        params![item_id, json],
    )?;
    Ok(())
}

/// Stores the owner's choice for the item (`None`: automatic).
pub fn set_choice(
    conn: &Connection,
    item_id: i64,
    audio: Option<i64>,
    subtitle: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO item_tracks (item_id, audio_track, subtitle_track) VALUES (?1, ?2, ?3)
         ON CONFLICT(item_id) DO UPDATE SET audio_track = excluded.audio_track,
           subtitle_track = excluded.subtitle_track",
        params![item_id, audio, subtitle],
    )?;
    Ok(())
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ItemTracks> {
    let streams: Option<String> = r.get(1)?;
    Ok(ItemTracks {
        streams: streams.and_then(|s| serde_json::from_str(&s).ok()),
        audio_track: r.get(2)?,
        subtitle_track: r.get(3)?,
    })
}

pub fn item_tracks(conn: &Connection, item_id: i64) -> rusqlite::Result<ItemTracks> {
    Ok(conn
        .query_row(
            "SELECT item_id, streams, audio_track, subtitle_track FROM item_tracks
             WHERE item_id = ?",
            [item_id],
            row,
        )
        .optional()?
        .unwrap_or_default())
}

/// What is kept for each of `ids` that has anything.
pub fn tracks_of(conn: &Connection, ids: &[i64]) -> rusqlite::Result<HashMap<i64, ItemTracks>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT item_id, streams, audio_track, subtitle_track FROM item_tracks
             WHERE item_id IN ({marks})"
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk), |r| Ok((r.get(0)?, row(r)?)))?;
        for r in rows {
            let (id, t) = r?;
            out.insert(id, t);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::probe::AudioTrack;

    #[test]
    fn tracks_streams_and_choice_are_kept_apart() {
        let conn = crate::db::open_database(std::path::Path::new(":memory:")).unwrap();
        assert_eq!(item_tracks(&conn, 5).unwrap(), ItemTracks::default());
        set_choice(&conn, 5, Some(1), Some(-1)).unwrap();
        let streams = StreamTracks {
            audio: vec![AudioTrack {
                index: 0,
                lang: "pol".into(),
                ..AudioTrack::default()
            }],
            subtitles: Vec::new(),
        };
        save_streams(&conn, 5, &streams).unwrap();
        save_streams(&conn, 6, &StreamTracks::default()).unwrap();
        let t = item_tracks(&conn, 5).unwrap();
        assert_eq!(t.streams.as_ref(), Some(&streams));
        assert_eq!((t.audio_track, t.subtitle_track), (Some(1), Some(-1)));
        set_choice(&conn, 5, None, Some(2)).unwrap();
        let all = tracks_of(&conn, &[5, 6, 7]).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[&5].streams.as_ref(), Some(&streams));
        assert_eq!(
            (all[&5].audio_track, all[&5].subtitle_track),
            (None, Some(2))
        );
        assert_eq!(all[&6].audio_track, None);
    }
}
