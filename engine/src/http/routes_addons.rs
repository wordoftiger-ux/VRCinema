//! Stremio-compatible addons (admin session): the owner's list and the catalog, details and
//! sources the addons give. The engine asks the addons (see [`crate::addons`]), so the window
//! and the remote panel need no CORS and a slow addon never blocks the page.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApiError, ApiResult, AppState, JsonBody, json_ok};
use crate::addons::{AddonError, CatalogHit, Manifest, MetaInfo, normalize_manifest_url};
use crate::db::addons::CINEMETA_URL;
use crate::db::{AddError, AddonRepo, AddonRow};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/addons", get(list).post(add))
        .route("/api/admin/addons/search", get(search))
        .route("/api/admin/addons/meta/{type}/{id}", get(meta))
        .route("/api/admin/addons/streams/{type}/{id}", get(streams))
        .route("/api/admin/addons/{id}", post(update).delete(remove))
}

/// An addon failure as an answer: the addon's own message, 502 (404 when nothing has details).
pub(super) fn addon_error(e: AddonError) -> ApiError {
    let status = if e == AddonError::no_meta() {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_GATEWAY
    };
    ApiError::from_msg(status, e.0)
}

fn not_found() -> ApiError {
    ApiError::coded(StatusCode::NOT_FOUND, "addon_not_found", &[])
}

fn provides(manifest: &Manifest, resource: &str) -> bool {
    manifest.resources.iter().any(|r| r.name == resource)
}

fn addon_json(a: &AddonRow) -> Value {
    let m = &a.manifest;
    json!({
        "id": a.id,
        "url": a.manifest_url,
        "name": m.name,
        "version": m.version,
        "description": m.description,
        "logo": m.logo,
        "enabled": a.enabled,
        "provides": {
            "catalogSearch": m.provides_search(),
            "meta": provides(m, "meta"),
            "stream": provides(m, "stream"),
        },
        "types": m.types,
    })
}

pub(super) fn hit_json(h: &CatalogHit) -> Value {
    json!({
        "type": h.type_,
        "id": h.id,
        "name": h.name,
        "year": h.year,
        "poster": h.poster,
    })
}

pub(super) fn meta_json(m: &MetaInfo) -> Value {
    json!({
        "type": m.type_,
        "id": m.id,
        "name": m.name,
        "year": m.year,
        "poster": m.poster,
        "background": m.background,
        "description": m.description,
        "runtime": m.runtime,
        "videos": m.videos.iter().map(|v| json!({
            "id": v.id,
            "season": v.season,
            "episode": v.episode,
            "title": v.title,
            "released": v.released,
        })).collect::<Vec<_>>(),
    })
}

fn stored(manifest: &Manifest) -> ApiResult<String> {
    serde_json::to_string(manifest).map_err(|e| ApiError::Internal(e.to_string()))
}

/// Puts Cinemeta in the list the first time it can be fetched. A failed fetch stores nothing
/// (the flag stays unset), so the next call tries again; a removal later sticks.
pub(super) async fn ensure_seeded(state: &AppState) {
    if !matches!(state.with_db(|c| AddonRepo::new(c).seeded()), Ok(false)) {
        return;
    }
    let Ok(manifest) = state.addons.fetch_manifest(CINEMETA_URL).await else {
        return;
    };
    let Ok(json) = serde_json::to_string(&manifest) else {
        return;
    };
    let _ = state.with_db(|c| AddonRepo::new(c).seed_cinemeta_once(&json));
}

/// The enabled addons as (manifest URL, manifest), in the owner's order.
pub(super) fn enabled_manifests(state: &AppState) -> ApiResult<Vec<(String, Manifest)>> {
    Ok(state
        .with_db(|c| AddonRepo::new(c).enabled())?
        .into_iter()
        .map(|a| (a.manifest_url, a.manifest))
        .collect())
}

/// Catalog addons that can search, in order (what viewers may use too).
pub(super) fn catalog_manifests(state: &AppState) -> ApiResult<Vec<(String, Manifest)>> {
    Ok(enabled_manifests(state)?
        .into_iter()
        .filter(|(_, m)| m.provides_search())
        .collect())
}

async fn list(State(state): State<AppState>) -> ApiResult {
    ensure_seeded(&state).await;
    let rows = state.with_db(|c| AddonRepo::new(c).list())?;
    Ok(json_ok(&Value::Array(
        rows.iter().map(addon_json).collect(),
    )))
}

