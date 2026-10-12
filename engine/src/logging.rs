//! Log of the desktop app: `logs\vrcinema.log` (in English) rotated over 5 files of 5 MB, plus the
//! last 200 entries in memory for the settings tab (`GET /api/desktop/log`). An entry keeps the
//! key and fields of its message ([`Msg`]) and is shown in the reader's language when read.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::i18n::{Lang, Msg};
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
    tail_entries: usize,
    inner: Mutex<Inner>,
}

struct Inner {
    /// `None` when the file cannot be opened: the in-memory tail keeps working.
    file: Option<File>,
    size: u64,
    tail: VecDeque<LogEntry>,
}

/// One entry of the log: when, and what (the key and the fields of a message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Local date and time, `2026-10-12 10:34:44`.
    pub at: String,
    pub msg: Msg,
}

impl LogEntry {
    /// The entry's key (`log.…`).
    pub fn code(&self) -> String {
        self.msg.key()
    }

    /// One line per non-empty line of the message, each with the time.
    pub fn lines(&self, lang: Lang) -> Vec<String> {
        self.msg
            .render(lang)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| format!("[{}] {line}", self.at))
            .collect()
    }
}

impl LogSink {
    /// Logs into `<dir>/vrcinema.log` (5 files × 5 MB, last 200 lines kept in memory).
    pub fn open(dir: &Path) -> Self {
        Self::with_limits(dir, MAX_BYTES, MAX_FILES, TAIL_LINES)
    }

    /// `max_files` counts the current file too (`vrcinema.log`, `vrcinema.1.log`, …).
    pub fn with_limits(dir: &Path, max_bytes: u64, max_files: usize, tail_entries: usize) -> Self {
        let (file, size) = match open_append(&dir.join(LOG_FILE)) {
            Some((file, size)) => (Some(file), size),
            None => (None, 0),
        };
        Self {
            dir: dir.to_path_buf(),
            max_bytes,
            max_files: max_files.max(1),
            tail_entries,
            inner: Mutex::new(Inner {
                file,
                size,
                tail: VecDeque::with_capacity(tail_entries),
            }),
        }
    }

    /// Keeps `msg` in the tail and writes its English text to the file, one line per non-empty
    /// line of the text, each with the date and time.
    pub fn log(&self, msg: Msg) {
        let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let entry = LogEntry { at, msg };
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        for line in entry.lines(Lang::En) {
            self.write(&mut inner, &line);
        }
        if self.tail_entries > 0 {
            if inner.tail.len() == self.tail_entries {
                inner.tail.pop_front();
            }
            inner.tail.push_back(entry);
        }
    }

    /// The newest entries, oldest first.
    pub fn entries(&self) -> Vec<LogEntry> {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.tail.iter().cloned().collect()
    }

    /// The newest entries as lines in `lang`, oldest first.
    pub fn tail(&self, lang: Lang) -> Vec<String> {
        self.entries()
            .iter()
            .flat_map(|entry| entry.lines(lang))
            .collect()
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
        Arc::new(move |m: Msg| sink.log(m))
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

    fn text(s: &str) -> Msg {
        Msg::text(s)
    }

    #[test]
    fn lines_get_a_timestamp_and_go_to_the_file_and_the_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::open(tmp.path());
        sink.log(text("First entry"));
        sink.log(text("Second entry"));
        let tail = sink.tail(Lang::En);
        assert_eq!(tail.len(), 2);
        assert!(tail[0].ends_with("] First entry"), "{}", tail[0]);
        assert!(tail[0].starts_with('['), "{}", tail[0]);
        let file = std::fs::read_to_string(tmp.path().join(LOG_FILE)).unwrap();
        assert_eq!(file.lines().collect::<Vec<_>>(), tail);
    }

    #[test]
    fn a_multi_line_message_becomes_several_lines_without_blank_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::open(tmp.path());
        sink.log(text(
            "\n  Channel is on.\n\n  Panel: http://localhost/admin\n",
        ));
        let tail = sink.tail(Lang::En);
        assert_eq!(tail.len(), 2, "{tail:?}");
        assert!(tail[1].ends_with("  Panel: http://localhost/admin"));
        assert_eq!(sink.entries().len(), 1);
    }

    #[test]
    fn the_same_entries_are_read_in_each_language_and_the_file_stays_english() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::open(tmp.path());
        sink.log(Msg::new("log.program_stopped"));
        sink.log(Msg::new("log.tools_job_done").arg_msg("what", Msg::new("log.tools_job_install")));
        let en = sink.tail(Lang::En);
        let pl = sink.tail(Lang::Pl);
        assert!(en[0].ends_with("] Program stopped."), "{en:?}");
        assert!(pl[0].ends_with("] Program zatrzymany."), "{pl:?}");
        assert!(en[1].ends_with("Tools (downloading): done."), "{en:?}");
        assert!(pl[1].ends_with("Narzędzia (pobieranie): gotowe."), "{pl:?}");
        let entries = sink.entries();
        assert_eq!(entries[0].code(), "log.program_stopped");
        assert_eq!(entries[0].at.len(), "2026-10-12 10:34:44".len());
        // The file is the English reading, whatever language the window reads in.
        let file = std::fs::read_to_string(tmp.path().join(LOG_FILE)).unwrap();
        assert_eq!(file.lines().map(String::from).collect::<Vec<_>>(), en);
        assert!(!file.contains("zatrzymany"));
    }

    #[test]
    fn the_tail_keeps_only_the_newest_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::with_limits(tmp.path(), 1 << 20, 5, 200);
        for i in 0..250 {
            sink.log(text(&format!("entry {i}")));
        }
        let tail = sink.tail(Lang::En);
        assert_eq!(tail.len(), 200);
        assert!(tail[0].ends_with("entry 50"));
        assert!(tail[199].ends_with("entry 249"));
    }

    #[test]
    fn the_file_rotates_and_keeps_at_most_the_configured_number_of_files() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::with_limits(tmp.path(), 200, 3, 1000);
        for i in 0..100 {
            sink.log(text(&format!("entry number {i:03}")));
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
        assert!(
            current.trim_end().ends_with("entry number 099"),
            "{current}"
        );
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
        sink.log(text("new entry after the start"));
        let rotated = std::fs::read_to_string(tmp.path().join("vrcinema.1.log")).unwrap();
        assert!(rotated.starts_with("xxx"));
        let current = std::fs::read_to_string(tmp.path().join(LOG_FILE)).unwrap();
        assert!(current.contains("new entry after the start"));
    }

    #[test]
    fn without_a_writable_folder_the_tail_still_works() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_dir = tmp.path().join("file");
        std::fs::write(&not_a_dir, "x").unwrap();
        let sink = LogSink::open(&not_a_dir.join("logs"));
        sink.log(text("entry"));
        assert_eq!(sink.tail(Lang::En).len(), 1);
    }

    #[test]
    fn log_fn_writes_into_the_sink() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = Arc::new(LogSink::open(tmp.path()));
        let f = sink.log_fn();
        f(text("from the function"));
        assert!(sink.tail(Lang::En)[0].ends_with("from the function"));
    }
}
