//! Viewer lists (`POST /api/channels/main/request-list`, the panel's `pendingLists` and
//! `POST /api/admin/channels/main/lists/{id}`).

use super::*;

const LIST: &str = "/api/channels/main/request-list";

fn lists_on(h: &Harness, extra: SettingsPatch) {
    h.update_settings(SettingsPatch {
        viewer_lists: Some(true),
        request_limit_count: extra.request_limit_count.or(Some(100)),
        max_active_per_requester: extra.max_active_per_requester.or(Some(20)),
        ..extra
    });
}

async fn send_list(h: &Harness, urls: &[&str]) -> Resp {
    h.send(
        call("POST", LIST)
            .ip("1.1.1.1")
            .json(json!({"urls": urls, "name": "Ola", "deviceKey": KEY_A})),
    )
    .await
}

fn statuses(res: &Resp) -> Vec<(i64, String, String)> {
    res.json()["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["line"].as_i64().unwrap(),
                r["title"].as_str().unwrap_or("").to_string(),
                r["code"]
                    .as_str()
                    .unwrap_or(r["status"].as_str().unwrap())
                    .to_string(),
            )
        })
        .collect()
}

fn add_playlist(h: &Harness, url: &str, entries: Vec<PlaylistEntry>) {
    h.metas.lock().unwrap().insert(
        url.into(),
        Ok(MediaMeta {
            title: "Mix".into(),
            duration_s: None,
            is_live: false,
            is_playlist: true,
            entries: Some(entries),
        }),
    );
}

fn entry(url: &str, title: &str, duration_s: f64, is_live: bool) -> PlaylistEntry {
    PlaylistEntry {
        url: url.into(),
        title: title.into(),
        duration_s: Some(duration_s),
        is_live,
    }
}

#[tokio::test]
async fn lists_are_refused_while_the_setting_is_off() {
    let h = Harness::new();
    let res = send_list(&h, &["https://youtu.be/a"]).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(res.json()["code"], "viewer_lists_off");
    assert!(h.list(ItemStatus::Queued).is_empty());
}

#[tokio::test]
async fn several_links_each_pass_the_checks_of_a_single_request() {
    let h = Harness::new();
    lists_on(&h, SettingsPatch::default());
    let res = send_list(
        &h,
        &[
            "https://youtu.be/a",
            "",
            "http://192.168.0.1/x",
            "https://evil.example/x",
            "https://youtu.be/long",
            "https://youtu.be/live",
            "https://youtu.be/a",
            "file:///C:/x.mp4",
            "https://youtu.be/b",
        ],
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text());
    // Blank lines are dropped before numbering.
    assert_eq!(
        statuses(&res),
        vec![
            (1, "A".into(), "queued".into()),
            (2, "".into(), "local_address".into()),
            (3, "".into(), "local_address".into()),
            (4, "Long".into(), "too_long".into()),
            (5, "Live".into(), "live_owner_only".into()),
            (6, "".into(), "duplicate_link".into()),
            (7, "".into(), "bad_scheme".into()),
            (8, "B".into(), "queued".into()),
        ]
    );
    assert_eq!(res.json()["added"], 2);
    assert!(res.json()["requesterBadge"]["hue"].is_number());
    assert_eq!(res.json()["skipped"], 6);
    let items = h.list(ItemStatus::Queued);
    assert_eq!(items.len(), 2);
    for item in &items {
        assert_eq!(item.added_by, AddedBy::Public);
        assert_eq!(item.requester_name.as_deref(), Some("Ola"));
        assert_eq!(item.requester_ip.as_deref(), Some("1.1.1.1"));
        assert!(item.requester_key_hash.is_some());
    }
    // Reasons are texts in the request's language; one log line per list, without addresses.
    let reason = res.json()["results"][1]["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        !reason.is_empty() && !reason.contains("192.168"),
        "{reason}"
    );
    let logs = h.logs();
    assert_eq!(logs.len(), 1);
    assert!(logs[0].contains("2 added, 6 skipped"), "{}", logs[0]);
}

