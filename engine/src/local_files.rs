//! Queue items from files dropped on the desktop window (`file:` source, never through HTTP).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use rusqlite::Connection;

use crate::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueItem, QueueRepo};
use crate::library::{is_video_path, natural_cmp};
use crate::resolver::MediaResolver;
use crate::resolver::file::file_source_url;

/// Outcome of a drop: the queued items and how many dropped entries were left out
/// (non-video files, unreadable or relative paths, files the resolver refused).
#[derive(Debug, Clone, PartialEq)]
pub struct AddReport {
    pub added: Vec<QueueItem>,
    pub skipped: usize,
}

/// Videos directly inside `dir` (no subfolders) in natural order, plus the count of
/// non-video files and unreadable entries. `None` when the folder cannot be read.
fn videos_in_dir(dir: &Path) -> Option<(Vec<PathBuf>, usize)> {
    let mut videos = Vec::new();
    let mut skipped = 0;
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else {
            skipped += 1;
            continue;
        };
        let path = entry.path();
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => {}
            Ok(m) if m.is_file() && is_video_path(&path) => videos.push(path),
            _ => skipped += 1,
        }
    }
    videos.sort_by(|a, b| {
        let name = |p: &PathBuf| {
            p.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        };
        natural_cmp(&name(a), &name(b))
    });
    Some((videos, skipped))
}

/// Queues dropped files (videos only) and the videos directly inside dropped folders.
/// Input order is kept; the contents of one folder are in natural order. The database lock
/// is held only for the inserts, never across an await.
pub async fn add_local_files(
    db: &Mutex<Connection>,
    resolver: &dyn MediaResolver,
    paths: Vec<PathBuf>,
) -> anyhow::Result<AddReport> {
    let mut skipped = 0;
    let mut files: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !path.is_absolute() {
            skipped += 1;
            continue;
        }
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => match videos_in_dir(&path) {
                Some((videos, rest)) => {
                    files.extend(videos);
                    skipped += rest;
                }
                None => skipped += 1,
            },
            Ok(m) if m.is_file() && is_video_path(&path) => files.push(path),
            _ => skipped += 1,
        }
    }

    let mut new_items = Vec::with_capacity(files.len());
    for file in files {
        let Some(source_url) = file_source_url(&file) else {
            skipped += 1;
            continue;
        };
        match resolver.fetch_meta(&source_url).await {
            Ok(meta) => new_items.push(NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url,
                title: meta.title,
                duration_s: meta.duration_s,
                is_live: false,
                status: ItemStatus::Queued,
                added_by: AddedBy::Admin,
                requester_name: None,
                requester_ip: None,
            }),
            Err(_) => skipped += 1,
        }
    }

    let added = {
        let conn = db.lock().unwrap_or_else(PoisonError::into_inner);
        let queue = QueueRepo::new(&conn);
        new_items
            .iter()
            .map(|item| queue.add(item))
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(AddReport { added, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_database;
    use crate::resolver::file::{FileResolver, parse_file_source_url};
    use std::fs;
    use std::sync::Arc;

    fn setup() -> (tempfile::TempDir, Arc<Mutex<Connection>>, FileResolver) {
        let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
        let resolver = FileResolver::new(Box::new(|_| Box::pin(async { Some(90.0) })));
        (tempfile::tempdir().unwrap(), db, resolver)
    }

    fn touch(p: &Path) -> PathBuf {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
        std::path::absolute(p).unwrap()
    }

    fn titles(r: &AddReport) -> Vec<&str> {
        r.added.iter().map(|i| i.title.as_str()).collect()
    }

    #[tokio::test]
    async fn adds_a_file_with_title_duration_and_file_url() {
        let (tmp, db, resolver) = setup();
        let f = touch(&tmp.path().join("Żółć 1.mkv"));
        let r = add_local_files(&db, &resolver, vec![f.clone()])
            .await
            .unwrap();
        assert_eq!(r.skipped, 0);
        let item = &r.added[0];
        assert_eq!(item.title, "Żółć 1");
        assert_eq!(item.duration_s, Some(90.0));
        assert_eq!(item.added_by, AddedBy::Admin);
        assert_eq!(item.status, ItemStatus::Queued);
        assert_eq!(item.channel_id, MAIN_CHANNEL_ID);
        assert!(!item.is_live);
        assert_eq!(parse_file_source_url(&item.source_url), Some(f));
        let stored = QueueRepo::new(&db.lock().unwrap())
            .list_by_status(MAIN_CHANNEL_ID, &[ItemStatus::Queued])
            .unwrap();
        assert_eq!(stored, r.added);
    }

    #[tokio::test]
    async fn folder_gives_its_videos_in_natural_order_without_subfolders() {
        let (tmp, db, resolver) = setup();
        let dir = tmp.path().join("Sezon");
        for n in [10, 2, 1] {
            touch(&dir.join(format!("Odcinek {n}.mkv")));
        }
        touch(&dir.join("Podfolder").join("ukryty.mp4"));
        touch(&dir.join("notatki.txt"));
        touch(&dir.join("Okładka.jpg"));
        let r = add_local_files(&db, &resolver, vec![dir]).await.unwrap();
        assert_eq!(titles(&r), ["Odcinek 1", "Odcinek 2", "Odcinek 10"]);
        assert_eq!(r.skipped, 2);
    }

    #[tokio::test]
    async fn mixed_input_keeps_given_order_and_counts_skipped() {
        let (tmp, db, resolver) = setup();
        let z = touch(&tmp.path().join("zzz.mp4"));
        let txt = touch(&tmp.path().join("a.txt"));
        let dir = tmp.path().join("F");
        touch(&dir.join("b 2.mp4"));
        touch(&dir.join("b 10.mp4"));
        let missing = std::path::absolute(tmp.path().join("nie ma.mp4")).unwrap();
        let a = touch(&tmp.path().join("aaa.mp4"));
        let r = add_local_files(
            &db,
            &resolver,
            vec![z, txt, dir, missing, PathBuf::from("wzgledna.mp4"), a],
        )
        .await
        .unwrap();
        assert_eq!(titles(&r), ["zzz", "b 2", "b 10", "aaa"]);
        assert_eq!(r.skipped, 3);
        let positions: Vec<i64> = r.added.iter().map(|i| i.position).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
    }

    #[tokio::test]
    async fn nothing_to_add_is_an_empty_report() {
        let (_tmp, db, resolver) = setup();
        let r = add_local_files(&db, &resolver, vec![]).await.unwrap();
        assert_eq!((r.added.len(), r.skipped), (0, 0));
    }
}
