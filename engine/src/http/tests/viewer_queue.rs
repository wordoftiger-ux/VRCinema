//! What the request page's Queue and Now views read from the public state: pictures, "mine",
//! the estimated start of each queued film.

use super::*;
use crate::http::routes_public::public_thumb;

fn on_air(h: &Harness, item_id: i64, duration_s: Option<f64>, position_s: i64, is_live: bool) {
    h.snapshots.send_replace(snapshot(
        h.live_dir.path(),
        &["5.ts"],
        ChannelState {
            status: ChannelStatus::Playing,
            current: Some(NowPlaying {
                item_id,
                title: "On air".into(),
                duration_s,
                position_s,
                requester_name: None,
                added_by: AddedBy::Admin,
                requester_key_hash: None,
                is_live,
            }),
            paused: false,
            pause_reasons: Vec::new(),
            control: Default::default(),
            obs: Default::default(),
        },
    ));
}

fn item(source: &str, duration_s: Option<f64>, live: bool, ip: Option<&str>) -> NewItem {
    NewItem {
        channel_id: MAIN_CHANNEL_ID,
        source_url: source.into(),
        title: format!("T {source}"),
        duration_s,
        is_live: live,
        status: ItemStatus::Queued,
        added_by: if ip.is_some() {
            AddedBy::Public
        } else {
            AddedBy::Admin
        },
        requester_name: None,
        requester_ip: ip.map(str::to_string),
        requester_key_hash: None,
    }
}

/// A film that is on air: started, so not in the queue any more.
fn add_playing(h: &Harness, source: &str, duration_s: Option<f64>, live: bool) -> i64 {
    let id = add(h, item(source, duration_s, live, None));
    h.with_db(|c| QueueRepo::new(c).start_playing(id).unwrap());
    id
}

fn add(h: &Harness, n: NewItem) -> i64 {
    h.with_db(|c| QueueRepo::new(c).add(&n).unwrap().id)
}

async fn state(h: &Harness, ip: &str) -> Value {
    h.send(get("/api/channels/main/state").ip(ip)).await.json()
}

#[tokio::test]
async fn queue_rows_have_start_estimates_and_mine() {
    let h = Harness::new();
    let playing = add_playing(&h, "https://example.com/now", Some(100.0), false);
    on_air(&h, playing, Some(100.0), 40, false);
    add(
        &h,
        item("https://example.com/a", Some(300.0), false, Some("1.1.1.1")),
    );
    add(
        &h,
        item("https://example.com/b", Some(50.0), false, Some("2.2.2.2")),
    );
    add(&h, item("https://example.com/c", None, false, None));
    add(&h, item("https://example.com/d", Some(10.0), false, None));
    let body = state(&h, "1.1.1.1").await;
    let rows = body["queue"].as_array().unwrap();
    let starts: Vec<&Value> = rows.iter().map(|r| &r["startsInS"]).collect();
    // 60 s left of the film on air, then 300, then 50, then c of unknown length: nothing after it.
    assert_eq!(json!(starts), json!([60, 360, 410, null]));
    let mine: Vec<bool> = rows.iter().map(|r| r["mine"].as_bool().unwrap()).collect();
    assert_eq!(mine, [true, false, false, false]);
    // The other viewer owns the second one; no address is ever shown.
    let other = state(&h, "2.2.2.2").await;
    assert_eq!(other["queue"][1]["mine"], true);
    assert_eq!(other["queue"][0]["mine"], false);
    assert!(!body.to_string().contains("1.1.1.1"));
    assert!(!other.to_string().contains("2.2.2.2"));
}

#[tokio::test]
async fn estimates_are_unknown_behind_a_live_stream() {
    let h = Harness::new();
    let playing = add_playing(&h, "https://example.com/live", None, true);
    on_air(&h, playing, None, 5, true);
    add(&h, item("https://example.com/a", Some(30.0), false, None));
    let body = state(&h, "1.1.1.1").await;
    assert_eq!(body["queue"][0]["startsInS"], Value::Null);
}

#[tokio::test]
async fn an_idle_channel_starts_the_first_film_now() {
    let h = Harness::new();
    add(&h, item("https://example.com/a", Some(30.0), false, None));
    add(&h, item("https://example.com/b", Some(30.0), false, None));
    let body = state(&h, "1.1.1.1").await;
    assert_eq!(body["queue"][0]["startsInS"], 0);
    assert_eq!(body["queue"][1]["startsInS"], 30);
}

#[tokio::test]
async fn the_film_on_air_has_its_picture() {
    let h = Harness::new();
    let playing = add_playing(&h, "https://youtu.be/dQw4w9WgXcQ", Some(100.0), false);
    on_air(&h, playing, Some(100.0), 0, false);
    let body = state(&h, "1.1.1.1").await;
    assert_eq!(
        body["current"]["thumb"],
        "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg"
    );
}

#[test]
fn only_public_pictures_reach_viewers() {
    let it = |source: &str| {
        let mut n = item(source, None, false, None);
        n.title = "x".into();
        crate::db::QueueItem {
            id: 1,
            channel_id: 1,
            position: 0,
            source_url: n.source_url,
            title: n.title,
            duration_s: None,
            is_live: false,
            status: ItemStatus::Queued,
            added_by: AddedBy::Admin,
            requester_name: None,
            requester_ip: None,
            requester_key_hash: None,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
            in_s: None,
            out_s: None,
            start_at: None,
        }
    };
    let own = |u: &str| Some(u.to_string());
    let page = it("https://example.com/v");
    for good in [
        "https://img.example.org/a.jpg",
        "https://cdn.example.net:8443/x",
    ] {
        assert_eq!(public_thumb(&page, own(good).as_ref()), own(good));
    }
    for bad in [
        "http://img.example.org/a.jpg",
        "https://10.0.0.1/a.jpg",
        "https://[::1]/a.jpg",
        "https://localhost/a.jpg",
        "https://nas.lan/a.jpg",
        "https://user:pw@img.example.org/a.jpg",
        "file:///c:/a.jpg",
    ] {
        assert_eq!(public_thumb(&page, own(bad).as_ref()), None, "{bad}");
    }
    // A frame of the owner's disk or a sent file is the owner's alone.
    assert_eq!(public_thumb(&it("local:0/a.mp4"), None), None);
    assert_eq!(public_thumb(&it("upload:0123.mp4"), None), None);
    assert_eq!(
        public_thumb(&it("https://youtu.be/dQw4w9WgXcQ"), None),
        own("https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg")
    );
}