#[tokio::test]
async fn a_playlist_is_expanded_and_every_entry_is_checked() {
    let h = Harness::new();
    lists_on(
        &h,
        SettingsPatch {
            link_mode: Some(LinkMode::Blocklist),
            blocked_domains: Some(vec!["bad.example".into()]),
            ..Default::default()
        },
    );
    add_playlist(
        &h,
        "https://youtube.com/playlist?list=mix",
        vec![
            entry("https://youtu.be/m1", "M1", 10.0, false),
            entry("https://evil.example/m2", "M2", 10.0, false),
            entry("https://bad.example/m3", "M3", 10.0, false),
            entry("https://youtu.be/m4", "M4", 99_999.0, false),
            entry("https://youtu.be/m5", "M5", 10.0, true),
            entry("file:///C:/m6.mp4", "M6", 10.0, false),
            entry("https://youtu.be/m1", "M1 again", 10.0, false),
            entry("https://youtu.be/m8", "M8", 10.0, false),
        ],
    );
    let res = send_list(&h, &["https://youtube.com/playlist?list=mix"]).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text());
    let codes: Vec<String> = statuses(&res).into_iter().map(|r| r.2).collect();
    assert_eq!(
        codes,
        [
            "queued",
            "local_address",
            "domain_blocked",
            "too_long",
            "live_owner_only",
            "invalid_link",
            "duplicate_link",
            "queued"
        ]
    );
    assert_eq!(queued_titles(&h), ["M1", "M8"]);
    assert_eq!(h.list(ItemStatus::Queued)[0].duration_s, Some(10.0));
}

