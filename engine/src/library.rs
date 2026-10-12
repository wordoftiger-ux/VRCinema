//! Owner's disk-file library (port of `src/library/library.ts`): browsing and
//! resolving video files inside configured root folders without ever leaving them.
//!
//! Containment is checked twice: lexically (after normalising `.` and `..`) and on
//! canonical real paths, so links and junctions that point outside a root are hidden
//! in listings and rejected by `resolve`. Canonical paths (which carry a `\\?\` prefix
//! on Windows) are only used for comparison and never returned to callers.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::i18n::Msg;

pub const VIDEO_EXTENSIONS: [&str; 11] = [
    "mp4", "mkv", "avi", "mov", "webm", "m4v", "ts", "wmv", "flv", "mpg", "mpeg",
];

/// Paths compare case-insensitively on Windows and macOS.
const CASE_INSENSITIVE: bool = cfg!(any(windows, target_os = "macos"));

/// Library error: the message for the person (an `errors.` one, shown in their language).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LibraryError(pub Msg);

impl LibraryError {
    pub fn no_such_root() -> Self {
        err("no_such_root")
    }
}

fn err(code: &'static str) -> LibraryError {
    LibraryError(Msg::error(code))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryRoot {
    pub index: usize,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    Dir,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntry {
    #[serde(rename = "type")]
    pub kind: EntryType,
    pub name: String,
    /// Path relative to the root folder, with "/" as the separator.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryListing {
    pub root: usize,
    pub path: String,
    /// Path of the parent folder, `None` in the root folder.
    pub parent: Option<String>,
    pub entries: Vec<LibraryEntry>,
}

pub fn is_video_file(name: &str) -> bool {
    // Like Node's `path.extname`: a leading dot alone does not start an extension.
    match name.rfind('.') {
        Some(i) if i > 0 => {
            let ext = name[i + 1..].to_lowercase();
            VIDEO_EXTENSIONS.contains(&ext.as_str())
        }
        _ => false,
    }
}

/// Lexically resolves `.` and `..` without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                let at_root = matches!(
                    out.components().next_back(),
                    None | Some(Component::RootDir | Component::Prefix(_))
                );
                if !at_root {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn fold(c: Component) -> String {
    let s = c.as_os_str().to_string_lossy();
    if CASE_INSENSITIVE {
        s.to_lowercase()
    } else {
        s.into_owned()
    }
}

fn is_inside(child: &Path, parent: &Path) -> bool {
    let mut child = child.components();
    parent
        .components()
        .all(|p| child.next().is_some_and(|c| fold(c) == fold(p)))
}

/// True for symlinks and, on Windows, any reparse point (junctions, mount points).
fn is_link_like(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_type().is_symlink() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// Sort key reproducing `Intl.Collator('pl', { numeric: true, sensitivity: 'base' })`
/// closely enough for file names: whitespace < punctuation < digits < letters, digit runs
/// compared as numbers (leading zeros ignored), case and accents ignored, Polish letters
/// as their own letters after the base letter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    Space(char),
    Punct(u32),
    /// Digits without leading zeros: shorter is smaller, then lexicographic.
    Number(usize, String),
    Letter(u32),
}

const POLISH_ALPHABET: &str = "aąbcćdeęfghijklłmnńoópqrsśtuvwxyzźż";
const PUNCTUATION_ORDER: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

fn letter_weight(c: char) -> u32 {
    let lower = c.to_lowercase().next().unwrap_or(c);
    let index = |ch: char| POLISH_ALPHABET.chars().position(|a| a == ch);
    if let Some(i) = index(lower) {
        return i as u32;
    }
    // Accent-insensitive: fall back to the base letter of the decomposed form.
    if let Some(i) = lower.nfd().next().and_then(index) {
        return i as u32;
    }
    1000 + lower as u32
}

fn sort_keys(name: &str) -> Vec<SortKey> {
    let mut keys = Vec::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut digits = String::from(c);
            while let Some(d) = chars.next_if(char::is_ascii_digit) {
                digits.push(d);
            }
            let trimmed = digits.trim_start_matches('0').to_string();
            keys.push(SortKey::Number(trimmed.len(), trimmed));
        } else if c.is_alphabetic() {
            keys.push(SortKey::Letter(letter_weight(c)));
        } else if c.is_whitespace() {
            keys.push(SortKey::Space(c));
        } else {
            let rank = PUNCTUATION_ORDER
                .chars()
                .position(|p| p == c)
                .map_or(100 + c as u32, |i| i as u32);
            keys.push(SortKey::Punct(rank));
        }
    }
    keys
}

