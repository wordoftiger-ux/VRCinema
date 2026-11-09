//! The pictures of the films in the panel's lists: the site's picture (yt-dlp's `thumbnail`,
//! kept when a film is resolved), a picture derived from a YouTube link, or for a file of the
//! owner's disk or a viewer's sent file a frame the engine makes itself
//! ([`crate::http`]'s `/api/admin/items/<id>/thumb`). The engine never fetches a site's
//! picture: the panel's browser loads it (without a referrer).

use std::collections::HashMap;

use rusqlite::{Connection, params_from_iter};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS item_thumbnails (
  item_id INTEGER PRIMARY KEY,
  url TEXT NOT NULL
);
";

/// Longest site picture address kept.
const MAX_URL_LEN: usize = 2048;

/// An address the owner's browser may load as a picture: `https`, a host name with a dot that
/// is not an IP address (never the owner's network or this computer), no user name, at most
/// [`MAX_URL_LEN`] long.
pub fn is_public_thumbnail(url: &str) -> bool {
    if url.len() > MAX_URL_LEN {
        return false;
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.to_ascii_lowercase();
            host.contains('.')
                && !host.ends_with(".local")
                && !host.ends_with(".lan")
                && !host.ends_with(".home")
                && !host.ends_with(".internal")
                && !host.ends_with(".localhost")
        }
        _ => false,
    }
}

/// The picture servers of the big video sites. A viewer's film keeps its site's picture only
/// from one of these: a viewer's own server named as the picture would otherwise learn the
/// owner's address when the panel shows it.
const KNOWN_PICTURE_HOSTS: [&str; 12] = [
    "ytimg.com",
    "ggpht.com",
    "googleusercontent.com",
    "sndcdn.com",
    "vimeocdn.com",
    "twimg.com",
    "dmcdn.net",
    "jtvnw.net",
    "hdslb.com",
    "nimg.jp",
    "tiktokcdn.com",
    "tiktokcdn-us.com",
];

/// A picture of one of [`KNOWN_PICTURE_HOSTS`] (or their subdomains), public as above.
pub fn is_known_site_picture(url: &str) -> bool {
    if !is_public_thumbnail(url) {
        return false;
    }
    let Some(host) = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
    else {
        return false;
    };
    KNOWN_PICTURE_HOSTS
        .iter()
        .any(|known| host == *known || host.ends_with(&format!(".{known}")))
}

/// Keeps the site's picture of an item (replacing an older one).
pub fn set_thumbnail(conn: &Connection, item_id: i64, url: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO item_thumbnails (item_id, url) VALUES (?1, ?2)
         ON CONFLICT(item_id) DO UPDATE SET url = excluded.url",
        rusqlite::params![item_id, url],
    )?;
    // Pictures of items that are gone.
    conn.execute(
        "DELETE FROM item_thumbnails WHERE item_id NOT IN (SELECT id FROM queue_items)",
        [],
    )?;
    Ok(())
}

/// The kept site pictures of `ids`.
pub fn thumbnails(conn: &Connection, ids: &[i64]) -> rusqlite::Result<HashMap<i64, String>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT item_id, url FROM item_thumbnails WHERE item_id IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, url) = row?;
            out.insert(id, url);
        }
    }
    Ok(out)
}