#[tokio::test]
async fn a_list_adds_at_most_its_size_and_the_viewers_free_places() {
    let h = Harness::new();
    lists_on(
        &h,
        SettingsPatch {
            viewer_list_max: Some(3),
            ..Default::default()
        },
    );
    let res = send_list(
        &h,
        &[
            "https://youtube.com/playlist?list=1",
            "https://youtu.be/a",
            "https://youtu.be/b",
            "https://youtu.be/c",
        ],
    )
    .await;
    let codes: Vec<String> = statuses(&res).into_iter().map(|r| r.2).collect();
    assert_eq!(
        codes,
        ["queued", "queued", "queued", "list_full", "list_full"]
    );
    assert_eq!(queued_titles(&h), ["P1", "P2", "A"]);

    // Two places left of five: the list stops there with the viewer's own limit.
    let h = Harness::new();
    lists_on(
        &h,
        SettingsPatch {
            max_active_per_requester: Some(2),
            ..Default::default()
        },
    );
    let res = send_list(
        &h,
        &[
            "https://youtu.be/a",
            "https://youtu.be/b",
            "https://youtu.be/c",
        ],
    )
    .await;
    let codes: Vec<String> = statuses(&res).into_iter().map(|r| r.2).collect();
    assert_eq!(codes, ["queued", "queued", "too_many_active"]);
    let full = send_list(&h, &["https://youtu.be/d"]).await;
    assert_eq!(full.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(full.json()["code"], "too_many_active");
}

#[tokio::test]
async fn every_pasted_link_counts_towards_the_request_limit() {
    let h = Harness::new();
    lists_on(
        &h,
        SettingsPatch {
            request_limit_count: Some(2),
            ..Default::default()
        },
    );
    let res = send_list(
        &h,
        &[
            "https://youtu.be/a",
            "http://127.0.0.1/x",
            "https://youtube.com/playlist?list=1",
            "https://youtu.be/b",
        ],
    )
    .await;
    let codes: Vec<String> = statuses(&res).into_iter().map(|r| r.2).collect();
    // A link refused before the lookup costs nothing; the playlist is one hit.
    assert_eq!(
        codes,
        [
            "queued",
            "local_address",
            "queued",
            "queued",
            "too_many_requests"
        ]
    );
    // The single route shares the limiter.
    let single = h.request_default("https://youtu.be/c").await;
    assert_eq!(single.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_blocked_device_or_a_taken_nick_refuses_the_whole_list() {
    let h = Harness::new();
    lists_on(&h, SettingsPatch::default());
    assert_eq!(
        h.request("https://youtu.be/a", "2.2.2.2", Some("Ola"))
            .await
            .status,
        StatusCode::CREATED
    );
    let res = h
        .send(
            call("POST", LIST)
                .ip("3.3.3.3")
                .json(json!({"urls": ["https://youtu.be/b"], "name": "Ola", "deviceKey": KEY_B})),
        )
        .await;
    assert_eq!(res.status, StatusCode::CONFLICT);
    assert_eq!(res.json()["code"], "nick_taken");
    for bad in [
        json!({}),
        json!({"urls": []}),
        json!({"urls": ["  "]}),
        json!({"urls": [5]}),
    ] {
        let res = h.send(call("POST", LIST).json(bad.clone())).await;
        assert_eq!(res.json()["code"], "invalid_link", "{bad}");
    }
}

#[tokio::test]
async fn with_approval_a_list_waits_as_one_group_and_is_decided_whole_or_per_item() {
    let mut h = Harness::new();
    lists_on(
        &h,
        SettingsPatch {
            moderation: Some(true),
            ..Default::default()
        },
    );
    let res = send_list(
        &h,
        &[
            "https://youtube.com/playlist?list=1",
            "https://youtu.be/a",
            "https://youtu.be/b",
        ],
    )
    .await;
    assert_eq!(res.json()["status"], "pending_approval");
    let list_id = res.json()["listId"].as_i64().unwrap();
    let pending = h.list(ItemStatus::PendingApproval);
    assert_eq!(pending.len(), 4);
    let cookie = h.login().await;
    let panel = h
        .send(get("/api/admin/channels/main").cookie(&cookie))
        .await
        .json();
    let ids: Vec<i64> = pending.iter().map(|i| i.id).collect();
    assert_eq!(
        panel["pendingLists"],
        json!([{"id": list_id, "title": "PL", "itemIds": ids}])
    );
    // One item rejected on its own, the rest approved together.
    let res = h
        .send(
            call("PATCH", &format!("/api/admin/items/{}", ids[0]))
                .cookie(&cookie)
                .json(json!({"action": "reject"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    h.commands();
    let decide = |action: &str| {
        call("POST", &format!("/api/admin/channels/main/lists/{list_id}"))
            .cookie(&cookie)
            .json(json!({ "action": action }))
    };
    let res = h.send(decide("approve")).await;
    assert_eq!(res.json(), json!({"changed": 3}));
    assert!(h.commands().contains(&Command::QueueChanged));
    assert_eq!(queued_titles(&h), ["P2", "A", "B"]);
    assert_eq!(h.item(ids[0]).status, ItemStatus::Rejected);
    let res = h.send(decide("approve")).await;
    assert_eq!(res.json()["code"], "not_pending");
    let res = h
        .send(
            call("POST", "/api/admin/channels/main/lists/999")
                .cookie(&cookie)
                .json(json!({"action": "reject"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    let res = h
        .send(
            call("POST", &format!("/api/admin/channels/main/lists/{list_id}"))
                .json(json!({"action": "reject"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_bulk_tools_take_list_items_like_any_other() {
    let h = Harness::new();
    lists_on(
        &h,
        SettingsPatch {
            moderation: Some(true),
            ..Default::default()
        },
    );
    send_list(&h, &["https://youtube.com/playlist?list=1"]).await;
    h.update_settings(SettingsPatch {
        moderation: Some(false),
        ..Default::default()
    });
    send_list(&h, &["https://youtu.be/a", "https://youtu.be/b"]).await;
    let other = h.request("https://youtu.be/c", "9.9.9.9", None).await;
    assert_eq!(other.status, StatusCode::CREATED);
    let cookie = h.login().await;
    let first = h.list(ItemStatus::Queued)[0].id;
    let res = h
        .send(
            call("POST", "/api/admin/channels/main/remove-requester")
                .cookie(&cookie)
                .json(json!({"itemId": first, "block": true})),
        )
        .await;
    assert_eq!(res.json()["removed"], 4, "{}", res.text());
    assert_eq!(queued_titles(&h), ["C"]);
    assert!(h.list(ItemStatus::PendingApproval).is_empty());
    // The blocked device cannot send another list.
    let res = send_list(&h, &["https://youtu.be/d"]).await;
    assert_eq!(res.json()["code"], "device_blocked");
    assert_eq!(h.item(first).status, ItemStatus::Removed);
    let res = h
        .send(
            call("POST", "/api/admin/channels/main/clear")
                .cookie(&cookie)
                .json(json!({})),
        )
        .await;
    assert_eq!(res.json()["removed"], 1);
}
