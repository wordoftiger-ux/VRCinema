//! Log of the desktop app: `logs\vrcinema.log` rotated over 5 files of 5 MB, plus the last 200
//! lines in memory for the settings tab (`GET /api/desktop/log`).

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::pipeline::clip_factory::SharedLogFn;

pub const LOG_FILE: &str = "vrcinema.log";
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const MAX_FILES: usize = 5;
const TAIL_LINES: usize = 200;

/// The log file opened for appending, with its current size.
fn open_append(path: &Path) -> Option<(File, u64)> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    Some((file, size))
}

pub struct LogSink {
    dir: PathBuf,
    max_bytes: u64,
    max_files: usize,
    tail_lines: usize,
    inner: Mutex<Inner>,
}

struct Inner {
    /// `None` when the file cannot be opened: the in-memory tail keeps working.
    file: Option<File>,
    size: u64,
    tail: VecDeque<String>,
}

impl LogSink {
    /// Logs into `<dir>/vrcinema.log` (5 files × 5 MB, last 200 lines kept in memory).
    pub fn open(dir: &Path) -> Self {
        Self::with_limits(dir, MAX_BYTES, MAX_FILES, TAIL_LINES)
    }

    /// `max_files` counts the current file too (`vrcinema.log`, `vrcinema.1.log`, …).
    pub fn with_limits(dir: &Path, max_bytes: u64, max_files: usize, tail_lines: usize) -> Self {
        let (file, size) = match open_append(&dir.join(LOG_FILE)) {
            Some((file, size)) => (Some(file), size),
            None => (None, 0),
        };
        Self {
            dir: dir.to_path_buf(),
            max_bytes,
            max_files: max_files.max(1),
            tail_lines,
            inner: Mutex::new(Inner {
                file,
                size,
                tail: VecDeque::with_capacity(tail_lines),
            }),
        }
    }

    /// One entry per non-empty line of `msg`, each with the date and time.
    pub fn log(&self, msg: &str) {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        for line in msg.lines().filter(|l| !l.trim().is_empty()) {
            let entry = format!("[{stamp}] {line}");
            self.write(&mut inner, &entry);
            if inner.tail.len() == self.tail_lines {
                inner.tail.pop_front();
            }
            if self.tail_lines > 0 {
                inner.tail.push_back(entry);
            }
        }
    }

    /// The newest lines, oldest first.
    pub fn tail(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.tail.iter().cloned().collect()
    }

    fn write(&self, inner: &mut Inner, entry: &str) {
        let len = entry.len() as u64 + 1;
        if inner.size > 0 && inner.size + len > self.max_bytes {
            inner.file = None;
            self.rotate();
            match open_append(&self.dir.join(LOG_FILE)) {
                Some((file, size)) => {
                    inner.file = Some(file);
                    inner.size = size;
                }
                None => inner.size = 0,
            }
        }
        if let Some(file) = inner.file.as_mut()
            && writeln!(file, "{entry}").is_ok()
        {
            inner.size += len;
        }
    }

    fn numbered(&self, n: usize) -> PathBuf {
        if n == 0 {
            self.dir.join(LOG_FILE)
        } else {
            self.dir.join(format!("vrcinema.{n}.log"))
        }
    }

    /// Drops the oldest file and shifts the others up by one: `vrcinema.log` → `vrcinema.1.log`.
    fn rotate(&self) {
        let _ = std::fs::remove_file(self.numbered(self.max_files - 1));
        for n in (0..self.max_files - 1).rev() {
            let _ = std::fs::rename(self.numbered(n), self.numbered(n + 1));
        }
    }

    /// The sink as the engine's log function.
    pub fn log_fn(self: &Arc<Self>) -> SharedLogFn {
        let sink = Arc::clone(self);
        Arc::new(move |m: &str| sink.log(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn lines_get_a_timestamp_and_go_to_the_file_and_the_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::open(tmp.path());
        sink.log("Pierwszy wpis");
        sink.log("Drugi wpis");
        let tail = sink.tail();
        assert_eq!(tail.len(), 2);
        assert!(tail[0].ends_with("] Pierwszy wpis"), "{}", tail[0]);
        assert!(tail[0].starts_with('['), "{}", tail[0]);
        let text = std::fs::read_to_string(tmp.path().join(LOG_FILE)).unwrap();
        assert_eq!(text.lines().collect::<Vec<_>>(), tail);
    }

    #[test]
    fn a_multi_line_message_becomes_several_lines_without_blank_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::open(tmp.path());
        sink.log("\n  Kanał działa.\n\n  Panel: http://localhost/admin\n");
        let tail = sink.tail();
        assert_eq!(tail.len(), 2, "{tail:?}");
        assert!(tail[1].ends_with("  Panel: http://localhost/admin"));
    }

    #[test]
    fn the_tail_keeps_only_the_newest_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::with_limits(tmp.path(), 1 << 20, 5, 200);
        for i in 0..250 {
            sink.log(&format!("wpis {i}"));
        }
        let tail = sink.tail();
        assert_eq!(tail.len(), 200);
        assert!(tail[0].ends_with("wpis 50"));
        assert!(tail[199].ends_with("wpis 249"));
    }

    #[test]
    fn the_file_rotates_and_keeps_at_most_the_configured_number_of_files() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::with_limits(tmp.path(), 200, 3, 1000);
        for i in 0..100 {
            sink.log(&format!("wpis numer {i:03}"));
        }
        assert_eq!(
            files(tmp.path()),
            vec!["vrcinema.1.log", "vrcinema.2.log", "vrcinema.log"]
        );
        for name in files(tmp.path()) {
            let len = std::fs::metadata(tmp.path().join(&name)).unwrap().len();
            assert!(len <= 200, "{name}: {len}");
        }
        let current = std::fs::read_to_string(tmp.path().join(LOG_FILE)).unwrap();
        assert!(current.trim_end().ends_with("wpis numer 099"), "{current}");
        let older = std::fs::read_to_string(tmp.path().join("vrcinema.1.log")).unwrap();
        let first_current = current.lines().next().unwrap().to_string();
        let last_older = older.lines().last().unwrap().to_string();
        // The rotated file holds exactly the lines before the current file.
        let n = |l: &str| l.rsplit(' ').next().unwrap().parse::<u32>().unwrap();
        assert_eq!(n(&last_older) + 1, n(&first_current));
    }

    #[test]
    fn an_existing_file_is_appended_to_and_counts_towards_rotation() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(LOG_FILE), "x".repeat(190) + "\n").unwrap();
        let sink = LogSink::with_limits(tmp.path(), 200, 3, 10);
        sink.log("nowy wpis po starcie");
        let rotated = std::fs::read_to_string(tmp.path().join("vrcinema.1.log")).unwrap();
        assert!(rotated.starts_with("xxx"));
        let current = std::fs::read_to_string(tmp.path().join(LOG_FILE)).unwrap();
        assert!(current.contains("nowy wpis po starcie"));
    }

    #[test]
    fn without_a_writable_folder_the_tail_still_works() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_dir = tmp.path().join("plik");
        std::fs::write(&not_a_dir, "x").unwrap();
        let sink = LogSink::open(&not_a_dir.join("logs"));
        sink.log("wpis");
        assert_eq!(sink.tail().len(), 1);
    }

    #[test]
    fn log_fn_writes_into_the_sink() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = Arc::new(LogSink::open(tmp.path()));
        let f = sink.log_fn();
        f("z funkcji");
        assert!(sink.tail()[0].ends_with("z funkcji"));
    }
}