/// Natural order of file names (see [`SortKey`]); ties fall back to the plain byte order.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    sort_keys(a).cmp(&sort_keys(b)).then_with(|| a.cmp(b))
}

/// True when the path's file name has a video extension.
pub fn is_video_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_video_file)
}

/// Folders with the owner's videos. Every path is checked so it cannot leave them.
pub struct MediaLibrary {
    dirs: Vec<PathBuf>,
}

impl MediaLibrary {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        let dirs = dirs
            .into_iter()
            .map(|d| normalize(&std::path::absolute(&d).unwrap_or(d)))
            .collect();
        Self { dirs }
    }

    pub fn enabled(&self) -> bool {
        !self.dirs.is_empty()
    }

    pub fn roots(&self) -> Vec<LibraryRoot> {
        self.dirs
            .iter()
            .enumerate()
            .map(|(index, p)| LibraryRoot {
                index,
                name: p.file_name().map_or_else(
                    || p.to_string_lossy().into_owned(),
                    |n| n.to_string_lossy().into_owned(),
                ),
                path: p.clone(),
            })
            .collect()
    }

    /// Absolute path of an existing file or folder inside the root folder.
    pub fn resolve(&self, root: usize, rel: &str) -> Result<PathBuf, LibraryError> {
        let base = self.dirs.get(root).ok_or_else(LibraryError::no_such_root)?;
        let mut chars = rel.chars();
        let drive_like =
            matches!((chars.next(), chars.next()), (Some(a), Some(':')) if a.is_ascii_alphabetic());
        if rel.contains('\0') || drive_like || Path::new(rel).has_root() {
            return Err(err("invalid_path"));
        }
        let full = normalize(&base.join(rel));
        if !is_inside(&full, base) {
            return Err(err("path_outside_library"));
        }
        let missing = || {
            let name = full.file_name().map_or_else(
                || full.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            LibraryError(Msg::error("file_missing").arg("name", name))
        };
        if !full.exists() {
            return Err(missing());
        }
        let (real_base, real_full) = match (fs::canonicalize(base), fs::canonicalize(&full)) {
            (Ok(b), Ok(f)) => (b, f),
            _ => return Err(missing()),
        };
        if !is_inside(&real_full, &real_base) {
            return Err(err("path_outside_library"));
        }
        Ok(full)
    }

    pub fn list(&self, root: usize, rel: &str) -> Result<LibraryListing, LibraryError> {
        let dir = self.resolve(root, rel)?;
        if !fs::metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Err(err("not_a_folder"));
        }
        let base = &self.dirs[root];
        let real_base = fs::canonicalize(base).map_err(|_| err("folder_unreadable"))?;
        let rel_norm = dir
            .components()
            .skip(base.components().count())
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let read = fs::read_dir(&dir)
            .map_err(|e| LibraryError(Msg::error("folder_unreadable_detail").arg("detail", e)))?;
        let mut entries = Vec::new();
        for item in read.flatten() {
            let file_name = item.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if name.starts_with('.') || name.starts_with('$') {
                continue;
            }
            let child = dir.join(name);
            let Ok(stat) = fs::metadata(&child) else {
                continue;
            };
            // Links and junctions leading outside the library are not shown.
            let link = fs::symlink_metadata(&child).is_ok_and(|m| is_link_like(&m));
            if link && !fs::canonicalize(&child).is_ok_and(|real| is_inside(&real, &real_base)) {
                continue;
            }
            let child_rel = if rel_norm.is_empty() {
                name.to_string()
            } else {
                format!("{rel_norm}/{name}")
            };
            if stat.is_dir() {
                entries.push(LibraryEntry {
                    kind: EntryType::Dir,
                    name: name.to_string(),
                    path: child_rel,
                    size_bytes: None,
                });
            } else if stat.is_file() && is_video_file(name) {
                entries.push(LibraryEntry {
                    kind: EntryType::File,
                    name: name.to_string(),
                    path: child_rel,
                    size_bytes: Some(stat.len()),
                });
            }
        }
        entries.sort_by_cached_key(|e| {
            (
                e.kind == EntryType::File,
                sort_keys(&e.name),
                e.name.clone(),
            )
        });
        let parent = if rel_norm.is_empty() {
            None
        } else {
            Some(
                rel_norm
                    .rfind('/')
                    .map_or(String::new(), |i| rel_norm[..i].to_string()),
            )
        };
        Ok(LibraryListing {
            root,
            path: rel_norm,
            parent,
            entries,
        })
    }

    /// Videos directly in the folder (no subfolders), in natural order.
    pub fn videos_in(&self, root: usize, rel: &str) -> Result<Vec<LibraryEntry>, LibraryError> {
        Ok(self
            .list(root, rel)?
            .entries
            .into_iter()
            .filter(|e| e.kind == EntryType::File)
            .collect())
    }

    pub fn to_source_url(&self, root: usize, rel: &str) -> String {
        let encoded: Vec<String> = rel.split('/').map(encode_uri_component).collect();
        format!("local:{root}/{}", encoded.join("/"))
    }
}

