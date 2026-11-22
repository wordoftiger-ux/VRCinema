//! Client for Stremio-compatible addons (manifest, catalog search, meta, streams).
//!
//! Addons are plain HTTP + JSON. This module fetches and normalises their answers; storage and
//! routes live elsewhere. The app ships no addons: the owner adds manifest URLs.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::i18n::Msg;

#[cfg(test)]
mod tests;

pub const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const CATALOG_TIMEOUT: Duration = Duration::from_secs(8);
pub const META_TIMEOUT: Duration = Duration::from_secs(8);
pub const STREAMS_TIMEOUT: Duration = Duration::from_secs(15);
/// Answers larger than this are refused.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_SEARCH_HITS: usize = 40;
/// Answers kept in the cache (the oldest goes first).
const CACHE_MAX_ENTRIES: usize = 256;
/// A failed Cinemeta seeding is not retried for this long.
pub const SEED_RETRY: Duration = Duration::from_secs(5 * 60);
/// Search text longer than this is cut.
pub const MAX_QUERY_CHARS: usize = 100;

// ---------- Errors ----------

/// Codes: `addon_unreachable`, `addon_invalid`, `addon_no_meta` (`errors.<code>`).
#[derive(Debug, Clone, PartialEq)]
pub struct AddonError(pub Msg);

impl AddonError {
    pub fn unreachable() -> Self {
        Self(Msg::error("addon_unreachable"))
    }

    pub fn invalid() -> Self {
        Self(Msg::error("addon_invalid"))
    }

    pub fn no_meta() -> Self {
        Self(Msg::error("addon_no_meta"))
    }
}

impl fmt::Display for AddonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for AddonError {}

impl From<AddonError> for Msg {
    fn from(e: AddonError) -> Msg {
        e.0
    }
}

// ---------- HTTP ----------

/// Minimal JSON GET so the client can be tested without a network.
/// Transport failures and non-2xx answers are `addon_unreachable`, unreadable JSON or an
/// oversized body is `addon_invalid`.
#[async_trait]
pub trait AddonHttp: Send + Sync {
    async fn get_json(&self, url: &str, timeout: Duration) -> Result<Value, AddonError>;
}

pub struct ReqwestAddonHttp {
    client: reqwest::Client,
}

impl ReqwestAddonHttp {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("VRCinema/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }
}

#[async_trait]
impl AddonHttp for ReqwestAddonHttp {
    async fn get_json(&self, url: &str, timeout: Duration) -> Result<Value, AddonError> {
        let mut response = self
            .client
            .get(url)
            .timeout(timeout)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| AddonError::unreachable())?;
        if !response.status().is_success() {
            return Err(AddonError::unreachable());
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            return Err(AddonError::invalid());
        }
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AddonError::unreachable())?
        {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(AddonError::invalid());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| AddonError::invalid())
    }
}

// ---------- URLs ----------

/// Accepts `stremio://` (rewritten to `https://`) and http(s) URLs that end with
/// `/manifest.json`.
pub fn normalize_manifest_url(input: &str) -> Result<String, AddonError> {
    let input = input.trim();
    let url = match input.strip_prefix("stremio://") {
        Some(rest) => format!("https://{rest}"),
        None => input.to_string(),
    };
    let parsed = url::Url::parse(&url).map_err(|_| AddonError::invalid())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(AddonError::invalid());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(AddonError::invalid());
    }
    if !parsed.path().ends_with("/manifest.json") {
        return Err(AddonError::invalid());
    }
    Ok(url)
}

/// The source address of a viewer's title request: no link yet, the owner chooses one.
pub fn title_source(type_: &str, id: &str) -> String {
    format!("title:{type_}:{id}")
}

/// `(type, id)` of a [`title_source`] address.
pub fn parse_title_source(s: &str) -> Option<(&str, &str)> {
    let (type_, id) = s.strip_prefix("title:")?.split_once(':')?;
    (!type_.is_empty() && !id.is_empty()).then_some((type_, id))
}

/// The manifest URL without the trailing `/manifest.json`.
pub fn base_url(manifest_url: &str) -> &str {
    manifest_url
        .strip_suffix("/manifest.json")
        .unwrap_or(manifest_url)
}

