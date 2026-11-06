//! Canned resolver for black-box tests (`VRCINEMA_FAKE_RESOLVER=<file.json>`).
//!
//! The file maps a URL either to a `MediaMeta`-like object
//! `{"title", "durationS", "isLive", "isPlaylist", "entries": [{"url","title","durationS"}]}`
//! or to `{"error": "message"}`. An entry with `"inputs": [{"url", "headers"?}]` can also be
//! resolved; without it `resolve` fails.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use super::{MediaMeta, MediaResolver, PlaylistEntry, ResolveError, ResolvedStream, StreamInput};

enum FakeEntry {
    Error(String),
    Media {
        meta: MediaMeta,
        inputs: Option<Vec<StreamInput>>,
    },
}

pub struct FakeResolver {
    entries: HashMap<String, FakeEntry>,
}

fn duration(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn parse_entry(v: &Value) -> FakeEntry {
    if let Some(message) = v.get("error").and_then(Value::as_str) {
        return FakeEntry::Error(message.to_string());
    }
    let entries = v.get("entries").and_then(Value::as_array).map(|list| {
        list.iter()
            .map(|e| PlaylistEntry {
                url: e
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                title: e
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                duration_s: duration(e.get("durationS")),
                is_live: false,
            })
            .collect()
    });
    let inputs = v.get("inputs").and_then(Value::as_array).map(|list| {
        list.iter()
            .map(|i| StreamInput {
                url: i
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                headers: i
                    .get("headers")
                    .and_then(Value::as_object)
                    .map(|h| {
                        h.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    });
    FakeEntry::Media {
        meta: MediaMeta {
            title: v
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            duration_s: duration(v.get("durationS")),
            is_live: v.get("isLive").and_then(Value::as_bool).unwrap_or(false),
            is_playlist: v
                .get("isPlaylist")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            entries,
        },
        inputs,
    }
}

impl FakeResolver {
    pub fn from_json(json: &Value) -> Result<Self, ResolveError> {
        let map = json.as_object().ok_or_else(|| {
            ResolveError::new("Plik fałszywego resolvera musi być obiektem JSON.")
        })?;
        Ok(Self {
            entries: map
                .iter()
                .map(|(k, v)| (k.clone(), parse_entry(v)))
                .collect(),
        })
    }

    pub fn from_file(path: &Path) -> Result<Self, ResolveError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            ResolveError::new(format!(
                "Nie można odczytać pliku fałszywego resolvera {}: {e}",
                path.display()
            ))
        })?;
        let json: Value = serde_json::from_str(&text).map_err(|e| {
            ResolveError::new(format!(
                "Niepoprawny JSON w pliku fałszywego resolvera {}: {e}",
                path.display()
            ))
        })?;
        Self::from_json(&json)
    }

    fn entry(&self, url: &str) -> Result<&FakeEntry, ResolveError> {
        match self.entries.get(url) {
            Some(FakeEntry::Error(message)) => Err(ResolveError::new(message.clone())),
            Some(entry) => Ok(entry),
            None => Err(ResolveError::new("Unsupported URL")),
        }
    }
}

#[async_trait]
impl MediaResolver for FakeResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        match self.entry(url)? {
            FakeEntry::Media { meta, .. } => Ok(meta.clone()),
            FakeEntry::Error(message) => Err(ResolveError::new(message.clone())),
        }
    }

    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        match self.entry(url)? {
            FakeEntry::Media {
                meta,
                inputs: Some(inputs),
            } => Ok(ResolvedStream {
                inputs: inputs.clone(),
                title: meta.title.clone(),
                duration_s: meta.duration_s,
                is_live: meta.is_live,
            }),
            _ => Err(ResolveError::new("fałszywy resolver nie odtwarza")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fake() -> FakeResolver {
        FakeResolver::from_json(&json!({
            "https://ok/video": {"title": "Film", "durationS": 12.5, "isLive": false, "isPlaylist": false},
            "https://ok/playlist": {"title": "Lista", "durationS": null, "isLive": false, "isPlaylist": true,
                "entries": [{"url": "https://ok/video", "title": "Film", "durationS": 12.5}]},
            "https://ok/play": {"title": "Gra", "durationS": 3, "inputs": [{"url": "http://x/a.mp4", "headers": {"A": "b"}}]},
            "https://bad": {"error": "Video unavailable"}
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn returns_canned_metadata_and_errors() {
        let f = fake();
        let m = f.fetch_meta("https://ok/video").await.unwrap();
        assert_eq!(m.title, "Film");
        assert_eq!(m.duration_s, Some(12.5));
        let p = f.fetch_meta("https://ok/playlist").await.unwrap();
        assert!(p.is_playlist);
        assert_eq!(p.entries.unwrap()[0].url, "https://ok/video");
        assert_eq!(
            f.fetch_meta("https://bad").await.unwrap_err().to_string(),
            "Video unavailable"
        );
        assert_eq!(
            f.fetch_meta("https://nope").await.unwrap_err().to_string(),
            "Unsupported URL"
        );
    }

    #[tokio::test]
    async fn resolve_only_works_with_inputs() {
        let f = fake();
        assert_eq!(
            f.resolve("https://ok/video").await.unwrap_err().to_string(),
            "fałszywy resolver nie odtwarza"
        );
        let s = f.resolve("https://ok/play").await.unwrap();
        assert_eq!(s.title, "Gra");
        assert_eq!(s.inputs[0].url, "http://x/a.mp4");
        assert_eq!(
            s.inputs[0].headers,
            vec![("A".to_string(), "b".to_string())]
        );
    }

    #[tokio::test]
    async fn loads_from_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fake.json");
        std::fs::write(&path, r#"{"https://a": {"title": "A"}}"#).unwrap();
        let f = FakeResolver::from_file(&path).unwrap();
        assert_eq!(f.fetch_meta("https://a").await.unwrap().title, "A");
        assert!(FakeResolver::from_file(&tmp.path().join("missing.json")).is_err());
        std::fs::write(&path, "nie json").unwrap();
        assert!(FakeResolver::from_file(&path).is_err());
    }
}