/// Like JS `encodeURIComponent`.
fn encode_uri_component(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for b in segment.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Like JS `decodeURIComponent`: `None` on malformed escapes or invalid UTF-8.
fn decode_uri_component(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = segment.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Parses `local:<root>/<percent-encoded path>` into `(root, path)`.
pub fn parse_source_url(url: &str) -> Option<(usize, String)> {
    let (root, path) = url.strip_prefix("local:")?.split_once('/')?;
    if root.is_empty() || !root.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // JS `.` does not match line terminators.
    if path.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let root = root.parse().ok()?;
    let decoded: Option<Vec<String>> = path.split('/').map(decode_uri_component).collect();
    Some((root, decoded?.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    struct Fixture {
        _tmp: tempfile::TempDir,
        films: PathBuf,
        series: PathBuf,
        outside: PathBuf,
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    }

    /// Directory link pointing outside: a junction on Windows, a symlink elsewhere.
    fn link_dir(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let out = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(out.status.success(), "mklink failed: {out:?}");
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).unwrap();
        }
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let films = base.join("Filmy");
        let series = base.join("Seriale");
        let outside = base.join("Prywatne");
        touch(&films.join("Film A.mp4"));
        touch(&films.join("notatki.txt"));
        touch(&films.join("Okładka.JPG"));
        touch(&films.join("Stary.AVI"));
        for n in [10, 2, 1] {
            touch(&series.join("Sezon 1").join(format!("Odcinek {n}.mkv")));
        }
        touch(&series.join("Sezon 1").join("Podkatalog").join("x.mp4"));
        touch(&outside.join("tajne.mp4"));
        link_dir(&films.join("skrot"), &outside);
        Fixture {
            _tmp: tmp,
            films,
            series,
            outside,
        }
    }

    fn names(l: &LibraryListing) -> Vec<(EntryType, &str)> {
        l.entries
            .iter()
            .map(|e| (e.kind, e.name.as_str()))
            .collect()
    }

    #[test]
    fn roots_show_names() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.films.clone(), f.series.clone()]);
        assert!(lib.enabled());
        assert_eq!(
            lib.roots(),
            vec![
                LibraryRoot {
                    index: 0,
                    name: "Filmy".into(),
                    path: std::path::absolute(&f.films).unwrap()
                },
                LibraryRoot {
                    index: 1,
                    name: "Seriale".into(),
                    path: std::path::absolute(&f.series).unwrap()
                },
            ]
        );
        assert!(!MediaLibrary::new(vec![]).enabled());
    }

    #[test]
    fn lists_dirs_and_only_video_files_and_hides_escaping_links() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        let l = lib.list(0, "").unwrap();
        assert_eq!(
            names(&l),
            vec![
                (EntryType::File, "Film A.mp4"),
                (EntryType::File, "Stary.AVI")
            ]
        );
        assert_eq!(l.parent, None);
        assert_eq!(l.path, "");
        assert_eq!(l.entries[0].size_bytes, Some(1));
    }

    #[test]
    fn hides_dot_and_dollar_entries() {
        let f = fixture();
        touch(&f.films.join(".ukryty.mp4"));
        touch(&f.films.join("$RECYCLE.BIN").join("a.mp4"));
        touch(&f.films.join("$x.mp4"));
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        let l = lib.list(0, "").unwrap();
        assert_eq!(l.entries.len(), 2);
    }

    #[test]
    fn natural_sort_and_relative_paths() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.series.clone()]);
        let l = lib.list(0, "Sezon 1").unwrap();
        let n: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            n,
            [
                "Podkatalog",
                "Odcinek 1.mkv",
                "Odcinek 2.mkv",
                "Odcinek 10.mkv"
            ]
        );
        assert_eq!(l.entries[1].path, "Sezon 1/Odcinek 1.mkv");
        assert_eq!(l.parent.as_deref(), Some(""));
        let deeper = lib.list(0, "Sezon 1/Podkatalog").unwrap();
        assert_eq!(deeper.parent.as_deref(), Some("Sezon 1"));
        assert_eq!(deeper.path, "Sezon 1/Podkatalog");
    }

    #[test]
    fn sort_is_case_insensitive_and_knows_polish_letters() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("L");
        for n in [
            "zebra.mp4",
            "Żaba.mp4",
            "Ćma.mp4",
            "czapka.mp4",
            "Ąąą.mp4",
            "ala.mp4",
            "B.mp4",
            "ćma 2.mp4",
            "Łódź.mp4",
            "lis.mp4",
            "Mama.mp4",
            "Świat.mp4",
            "sok.mp4",
            "Zorza.mp4",
            "źrebak.mp4",
        ] {
            touch(&dir.join(n));
        }
        let lib = MediaLibrary::new(vec![dir]);
        let n: Vec<String> = lib
            .list(0, "")
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            n,
            [
                "ala.mp4",
                "Ąąą.mp4",
                "B.mp4",
                "czapka.mp4",
                "ćma 2.mp4",
                "Ćma.mp4",
                "lis.mp4",
                "Łódź.mp4",
                "Mama.mp4",
                "sok.mp4",
                "Świat.mp4",
                "zebra.mp4",
                "Zorza.mp4",
                "źrebak.mp4",
                "Żaba.mp4",
            ]
        );
    }

    #[test]
    fn natural_sort_ignores_leading_zeros_and_orders_digits_before_letters() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("L");
        for n in [
            "a.mp4", "9.mp4", "010.mp4", "2.mp4", "-x.mp4", "a2.mp4", "a10.mp4", "A1.mp4",
        ] {
            touch(&dir.join(n));
        }
        let lib = MediaLibrary::new(vec![dir]);
        let n: Vec<String> = lib
            .list(0, "")
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            n,
            [
                "-x.mp4", "2.mp4", "9.mp4", "010.mp4", "a.mp4", "A1.mp4", "a2.mp4", "a10.mp4"
            ]
        );
    }

    #[test]
    fn videos_in_returns_only_files_in_natural_order() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.series.clone()]);
        let v: Vec<_> = lib
            .videos_in(0, "Sezon 1")
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(
            v,
            [
                "Sezon 1/Odcinek 1.mkv",
                "Sezon 1/Odcinek 2.mkv",
                "Sezon 1/Odcinek 10.mkv"
            ]
        );
    }

    #[test]
    fn blocks_escaping_paths_and_bad_input() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        let outside_file = f.outside.join("tajne.mp4");
        let err = |r: Result<PathBuf, LibraryError>| r.unwrap_err().0.render(Lang::Pl);
        assert_eq!(
            err(lib.resolve(0, "../Prywatne/tajne.mp4")),
            "Ścieżka wychodzi poza folder z filmami."
        );
        assert_eq!(
            err(lib.resolve(0, "Film A.mp4/../../Prywatne/tajne.mp4")),
            "Ścieżka wychodzi poza folder z filmami."
        );
        assert_eq!(
            err(lib.resolve(0, outside_file.to_str().unwrap())),
            "Niepoprawna ścieżka."
        );
        assert_eq!(
            err(lib.resolve(0, "C:/Windows/win.ini")),
            "Niepoprawna ścieżka."
        );
        assert_eq!(err(lib.resolve(0, "C:win.ini")), "Niepoprawna ścieżka.");
        assert_eq!(err(lib.resolve(0, "/etc/passwd")), "Niepoprawna ścieżka.");
        assert_eq!(
            err(lib.resolve(5, "Film A.mp4")),
            "Nie ma takiego folderu głównego."
        );
        assert_eq!(
            err(lib.resolve(0, "Film A.mp4\0.txt")),
            "Niepoprawna ścieżka."
        );
    }

    #[test]
    fn blocks_links_leading_outside_the_library() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        assert_eq!(
            lib.resolve(0, "skrot/tajne.mp4")
                .unwrap_err()
                .0
                .render(Lang::Pl),
            "Ścieżka wychodzi poza folder z filmami."
        );
        assert!(lib.list(0, "skrot").is_err());
        assert!(lib.resolve(0, "skrot").is_err());
    }

    #[test]
    fn links_staying_inside_the_library_are_allowed() {
        let f = fixture();
        link_dir(&f.series.join("alias"), &f.series.join("Sezon 1"));
        let lib = MediaLibrary::new(vec![f.series.clone()]);
        assert!(lib.resolve(0, "alias/Odcinek 1.mkv").is_ok());
        let l = lib.list(0, "").unwrap();
        assert!(l.entries.iter().any(|e| e.name == "alias"));
    }

    #[test]
    fn resolve_returns_absolute_existing_path_without_verbatim_prefix() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        let p = lib.resolve(0, "Film A.mp4").unwrap();
        assert!(p.is_absolute());
        assert!(!p.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(
            fs::canonicalize(&p).unwrap(),
            fs::canonicalize(f.films.join("Film A.mp4")).unwrap()
        );
        assert_eq!(
            lib.resolve(0, "./Film A.mp4").unwrap(),
            lib.resolve(0, "Film A.mp4").unwrap()
        );
        assert!(lib.resolve(0, "").unwrap().is_dir());
    }

    #[test]
    fn missing_file_gives_readable_error() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        assert_eq!(
            lib.resolve(0, "Nie ma.mp4").unwrap_err().0.render(Lang::Pl),
            "Plik nie istnieje: Nie ma.mp4"
        );
    }

    #[test]
    fn list_of_a_file_is_an_error() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        assert_eq!(
            lib.list(0, "Film A.mp4").unwrap_err().0.render(Lang::Pl),
            "To nie jest folder."
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_are_case_insensitive() {
        let f = fixture();
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        assert!(lib.resolve(0, "film a.MP4").is_ok());
        let upper = PathBuf::from(f.films.to_string_lossy().to_uppercase());
        let lib = MediaLibrary::new(vec![upper]);
        assert!(lib.resolve(0, "Film A.mp4").is_ok());
    }

    #[test]
    fn polish_file_name_lists_resolves_and_roundtrips_through_local_url() {
        let f = fixture();
        touch(&f.films.join("Żółć 1.mkv"));
        let lib = MediaLibrary::new(vec![f.films.clone()]);
        let l = lib.list(0, "").unwrap();
        let e = l
            .entries
            .iter()
            .find(|e| e.name == "Żółć 1.mkv")
            .expect("Żółć 1.mkv must be listed");
        assert_eq!(e.path, "Żółć 1.mkv");
        assert_eq!(e.kind, EntryType::File);
        let abs = lib.resolve(0, &e.path).unwrap();
        assert!(abs.is_file());
        let url = lib.to_source_url(0, &e.path);
        assert_eq!(url, "local:0/%C5%BB%C3%B3%C5%82%C4%87%201.mkv");
        let (root, rel) = parse_source_url(&url).unwrap();
        assert_eq!((root, rel.as_str()), (0, "Żółć 1.mkv"));
        assert_eq!(lib.resolve(root, &rel).unwrap(), abs);
    }

    #[test]
    fn local_source_url_roundtrip_with_polish_chars_spaces_and_hash() {
        let lib = MediaLibrary::new(vec![PathBuf::from("x")]);
        let url = lib.to_source_url(0, "Sezon 1/Żółć #1.mp4");
        assert!(url.starts_with("local:0/"));
        assert_eq!(url, "local:0/Sezon%201/%C5%BB%C3%B3%C5%82%C4%87%20%231.mp4");
        assert_eq!(
            parse_source_url(&url),
            Some((0, "Sezon 1/Żółć #1.mp4".to_string()))
        );
        assert_eq!(parse_source_url("https://youtu.be/x"), None);
    }

    #[test]
    fn source_url_matches_encode_uri_component() {
        let lib = MediaLibrary::new(vec![PathBuf::from("x")]);
        assert_eq!(
            lib.to_source_url(3, "a-b_c.d!e~f*g'h(i)j k&l=m+n,o;p@q$r"),
            "local:3/a-b_c.d!e~f*g'h(i)j%20k%26l%3Dm%2Bn%2Co%3Bp%40q%24r"
        );
        assert_eq!(lib.to_source_url(0, ""), "local:0/");
        assert_eq!(parse_source_url("local:0/"), Some((0, String::new())));
    }

    #[test]
    fn parse_source_url_rejects_malformed_input() {
        assert_eq!(parse_source_url("local:x/abc"), None);
        assert_eq!(parse_source_url("local:0abc"), None);
        assert_eq!(parse_source_url("local:0/%"), None);
        assert_eq!(parse_source_url("local:0/%zz"), None);
        assert_eq!(parse_source_url("local:0/%FF"), None);
        assert_eq!(parse_source_url("local:0/a\nb"), None);
        assert_eq!(parse_source_url("LOCAL:0/a"), None);
        assert_eq!(parse_source_url("local:99999999999999999999999/a"), None);
    }

    #[test]
    fn video_extension_check_is_case_insensitive() {
        assert!(is_video_file("a.MKV"));
        assert!(is_video_file("a.b.Mp4"));
        assert!(!is_video_file("a.jpg"));
        assert!(!is_video_file("mp4"));
        assert!(!is_video_file(".mp4"));
    }
}
