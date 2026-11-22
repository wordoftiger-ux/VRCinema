//! The addon routes (`/api/admin/addons/...`) and the `viewerCatalog` setting, with a fake
//! network for the addons (fixtures live on `addons.example`).

use std::time::Duration;

use super::*;
use crate::addons::{AddonClient, AddonError, AddonHttp};
use crate::db::addons::CINEMETA_URL;

const CATALOG_URL: &str = "https://addons.example/catalog/manifest.json";
const MIXED_URL: &str = "https://addons.example/mixed/manifest.json";
const STREAMS_URL: &str = "https://addons.example/streams/manifest.json";

struct FakeNet {
    answers: Mutex<HashMap<String, Value>>,
    /// Every URL asked, in order.
    seen: Mutex<Vec<String>>,
}

#[async_trait]
impl AddonHttp for FakeNet {
    async fn get_json(&self, url: &str, _timeout: Duration) -> Result<Value, AddonError> {
        self.seen.lock().unwrap().push(url.to_string());
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
        seen: Mutex::new(Vec::new()),
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
    let client = Arc::new(AddonClient::new(Arc::clone(&n) as Arc<dyn AddonHttp>));
    let h = Harness::with_state(config(), |s| s.with_addons(Arc::clone(&client)));
    let cookie = h.login().await;
    assert!(list(&h, &cookie).await.is_empty());
    // The failure is remembered for a while: no second wait, no second request.
    n.answers
        .lock()
        .unwrap()
        .insert(CINEMETA_URL.to_string(), cinemeta());
    assert!(list(&h, &cookie).await.is_empty());
    let asked = n
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|u| *u == CINEMETA_URL)
        .count();
    assert_eq!(asked, 1);
    // After the pause it is tried again.
    *client.seed_gate.lock().await = Some(std::time::Instant::now() - crate::addons::SEED_RETRY);
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
        json!({"streams": [{"url": "https://cdn.example/a.mp4", "name": "Fast", "title": "1080p", "torrent": false}]})
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

fn meta_answers(n: &FakeNet) {
    let base = crate::addons::base_url(CINEMETA_URL).to_string();
    let mut a = n.answers.lock().unwrap();
    a.insert(
        format!("{base}/meta/movie/tt1.json"),
        json!({"meta": {"id": "tt1", "type": "movie", "name": "Example Film", "releaseInfo": "1999"}}),
    );
    a.insert(
        format!("{base}/meta/series/tt2.json"),
        json!({"meta": {"id": "tt2", "type": "series", "name": "Example Show", "videos": [
            {"id": "tt2:1:1", "season": 1, "episode": 1, "title": "First"},
            {"id": "tt2:1:2", "season": 1, "episode": 2, "title": "Second"},
        ]}}),
    );
}

fn title_harness() -> Harness {
    let n = net(true);
    meta_answers(&n);
    harness(n)
}

async fn request_title(h: &Harness, type_: &str, id: &str) -> Resp {
    h.send(
        call("POST", "/api/channels/main/requests")
            .ip("1.1.1.1")
            .json(json!({"title": {"type": type_, "id": id}})),
    )
    .await
}

#[tokio::test]
async fn viewer_title_request_is_always_pending() {
    let h = title_harness();
    h.update_settings(SettingsPatch {
        moderation: Some(false),
        ..Default::default()
    });
    let r = request_title(&h, "movie", "tt1").await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!(r.json()["status"], "pending_approval");
    let pending = h.list(ItemStatus::PendingApproval);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].title, "Example Film (1999)");
    assert_eq!(pending[0].source_url, "title:movie:tt1");
    assert_eq!(pending[0].duration_s, None);
    assert_eq!(pending[0].added_by, AddedBy::Public);
    let cookie = h.login().await;
    let panel = h
        .send(get("/api/admin/channels/main").cookie(&cookie))
        .await
        .json();
    assert_eq!(
        panel["pending"][0]["titleRequest"],
        json!({"type": "movie", "id": "tt1"})
    );
}

#[tokio::test]
async fn series_title_request_needs_known_episode() {
    let h = title_harness();
    let r = request_title(&h, "series", "tt2").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["code"], "title_invalid");
    let r = request_title(&h, "series", "tt2:1:2").await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!(
        h.list(ItemStatus::PendingApproval)[0].title,
        "Example Show S01E02 \u{2013} Second"
    );
    for (t, id) in [
        ("movie", "bad id"),
        ("tv", "tt1"),
        ("movie", &"a".repeat(81)),
    ] {
        let r = request_title(&h, t, id).await;
        assert_eq!(r.json()["code"], "title_invalid", "{t} {id}");
    }
}