/// The picture YouTube shows for a video of `source` (a watch, short, live, embed or youtu.be
/// link), from its id alone: nothing is fetched to know it.
pub fn youtube_thumbnail(source: &str) -> Option<String> {
    let url = url::Url::parse(source).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    let id = match host {
        "youtu.be" => url.path_segments()?.next().map(str::to_string),
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            let mut segments = url.path_segments()?;
            match segments.next()? {
                "watch" => url
                    .query_pairs()
                    .find(|(k, _)| k == "v")
                    .map(|(_, v)| v.into_owned()),
                "shorts" | "live" | "embed" | "v" => segments.next().map(str::to_string),
                _ => None,
            }
        }
        _ => None,
    }?;
    let valid = id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then(|| format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_addresses_of_named_public_hosts_are_kept() {
        assert!(is_public_thumbnail("https://i.ytimg.com/vi/x/hq.jpg"));
        assert!(is_public_thumbnail(
            "https://i1.sndcdn.com/a-t500x500.jpg?x=1"
        ));
        for bad in [
            "http://i.ytimg.com/a.jpg",
            "https://192.168.1.1/a.jpg",
            "https://[::1]/a.jpg",
            "https://localhost/a.jpg",
            "https://router/a.jpg",
            "https://nas.local/a.jpg",
            "https://user@cdn.example/a.jpg",
            "file:///C:/a.jpg",
            "data:image/png;base64,AAAA",
            "not a link",
        ] {
            assert!(!is_public_thumbnail(bad), "{bad}");
        }
        assert!(is_known_site_picture("https://i.ytimg.com/vi/x/hq.jpg"));
        assert!(is_known_site_picture("https://i1.sndcdn.com/a.jpg"));
        assert!(!is_known_site_picture("https://viewer.example/a.jpg"));
        assert!(!is_known_site_picture(
            "https://ytimg.com.viewer.example/a.jpg"
        ));
        assert!(!is_known_site_picture("https://evilytimg.com/a.jpg"));
        assert!(!is_known_site_picture("http://i.ytimg.com/a.jpg"));
        assert!(!is_public_thumbnail(&format!(
            "https://cdn.example/{}",
            "a".repeat(MAX_URL_LEN)
        )));
    }

    #[test]
    fn youtube_links_give_their_picture_from_the_id() {
        let pic = |id: &str| Some(format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"));
        assert_eq!(
            youtube_thumbnail("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1"),
            pic("dQw4w9WgXcQ")
        );
        assert_eq!(
            youtube_thumbnail("https://youtu.be/dQw4w9WgXcQ"),
            pic("dQw4w9WgXcQ")
        );
        assert_eq!(
            youtube_thumbnail("https://m.youtube.com/shorts/abcdefghijk"),
            pic("abcdefghijk")
        );
        assert_eq!(
            youtube_thumbnail("https://www.youtube.com/playlist?list=PL1"),
            None
        );
        assert_eq!(youtube_thumbnail("https://youtu.be/short"), None);
        assert_eq!(youtube_thumbnail("https://vimeo.com/123"), None);
        assert_eq!(
            youtube_thumbnail("https://www.youtube.com/watch?v=../../x/y"),
            None
        );
    }

    #[test]
    fn pictures_are_kept_per_item_and_go_with_their_items() {
        let conn = crate::db::open_database(std::path::Path::new(":memory:")).unwrap();
        let q = crate::db::QueueRepo::new(&conn);
        let item = |url: &str| crate::db::NewItem {
            channel_id: 1,
            source_url: url.into(),
            title: "t".into(),
            duration_s: None,
            is_live: false,
            status: crate::db::ItemStatus::Queued,
            added_by: crate::db::AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            requester_key_hash: None,
        };
        let a = q.add(&item("https://a.example/1")).unwrap();
        let b = q.add(&item("https://a.example/2")).unwrap();
        set_thumbnail(&conn, a.id, "https://cdn.example/a.jpg").unwrap();
        set_thumbnail(&conn, a.id, "https://cdn.example/a2.jpg").unwrap();
        let got = thumbnails(&conn, &[a.id, b.id]).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[&a.id], "https://cdn.example/a2.jpg");
        conn.execute("DELETE FROM queue_items WHERE id = ?", [a.id])
            .unwrap();
        set_thumbnail(&conn, b.id, "https://cdn.example/b.jpg").unwrap();
        let got = thumbnails(&conn, &[a.id, b.id]).unwrap();
        assert_eq!(got.keys().copied().collect::<Vec<_>>(), vec![b.id]);
    }
}
