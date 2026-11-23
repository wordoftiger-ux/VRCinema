//! Skip voting: `POST /api/channels/main/skip-vote`, the `skipVote` of the public and the panel
//! state and the threshold math.

use super::*;
use crate::db::{SettingsRepo, SkipVoteMode, sanitize_settings_patch};
use crate::http::routes_skip_vote::needed_votes;
use std::net::{IpAddr, Ipv4Addr};

const VOTE: &str = "/api/channels/main/skip-vote";
const KEY_C: &str = "cccccccccccccccccccccccccccccccc";

fn settings(patch: Value) -> crate::db::ChannelSettings {
    let mut s = crate::db::ChannelSettings::default();
    let patch = sanitize_settings_patch(&patch).unwrap();
    if let Some(m) = patch.skip_vote_mode {
        s.skip_vote_mode = m;
    }
    s.skip_vote_percent = patch.skip_vote_percent.unwrap_or(s.skip_vote_percent);
    s.skip_vote_count = patch.skip_vote_count.unwrap_or(s.skip_vote_count);
    s
}

fn configure(h: &Harness, patch: Value) {
    let patch = sanitize_settings_patch(&patch).unwrap();
    h.with_db(|c| {
        SettingsRepo::new(c)
            .update(MAIN_CHANNEL_ID, &patch)
            .unwrap()
    });
}

fn on_air(h: &Harness, item_id: i64) {
    h.snapshots.send_replace(snapshot(
        h.live_dir.path(),
        &["5.ts"],
        ChannelState {
            status: ChannelStatus::Playing,
            current: Some(NowPlaying {
                item_id,
                title: format!("Film {item_id}"),
                duration_s: Some(100.0),
                position_s: 8,
                requester_name: None,
                added_by: AddedBy::Admin,
                requester_key_hash: None,
                is_live: false,
            }),
            paused: false,
            pause_reasons: Vec::new(),
            control: Default::default(),
            obs: Default::default(),
        },
    ));
}

async fn vote(h: &Harness, ip: &str, key: &str, yes: bool) -> Resp {
    h.send(
        call("POST", VOTE)
            .ip(ip)
            .json(json!({"deviceKey": key, "vote": yes})),
    )
    .await
}

fn watchers(n: u8) -> Arc<crate::auto_pause::ViewerActivity> {
    let viewers = Arc::new(crate::auto_pause::ViewerActivity::new());
    for i in 0..n {
        viewers.record(Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i + 1))), 1000);
    }
    viewers
}

#[test]
fn needed_votes_math() {
    let pct = |p, w| {
        needed_votes(
            &settings(json!({"skipVoteMode": "percent", "skipVotePercent": p})),
            w,
        )
    };
    assert_eq!(pct(50, 0), None, "nobody watching: unknown, never fires");
    assert_eq!(pct(50, 1), Some(1));
    assert_eq!(pct(50, 3), Some(2));
    assert_eq!(pct(50, 4), Some(2));
    assert_eq!(pct(100, 7), Some(7));
    assert_eq!(pct(1, 5), Some(1));
    let count = needed_votes(
        &settings(json!({"skipVoteMode": "count", "skipVoteCount": 4})),
        99,
    );
    assert_eq!(count, Some(4));
    let signal = needed_votes(&settings(json!({"skipVoteMode": "signal"})), 9);
    assert_eq!(signal, None);
    assert_eq!(
        needed_votes(&settings(json!({"skipVoteMode": "off"})), 9),
        None
    );
}

#[test]
fn settings_are_validated_and_clamped() {
    let p = sanitize_settings_patch(
        &json!({"skipVoteMode": "count", "skipVotePercent": 0, "skipVoteCount": 500}),
    )
    .unwrap();
    assert_eq!(p.skip_vote_mode, Some(SkipVoteMode::Count));
    assert_eq!(
        (p.skip_vote_percent, p.skip_vote_count),
        (Some(1), Some(100))
    );
    assert!(sanitize_settings_patch(&json!({"skipVoteMode": "most"})).is_err());
    assert!(sanitize_settings_patch(&json!({"skipVoteCount": "x"})).is_err());
}