#[tokio::test]
async fn viewer_routes_never_reach_stream_addons() {
    let n = net(true);
    meta_answers(&n);
    {
        let mut a = n.answers.lock().unwrap();
        a.insert(
            MIXED_URL.to_string(),
            manifest("mixed", "Mixed", json!(["catalog", "meta", "stream"]), true),
        );
        a.insert(
            "https://addons.example/mixed/meta/movie/tt1.json".to_string(),
            json!({"meta": {"id": "tt1", "type": "movie", "name": "From Mixed"}}),
        );
        a.insert(
            "https://addons.example/mixed/catalog/movie/top/search=matrix.json".to_string(),
            json!({"metas": [{"id": "tt9", "type": "movie", "name": "Mixed Hit"}]}),
        );
    }
    let h = harness(Arc::clone(&n));
    let cookie = h.login().await;
    list(&h, &cookie).await;
    let id = add(&h, &cookie, MIXED_URL).await.json()["id"]
        .as_i64()
        .unwrap();
    post_id(&h, &cookie, id, json!({"move": "up"})).await;
    n.seen.lock().unwrap().clear();

    let r = h
        .send(get("/api/channels/main/catalog/search?q=matrix").ip("4.4.4.4"))
        .await;
    assert_eq!(r.json(), json!([]));
    let r = h
        .send(get("/api/channels/main/catalog/meta/movie/tt1").ip("4.4.4.4"))
        .await;
    assert_eq!(r.json()["name"], "Example Film");
    let r = request_title(&h, "movie", "tt1").await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!(
        h.list(ItemStatus::PendingApproval)[0].title,
        "Example Film (1999)"
    );
    let asked = n.seen.lock().unwrap().clone();
    assert!(!asked.is_empty());
    assert!(
        asked
            .iter()
            .all(|u| !u.starts_with("https://addons.example/mixed/")),
        "{asked:?}"
    );

    // The owner's own search still uses it.
    let r = h
        .send(get("/api/admin/addons/search?q=matrix").cookie(&cookie))
        .await;
    assert_eq!(r.json()[0]["name"], "Mixed Hit");
}

#[tokio::test]
async fn bad_types_and_ids_are_refused() {
    let h = title_harness();
    for path in [
        "/api/channels/main/catalog/meta/tv/tt1",
        "/api/channels/main/catalog/meta/movie/bad%20id",
    ] {
        let r = h.send(get(path).ip("5.5.5.5")).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(r.json()["code"], "title_invalid", "{path}");
    }
    let cookie = h.login().await;
    for path in [
        "/api/admin/addons/meta/..%2Fx/tt1",
        "/api/admin/addons/meta/a.b/tt1",
        "/api/admin/addons/streams/..%2Fx/tt1",
    ] {
        let r = h.send(get(path).cookie(&cookie)).await;
        assert_eq!(r.json()["code"], "title_invalid", "{path}");
    }
}

#[tokio::test]
async fn catalog_routes_404_when_disabled_and_rate_limited() {
    let h = title_harness();
    h.update_settings(SettingsPatch {
        viewer_catalog: Some(false),
        ..Default::default()
    });
    let search = || h.send(get("/api/channels/main/catalog/search?q=matrix").ip("2.2.2.2"));
    assert_eq!(search().await.status, StatusCode::NOT_FOUND);
    let r = h
        .send(get("/api/channels/main/catalog/meta/movie/tt1").ip("2.2.2.2"))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        request_title(&h, "movie", "tt1").await.status,
        StatusCode::NOT_FOUND
    );
    h.update_settings(SettingsPatch {
        viewer_catalog: Some(true),
        ..Default::default()
    });
    let r = h
        .send(get("/api/channels/main/catalog/meta/movie/tt1").ip("2.2.2.2"))
        .await;
    assert_eq!(r.json()["name"], "Example Film");
    // 1 meta call above + 29 searches fill the 30 of the minute.
    for _ in 0..29 {
        assert_eq!(search().await.status, StatusCode::OK);
    }
    let r = search().await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(r.json()["code"], "too_many_requests");
    // Another address has its own count.
    let r = h
        .send(get("/api/channels/main/catalog/search?q=matrix").ip("3.3.3.3"))
        .await;
    assert_eq!(r.status, StatusCode::OK);
}

async fn choose(h: &Harness, cookie: &str, id: i64, url: &str) -> Resp {
    h.send(
        call(
            "POST",
            &format!("/api/admin/channels/main/items/{id}/source"),
        )
        .cookie(cookie)
        .json(json!({ "url": url })),
    )
    .await
}

