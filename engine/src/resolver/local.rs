//! Disk files of the owner, stored in the queue as `local:<root>/<relative path>`
//! (port of `src/resolver/local.ts`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use super::{MediaMeta, MediaResolver, ProbeFuture, ResolveError, ResolvedStream, StreamInput};
use crate::library::{MediaLibrary, parse_source_url};

/// Duration probe for a local file.
pub type FileDurationProbe = Box<dyn Fn(PathBuf) -> ProbeFuture + Send + Sync>;

pub struct LocalResolver {
    pub library: Arc<MediaLibrary>,
    pub probe_duration: FileDurationProbe,
}

fn title_of(file: &Path) -> String {
    file.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl LocalResolver {
    pub fn new(library: Arc<MediaLibrary>, probe_duration: FileDurationProbe) -> Self {
        Self {
            library,
            probe_duration,
        }
    }

    fn file(&self, url: &str) -> Result<PathBuf, ResolveError> {
        let (root, rel) = parse_source_url(url)
            .ok_or_else(|| ResolveError::new("Niepoprawny adres pliku z dysku."))?;
        let full = self.library.resolve(root, &rel)?;
        if !std::fs::metadata(&full).is_ok_and(|m| m.is_file()) {
            let name = full
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Err(ResolveError::new(format!(
                "To nie jest plikiem wideo: {name}"
            )));
        }
        Ok(full)
    }
}

#[async_trait]
impl MediaResolver for LocalResolver {
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        _tmp: tempfile::TempDir,
        dir: PathBuf,
        lib: Arc<MediaLibrary>,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::create_dir(dir.join("Sezon 1")).unwrap();
        fs::write(dir.join("Sezon 1").join("Odcinek 1.mkv"), "x").unwrap();
        let lib = Arc::new(MediaLibrary::new(vec![dir.clone()]));
        Fixture {
            _tmp: tmp,
            dir,
            lib,
        }
    }

    fn resolver(f: &Fixture) -> LocalResolver {
        LocalResolver::new(
            f.lib.clone(),
            Box::new(|_| Box::pin(async { Some(1440.0) })),
        )
    }

    #[tokio::test]
    async fn meta_title_is_file_stem_and_duration_comes_from_probe() {
        let f = fixture();
        let url = f.lib.to_source_url(0, "Sezon 1/Odcinek 1.mkv");
        assert_eq!(
            resolver(&f).fetch_meta(&url).await.unwrap(),
            MediaMeta {
                title: "Odcinek 1".into(),
                duration_s: Some(1440.0),
                is_live: false,
                is_playlist: false,
                entries: None,
            }
        );
    }

    #[tokio::test]
    async fn resolve_gives_the_file_path_without_headers() {
        let f = fixture();
        let url = f.lib.to_source_url(0, "Sezon 1/Odcinek 1.mkv");
        let s = resolver(&f).resolve(&url).await.unwrap();
        assert_eq!(
            s.inputs,
            vec![StreamInput {
                url: f
                    .dir
                    .join("Sezon 1")
                    .join("Odcinek 1.mkv")
                    .to_string_lossy()
                    .into_owned(),
                headers: vec![],
            }]
        );
        assert_eq!(s.title, "Odcinek 1");
        assert_eq!(s.duration_s, None);
    }

    #[tokio::test]
    async fn deleted_file_gives_does_not_exist_error() {
        let f = fixture();
        let url = f.lib.to_source_url(0, "Sezon 1/Usunięty.mkv");
        let e = resolver(&f).resolve(&url).await.unwrap_err();
        assert_eq!(e.to_string(), "Plik nie istnieje: Usunięty.mkv");
    }

    #[tokio::test]
    async fn folder_instead_of_file_is_an_error() {
        let f = fixture();
        let url = f.lib.to_source_url(0, "Sezon 1");
        let e = resolver(&f).resolve(&url).await.unwrap_err();
        assert_eq!(e.to_string(), "To nie jest plikiem wideo: Sezon 1");
        assert!(resolver(&f).fetch_meta(&url).await.is_err());
    }

    #[tokio::test]
    async fn malformed_source_url_is_an_error() {
        let f = fixture();
        let e = resolver(&f).resolve("local:x/abc").await.unwrap_err();
        assert_eq!(e.to_string(), "Niepoprawny adres pliku z dysku.");
    }
}
