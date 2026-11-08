//! Media resolvers: turn a user-supplied source URL into metadata and playable inputs.
//!
//! Port of `src/resolver/{types,index}.ts`.

pub mod fake;
pub mod file;
pub mod local;
pub mod pixeldrain;
pub mod types;
pub mod ytdlp;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use async_trait::async_trait;

pub use types::{MediaMeta, PlaylistEntry, ResolvedStream, StreamInput};

use crate::i18n::Msg;

/// Resolver failure: the message for the person (an `errors.` one, rendered in their
/// language; `Display`: English).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ResolveError(pub Msg);

impl ResolveError {
    /// A message from outside the program (yt-dlp, a server), shown as it is.
    pub fn new(message: impl Into<String>) -> Self {
        Self(Msg::error("source_message").arg("detail", message.into()))
    }

    /// The message `errors.<code>`.
    pub fn coded(code: &'static str) -> Self {
        Self(Msg::error(code))
    }
}

impl From<Msg> for ResolveError {
    fn from(msg: Msg) -> Self {
        Self(msg)
    }
}

impl From<crate::library::LibraryError> for ResolveError {
    fn from(e: crate::library::LibraryError) -> Self {
        Self(e.0)
    }
}

/// Future returned by injectable duration probes (`None` = unknown).
pub type ProbeFuture = Pin<Box<dyn Future<Output = Option<f64>> + Send>>;

#[async_trait]
pub trait MediaResolver: Send + Sync {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError>;
    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError>;

    /// [`Self::fetch_meta`] reading at most `max_entries` entries of a playlist (a viewer's
    /// list must not make yt-dlp walk a playlist of thousands). The default reads everything
    /// and keeps the first `max_entries`.
    async fn fetch_meta_capped(
        &self,
        url: &str,
        max_entries: usize,
    ) -> Result<MediaMeta, ResolveError> {
        let mut meta = self.fetch_meta(url).await?;
        if let Some(entries) = meta.entries.as_mut() {
            entries.truncate(max_entries);
        }
        Ok(meta)
    }

    /// Resolves `url` again with yt-dlp's fallback YouTube client
    /// ([`ytdlp::FALLBACK_YOUTUBE_CLIENT`]), for an address YouTube refused; `None`: this
    /// resolver has no such fallback (everything but yt-dlp).
    async fn resolve_with_fallback_client(
        &self,
        _url: &str,
    ) -> Option<Result<ResolvedStream, ResolveError>> {
        None
    }

    /// Whether yt-dlp is given a JavaScript runtime for `url` (`None`: not resolved by yt-dlp).
    fn js_runtime_passed(&self, _url: &str) -> Option<bool> {
        None
    }

    /// Downloads the film behind `url` into a file `<stem>.<ext>` and returns its path, for a
    /// site whose media addresses only yt-dlp itself may fetch (TikTok); `None`: this resolver
    /// cannot (everything but yt-dlp).
    async fn download(&self, _url: &str, _stem: &Path) -> Option<Result<PathBuf, ResolveError>> {
        None
    }

    /// Like [`Self::download`], for the window's Preview: larger films and more time
    /// (`None`: this resolver cannot download).
    async fn download_preview(
        &self,
        _url: &str,
        _stem: &Path,
    ) -> Option<Result<PathBuf, ResolveError>> {
        None
    }

    /// Fetches the subtitles of `url` in `lang` (Settings → Advanced) into a file next to
    /// `stem` and returns its path; `Some(Ok(None))`: the film has none in that language;
    /// `None`: this resolver cannot (everything but yt-dlp).
    async fn subtitles(
        &self,
        _url: &str,
        _lang: &str,
        _stem: &Path,
    ) -> Option<Result<Option<PathBuf>, ResolveError>> {
        None
    }
}

/// Routes disk files (`local:` library files, `file:` absolute paths) to the file modules, pixeldrain hosts to their own module and
/// everything else to yt-dlp.
pub struct CompositeResolver {
    pub pixeldrain: Box<dyn MediaResolver>,
    pub ytdlp: Box<dyn MediaResolver>,
    pub local: Option<Box<dyn MediaResolver>>,
    /// Files given by absolute path (`file:`), created only by the desktop window.
    pub file: Option<Box<dyn MediaResolver>>,
    /// Files viewers sent (`upload:`), see [`crate::uploads`].
    pub upload: Option<Box<dyn MediaResolver>>,
}

impl CompositeResolver {
    pub fn new(
        pixeldrain: Box<dyn MediaResolver>,
        ytdlp: Box<dyn MediaResolver>,
        local: Option<Box<dyn MediaResolver>>,
    ) -> Self {
        Self {
            pixeldrain,
            ytdlp,
            local,
            file: None,
            upload: None,
        }
    }

    pub fn with_file(mut self, file: Box<dyn MediaResolver>) -> Self {
        self.file = Some(file);
        self
    }

    pub fn with_upload(mut self, upload: Box<dyn MediaResolver>) -> Self {
        self.upload = Some(upload);
        self
    }