#[tokio::test]
async fn choose_source_queues_the_request() {
    let mut h = title_harness();
    request_title(&h, "movie", "tt1").await;
    let id = h.list(ItemStatus::PendingApproval)[0].id;
    let cookie = h.login().await;
    h.commands();
    let r = choose(&h, &cookie, id, "https://youtu.be/a").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let item = h.item(id);
    assert_eq!(item.status, ItemStatus::Queued);
    assert_eq!(item.source_url, "https://youtu.be/a");
    assert_eq!(item.duration_s, Some(100.0));
    assert_eq!(item.title, "Example Film (1999)");
    assert!(
        h.commands()
            .iter()
            .any(|c| matches!(c, Command::QueueChanged))
    );
    // It is queued now: choosing again conflicts.
    let r = choose(&h, &cookie, id, "https://youtu.be/b").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn choose_source_on_rejected_item_conflicts() {
    let h = title_harness();
    request_title(&h, "movie", "tt1").await;
    let id = h.list(ItemStatus::PendingApproval)[0].id;
    let cookie = h.login().await;
    let r = h
        .send(
            call("PATCH", &format!("/api/admin/items/{id}"))
                .cookie(&cookie)
                .json(json!({"action": "reject"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let r = choose(&h, &cookie, id, "https://youtu.be/a").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.json()["code"], "not_title_request");
    assert_eq!(h.item(id).status, ItemStatus::Rejected);
    // An ordinary pending item is no title request either.
    let plain = h.add("Plain", ItemStatus::PendingApproval);
    let r = choose(&h, &cookie, plain.id, "https://youtu.be/a").await;
    assert_eq!(r.json()["code"], "not_title_request");
}

#[tokio::test]
async fn approve_title_request_is_refused() {
    let h = title_harness();
    request_title(&h, "movie", "tt1").await;
    let id = h.list(ItemStatus::PendingApproval)[0].id;
    let cookie = h.login().await;
    let r = h
        .send(
            call("PATCH", &format!("/api/admin/items/{id}"))
                .cookie(&cookie)
                .json(json!({"action": "approve"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.json()["code"], "title_needs_source");
    assert_eq!(h.item(id).status, ItemStatus::PendingApproval);
}

#[tokio::test]
async fn addon_streams_include_torrents_only_when_enabled() {
    let n = net(false);
    n.answers.lock().unwrap().insert(
        "https://addons.example/streams/stream/movie/tt2.json".to_string(),
        json!({"streams": [
            {"url": "https://cdn.example/a.mp4", "name": "Web", "title": "web"},
            {"infoHash": "0123456789abcdef0123456789abcdef01234567", "fileIdx": 2,
             "name": "Tor", "title": "Film 1080p\nseeders 10",
             "sources": ["tracker:udp://t.example:80", "dht:0123"]}
        ]}),
    );
    let hub = crate::torrent::hub::TorrentHub::with_factory(
        std::env::temp_dir().join("vrcinema-test-unused"),
        Arc::new(|_, _, _| Box::pin(async { Err(Msg::error("torrent_session")) })),
    );
    let h = Harness::with_state(config(), {
        let hub = Arc::clone(&hub);
        move |s| {
            s.with_addons(Arc::new(AddonClient::new(n)))
                .with_torrents(hub)
        }
    });
    let cookie = h.login().await;
    let id = add(&h, &cookie, STREAMS_URL).await.json()["id"]
        .as_i64()
        .unwrap();
    let url = format!("/api/admin/addons/streams/movie/tt2?addon={id}");

    let off = h.send(get(&url).cookie(&cookie)).await.json();
    assert_eq!(off["streams"].as_array().unwrap().len(), 1);
    assert_eq!(off["streams"][0]["torrent"], false);
    assert_eq!(
        off["hiddenTorrents"], 1,
        "the window says how many the setting hides"
    );

    h.send(
        call("PUT", "/api/admin/channels/main/settings")
            .cookie(&cookie)
            .json(json!({"torrentsEnabled": true})),
    )
    .await;
    let on = h.send(get(&url).cookie(&cookie)).await.json();
    let streams = on["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    assert_eq!(on["hiddenTorrents"], 0);
    assert_eq!(streams[1]["torrent"], true);
    let magnet = streams[1]["url"].as_str().unwrap();
    let parsed = crate::torrent::parse_magnet(magnet).unwrap();
    assert_eq!(parsed.info_hash, "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(parsed.file_idx, Some(2));
    assert_eq!(parsed.trackers, vec!["udp://t.example:80"]);
    assert_eq!(parsed.name.as_deref(), Some("Film 1080p"));
}
