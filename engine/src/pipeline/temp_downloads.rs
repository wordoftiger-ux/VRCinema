//! Films yt-dlp downloads whole before they air: TikTok (its media addresses answer 403 to
//! ffprobe and ffmpeg, even with yt-dlp's headers and cookies) and any source whose resolved
//! address was refused to ffprobe (HTTP 403/401). One file per queue item in `<DATA_DIR>/temp`, shared by
//! that item's clips (a resume or a seek starts a new clip from the same file) and removed
//! [`GRACE`] after the last of them is gone; the server empties the folder when it starts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long a film stays after its last clip ended: a resume after a short pause, or a seek,
/// plays it without downloading it again.
pub const GRACE: Duration = Duration::from_secs(180);

const REMOVE_RETRIES: u32 = 3;
const REMOVE_RETRY_DELAY: Duration = Duration::from_millis(500);

/// A TikTok address (`tiktok.com` and its subdomains: `www.`, `m.`, `vm.`, `vt.`).
pub fn is_tiktok(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|h| h == "tiktok.com" || h.ends_with(".tiktok.com"))
}

/// Empties the folder (creating it when missing) at server start: no clip uses its files yet.
/// A file still locked by a leftover process stays until the next start.
pub fn reset_dir(dir: &Path) -> std::io::Result<()> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
        }
    }
    std::fs::create_dir_all(dir)
}

struct Slot {
    file: PathBuf,
    /// Clips holding a [`Lease`] on the file.
    users: usize,
    /// Which release scheduled the removal (a later one, or a new user, cancels it).
    generation: u64,
}

struct Inner {
    dir: PathBuf,
    grace: Duration,
    slots: Mutex<HashMap<i64, Slot>>,
    counter: AtomicU64,
}

impl Inner {
    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The file of `item_id` once nobody took it again since the release `generation`.
    fn expire(&self, item_id: i64, generation: u64) -> Option<PathBuf> {
        let mut slots = self.slots();
        match slots.get(&item_id) {
            Some(slot) if slot.users == 0 && slot.generation == generation => {
                slots.remove(&item_id).map(|s| s.file)
            }
            _ => None,
        }
    }
}

/// The downloaded films of one channel's clips. Cheap to clone.
#[derive(Clone)]
pub struct TempDownloads(Arc<Inner>);

impl TempDownloads {
    pub fn new(dir: PathBuf) -> Self {
        Self::with_grace(dir, GRACE)
    }

    pub fn with_grace(dir: PathBuf, grace: Duration) -> Self {
        Self(Arc::new(Inner {
            dir,
            grace,
            slots: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        }))
    }

    /// The film of `item_id` an earlier clip downloaded, while it is still on disk.
    pub fn reuse(&self, item_id: i64) -> Option<Lease> {
        let mut slots = self.0.slots();
        let file = slots.get(&item_id)?.file.clone();
        if !file.is_file() {
            slots.remove(&item_id);
            return None;
        }
        let slot = slots.get_mut(&item_id)?;
        slot.users += 1;
        Some(self.lease(item_id, file))
    }

    /// Where the next download of `item_id` goes: `<dir>/<item>-<n>`, without its extension
    /// (yt-dlp adds it). Creates the folder.
    pub fn stem(&self, item_id: i64) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.0.dir)?;
        let n = self.0.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let stem = self.0.dir.join(format!("{item_id}-{n}"));
        Ok(std::path::absolute(&stem).unwrap_or(stem))
    }

    /// Takes in a finished download of `item_id`. When another clip of the item finished its
    /// download first, that film is used and `file` removed.
    pub fn adopt(&self, item_id: i64, file: PathBuf) -> Lease {
        let mut slots = self.0.slots();
        if let Some(slot) = slots.get_mut(&item_id)
            && slot.file != file
            && slot.file.is_file()
        {
            let _ = std::fs::remove_file(&file);
            slot.users += 1;
            let kept = slot.file.clone();
            return self.lease(item_id, kept);
        }
        let generation = self.0.counter.fetch_add(1, Ordering::SeqCst);
        slots.insert(
            item_id,
            Slot {
                file: file.clone(),
                users: 1,
                generation,
            },
        );
        self.lease(item_id, file)
    }

    fn lease(&self, item_id: i64, file: PathBuf) -> Lease {
        Lease {
            inner: Arc::clone(&self.0),
            item_id,
            file,
        }
    }
}

/// A clip's hold on a downloaded film: the file stays while any clip of the item holds one,
/// and is removed [`GRACE`] after the last is dropped (ffmpeg has long exited by then).
pub struct Lease {
    inner: Arc<Inner>,
    item_id: i64,
    file: PathBuf,
}