    fn pick(&self, url: &str) -> Result<&dyn MediaResolver, ResolveError> {
        if crate::uploads::is_upload_source(url) {
            return self
                .upload
                .as_deref()
                .ok_or_else(|| ResolveError::coded("upload_gone"));
        }
        if url.starts_with("local:") {
            return match &self.local {
                Some(local) => Ok(local.as_ref()),
                None => Err(ResolveError::coded("local_files_disabled")),
            };
        }
        if url
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case("file:"))
        {
            return match &self.file {
                Some(file) => Ok(file.as_ref()),
                None => Err(ResolveError::coded("file_paths_unavailable")),
            };
        }
        let parsed = url::Url::parse(url)
            .map_err(|_| ResolveError(Msg::error("invalid_url").arg("url", url)))?;
        Ok(if pixeldrain::is_pixeldrain_host(&parsed) {
            self.pixeldrain.as_ref()
        } else {
            self.ytdlp.as_ref()
        })
    }
}

#[async_trait]
impl MediaResolver for CompositeResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        self.pick(url)?.fetch_meta(url).await
    }

    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        self.pick(url)?.resolve(url).await
    }

    async fn fetch_meta_capped(
        &self,
        url: &str,
        max_entries: usize,
    ) -> Result<MediaMeta, ResolveError> {
        self.pick(url)?.fetch_meta_capped(url, max_entries).await
    }

    async fn resolve_with_fallback_client(
        &self,
        url: &str,
    ) -> Option<Result<ResolvedStream, ResolveError>> {
        self.pick(url).ok()?.resolve_with_fallback_client(url).await
    }

    fn js_runtime_passed(&self, url: &str) -> Option<bool> {
        self.pick(url).ok()?.js_runtime_passed(url)
    }

    async fn download(&self, url: &str, stem: &Path) -> Option<Result<PathBuf, ResolveError>> {
        self.pick(url).ok()?.download(url, stem).await
    }

    async fn download_preview(
        &self,
        url: &str,
        stem: &Path,
    ) -> Option<Result<PathBuf, ResolveError>> {
        self.pick(url).ok()?.download_preview(url, stem).await
    }

    async fn subtitles(
        &self,
        url: &str,
        lang: &str,
        stem: &Path,
    ) -> Option<Result<Option<PathBuf>, ResolveError>> {
        self.pick(url).ok()?.subtitles(url, lang, stem).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tag(&'static str);

    #[async_trait]
    impl MediaResolver for Tag {
        async fn fetch_meta(&self, _url: &str) -> Result<MediaMeta, ResolveError> {
            Ok(MediaMeta {
                title: self.0.into(),
                duration_s: Some(1.0),
                is_live: false,
                is_playlist: false,
                entries: None,
            })
        }
        async fn resolve(&self, _url: &str) -> Result<ResolvedStream, ResolveError> {
            Ok(ResolvedStream {
                inputs: vec![],
                title: self.0.into(),
                duration_s: Some(1.0),
                is_live: false,
            })
        }
    }

    fn tag(name: &'static str) -> Box<dyn MediaResolver> {
        Box::new(Tag(name))
    }

    async fn title(c: &CompositeResolver, url: &str) -> String {
        c.fetch_meta(url).await.unwrap().title
    }

    #[tokio::test]
    async fn routes_pixeldrain_to_its_module_and_the_rest_to_ytdlp() {
        let c = CompositeResolver::new(tag("pd"), tag("yt"), None);
        assert_eq!(title(&c, "https://pixeldrain.com/u/a").await, "pd");
        assert_eq!(title(&c, "https://www.cda.pl/video/1").await, "yt");
        assert_eq!(
            c.resolve("https://pixeldrain.com/u/a").await.unwrap().title,
            "pd"
        );
        assert_eq!(c.resolve("https://youtu.be/x").await.unwrap().title, "yt");
    }

    #[tokio::test]
    async fn routes_local_to_the_file_module() {
        let c = CompositeResolver::new(tag("pd"), tag("yt"), Some(tag("local")));
        assert_eq!(title(&c, "local:0/a.mp4").await, "local");
        assert_eq!(title(&c, "https://youtu.be/x").await, "yt");
        assert_eq!(c.resolve("local:0/a.mp4").await.unwrap().title, "local");
    }

    #[tokio::test]
    async fn local_without_libraries_is_rejected() {
        let c = CompositeResolver::new(tag("pd"), tag("yt"), None);
        let msg = c
            .fetch_meta("local:0/a.mp4")
            .await
            .unwrap_err()
            .0
            .render(crate::i18n::Lang::Pl);
        assert_eq!(msg, "Pliki z dysku są wyłączone (brak MEDIA_DIRS w .env).");
        assert!(c.resolve("local:0/a.mp4").await.is_err());
    }

    #[tokio::test]
    async fn routes_file_urls_to_the_file_resolver() {
        let c =
            CompositeResolver::new(tag("pd"), tag("yt"), Some(tag("local"))).with_file(tag("file"));
        assert_eq!(title(&c, "file:///D:/Filmy/Odcinek%201.mkv").await, "file");
        assert_eq!(
            c.resolve("file:///D:/Filmy/a.mkv").await.unwrap().title,
            "file"
        );
        assert_eq!(title(&c, "local:0/a.mp4").await, "local");
        assert_eq!(title(&c, "https://youtu.be/x").await, "yt");
    }

    #[tokio::test]
    async fn file_urls_without_a_file_resolver_never_reach_ytdlp() {
        let c = CompositeResolver::new(tag("pd"), tag("yt"), None);
        assert!(c.fetch_meta("file:///D:/a.mkv").await.is_err());
        assert!(c.resolve("FILE:///D:/a.mkv").await.is_err());
    }
    #[tokio::test]
    async fn malformed_url_is_an_error_not_a_panic() {
        let c = CompositeResolver::new(tag("pd"), tag("yt"), None);
        assert!(c.fetch_meta("nie url").await.is_err());
    }
}