#[tokio::test]
async fn off_is_404_and_the_state_has_no_skip_vote() {
    let h = Harness::new();
    on_air(&h, 1);
    // The default is percent: voting works and the state has the block.
    assert!(h.send(get("/api/channels/main/state")).await.json()["skipVote"].is_object());
    configure(&h, json!({"skipVoteMode": "off"}));
    assert_eq!(
        vote(&h, "1.1.1.1", KEY_A, true).await.status,
        StatusCode::NOT_FOUND
    );
    let state = h.send(get("/api/channels/main/state")).await.json();
    assert!(state.get("skipVote").is_none());
}

#[tokio::test]
async fn one_vote_per_address_withdraw_and_mine() {
    let h = Harness::with_state(config(), |s| s.with_viewers(watchers(10)));
    configure(&h, json!({"skipVoteMode": "count", "skipVoteCount": 3}));
    on_air(&h, 1);
    let r = vote(&h, "1.1.1.1", KEY_A, true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.json()["skipVote"],
        json!({"mode": "count", "votes": 1, "needed": 3, "mine": true})
    );
    // Another key (client-made) from the same address is not another vote.
    let minted = vote(&h, "1.1.1.1", KEY_B, true).await;
    assert_eq!(minted.json()["skipVote"]["votes"], 1);
    assert_eq!(minted.json()["skipVote"]["mine"], true);
    // The same key from another address is another voter.
    let other = vote(&h, "2.2.2.2", KEY_A, true).await;
    assert_eq!(other.json()["skipVote"]["votes"], 2);
    // `mine` of the state is by address; the panel has none.
    let mine = h
        .send(get("/api/channels/main/state").ip("1.1.1.1"))
        .await
        .json();
    assert_eq!(mine["skipVote"]["mine"], true);
    let stranger = h
        .send(get("/api/channels/main/state").ip("9.9.9.9"))
        .await
        .json();
    assert_eq!(stranger["skipVote"]["mine"], false);
    // Withdrawing from the address, whatever key.
    let r = vote(&h, "1.1.1.1", KEY_C, false).await.json();
    assert_eq!(
        r["skipVote"],
        json!({"mode": "count", "votes": 1, "needed": 3, "mine": false})
    );
    let cookie = h.login().await;
    let panel = h
        .send(get("/api/admin/channels/main").cookie(&cookie))
        .await
        .json();
    assert_eq!(
        panel["skipVote"],
        json!({"mode": "count", "votes": 1, "needed": 3})
    );
    assert_eq!(panel["settings"]["skipVoteMode"], "count");
    // A vote without a device key or without a value is a bad request.
    let no_key = h.send(call("POST", VOTE).json(json!({"vote": true}))).await;
    assert_eq!(no_key.status, StatusCode::BAD_REQUEST);
    let no_vote = h
        .send(call("POST", VOTE).json(json!({"deviceKey": KEY_A})))
        .await;
    assert_eq!(no_vote.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn many_keys_from_one_address_never_skip() {
    let mut h = Harness::with_state(config(), |s| s.with_viewers(watchers(10)));
    configure(&h, json!({"skipVoteMode": "count", "skipVoteCount": 2}));
    on_air(&h, 1);
    for i in 0..10 {
        let key = format!("{i:032x}");
        vote(&h, "1.1.1.1", &key, true).await;
    }
    assert!(h.commands.try_recv().is_err());
}

#[tokio::test]
async fn a_new_film_starts_from_no_votes_and_an_old_item_is_ignored() {
    let h = Harness::with_state(config(), |s| s.with_viewers(watchers(10)));
    configure(&h, json!({"skipVoteMode": "count", "skipVoteCount": 5}));
    on_air(&h, 1);
    vote(&h, "1.1.1.1", KEY_A, true).await;
    on_air(&h, 2);
    let state = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(state["skipVote"]["votes"], 0);
    // A vote for the film that was on air is refused and counts for nothing.
    let stale = h
        .send(call("POST", VOTE).json(json!({"deviceKey": KEY_B, "vote": true, "itemId": 1})))
        .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.json()["code"], "skip_vote_stale");
    let state = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(state["skipVote"]["votes"], 0);
    // The film on air is accepted.
    let ok = h
        .send(call("POST", VOTE).json(json!({"deviceKey": KEY_B, "vote": true, "itemId": 2})))
        .await;
    assert_eq!(ok.json()["skipVote"]["votes"], 1);
    // Nothing on air: nothing to vote for.
    h.snapshots
        .send_replace(snapshot(h.live_dir.path(), &["5.ts"], idle_state()));
    assert_eq!(
        vote(&h, "1.1.1.1", KEY_A, true).await.status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn a_blocked_device_is_refused_like_a_request() {
    let h = Harness::new();
    let cookie = h.login().await;
    h.request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
        .await;
    let skeleton = crate::nicks::skeleton("Kasia");
    let block = h
        .send(
            call("POST", "/api/admin/devices/block")
                .cookie(&cookie)
                .json(json!({"skeleton": skeleton})),
        )
        .await;
    assert_eq!(block.status, StatusCode::OK);
    on_air(&h, 1);
    let r = vote(&h, "1.1.1.1", KEY_A, true).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json()["code"], "device_blocked");
}

#[tokio::test]
async fn reaching_the_count_skips_once_and_logs() {
    let mut h = Harness::with_state(config(), |s| s.with_viewers(watchers(10)));
    configure(&h, json!({"skipVoteMode": "count", "skipVoteCount": 2}));
    on_air(&h, 1);
    vote(&h, "1.1.1.1", KEY_A, true).await;
    assert!(
        h.commands.try_recv().is_err(),
        "one vote of two does not skip"
    );
    vote(&h, "2.2.2.2", KEY_B, true).await;
    assert!(matches!(h.commands.try_recv(), Ok(Command::Skip)));
    assert!(
        h.logs()
            .iter()
            .any(|l| l.contains("Skipped by viewer vote") && l.contains("Film 1"))
    );
    // A third vote before the engine moved on does not skip a second time.
    vote(&h, "3.3.3.3", KEY_C, true).await;
    assert!(h.commands.try_recv().is_err());
}

#[tokio::test]
async fn percent_with_nobody_watching_never_skips_and_needs_nothing() {
    let mut h = Harness::new();
    configure(&h, json!({"skipVoteMode": "percent"}));
    on_air(&h, 1);
    let r = vote(&h, "1.1.1.1", KEY_A, true).await;
    assert_eq!(r.json()["skipVote"]["needed"], Value::Null);
    assert_eq!(r.json()["skipVote"]["votes"], 1);
    assert!(h.commands.try_recv().is_err());
}

#[tokio::test]
async fn a_state_request_completes_a_skip_when_the_threshold_drops() {
    let mut h = Harness::with_state(config(), |s| s.with_viewers(watchers(10)));
    configure(
        &h,
        json!({"skipVoteMode": "percent", "skipVotePercent": 100}),
    );
    on_air(&h, 1);
    for (ip, key) in [("1.1.1.1", KEY_A), ("2.2.2.2", KEY_B), ("3.3.3.3", KEY_C)] {
        vote(&h, ip, key, true).await;
    }
    assert!(h.commands.try_recv().is_err(), "3 of 10 needed");
    // The needed number falls (here: the owner lowers the percentage).
    configure(&h, json!({"skipVotePercent": 30}));
    let state = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(state["skipVote"]["needed"], 3);
    assert!(matches!(h.commands.try_recv(), Ok(Command::Skip)));
    h.send(get("/api/channels/main/state")).await;
    assert!(h.commands.try_recv().is_err(), "skipped once");
}

#[tokio::test]
async fn signal_mode_never_skips() {
    let mut h = Harness::with_state(config(), |s| s.with_viewers(watchers(1)));
    configure(&h, json!({"skipVoteMode": "signal"}));
    on_air(&h, 1);
    for (ip, key) in [("1.1.1.1", KEY_A), ("2.2.2.2", KEY_B), ("3.3.3.3", KEY_C)] {
        let r = vote(&h, ip, key, true).await;
        assert_eq!(r.json()["skipVote"]["needed"], Value::Null);
    }
    assert!(h.commands.try_recv().is_err());
    let state = h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(
        state["skipVote"],
        json!({"mode": "signal", "votes": 3, "needed": null, "mine": false})
    );
}

#[tokio::test]
async fn votes_are_limited_per_address() {
    let h = Harness::with_state(config(), |s| s.with_viewers(watchers(10)));
    configure(&h, json!({"skipVoteMode": "count", "skipVoteCount": 100}));
    on_air(&h, 1);
    for _ in 0..20 {
        assert_eq!(
            vote(&h, "1.1.1.1", KEY_A, true).await.status,
            StatusCode::OK
        );
    }
    let r = vote(&h, "1.1.1.1", KEY_A, false).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(r.json()["code"], "too_many_requests");
    assert_eq!(
        vote(&h, "2.2.2.2", KEY_B, true).await.status,
        StatusCode::OK
    );
}
