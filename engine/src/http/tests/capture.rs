//! The live capture routes (spec 2026-11-08-capture-design): the program's window from this
//! computer only; a capture never comes from a link, a viewer or a remote panel; window titles
//! and device names never reach the log; Stop ends everything.

use super::*;
use crate::capture::{CaptureRegistry, Catalog, Device};

const ADD: &str = "/api/desktop/capture";
const ROUTES: [(&str, &str); 4] = [
    ("GET", "/api/desktop/capture/sources"),
    ("POST", "/api/desktop/capture/preview"),
    ("POST", ADD),
    ("POST", "/api/desktop/capture/stop"),
];

struct Setup {
    a: AppHarness,
    desktop: String,
    remote: String,
    registry: Arc<CaptureRegistry>,
    state: AppState,
    router: Router,
}

async fn setup() -> Setup {
    let (a, key) = app_harness(remote_on("sekret"));
    let desktop = window_cookie(&a, &key).await;
    let remote = cookie_of(&remote_login(&a, "sekret").await);
    let registry = Arc::new(CaptureRegistry::new("no-ffmpeg".into()));
    registry.set_catalog(Catalog {
        platform: "windows",
        desktop: true,
        region: true,
        windows: vec!["Bankowość — konto".into()],
        cameras: vec![Device {
            id: "@device_pnp_cam".into(),
            name: "Kamera Brio".into(),
        }],
        audio: vec![Device {
            id: "@device_cm_mic".into(),
            name: "Mikrofon Scarlett".into(),
        }],
        ..Catalog::default()
    });
    let state = a.state.clone().with_capture(Arc::clone(&registry));
    let router = super::super::routes_capture::router(state.clone()).with_state(state.clone());
    Setup {
        a,
        desktop,
        remote,
        registry,
        state,
        router,
    }
}

fn window_capture() -> Value {
    json!({
        "kind": "window",
        "source": "Bankowość — konto",
        "audio": "@device_cm_mic",
        "title": "  Mój   ekran ",
    })
}

#[tokio::test]
async fn only_the_window_from_this_computer_may_capture() {
    let s = setup().await;
    for (method, url) in ROUTES {
        let res = send_to(s.router.clone(), call(method, url).json(json!({}))).await;
        assert_eq!(
            res.status,
            StatusCode::UNAUTHORIZED,
            "{url} without a session"
        );
        let res = send_to(
            s.router.clone(),
            call(method, url).cookie(&s.remote).json(window_capture()),
        )
        .await;
        assert_eq!(
            res.status,
            StatusCode::FORBIDDEN,
            "{url} from a remote panel"
        );
        assert_eq!(res.json()["code"], "window_only");
        // The window's cookie from another device, or passed on by cloudflared or a proxy.
        for c in [
            call(method, url).ip("192.168.1.5"),
            call(method, url).header("cf-connecting-ip", "1.2.3.4"),
            call(method, url).header("x-forwarded-for", "1.2.3.4"),
            call(method, url).header("forwarded", "for=1.2.3.4"),
        ] {
            let res = send_to(
                s.router.clone(),
                c.cookie(&s.desktop).json(window_capture()),
            )
            .await;
            assert_eq!(res.status, StatusCode::FORBIDDEN, "{url}");
        }
    }
    assert!(
        s.a.h.list(ItemStatus::Queued).is_empty(),
        "nothing was added"
    );
}

