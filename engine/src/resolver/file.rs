//! Files given by absolute path, stored in the queue as `file:///D:/Filmy/Odcinek%201.mkv`.
//!
//! Such items are only created by the desktop window (drag & drop, see `local_files`); the
//! HTTP routes accept http/https links only.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::local::FileDurationProbe;
use super::{MediaMeta, MediaResolver, ResolveError, ResolvedStream, StreamInput};
use crate::i18n::Msg;
use crate::library::is_video_path;

/// `file:` URL of an absolute path (percent-encoded; drive letters and UNC paths included).
/// `None` for relative paths.
pub fn file_source_url(path: &Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

/// Absolute path behind a `file:` URL; `None` for anything else.
pub fn parse_file_source_url(url: &str) -> Option<PathBuf> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    let path = parsed.to_file_path().ok()?;
    path.is_absolute().then_some(path)
}

fn name_of(file: &Path) -> String {
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn title_of(file: &Path) -> String {
    file.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub struct FileResolver {
    pub probe_duration: FileDurationProbe,
}

impl FileResolver {
    pub fn new(probe_duration: FileDurationProbe) -> Self {
        Self { probe_duration }
    }

    fn file(&self, url: &str) -> Result<PathBuf, ResolveError> {
        let path = parse_file_source_url(url)
            .ok_or_else(|| ResolveError::coded("invalid_disk_file_url"))?;
        let name = name_of(&path);
        match std::fs::metadata(&path) {
            Err(_) => Err(Msg::error("file_missing").arg("name", name).into()),
            Ok(m) if m.is_file() && is_video_path(&path) => Ok(path),
            Ok(_) => Err(Msg::error("not_a_video_file").arg("name", name).into()),
        }
    }
}

#[async_trait]
impl MediaResolver for FileResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        let full = self.file(url)?;
        let title = title_of(&full);
        let duration_s = (self.probe_duration)(full).await;
        Ok(MediaMeta {
            title,
            duration_s,
            is_live: false,
            is_playlist: false,
            entries: None,
        })
    }

    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        let full = self.file(url)?;
        Ok(ResolvedStream {
            title: title_of(&full),
            inputs: vec![StreamInput {
                url: full.to_string_lossy().into_owned(),
                headers: Vec::new(),
            }],
            duration_s: None,
            is_live: false,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn resolver() -> FileResolver {
        FileResolver::new(Box::new(|_| Box::pin(async { Some(1440.0) })))
    }

    fn abs(p: &Path) -> PathBuf {
        std::path::absolute(p).unwrap()
    }

    #[test]
    fn url_roundtrips_polish_chars_spaces_hash_and_percent() {
        let tmp = tempfile::tempdir().unwrap();
        for name in [
            "Odcinek 1.mkv",
            "Żółć gęślą jaźń.mp4",
            "Film #1 100%.mkv",
            "a%20b.mp4",
        ] {
            let path = abs(&tmp.path().join(name));
            let url = file_source_url(&path).unwrap();
            assert!(url.starts_with("file:///"), "{url}");
            assert!(!url.contains(' ') && !url.contains('#'), "{url}");
            assert_eq!(parse_file_source_url(&url), Some(path), "{url}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_and_unc_paths_roundtrip() {
        let drive = PathBuf::from(r"D:\Filmy\Odcinek 1.mkv");
        let url = file_source_url(&drive).unwrap();
        assert_eq!(url, "file:///D:/Filmy/Odcinek%201.mkv");
        assert_eq!(parse_file_source_url(&url), Some(drive));
        let unc = PathBuf::from(r"\\serwer\udział\Filmy\Żółć #1.mkv");
        let url = file_source_url(&unc).unwrap();
        assert!(url.starts_with("file://"), "{url}");
        assert_eq!(parse_file_source_url(&url), Some(unc));
    }

    #[test]
    fn relative_paths_and_foreign_schemes_are_rejected() {
        assert_eq!(file_source_url(Path::new("Filmy/a.mkv")), None);
        assert_eq!(parse_file_source_url("file:relative/a.mkv"), None);
        assert_eq!(parse_file_source_url("https://youtu.be/x"), None);
        assert_eq!(parse_file_source_url("local:0/a.mp4"), None);
        assert_eq!(parse_file_source_url("nie url"), None);
    }

    #[tokio::test]
    async fn meta_title_is_file_stem_and_duration_comes_from_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let path = abs(&tmp.path().join("Żółć 1.mkv"));
        fs::write(&path, "x").unwrap();
        let url = file_source_url(&path).unwrap();
        assert_eq!(
            resolver().fetch_meta(&url).await.unwrap(),
            MediaMeta {
                title: "Żółć 1".into(),
                duration_s: Some(1440.0),
                is_live: false,
                is_playlist: false,
                entries: None,
            }
        );
        let s = resolver().resolve(&url).await.unwrap();
        assert_eq!(s.title, "Żółć 1");
        assert_eq!(
            s.inputs,
            vec![StreamInput {
                url: path.to_string_lossy().into_owned(),
                headers: vec![],
            }]
        );
    }

    #[tokio::test]
    async fn missing_file_gives_does_not_exist_error() {
        let tmp = tempfile::tempdir().unwrap();
        let url = file_source_url(&abs(&tmp.path().join("Usunięty.mkv"))).unwrap();
        for e in [
            resolver().resolve(&url).await.unwrap_err(),
            resolver().fetch_meta(&url).await.unwrap_err(),
        ] {
            assert_eq!(
                e.0.render(crate::i18n::Lang::Pl),
                "Plik nie istnieje: Usunięty.mkv"
            );
        }
    }

    #[tokio::test]
    async fn folder_and_non_video_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = abs(&tmp.path().join("Sezon 1.mkv"));
        fs::create_dir(&dir).unwrap();
        let txt = abs(&tmp.path().join("notatki.txt"));
        fs::write(&txt, "x").unwrap();
        for (p, name) in [(dir, "Sezon 1.mkv"), (txt, "notatki.txt")] {
            let url = file_source_url(&p).unwrap();
            let e = resolver().resolve(&url).await.unwrap_err();
            assert_eq!(
                e.0.render(crate::i18n::Lang::Pl),
                format!("To nie jest plik wideo: {name}")
            );
        }
    }

    #[tokio::test]
    async fn malformed_url_is_an_error() {
        let e = resolver().resolve("file:relative.mkv").await.unwrap_err();
        assert_eq!(
            e.0.render(crate::i18n::Lang::Pl),
            "Niepoprawny adres pliku z dysku."
        );
    }
}
