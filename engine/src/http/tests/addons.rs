//! The addon routes (`/api/admin/addons/...`) and the `viewerCatalog` setting, with a fake
//! network for the addons (fixtures live on `addons.example`).

use std::time::Duration;

use super::*;
use crate::addons::{AddonClient, AddonError, AddonHttp};
use crate::db::addons::CINEMETA_URL;

const CATALOG_URL: &str = "https://addons.example/catalog/manifest.json";
const STREAMS_URL: &str = "https://addons.example/streams/manifest.json";

struct FakeNet {
    answers: Mutex<HashMap<String, Value>>,
}

#[async_trait]
impl AddonHttp for FakeNet {
    async fn get_json(&self, url: &str, _timeout: Duration) -> Result<Value, AddonError> {
        self.answers
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(AddonError::unreachable)
    }
}

fn manifest(id: &str, name: &str, resources: Value, search: bool) -> Value {
    json!({
        "id": id,
        "name": name,
        "version": "1.0.0",
        "types": ["movie", "series"],
        "idPrefixes": ["tt"],
        "resources": resources,
        "catalogs": if search {
            json!([{"type": "movie", "id": "top", "extra": [{"name": "search"}]}])
        } else {
            json!([])
        },
    })
}

fn cinemeta() -> Value {
    manifest("cinemeta", "Cinemeta", json!(["catalog", "meta"]), true)
}

fn net(with_cinemeta: bool) -> Arc<FakeNet> {
    let mut answers = HashMap::new();
    if with_cinemeta {
        answers.insert(CINEMETA_URL.to_string(), cinemeta());
    }
    answers.insert(
        CATALOG_URL.to_string(),
        manifest("cat", "Catalog", json!(["catalog"]), true),
    );
    answers.insert(
        STREAMS_URL.to_string(),
        manifest("str", "Streams", json!(["stream"]), false),
    );
    answers.insert(
        "https://addons.example/catalog/catalog/movie/top/search=matrix.json".to_string(),
        json!({"metas": [{"id": "tt1", "type": "movie", "name": "The Matrix", "releaseInfo": "1999"}]}),
    );
    answers.insert(
        "https://addons.example/streams/stream/movie/tt1.json".to_string(),
        json!({"streams": [{"url": "https://cdn.example/a.mp4", "name": "Fast", "title": "1080p"}]}),
    );
    Arc::new(FakeNet {
        answers: Mutex::new(answers),
    })
}

fn harness(net: Arc<FakeNet>) -> Harness {
    Harness::with_state(config(), |s| s.with_addons(Arc::new(AddonClient::new(net))))
}