async fn add(State(state): State<AppState>, JsonBody(body): JsonBody) -> ApiResult {
    let url = body
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::from_msg(StatusCode::BAD_REQUEST, AddonError::invalid().0))?;
    let url = normalize_manifest_url(url)
        .map_err(|e| ApiError::from_msg(StatusCode::BAD_REQUEST, e.0))?;
    let exists = || ApiError::coded(StatusCode::CONFLICT, "addon_exists", &[]);
    if state.with_db(|c| AddonRepo::new(c).exists(&url))? {
        return Err(exists());
    }
    let manifest = state
        .addons
        .fetch_manifest(&url)
        .await
        .map_err(addon_error)?;
    let json = stored(&manifest)?;
    let added = state.with_db(|c| Ok(AddonRepo::new(c).add(&url, &json)))?;
    let id = match added {
        Ok(id) => id,
        Err(AddError::Exists) => return Err(exists()),
        Err(AddError::Db(e)) => return Err(e.into()),
    };
    let row = state
        .with_db(|c| AddonRepo::new(c).get(id))?
        .ok_or_else(not_found)?;
    Ok(json_ok(&addon_json(&row)))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let row = state
        .with_db(|c| AddonRepo::new(c).get(id))?
        .ok_or_else(not_found)?;
    let bad = || ApiError::bad_request("bad_request");
    let enabled = match body.get("enabled") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_bool().ok_or_else(bad)?),
    };
    let up = match body.get("move").and_then(Value::as_str) {
        None => None,
        Some("up") => Some(true),
        Some("down") => Some(false),
        Some(_) => return Err(bad()),
    };
    if body.get("refresh").and_then(Value::as_bool) == Some(true) {
        let manifest = state
            .addons
            .fetch_manifest(&row.manifest_url)
            .await
            .map_err(addon_error)?;
        let json = stored(&manifest)?;
        state.with_db(|c| AddonRepo::new(c).update_manifest(id, &json))?;
    }
    state.with_db(|c| {
        let repo = AddonRepo::new(c);
        if let Some(enabled) = enabled {
            repo.set_enabled(id, enabled)?;
        }
        if let Some(up) = up {
            repo.move_(id, up)?;
        }
        Ok(())
    })?;
    let row = state
        .with_db(|c| AddonRepo::new(c).get(id))?
        .ok_or_else(not_found)?;
    Ok(json_ok(&addon_json(&row)))
}

async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult {
    if state.with_db(|c| AddonRepo::new(c).remove(id))? {
        Ok(json_ok(&json!({ "ok": true })))
    } else {
        Err(not_found())
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn search(State(state): State<AppState>, Query(query): Query<SearchQuery>) -> ApiResult {
    ensure_seeded(&state).await;
    let q = query.q.trim();
    if q.chars().count() < 2 {
        return Ok(json_ok(&json!([])));
    }
    let addons = catalog_manifests(&state)?;
    let hits = state.addons.search(&addons, q).await;
    Ok(json_ok(&Value::Array(hits.iter().map(hit_json).collect())))
}

async fn meta(
    State(state): State<AppState>,
    Path((type_, id)): Path<(String, String)>,
) -> ApiResult {
    ensure_seeded(&state).await;
    let addons = enabled_manifests(&state)?;
    let meta = state
        .addons
        .meta(&addons, &type_, &id)
        .await
        .map_err(addon_error)?;
    Ok(json_ok(&meta_json(&meta)))
}

#[derive(Deserialize)]
struct StreamsQuery {
    addon: Option<i64>,
}

/// Without `?addon=`: the stream addons that apply (the page asks each one itself, so a slow
/// addon blocks nobody). With it: that addon's sources.
async fn streams(
    State(state): State<AppState>,
    Path((type_, id)): Path<(String, String)>,
    Query(query): Query<StreamsQuery>,
) -> ApiResult {
    let applicable: Vec<AddonRow> = state
        .with_db(|c| AddonRepo::new(c).enabled())?
        .into_iter()
        .filter(|a| a.manifest.supports("stream", &type_, &id))
        .collect();
    let Some(wanted) = query.addon else {
        let addons: Vec<Value> = applicable
            .iter()
            .map(|a| json!({ "id": a.id, "name": a.manifest.name }))
            .collect();
        return Ok(json_ok(&json!({ "addons": addons })));
    };
    let addon = applicable
        .iter()
        .find(|a| a.id == wanted)
        .ok_or_else(not_found)?;
    let found = state
        .addons
        .streams(&addon.manifest_url, &type_, &id)
        .await
        .map_err(addon_error)?;
    Ok(json_ok(&json!({
        "streams": found.iter().map(|s| json!({
            "url": s.url, "name": s.name, "title": s.title,
        })).collect::<Vec<_>>(),
    })))
}