/// Percent-encodes everything but unreserved characters (and the extra `keep` characters).
fn encode(text: &str, keep: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || keep.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------- Manifest ----------

/// A manifest in the app's own normalised (and storable) shape; [`Manifest::parse`] reads the
/// wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub id_prefixes: Option<Vec<String>>,
    #[serde(default)]
    pub catalogs: Vec<CatalogDef>,
    #[serde(default)]
    pub resources: Vec<ResourceDef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogDef {
    pub type_: String,
    pub id: String,
    /// The catalog accepts a `search` extra.
    #[serde(default)]
    pub search: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceDef {
    pub name: String,
    /// Object form only; falls back to the manifest's `types`.
    #[serde(default)]
    pub types: Option<Vec<String>>,
    /// Object form only; falls back to the manifest's `id_prefixes`.
    #[serde(default)]
    pub id_prefixes: Option<Vec<String>>,
}

fn string_list(v: Option<&Value>) -> Option<Vec<String>> {
    Some(
        v?.as_array()?
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
    )
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl Manifest {
    /// Needs `id`, `name` and `resources`; everything else is optional.
    pub fn parse(v: &Value) -> Result<Manifest, AddonError> {
        let id = text(v, "id").ok_or_else(AddonError::invalid)?;
        let name = text(v, "name").ok_or_else(AddonError::invalid)?;
        let resources = v
            .get("resources")
            .and_then(Value::as_array)
            .ok_or_else(AddonError::invalid)?
            .iter()
            .filter_map(|r| match r {
                Value::String(name) => Some(ResourceDef {
                    name: name.clone(),
                    types: None,
                    id_prefixes: None,
                }),
                Value::Object(_) => Some(ResourceDef {
                    name: text(r, "name")?,
                    types: string_list(r.get("types")),
                    id_prefixes: string_list(r.get("idPrefixes")),
                }),
                _ => None,
            })
            .collect();
        let catalogs = v
            .get("catalogs")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|c| {
                let by_extra = c
                    .get("extra")
                    .and_then(Value::as_array)
                    .is_some_and(|extra| {
                        extra
                            .iter()
                            .any(|e| e.get("name").and_then(Value::as_str) == Some("search"))
                    });
                let legacy = string_list(c.get("extraSupported"))
                    .is_some_and(|l| l.iter().any(|s| s == "search"));
                Some(CatalogDef {
                    type_: text(c, "type")?,
                    id: text(c, "id")?,
                    search: by_extra || legacy,
                })
            })
            .collect();
        Ok(Manifest {
            id,
            name,
            version: text(v, "version").unwrap_or_default(),
            description: text(v, "description"),
            logo: text(v, "logo"),
            types: string_list(v.get("types")).unwrap_or_default(),
            id_prefixes: string_list(v.get("idPrefixes")),
            catalogs,
            resources,
        })
    }

    pub fn provides_search(&self) -> bool {
        self.catalogs.iter().any(|c| c.search)
    }

    /// Whether the addon offers `resource` for this type and id (type list and id prefixes come
    /// from the resource entry, else from the manifest).
    pub fn supports(&self, resource: &str, type_: &str, id: &str) -> bool {
        self.resources
            .iter()
            .filter(|r| r.name == resource)
            .any(|r| {
                let types = r.types.as_ref().unwrap_or(&self.types);
                if !types.iter().any(|t| t == type_) {
                    return false;
                }
                match r.id_prefixes.as_ref().or(self.id_prefixes.as_ref()) {
                    Some(prefixes) => prefixes.iter().any(|p| id.starts_with(p.as_str())),
                    None => true,
                }
            })
    }
}

// ---------- Results ----------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogHit {
    pub type_: String,
    pub id: String,
    pub name: String,
    pub year: Option<String>,
    pub poster: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Episode {
    pub id: String,
    pub season: u32,
    pub episode: u32,
    pub title: String,
    pub released: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetaInfo {
    pub type_: String,
    pub id: String,
    pub name: String,
    pub year: Option<String>,
    pub poster: Option<String>,
    pub background: Option<String>,
    pub description: Option<String>,
    pub runtime: Option<String>,
    /// Sorted by season then episode; season 0 (specials) last.
    pub videos: Vec<Episode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamOption {
    pub url: String,
    pub name: String,
    pub title: String,
    /// A magnet link (a torrent stream), played by the in-process torrent client.
    pub torrent: bool,
}

/// `releaseInfo` ("2011-2019"), else `year` (string or number).
fn year_of(v: &Value) -> Option<String> {
    text(v, "releaseInfo").or_else(|| match v.get("year")? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn number(v: &Value, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|k| {
        let x = v.get(*k)?;
        x.as_u64()
            .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
            .and_then(|n| u32::try_from(n).ok())
    })
}

fn parse_hit(v: &Value, default_type: &str) -> Option<CatalogHit> {
    Some(CatalogHit {
        type_: text(v, "type").unwrap_or_else(|| default_type.to_string()),
        id: text(v, "id")?,
        name: text(v, "name")?,
        year: year_of(v),
        poster: text(v, "poster"),
    })
}

fn parse_meta(v: &Value, type_: &str, id: &str) -> Option<MetaInfo> {
    if !v.is_object() {
        return None;
    }
    let mut videos: Vec<Episode> = v
        .get("videos")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            Some(Episode {
                id: text(e, "id")?,
                season: number(e, &["season"]).unwrap_or(0),
                episode: number(e, &["episode", "number"])?,
                title: text(e, "title")
                    .or_else(|| text(e, "name"))
                    .unwrap_or_default(),
                released: text(e, "released"),
            })
        })
        .collect();
    videos.sort_by_key(|e| (e.season == 0, e.season, e.episode));
    Some(MetaInfo {
        type_: text(v, "type").unwrap_or_else(|| type_.to_string()),
        id: text(v, "id").unwrap_or_else(|| id.to_string()),
        name: text(v, "name")?,
        year: year_of(v),
        poster: text(v, "poster"),
        background: text(v, "background"),
        description: text(v, "description"),
        runtime: text(v, "runtime"),
        videos,
    })
}

/// The magnet of a stream with an `infoHash` (`None`: the hash is not a valid one).
fn magnet_of(s: &Value, title: &str) -> Option<String> {
    let hash = text(s, "infoHash")?;
    let idx = number(s, &["fileIdx"]).map(|n| n as usize);
    let name = text(s, "name").unwrap_or_default();
    let sources: Vec<String> = s
        .get("sources")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let label = title
        .lines()
        .next()
        .filter(|l| !l.trim().is_empty())
        .unwrap_or(&name);
    let link = crate::torrent::magnet_from_stream(&hash, idx, label, &sources);
    crate::torrent::parse_magnet(&link)?;
    Some(link)
}

/// `torrents`: keep the streams with an `infoHash` as magnet links (otherwise they are dropped).
fn parse_streams(v: &Value, torrents: bool) -> Result<Vec<StreamOption>, AddonError> {
    let list = v
        .get("streams")
        .and_then(Value::as_array)
        .ok_or_else(AddonError::invalid)?;
    Ok(list
        .iter()
        .filter_map(|s| {
            let title = text(s, "title")
                .or_else(|| text(s, "description"))
                .unwrap_or_default();
            let mut torrent = false;
            let url = match text(s, "url") {
                Some(u) if u.starts_with("http://") || u.starts_with("https://") => u,
                Some(_) => return None,
                None if torrents && text(s, "infoHash").is_some() => {
                    torrent = true;
                    magnet_of(s, &title)?
                }
                None => format!("https://www.youtube.com/watch?v={}", text(s, "ytId")?),
            };
            Some(StreamOption {
                url,
                name: text(s, "name").unwrap_or_default(),
                title,
                torrent,
            })
        })
        .collect())
}

// ---------- Client ----------

pub struct AddonClient {
    http: Arc<dyn AddonHttp>,
    cache: Mutex<HashMap<String, (Instant, Arc<Value>)>>,
    /// Held while Cinemeta is being seeded (one attempt at a time); the time of the last failure.
    pub seed_gate: tokio::sync::Mutex<Option<Instant>>,
}

impl AddonClient {
    pub fn new(http: Arc<dyn AddonHttp>) -> Self {
        Self {
            http,
            cache: Mutex::new(HashMap::new()),
            seed_gate: tokio::sync::Mutex::new(None),
        }
    }

    pub async fn fetch_manifest(&self, url: &str) -> Result<Manifest, AddonError> {
        let value = self.http.get_json(url, MANIFEST_TIMEOUT).await?;
        Manifest::parse(&value)
    }

    /// GET with a 5 minute in-memory cache (successful answers only).
    async fn cached(&self, url: &str, timeout: Duration) -> Result<Arc<Value>, AddonError> {
        let now = Instant::now();
        if let Some((at, v)) = self.cache.lock().unwrap().get(url)
            && now.duration_since(*at) < CACHE_TTL
        {
            return Ok(Arc::clone(v));
        }
        let value = Arc::new(self.http.get_json(url, timeout).await?);
        let mut cache = self.cache.lock().unwrap();
        cache.retain(|_, (at, _)| now.duration_since(*at) < CACHE_TTL);
        while cache.len() >= CACHE_MAX_ENTRIES {
            let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            cache.remove(&oldest);
        }
        cache.insert(url.to_string(), (now, Arc::clone(&value)));
        Ok(value)
    }

    /// All search-capable catalogs (the first one per type of each addon) are asked at once;
    /// failing addons are skipped. Addon order is kept, ids are unique, at most 40 hits.
    pub async fn search(&self, addons: &[(String, Manifest)], q: &str) -> Vec<CatalogHit> {
        let q: String = q.trim().chars().take(MAX_QUERY_CHARS).collect();
        let q = encode(&q, "");
        let mut requests: Vec<(String, String)> = Vec::new();
        for (manifest_url, manifest) in addons {
            let mut seen_types = HashSet::new();
            for catalog in manifest.catalogs.iter().filter(|c| c.search) {
                if !seen_types.insert(catalog.type_.as_str()) {
                    continue;
                }
                requests.push((
                    catalog.type_.clone(),
                    format!(
                        "{}/catalog/{}/{}/search={q}.json",
                        base_url(manifest_url),
                        encode(&catalog.type_, ""),
                        encode(&catalog.id, ""),
                    ),
                ));
            }
        }
        let answers = futures_util::future::join_all(
            requests
                .iter()
                .map(|(_, url)| self.cached(url, CATALOG_TIMEOUT)),
        )
        .await;
        let mut hits: Vec<CatalogHit> = Vec::new();
        let mut ids = HashSet::new();
        for ((type_, _), answer) in requests.iter().zip(answers) {
            let Ok(answer) = answer else { continue };
            let metas = answer
                .get("metas")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let movies_and_series = metas
                .iter()
                .filter_map(|m| parse_hit(m, type_))
                .filter(|h| matches!(h.type_.as_str(), "movie" | "series"));
            for hit in movies_and_series {
                if hits.len() < MAX_SEARCH_HITS && ids.insert(hit.id.clone()) {
                    hits.push(hit);
                }
            }
        }
        hits
    }

    /// Details from the first addon (in the given order) that supports the title and answers
    /// with a `meta`.
    pub async fn meta(
        &self,
        addons: &[(String, Manifest)],
        type_: &str,
        id: &str,
    ) -> Result<MetaInfo, AddonError> {
        for (manifest_url, manifest) in addons {
            if !manifest.supports("meta", type_, id) {
                continue;
            }
            let url = format!(
                "{}/meta/{}/{}.json",
                base_url(manifest_url),
                encode(type_, ""),
                encode(id, ":")
            );
            let Ok(answer) = self.cached(&url, META_TIMEOUT).await else {
                continue;
            };
            if let Some(meta) = answer.get("meta").and_then(|m| parse_meta(m, type_, id)) {
                return Ok(meta);
            }
        }
        Err(AddonError::no_meta())
    }

    /// Playable sources of one stream addon (not cached: links expire).
    pub async fn streams(
        &self,
        manifest_url: &str,
        type_: &str,
        id: &str,
        torrents: bool,
    ) -> Result<Vec<StreamOption>, AddonError> {
        let url = format!(
            "{}/stream/{}/{}.json",
            base_url(manifest_url),
            encode(type_, ""),
            encode(id, ":")
        );
        parse_streams(&self.http.get_json(&url, STREAMS_TIMEOUT).await?, torrents)
    }
}

#[cfg(test)]
impl AddonClient {
    fn cache_len(&self) -> usize {
        self.cache.lock().unwrap().len()
    }
}