async fn list(h: &Harness, cookie: &str) -> Vec<Value> {
    let r = h.send(get("/api/admin/addons").cookie(cookie)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    r.json().as_array().unwrap().clone()
}

fn order(rows: &[Value]) -> Vec<i64> {
    rows.iter().map(|r| r["id"].as_i64().unwrap()).collect()
}

async fn add(h: &Harness, cookie: &str, url: &str) -> Resp {
    h.send(
        call("POST", "/api/admin/addons")
            .cookie(cookie)
            .json(json!({ "url": url })),
    )
    .await
}

async fn post_id(h: &Harness, cookie: &str, id: i64, body: Value) -> Resp {
    h.send(
        call("POST", &format!("/api/admin/addons/{id}"))
            .cookie(cookie)
            .json(body),
    )
    .await
}

async fn delete(h: &Harness, cookie: &str, id: i64) -> Resp {
    h.send(call("DELETE", &format!("/api/admin/addons/{id}")).cookie(cookie))
        .await
}

#[tokio::test]
async fn add_list_toggle_move_remove_addon() {
    let h = harness(net(false));
    let cookie = h.login().await;
    assert!(list(&h, &cookie).await.is_empty());

    let r = add(&h, &cookie, CATALOG_URL).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let first = r.json();
    assert_eq!(first["name"], "Catalog");
    assert_eq!(first["url"], CATALOG_URL);
    assert_eq!(first["enabled"], true);
    assert_eq!(first["provides"]["catalogSearch"], true);
    assert_eq!(first["provides"]["stream"], false);
    // The stremio:// scheme is accepted.
    let r = add(
        &h,
        &cookie,
        "stremio://addons.example/streams/manifest.json",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let second = r.json();
    assert_eq!(second["provides"]["stream"], true);

    let dup = add(&h, &cookie, CATALOG_URL).await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert_eq!(dup.json()["code"], "addon_exists");
    let missing = add(&h, &cookie, "https://addons.example/none/manifest.json").await;
    assert_eq!(missing.json()["code"], "addon_unreachable");
    let bad = add(&h, &cookie, "https://addons.example/x").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad.json()["code"], "addon_invalid");

    let a = first["id"].as_i64().unwrap();
    let b = second["id"].as_i64().unwrap();
    let r = post_id(&h, &cookie, a, json!({"enabled": false})).await;
    assert_eq!(r.json()["enabled"], false);
    assert_eq!(order(&list(&h, &cookie).await), [a, b]);
    let r = post_id(&h, &cookie, b, json!({"move": "up"})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(order(&list(&h, &cookie).await), [b, a]);
    // Moving the top one up changes nothing.
    post_id(&h, &cookie, b, json!({"move": "up"})).await;
    assert_eq!(order(&list(&h, &cookie).await), [b, a]);
    let r = post_id(&h, &cookie, a, json!({"refresh": true})).await;
    assert_eq!(r.json()["name"], "Catalog");

    assert_eq!(delete(&h, &cookie, a).await.status, StatusCode::OK);
    assert_eq!(order(&list(&h, &cookie).await), [b]);
    let r = post_id(&h, &cookie, a, json!({"enabled": true})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["code"], "addon_not_found");
}

#[tokio::test]
async fn cinemeta_seeded_once_and_removal_sticks() {
    let h = harness(net(true));
    let cookie = h.login().await;
    let rows = list(&h, &cookie).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Cinemeta");
    assert_eq!(rows[0]["url"], CINEMETA_URL);
    assert_eq!(rows[0]["provides"]["meta"], true);
    let id = rows[0]["id"].as_i64().unwrap();
    assert_eq!(delete(&h, &cookie, id).await.status, StatusCode::OK);
    assert!(list(&h, &cookie).await.is_empty());
}

#[tokio::test]
async fn a_failed_cinemeta_fetch_stores_nothing_and_is_retried() {
    let n = net(false);
    let h = harness(Arc::clone(&n));
    let cookie = h.login().await;
    assert!(list(&h, &cookie).await.is_empty());
    n.answers
        .lock()
        .unwrap()
        .insert(CINEMETA_URL.to_string(), cinemeta());
    assert_eq!(list(&h, &cookie).await.len(), 1);
}

#[tokio::test]
async fn search_and_streams_only_use_enabled_addons() {
    let h = harness(net(false));
    let cookie = h.login().await;
    let cat = add(&h, &cookie, CATALOG_URL).await.json()["id"]
        .as_i64()
        .unwrap();
    let streams = add(&h, &cookie, STREAMS_URL).await.json()["id"]
        .as_i64()
        .unwrap();

    let r = h
        .send(get("/api/admin/addons/search?q=matrix").cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.json(),
        json!([{"type": "movie", "id": "tt1", "name": "The Matrix", "year": "1999", "poster": null}])
    );
    let r = h
        .send(get("/api/admin/addons/streams/movie/tt1").cookie(&cookie))
        .await;
    assert_eq!(
        r.json(),
        json!({"addons": [{"id": streams, "name": "Streams"}]})
    );
    let one = format!("/api/admin/addons/streams/movie/tt1?addon={streams}");
    let r = h.send(get(&one).cookie(&cookie)).await;
    assert_eq!(
        r.json(),
        json!({"streams": [{"url": "https://cdn.example/a.mp4", "name": "Fast", "title": "1080p"}]})
    );
    // A catalog addon has no sources to ask.
    let other = format!("/api/admin/addons/streams/movie/tt1?addon={cat}");
    let r = h.send(get(&other).cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    post_id(&h, &cookie, streams, json!({"enabled": false})).await;
    post_id(&h, &cookie, cat, json!({"enabled": false})).await;
    let r = h
        .send(get("/api/admin/addons/streams/movie/tt1").cookie(&cookie))
        .await;
    assert_eq!(r.json(), json!({"addons": []}));
    let r = h.send(get(&one).cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = h
        .send(get("/api/admin/addons/search?q=matrix").cookie(&cookie))
        .await;
    assert_eq!(r.json(), json!([]));
    let r = h
        .send(get("/api/admin/addons/meta/movie/tt1").cookie(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["code"], "addon_no_meta");
}

#[tokio::test]
async fn addon_routes_need_session() {
    let h = harness(net(true));
    for (method, url) in [
        ("GET", "/api/admin/addons"),
        ("POST", "/api/admin/addons"),
        ("POST", "/api/admin/addons/1"),
        ("DELETE", "/api/admin/addons/1"),
        ("GET", "/api/admin/addons/search?q=ab"),
        ("GET", "/api/admin/addons/meta/movie/tt1"),
        ("GET", "/api/admin/addons/streams/movie/tt1"),
    ] {
        let r = h.send(call(method, url)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{method} {url}");
    }
}

#[tokio::test]
async fn viewer_catalog_defaults_true_and_patches() {
    let h = Harness::new();
    let cookie = h.login().await;
    let settings = || h.send(get("/api/admin/channels/main/settings").cookie(&cookie));
    assert_eq!(settings().await.json()["viewerCatalog"], true);
    let state = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(state["rules"]["viewerCatalog"], true);
    let r = h
        .send(
            call("PUT", "/api/admin/channels/main/settings")
                .cookie(&cookie)
                .json(json!({"viewerCatalog": false})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(settings().await.json()["viewerCatalog"], false);
    let state = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(state["rules"]["viewerCatalog"], false);
}
