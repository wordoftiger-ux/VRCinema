use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::*;

#[derive(Default)]
struct FakeHttp {
    answers: HashMap<String, Value>,
    calls: AtomicUsize,
}

impl FakeHttp {
    fn with(mut self, url: &str, answer: Value) -> Self {
        self.answers.insert(url.to_string(), answer);
        self
    }
}

#[async_trait]
impl AddonHttp for FakeHttp {
    async fn get_json(&self, url: &str, _timeout: Duration) -> Result<Value, AddonError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answers
            .get(url)
            .cloned()
            .ok_or_else(AddonError::unreachable)
    }
}

fn client(http: FakeHttp) -> (AddonClient, Arc<FakeHttp>) {
    let http = Arc::new(http);
    (AddonClient::new(http.clone()), http)
}

fn code(e: &AddonError) -> String {
    e.0.code().unwrap().to_string()
}

fn catalog_manifest(name: &str) -> Manifest {
    Manifest::parse(&json!({
        "id": name, "name": name, "version": "1.0.0",
        "resources": ["catalog", "meta"], "types": ["movie", "series"],
        "catalogs": [
            {"type": "movie", "id": "top", "extra": [{"name": "search"}]},
            {"type": "movie", "id": "second", "extra": [{"name": "search"}]},
            {"type": "series", "id": "top", "extra": [{"name": "genre"}]},
        ]
    }))
    .unwrap()
}

fn entry(base: &str, name: &str) -> (String, Manifest) {
    (format!("{base}/manifest.json"), catalog_manifest(name))
}

#[tokio::test]
async fn cache_is_capped_and_search_text_clamped() {
    let url = |q: &str| format!("https://one.example/catalog/movie/top/search={q}.json");
    let mut http = FakeHttp::default();
    for i in 0..300 {
        http = http.with(&url(&format!("q{i}")), json!({"metas": []}));
    }
    let long = "a".repeat(100);
    http = http.with(&url(&long), json!({"metas": []}));
    let (c, http) = client(http);
    let addons = vec![entry("https://one.example", "one")];
    for i in 0..300 {
        c.search(&addons, &format!("q{i}")).await;
    }
    assert_eq!(c.cache_len(), 256);
    // Search text is cut to 100 characters.
    c.search(&addons, &"a".repeat(500)).await;
    assert_eq!(http.calls.load(Ordering::SeqCst), 301);
    assert_eq!(c.cache_len(), 256);
}

#[tokio::test]
async fn search_drops_types_other_than_movie_and_series() {
    let http = FakeHttp::default().with(
        &hit_url("https://one.example", "top"),
        json!({"metas": [
            {"id": "a", "type": "movie", "name": "A"},
            {"id": "b", "type": "tv", "name": "B"},
            {"id": "c", "type": "series", "name": "C"},
        ]}),
    );
    let (c, _) = client(http);
    let addons = vec![entry("https://one.example", "one")];
    let ids: Vec<_> = c
        .search(&addons, "the matrix")
        .await
        .into_iter()
        .map(|h| h.id)
        .collect();
    assert_eq!(ids, ["a", "c"]);
}

