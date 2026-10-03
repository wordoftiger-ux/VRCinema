//! Media resolvers: turn a user-supplied source URL into metadata and playable inputs.
//!
//! Port of `src/resolver/{types,index}.ts`.

pub mod fake;
pub mod local;
pub mod pixeldrain;
pub mod types;
pub mod ytdlp;

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

pub use types::{MediaMeta, PlaylistEntry, ResolvedStream, StreamInput};

/// Resolver failure; the `Display` text is the user-facing (Polish) message and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ResolveError(pub String);

impl ResolveError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
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
}

/// Routes disk files (`local:`) to the file module, pixeldrain hosts to their own module and
/// everything else to yt-dlp.
pub struct CompositeResolver {
    pub pixeldrain: Box<dyn MediaResolver>,
    pub ytdlp: Box<dyn MediaResolver>,
    pub local: Option<Box<dyn MediaResolver>>,
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
        }
    }

    fn pick(&self, url: &str) -> Result<&dyn MediaResolver, ResolveError> {
        if url.starts_with("local:") {
            return match &self.local {
                Some(local) => Ok(local.as_ref()),
                None => Err(ResolveError::new(
                    "Pliki z dysku są wyłączone (brak MEDIA_DIRS w .env).",
                )),
            };
        }
        let parsed = url::Url::parse(url)
            .map_err(|_| ResolveError::new(format!("Niepoprawny adres URL: {url}")))?;
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
        let msg = c.fetch_meta("local:0/a.mp4").await.unwrap_err().to_string();
        assert_eq!(msg, "Pliki z dysku są wyłączone (brak MEDIA_DIRS w .env).");
        assert!(c.resolve("local:0/a.mp4").await.is_err());
    }

    #[tokio::test]
    async fn malformed_url_is_an_error_not_a_panic() {
        let c = CompositeResolver::new(tag("pd"), tag("yt"), None);
        assert!(c.fetch_meta("nie url").await.is_err());
    }
}