#[tokio::test]
async fn the_window_adds_a_live_capture_without_naming_the_window_anywhere() {
    let mut s = setup().await;
    let res = send_to(
        s.router.clone(),
        call("POST", ADD).cookie(&s.desktop).json(window_capture()),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    let id = res.json()["itemId"].as_i64().unwrap();
    assert_eq!(res.json()["title"], "Mój ekran");
    let item = s.a.h.item(id);
    assert!(item.is_live);
    assert_eq!(item.added_by, AddedBy::Admin);
    assert_eq!(item.status, ItemStatus::Queued);
    let token = item.source_url.strip_prefix("capture:").unwrap();
    assert_eq!(token.len(), 32);
    assert!(!item.source_url.contains("Bank"));
    assert!(s.registry.lookup(token, id).is_some());
    assert!(s.a.h.commands().contains(&Command::QueueChanged));
    let logs = s.a.h.logs().join("\n");
    assert!(!logs.is_empty());
    for secret in ["Bank", "konto", "device_cm", "Scarlett"] {
        assert!(!logs.contains(secret), "{secret} in the log: {logs}");
    }

    // The default title, in the window's language.
    let res = send_to(
        s.router.clone(),
        call("POST", ADD)
            .cookie(&s.desktop)
            .header("x-vrcinema-lang", "en")
            .json(json!({"kind": "desktop"})),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
    assert_eq!(res.json()["title"], "Live");

    // Only what the last listing named; nothing malformed.
    for (body, status) in [
        (
            json!({"kind": "window", "source": "Inne okno"}),
            StatusCode::CONFLICT,
        ),
        (
            json!({"kind": "camera", "source": "@device_pnp_other"}),
            StatusCode::CONFLICT,
        ),
        (json!({"kind": "none"}), StatusCode::BAD_REQUEST),
        (
            json!({"kind": "desktop", "title": "x".repeat(121)}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind": "desktop", "fps": 60}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let res = send_to(
            s.router.clone(),
            call("POST", ADD).cookie(&s.desktop).json(body.clone()),
        )
        .await;
        assert_eq!(res.status, status, "{body}");
    }
    // A preview of sound only has no picture.
    let res = send_to(
        s.router.clone(),
        call("POST", "/api/desktop/capture/preview")
            .cookie(&s.desktop)
            .json(json!({"kind": "none", "audio": "@device_cm_mic"})),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(s.a.h.list(ItemStatus::Queued).len(), 2);
}

#[tokio::test]
async fn stop_drops_every_capture_and_removes_the_waiting_ones() {
    let mut s = setup().await;
    let mut ids = Vec::new();
    for _ in 0..2 {
        let res = send_to(
            s.router.clone(),
            call("POST", ADD)
                .cookie(&s.desktop)
                .json(json!({"kind": "desktop"})),
        )
        .await;
        ids.push(res.json()["itemId"].as_i64().unwrap());
    }
    let film = s.a.h.add("Film", ItemStatus::Queued);
    // The first one is on air.
    s.a.h
        .with_db(|c| QueueRepo::new(c).start_playing(ids[0]).unwrap());
    let info = super::super::routes_capture::capture_info(&s.state).unwrap();
    assert_eq!(info["queued"], 1);
    s.a.h.commands();

    let res = send_to(
        s.router.clone(),
        call("POST", "/api/desktop/capture/stop").cookie(&s.desktop),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json(), json!({"stopped": 2, "removed": 1}));
    assert_eq!(s.a.h.item(ids[1]).status, ItemStatus::Removed);
    // The one on air ends by itself (its clip sees the capture gone); the film stays.
    assert_eq!(s.a.h.item(ids[0]).status, ItemStatus::Playing);
    assert_eq!(s.a.h.item(film.id).status, ItemStatus::Queued);
    let token = s.a.h.item(ids[0]).source_url.replace("capture:", "");
    assert!(!s.registry.is_active(&token));
    assert!(s.a.h.commands().contains(&Command::QueueChanged));
    let info = super::super::routes_capture::capture_info(&s.state).unwrap();
    assert_eq!(info, json!({"running": [], "queued": 0}));
}

/// A capture is no link: the owner's link field, a viewer's request and a remote panel's
/// "play next" of its copy never start one.
#[tokio::test]
async fn a_capture_never_comes_from_a_link() {
    let s = setup().await;
    let res =
        s.a.h
            .send(
                call("POST", "/api/admin/channels/main/items")
                    .cookie(&s.remote)
                    .json(json!({"url": "capture:0123456789abcdef0123456789abcdef"})),
            )
            .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    let res =
        s.a.h
            .request_default("capture:0123456789abcdef0123456789abcdef")
            .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert!(s.a.h.list(ItemStatus::Queued).is_empty());
    assert!(s.a.h.list(ItemStatus::PendingApproval).is_empty());
}
