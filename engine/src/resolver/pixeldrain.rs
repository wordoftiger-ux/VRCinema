//! pixeldrain resolver (port of `src/resolver/pixeldrain.ts`).

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::Value;
use url::Url;

use crate::live::LiveValue;

use super::{
    MediaMeta, MediaResolver, PlaylistEntry, ProbeFuture, ResolveError, ResolvedStream, StreamInput,
};

const API: &str = "https://pixeldrain.com/api";
const HOSTS: [&str; 3] = ["pixeldrain.com", "pixeldrain.net", "pixeldra.in"];
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

pub fn is_pixeldrain_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_lowercase();
    HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixeldrainKind {
    File,
    List,
}

/// Recognises `/u/<id>`, `/l/<id>`, `/api/file/<id>` and `/api/list/<id>` (anything after the id
/// is ignored).
pub fn parse_pixeldrain_url(url: &Url) -> Option<(PixeldrainKind, String)> {
    let path = url.path().strip_prefix('/')?;
    let (kind, rest) = [
        ("u/", PixeldrainKind::File),
        ("l/", PixeldrainKind::List),
        ("api/file/", PixeldrainKind::File),
        ("api/list/", PixeldrainKind::List),
    ]
    .into_iter()
    .find_map(|(prefix, kind)| path.strip_prefix(prefix).map(|rest| (kind, rest)))?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if id.is_empty() {
        return None;
    }
    Some((kind, id))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Minimal GET abstraction so the resolver can be tested without a network.
/// The error string is a user-facing description of the transport failure.
#[async_trait]
pub trait HttpGet: Send + Sync {
    async fn get(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, String>;
}

/// reqwest (rustls) implementation with a 20 s timeout.
pub struct ReqwestHttp {
    client: reqwest::Client,
}

impl ReqwestHttp {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?,
        })
    }
}

#[async_trait]
impl HttpGet for ReqwestHttp {
    async fn get(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, String> {
        let mut request = self.client.get(url);
        for (k, v) in headers {
            request = request.header(k.as_str(), v.as_str());
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("pixeldrain: błąd połączenia ({e})."))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| format!("pixeldrain: błąd odczytu odpowiedzi ({e})."))?;
        Ok(HttpResponse { status, body })
    }
}

/// Duration probe: `(url, headers)` -> seconds.
pub type DurationProbe = Box<dyn Fn(String, Vec<(String, String)>) -> ProbeFuture + Send + Sync>;

pub struct PixeldrainResolver {
    pub http: Box<dyn HttpGet>,
    /// File length in seconds (ffprobe), because the pixeldrain API does not provide it.
    pub probe_duration: DurationProbe,
    /// Read at every request: the desktop app may change it while the server runs.
    pub api_key: LiveValue<Option<String>>,
}

struct FileInfo {
    name: String,
    mime_type: String,
}

impl FileInfo {
    fn from_json(v: &Value) -> Self {
        let text = |key: &str| {
            v.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Self {
            name: text("name"),
            mime_type: text("mime_type"),
        }
    }
}

fn is_media(mime: &str) -> bool {
    mime.starts_with("video/") || mime.starts_with("audio/")
}

impl PixeldrainResolver {
    pub fn new(
        http: Box<dyn HttpGet>,
        probe_duration: DurationProbe,
        api_key: Option<String>,
    ) -> Self {
        Self {
            http,
            probe_duration,
            api_key: LiveValue::new(api_key),
        }
    }

    /// Shares the API-key setting with the app (see [`crate::live`]).
    pub fn with_api_key(mut self, api_key: LiveValue<Option<String>>) -> Self {
        self.api_key = api_key;
        self
    }

    fn headers(&self) -> Vec<(String, String)> {
        match self.api_key.get().filter(|k| !k.is_empty()) {
            Some(key) => {
                let token = base64::engine::general_purpose::STANDARD.encode(format!(":{key}"));
                vec![("Authorization".to_string(), format!("Basic {token}"))]
            }
            None => Vec::new(),
        }
    }

    async fn get_json(&self, url: &str) -> Result<Value, ResolveError> {
        let res = self
            .http
            .get(url, &self.headers())
            .await
            .map_err(ResolveError)?;
        if !(200..300).contains(&res.status) {
            return Err(ResolveError::new(format!(
                "pixeldrain: plik niedostępny (HTTP {}).",
                res.status
            )));
        }
        serde_json::from_str(&res.body)
            .map_err(|_| ResolveError::new("pixeldrain: niepoprawna odpowiedź serwera."))
    }