#[test]
fn manifest_tolerates_missing_newer_fields() {
    let m: Manifest = serde_json::from_str(r#"{"id":"x","name":"X"}"#).unwrap();
    assert!(m.resources.is_empty() && m.catalogs.is_empty());
}

#[test]
fn normalizes_stremio_scheme_and_rejects_non_manifest() {
    assert_eq!(
        normalize_manifest_url("stremio://a.example/x/manifest.json").unwrap(),
        "https://a.example/x/manifest.json"
    );
    assert_eq!(
        normalize_manifest_url(" https://a.example/x/manifest.json ").unwrap(),
        "https://a.example/x/manifest.json"
    );
    let e = normalize_manifest_url("https://a.example/x").unwrap_err();
    assert_eq!(code(&e), "addon_invalid");
    for bad in [
        "https://a.example/x/manifest.json?key=1",
        "https://a.example/x/manifest.json#top",
    ] {
        assert_eq!(
            code(&normalize_manifest_url(bad).unwrap_err()),
            "addon_invalid"
        );
    }
    assert_eq!(
        base_url("https://a.example/x/manifest.json"),
        "https://a.example/x"
    );
}

#[test]
fn manifest_resources_string_and_object_forms() {
    let m = Manifest::parse(&json!({
        "id": "a", "name": "A", "version": "1",
        "resources": ["stream"], "types": ["movie"], "idPrefixes": ["tt"]
    }))
    .unwrap();
    assert!(m.supports("stream", "movie", "tt1"));
    assert!(!m.supports("stream", "movie", "kitsu:1"));
    assert!(!m.supports("stream", "series", "tt1"));
    assert!(!m.supports("meta", "movie", "tt1"));

    let m = Manifest::parse(&json!({
        "id": "a", "name": "A", "version": "1", "types": ["movie", "series"],
        "resources": [{"name": "meta", "types": ["series"]}, "catalog"]
    }))
    .unwrap();
    assert!(m.supports("meta", "series", "tt1"));
    assert!(!m.supports("meta", "movie", "tt1"));

    assert!(Manifest::parse(&json!({"name": "no id"})).is_err());
    // The stored form survives a round trip.
    let again: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
    assert_eq!(again, m);
}

#[test]
fn legacy_extra_supported_means_search() {
    let m = Manifest::parse(&json!({
        "id": "a", "name": "A", "version": "1", "resources": ["catalog"], "types": ["movie"],
        "catalogs": [{"type": "movie", "id": "c", "extraSupported": ["search"]}]
    }))
    .unwrap();
    assert!(m.provides_search());
}

fn hit_url(base: &str, catalog: &str) -> String {
    format!("{base}/catalog/movie/{catalog}/search=the%20matrix.json")
}

#[tokio::test]
async fn search_merges_dedupes_and_skips_failing_addons() {
    let http = FakeHttp::default()
        .with(
            &hit_url("https://one.example", "top"),
            json!({"metas": [
                {"id": "tt1", "type": "movie", "name": "One", "releaseInfo": "1999", "poster": "https://p/1.jpg"}
            ]}),
        )
        .with(
            &hit_url("https://two.example", "top"),
            json!({"metas": [
                {"id": "tt1", "type": "movie", "name": "One again"},
                {"id": "tt2", "type": "movie", "name": "Two"}
            ]}),
        );
    let (c, _) = client(http);
    let addons = vec![
        entry("https://one.example", "one"),
        entry("https://two.example", "two"),
        entry("https://bad.example", "bad"),
    ];
    let hits = c.search(&addons, "the matrix").await;
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["tt1", "tt2"]);
    assert_eq!(hits[0].name, "One");
    assert_eq!(hits[0].year.as_deref(), Some("1999"));
}

#[tokio::test]
async fn meta_sorts_episodes_and_extracts_year() {
    let http = FakeHttp::default().with(
        "https://one.example/meta/series/tt9.json",
        json!({"meta": {
            "id": "tt9", "type": "series", "name": "Show", "releaseInfo": "2011-2019",
            "videos": [
                {"id": "tt9:2:1", "season": 2, "episode": 1, "title": "d"},
                {"id": "tt9:1:2", "season": 1, "episode": 2, "name": "b"},
                {"id": "tt9:1:1", "season": 1, "episode": 1, "title": "a"},
                {"id": "tt9:0:1", "season": 0, "episode": 1, "title": "sp"},
            ]
        }}),
    );
    let (c, _) = client(http);
    let addons = vec![entry("https://one.example", "one")];
    let m = c.meta(&addons, "series", "tt9").await.unwrap();
    assert_eq!(m.year.as_deref(), Some("2011-2019"));
    let order: Vec<(u32, u32)> = m.videos.iter().map(|v| (v.season, v.episode)).collect();
    assert_eq!(order, [(1, 1), (1, 2), (2, 1), (0, 1)]);
    assert_eq!(m.videos[1].title, "b");

    let e = c.meta(&addons, "movie", "tt404").await.unwrap_err();
    assert_eq!(code(&e), "addon_no_meta");
}

#[tokio::test]
async fn streams_keep_http_and_ytid_drop_torrent_only() {
    let http = FakeHttp::default().with(
        "https://s.example/stream/movie/tt1.json",
        json!({"streams": [
            {"url": "https://cdn.example/a.mp4", "name": "Ex", "title": "1080p"},
            {"infoHash": "abc"},
            {"ytId": "xyz"},
            {"externalUrl": "https://x"}
        ]}),
    );
    let (c, _) = client(http);
    let s = c
        .streams("https://s.example/manifest.json", "movie", "tt1")
        .await
        .unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].name, "Ex");
    assert_eq!(s[1].url, "https://www.youtube.com/watch?v=xyz");
}

#[tokio::test]
async fn streams_bad_addon_is_an_error_not_a_panic() {
    let http = FakeHttp::default().with(
        "https://s.example/stream/movie/tt1.json",
        Value::String("<html>".into()),
    );
    let (c, _) = client(http);
    let e = c
        .streams("https://s.example/manifest.json", "movie", "tt1")
        .await
        .unwrap_err();
    assert_eq!(code(&e), "addon_invalid");
}

#[tokio::test]
async fn cache_serves_second_search_without_http() {
    let http = FakeHttp::default().with(
        &hit_url("https://one.example", "top"),
        json!({"metas": [{"id": "tt1", "type": "movie", "name": "One"}]}),
    );
    let (c, http) = client(http);
    let addons = vec![entry("https://one.example", "one")];
    assert_eq!(c.search(&addons, "the matrix").await.len(), 1);
    assert_eq!(c.search(&addons, "the matrix").await.len(), 1);
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
}