impl Lease {
    pub fn path(&self) -> &Path {
        &self.file
    }
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Lease").field(&self.file).finish()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let generation = {
            let mut slots = self.inner.slots();
            let Some(slot) = slots.get_mut(&self.item_id) else {
                return;
            };
            if slot.file != self.file {
                return;
            }
            slot.users = slot.users.saturating_sub(1);
            if slot.users > 0 {
                return;
            }
            slot.generation = self.inner.counter.fetch_add(1, Ordering::SeqCst);
            slot.generation
        };
        let inner = Arc::clone(&self.inner);
        let item_id = self.item_id;
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move {
                    tokio::time::sleep(inner.grace).await;
                    if let Some(file) = inner.expire(item_id, generation) {
                        remove_file(&file).await;
                    }
                });
            }
            Err(_) => {
                if let Some(file) = inner.expire(item_id, generation) {
                    let _ = std::fs::remove_file(file);
                }
            }
        }
    }
}

/// Best effort, a few retries (Windows may hold a lock for a moment); a leftover is removed
/// at the next server start.
async fn remove_file(file: &Path) {
    for attempt in 0..=REMOVE_RETRIES {
        match tokio::fs::remove_file(file).await {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) if attempt < REMOVE_RETRIES => {
                tokio::time::sleep(REMOVE_RETRY_DELAY * (attempt + 1)).await;
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn tiktok_addresses_are_recognised() {
        for url in [
            "https://www.tiktok.com/@polishballers/video/7698743057218407702",
            "https://TIKTOK.com/@a/video/1",
            "https://vm.tiktok.com/ZMabc/",
            "https://m.tiktok.com/v/1.html",
        ] {
            assert!(is_tiktok(url), "{url}");
        }
        for url in [
            "https://nottiktok.com/@a/video/1",
            "https://tiktok.com.evil.example/a",
            "https://youtu.be/x",
            "C:\\film.mp4",
            "nie url",
        ] {
            assert!(!is_tiktok(url), "{url}");
        }
    }

    fn film(downloads: &TempDownloads, item_id: i64) -> PathBuf {
        let file = downloads.stem(item_id).unwrap().with_extension("mp4");
        std::fs::write(&file, b"film").unwrap();
        file
    }

    async fn gone(file: &Path) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if !file.exists() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn the_film_stays_while_a_clip_holds_it_and_goes_after_the_grace() {
        let dir = tempfile::tempdir().unwrap();
        let downloads =
            TempDownloads::with_grace(dir.path().join("temp"), Duration::from_millis(30));
        assert!(downloads.reuse(7).is_none());
        let file = film(&downloads, 7);
        assert!(file.starts_with(dir.path().join("temp")));
        let first = downloads.adopt(7, file.clone());
        let resumed = downloads.reuse(7).expect("the same item's film");
        assert_eq!(resumed.path(), file);
        drop(first);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(file.is_file(), "the resumed clip still plays it");
        drop(resumed);
        assert!(gone(&file).await);
        assert!(downloads.reuse(7).is_none());
    }

    #[tokio::test]
    async fn a_clip_taking_the_film_within_the_grace_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = TempDownloads::with_grace(dir.path().into(), Duration::from_millis(50));
        let file = film(&downloads, 3);
        drop(downloads.adopt(3, file.clone()));
        let seek = downloads.reuse(3).expect("within the grace");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(file.is_file());
        drop(seek);
        assert!(gone(&file).await);
    }

    #[test]
    fn a_second_download_of_the_same_item_gives_way_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = TempDownloads::with_grace(dir.path().into(), Duration::ZERO);
        let (a, b) = (film(&downloads, 5), film(&downloads, 5));
        assert_ne!(a, b, "every download has a name of its own");
        let first = downloads.adopt(5, a.clone());
        let second = downloads.adopt(5, b.clone());
        assert_eq!(second.path(), a);
        assert!(!b.exists());
        // Without a runtime the last release removes the film at once.
        drop(first);
        assert!(a.is_file());
        drop(second);
        assert!(!a.exists());
    }

    #[test]
    fn a_film_removed_from_disk_is_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = TempDownloads::new(dir.path().into());
        let file = film(&downloads, 9);
        let lease = downloads.adopt(9, file.clone());
        std::fs::remove_file(&file).unwrap();
        assert!(downloads.reuse(9).is_none());
        drop(lease);
    }

    #[test]
    fn the_folder_is_emptied_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join("temp");
        reset_dir(&temp).unwrap();
        assert!(temp.is_dir());
        std::fs::write(temp.join("7-1.mp4"), b"x").unwrap();
        std::fs::write(temp.join("7-2.mp4.part"), b"x").unwrap();
        std::fs::create_dir(temp.join("sub")).unwrap();
        reset_dir(&temp).unwrap();
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
    }
}