    fn parse(url: &str) -> Result<(PixeldrainKind, String), ResolveError> {
        Url::parse(url)
            .ok()
            .and_then(|u| parse_pixeldrain_url(&u))
            .ok_or_else(|| {
                ResolveError::new("pixeldrain: nierozpoznany adres. Użyj linku /u/… albo /l/….")
            })
    }

    async fn duration(&self, stream_url: &str) -> Option<f64> {
        (self.probe_duration)(stream_url.to_string(), self.headers()).await
    }
}

#[async_trait]
impl MediaResolver for PixeldrainResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        let (kind, id) = Self::parse(url)?;
        if kind == PixeldrainKind::List {
            let list = self.get_json(&format!("{API}/list/{id}")).await?;
            let entries = list
                .get("files")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter(|f| is_media(f.get("mime_type").and_then(Value::as_str).unwrap_or("")))
                .map(|f| PlaylistEntry {
                    url: format!(
                        "https://pixeldrain.com/u/{}",
                        f.get("id").and_then(Value::as_str).unwrap_or_default()
                    ),
                    title: f
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    duration_s: None,
                })
                .collect();
            let title = match list.get("title").and_then(Value::as_str) {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => "Lista pixeldrain".to_string(),
            };
            return Ok(MediaMeta {
                title,
                duration_s: None,
                is_live: false,
                is_playlist: true,
                entries: Some(entries),
            });
        }
        let info = FileInfo::from_json(&self.get_json(&format!("{API}/file/{id}/info")).await?);
        if !is_media(&info.mime_type) {
            return Err(ResolveError::new(format!(
                "pixeldrain: \"{}\" nie jest plikiem wideo.",
                info.name
            )));
        }
        let duration_s = self.duration(&format!("{API}/file/{id}")).await;
        Ok(MediaMeta {
            title: info.name,
            duration_s,
            is_live: false,
            is_playlist: false,
            entries: None,
        })
    }

    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        let (kind, id) = Self::parse(url)?;
        if kind == PixeldrainKind::List {
            return Err(ResolveError::new(
                "pixeldrain: listę trzeba najpierw rozwinąć na pojedyncze pliki.",
            ));
        }
        let info = FileInfo::from_json(&self.get_json(&format!("{API}/file/{id}/info")).await?);
        let stream_url = format!("{API}/file/{id}");
        let duration_s = self.duration(&stream_url).await;
        Ok(ResolvedStream {
            inputs: vec![StreamInput {
                url: stream_url,
                headers: self.headers(),
            }],
            title: info.name,
            duration_s,
            is_live: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn host_check_includes_subdomains_and_rejects_lookalikes() {
        assert!(is_pixeldrain_host(&u("https://pixeldrain.com/u/a")));
        assert!(is_pixeldrain_host(&u("https://PIXELDRAIN.net/u/a")));
        assert!(is_pixeldrain_host(&u("https://pixeldra.in/u/a")));
        assert!(is_pixeldrain_host(&u("https://cdn.pixeldrain.com/u/a")));
        assert!(!is_pixeldrain_host(&u("https://notpixeldrain.com/u/a")));
        assert!(!is_pixeldrain_host(&u("https://youtu.be/x")));
    }

    #[test]
    fn recognises_file_and_list_urls() {
        let file = |id: &str| Some((PixeldrainKind::File, id.to_string()));
        let list = |id: &str| Some((PixeldrainKind::List, id.to_string()));
        assert_eq!(
            parse_pixeldrain_url(&u("https://pixeldrain.com/u/abc123")),
            file("abc123")
        );
        assert_eq!(
            parse_pixeldrain_url(&u("https://pixeldrain.com/l/lst9#item=2")),
            list("lst9")
        );
        assert_eq!(
            parse_pixeldrain_url(&u("https://pixeldrain.com/api/file/abc123?download")),
            file("abc123")
        );
        assert_eq!(
            parse_pixeldrain_url(&u("https://pixeldrain.com/api/list/L_1-x")),
            list("L_1-x")
        );
        assert_eq!(parse_pixeldrain_url(&u("https://pixeldrain.com/")), None);
        assert_eq!(parse_pixeldrain_url(&u("https://pixeldrain.com/u/")), None);
        assert_eq!(
            parse_pixeldrain_url(&u("https://pixeldrain.com/x/abc")),
            None
        );
    }

    struct FakeHttp;

    #[async_trait]
    impl HttpGet for FakeHttp {
        async fn get(
            &self,
            url: &str,
            _headers: &[(String, String)],
        ) -> Result<HttpResponse, String> {
            let ok = |v: Value| {
                Ok(HttpResponse {
                    status: 200,
                    body: v.to_string(),
                })
            };
            if url.ends_with("/api/file/abc/info") {
                ok(json!({"id": "abc", "name": "Film ąę.mp4", "mime_type": "video/mp4"}))
            } else if url.ends_with("/api/file/doc/info") {
                ok(json!({"id": "doc", "name": "a.pdf", "mime_type": "application/pdf"}))
            } else if url.ends_with("/api/list/L1") {
                ok(json!({"id": "L1", "title": "Lista", "files": [
                    {"id": "f1", "name": "one.mkv", "mime_type": "video/x-matroska"},
                    {"id": "f2", "name": "readme.txt", "mime_type": "text/plain"},
                ]}))
            } else if url.ends_with("/api/file/bad/info") {
                Ok(HttpResponse {
                    status: 200,
                    body: "nie json".into(),
                })
            } else {
                Ok(HttpResponse {
                    status: 404,
                    body: "not found".into(),
                })
            }
        }
    }

    fn resolver(duration: Option<f64>, api_key: Option<&str>) -> PixeldrainResolver {
        PixeldrainResolver::new(
            Box::new(FakeHttp),
            Box::new(move |_, _| Box::pin(async move { duration })),
            api_key.map(str::to_string),
        )
    }

    #[tokio::test]
    async fn file_metadata_with_duration_from_ffprobe() {
        let m = resolver(Some(125.5), None)
            .fetch_meta("https://pixeldrain.com/u/abc")
            .await
            .unwrap();
        assert_eq!(
            m,
            MediaMeta {
                title: "Film ąę.mp4".into(),
                duration_s: Some(125.5),
                is_live: false,
                is_playlist: false,
                entries: None,
            }
        );
    }

    #[tokio::test]
    async fn rejects_files_that_are_not_video() {
        let e = resolver(None, None)
            .fetch_meta("https://pixeldrain.com/u/doc")
            .await
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "pixeldrain: \"a.pdf\" nie jest plikiem wideo."
        );
    }

    #[tokio::test]
    async fn missing_file_gives_a_readable_error() {
        let e = resolver(None, None)
            .fetch_meta("https://pixeldrain.com/u/zzz")
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "pixeldrain: plik niedostępny (HTTP 404).");
    }

    #[tokio::test]
    async fn invalid_json_and_unknown_urls_are_errors() {
        let e = resolver(None, None)
            .fetch_meta("https://pixeldrain.com/u/bad")
            .await
            .unwrap_err();
        assert!(e.to_string().starts_with("pixeldrain:"));
        let e = resolver(None, None)
            .fetch_meta("https://pixeldrain.com/")
            .await
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "pixeldrain: nierozpoznany adres. Użyj linku /u/… albo /l/…."
        );
    }

    #[tokio::test]
    async fn list_becomes_a_playlist_of_video_files_only() {
        let m = resolver(None, None)
            .fetch_meta("https://pixeldrain.com/l/L1")
            .await
            .unwrap();
        assert!(m.is_playlist);
        assert_eq!(m.title, "Lista");
        assert_eq!(
            m.entries.unwrap(),
            vec![PlaylistEntry {
                url: "https://pixeldrain.com/u/f1".into(),
                title: "one.mkv".into(),
                duration_s: None,
            }]
        );
        let e = resolver(None, None)
            .resolve("https://pixeldrain.com/l/L1")
            .await
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "pixeldrain: listę trzeba najpierw rozwinąć na pojedyncze pliki."
        );
    }

    #[tokio::test]
    async fn resolve_gives_the_api_url_and_key_when_set() {
        let s = resolver(Some(10.0), Some("KEY"))
            .resolve("https://pixeldrain.com/u/abc")
            .await
            .unwrap();
        assert_eq!(s.inputs.len(), 1);
        assert_eq!(s.inputs[0].url, "https://pixeldrain.com/api/file/abc");
        // base64(":KEY")
        assert_eq!(
            s.inputs[0].headers,
            vec![("Authorization".to_string(), "Basic OktFWQ==".to_string())]
        );
        assert_eq!(s.title, "Film ąę.mp4");
        assert_eq!(s.duration_s, Some(10.0));

        let s = resolver(None, None)
            .resolve("https://pixeldrain.com/u/abc")
            .await
            .unwrap();
        assert!(s.inputs[0].headers.is_empty());
        assert_eq!(s.duration_s, None);
    }

    #[test]
    fn live_api_key_is_read_at_each_request() {
        let key = crate::live::LiveValue::new(None);
        let r = resolver(None, Some("stary")).with_api_key(key.clone());
        assert!(r.headers().is_empty());
        key.set(Some("nowy".into()));
        let headers = r.headers();
        assert_eq!(headers[0].0, "Authorization");
        let token = base64::engine::general_purpose::STANDARD.encode(":nowy");
        assert_eq!(headers[0].1, format!("Basic {token}"));
    }
}
