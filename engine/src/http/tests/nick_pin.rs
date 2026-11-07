//! The PIN of a viewer's nick: `POST /api/channels/main/nick/protect`, `…/nick/adopt` and the
//! owner's `POST /api/admin/nicks/{skeleton}/reset-pin`.

use super::*;

const PROTECT: &str = "/api/channels/main/nick/protect";
const ADOPT: &str = "/api/channels/main/nick/adopt";
const KEY_C: &str = "cccccccccccccccccccccccccccccccc";

async fn post(h: &Harness, path: &str, ip: &str, body: Value) -> Resp {
    h.send(call("POST", path).ip(ip).json(body)).await
}

async fn protect(h: &Harness, key: &str, pin: &str) -> Resp {
    post(
        h,
        PROTECT,
        "1.1.1.1",
        json!({"deviceKey": key, "name": "Kasia", "pin": pin}),
    )
    .await
}

async fn adopt_pin(h: &Harness, ip: &str, key: &str, name: &str, pin: &str) -> Resp {
    post(
        h,
        ADOPT,
        ip,
        json!({"deviceKey": key, "name": name, "pin": pin}),
    )
    .await
}

#[tokio::test]
async fn a_pin_lets_another_device_adopt_the_nick_and_nothing_secret_comes_back() {
    let h = Harness::new();
    assert_eq!(
        h.request_as("1.1.1.1", Some("Kasia"), Some(KEY_A), None)
            .await
            .status,
        StatusCode::CREATED
    );
    let set = protect(&h, KEY_A, "2580").await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.text());
    let code = set.json()["recoveryCode"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 19);
    assert_eq!(set.json()["nick"], "Kasia");
    // Another device (a new address of the tunnel) cannot use it, until it adopts it.
    let taken = h
        .request_as("2.2.2.2", Some("kasia"), Some(KEY_B), None)
        .await;
    assert_eq!(taken.json()["code"], "nick_taken");
    let wrong = adopt_pin(&h, "2.2.2.2", KEY_B, "kasia", "2581").await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    assert_eq!(wrong.json()["code"], "nick_pin_wrong");
    let ok = adopt_pin(&h, "2.2.2.2", KEY_B, "KASIA", "2580").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    assert_eq!(ok.json()["nick"], "Kasia", "the nick as it is written");
    assert!(ok.json().get("recoveryCode").is_none());
    let b = crate::nicks::badge(&crate::nicks::hash_device_key(KEY_B).unwrap());
    assert_eq!(ok.json()["requesterBadge"]["hue"], b.hue);
    for (ip, key) in [("3.3.3.3", KEY_B), ("4.4.4.4", KEY_A)] {
        let res = h.request_as(ip, Some("Kasia"), Some(key), None).await;
        assert_eq!(res.status, StatusCode::CREATED, "{key} kept the nick");
    }
    // The owner's list: protected, two devices, no hash.
    let cookie = h.login().await;
    let list = h.send(get("/api/admin/nicks").cookie(&cookie)).await;
    assert_eq!(list.json()["nicks"][0]["protected"], true);
    assert_eq!(list.json()["nicks"][0]["devices"], 2);
    let hash = crate::nicks::hash_device_key(KEY_A).unwrap();
    for text in [set.text(), ok.text(), wrong.text(), list.text()] {
        assert!(
            !text.contains("2580") && !text.contains("$argon2"),
            "{text}"
        );
        assert!(!text.contains(&hash), "{text}");
    }
    assert!(list.text().find(&code).is_none());
    for line in h.logs() {
        assert!(!line.contains("2580") && !line.contains(&code), "{line}");
    }
}

#[tokio::test]
async fn every_mismatch_is_the_same_answer() {
    let h = Harness::new();
    h.request_as("1.1.1.1", Some("Basia"), Some(KEY_A), None)
        .await;
    h.request_as("1.1.1.2", Some("Kasia"), Some(KEY_C), None)
        .await;
    protect(&h, KEY_C, "2580").await;
    let mut answers = Vec::new();
    for (i, name) in ["Nikt", "Basia", "Kasia", "Host"].into_iter().enumerate() {
        let res = adopt_pin(&h, &format!("5.5.5.{i}"), KEY_B, name, "9999").await;
        answers.push((res.status, res.text()));
    }
    let code = post(
        &h,
        ADOPT,
        "6.6.6.6",
        json!({"deviceKey": KEY_B, "name": "Basia", "recoveryCode": "0000-0000-0000-0000", "newPin": "13579"}),
    )
    .await;
    answers.push((code.status, code.text()));
    assert!(answers.iter().all(|a| a == &answers[0]), "{answers:?}");
    assert_eq!(answers[0].0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_recovery_code_resets_the_pin_and_signs_the_other_devices_out() {
    let h = Harness::new();
    let first = protect(&h, KEY_A, "2580").await;
    assert_eq!(first.status, StatusCode::OK, "a free nick is claimed");
    let code = first.json()["recoveryCode"].as_str().unwrap().to_string();
    let weak = post(
        &h,
        ADOPT,
        "2.2.2.2",
        json!({"deviceKey": KEY_B, "name": "Kasia", "recoveryCode": code, "newPin": "1234"}),
    )
    .await;
    assert_eq!(weak.json()["code"], "nick_pin_weak");
    let typed = code.to_lowercase().replace('-', " ");
    let res = post(
        &h,
        ADOPT,
        "2.2.2.2",
        json!({"deviceKey": KEY_B, "name": "Kasia", "recoveryCode": typed, "newPin": "kawa z mlekiem"}),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text());
    let new_code = res.json()["recoveryCode"].as_str().unwrap().to_string();
    assert_ne!(new_code, code);
    let a = h
        .request_as("3.3.3.3", Some("Kasia"), Some(KEY_A), None)
        .await;
    assert_eq!(
        a.json()["code"],
        "nick_taken",
        "the old device is signed out"
    );
    let reused = post(
        &h,
        ADOPT,
        "2.2.2.3",
        json!({"deviceKey": KEY_C, "name": "Kasia", "recoveryCode": code, "newPin": "86420"}),
    )
    .await;
    assert_eq!(reused.json()["code"], "nick_pin_wrong", "a code works once");
    assert_eq!(
        adopt_pin(&h, "2.2.2.4", KEY_C, "Kasia", "2580")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        adopt_pin(&h, "2.2.2.4", KEY_C, "Kasia", "kawa z mlekiem")
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn changing_the_pin_needs_the_current_one() {
    let h = Harness::new();
    protect(&h, KEY_A, "2580").await;
    let again = protect(&h, KEY_A, "86420").await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.json()["code"], "nick_pin_required");
    let body = |current: &str| json!({"deviceKey": KEY_A, "name": "Kasia", "pin": "86420", "currentPin": current});
    let wrong = post(&h, PROTECT, "1.1.1.1", body("0001")).await;
    assert_eq!(wrong.json()["code"], "nick_pin_wrong");
    assert_eq!(
        post(&h, PROTECT, "1.1.1.1", body("2580")).await.status,
        StatusCode::OK
    );
    // Another device gets the request's answer, and bad PINs are refused first.
    assert_eq!(
        protect(&h, KEY_B, "86420").await.json()["code"],
        "nick_taken"
    );
    assert_eq!(
        protect(&h, KEY_B, "12").await.json()["code"],
        "nick_pin_invalid"
    );
    let none = post(
        &h,
        PROTECT,
        "1.1.1.1",
        json!({"deviceKey": KEY_B, "pin": "86420"}),
    )
    .await;
    assert_eq!(none.json()["code"], "nick_pin_no_nick");
}

#[tokio::test]
async fn wrong_pins_lock_the_nick_but_not_its_recovery_code() {
    let h = Harness::new();
    let code = protect(&h, KEY_A, "2580").await.json()["recoveryCode"]
        .as_str()
        .unwrap()
        .to_string();
    for i in 0..crate::nick_pin::NICK_FAILURES {
        let res = adopt_pin(&h, &format!("7.7.7.{i}"), KEY_B, "Kasia", "1357").await;
        assert_eq!(res.json()["code"], "nick_pin_wrong");
    }
    let locked = adopt_pin(&h, "8.8.8.8", KEY_B, "Kasia", "2580").await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(locked.json()["code"], "nick_pin_locked");
    let rescued = post(
        &h,
        ADOPT,
        "8.8.8.8",
        json!({"deviceKey": KEY_B, "name": "Kasia", "recoveryCode": code, "newPin": "97531"}),
    )
    .await;
    assert_eq!(rescued.status, StatusCode::OK, "{}", rescued.text());
}

#[tokio::test]
async fn the_owner_resets_a_pin_and_blocks_every_device() {
    let h = Harness::new();
    let cookie = h.login().await;
    let reset =
        |skel: &str| call("POST", &format!("/api/admin/nicks/{skel}/reset-pin")).cookie(&cookie);
    h.request_as("1.1.1.1", Some("Basia"), Some(KEY_C), None)
        .await;
    assert_eq!(h.send(reset("basia")).await.json()["code"], "nick_no_pin");
    protect(&h, KEY_A, "2580").await;
    adopt_pin(&h, "2.2.2.2", KEY_B, "Kasia", "2580").await;
    let skel = crate::nicks::skeleton("Kasia");
    assert_eq!(h.send(reset(&skel)).await.status, StatusCode::OK);
    let list = h.send(get("/api/admin/nicks").cookie(&cookie)).await.json();
    let kasia = list["nicks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["skeleton"] == skel.as_str())
        .unwrap()
        .clone();
    assert_eq!(kasia["protected"], false);
    assert_eq!(
        adopt_pin(&h, "3.3.3.3", KEY_C, "Kasia", "2580")
            .await
            .json()["code"],
        "nick_pin_wrong"
    );
    // Protected again, on two devices: blocking the nick blocks both.
    protect(&h, KEY_B, "2580").await;
    adopt_pin(&h, "3.3.3.4", KEY_A, "Kasia", "2580").await;
    let block = h
        .send(
            call("POST", "/api/admin/devices/block")
                .cookie(&cookie)
                .json(json!({"skeleton": skel})),
        )
        .await;
    assert_eq!(block.status, StatusCode::OK);
    for key in [KEY_A, KEY_B] {
        let res = h
            .request_as("4.4.4.4", Some("Kasia"), Some(key), None)
            .await;
        assert_eq!(res.json()["code"], "device_blocked", "{key}");
    }
    let fresh = adopt_pin(&h, "4.4.4.5", KEY_C, "Kasia", "2580").await;
    assert_eq!(fresh.json()["code"], "device_blocked");
}

/// Security review: a recovery code works once even when two requests bring it at the same
/// time (each verifies it before either writes; the write is conditional on the stored code).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recovery_code_works_once_under_concurrent_requests() {
    let h = Harness::new();
    let code = protect(&h, KEY_A, "2580").await.json()["recoveryCode"]
        .as_str()
        .unwrap()
        .to_string();
    let send = |ip: &'static str, key: &'static str, pin: &'static str| {
        post(
            &h,
            ADOPT,
            ip,
            json!({"deviceKey": key, "name": "Kasia", "recoveryCode": code, "newPin": pin}),
        )
    };
    let (b, c) = tokio::join!(
        send("2.2.2.2", KEY_B, "13579"),
        send("3.3.3.3", KEY_C, "97531")
    );
    let mut statuses = [b.status, c.status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::FORBIDDEN],
        "{} / {}",
        b.text(),
        c.text()
    );
    let loser = if b.status == StatusCode::OK { &c } else { &b };
    assert_eq!(loser.json()["code"], "nick_pin_wrong");
    // Only the winner's new PIN works.
    let (won_pin, lost_pin) = if b.status == StatusCode::OK {
        ("13579", "97531")
    } else {
        ("97531", "13579")
    };
    assert_eq!(
        adopt_pin(&h, "4.4.4.4", KEY_A, "Kasia", lost_pin)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        adopt_pin(&h, "4.4.4.5", KEY_A, "Kasia", won_pin)
            .await
            .status,
        StatusCode::OK
    );
}

/// Security review: correct PINs are not free either — an address has a cap on attempts of
/// any outcome.
#[tokio::test]
async fn correct_pins_are_capped_per_address_too() {
    let h = Harness::new();
    protect(&h, KEY_A, "2580").await;
    let mut last = StatusCode::OK;
    for _ in 0..=crate::nick_pin::IP_ATTEMPTS {
        last = adopt_pin(&h, "9.9.9.9", KEY_B, "Kasia", "2580")
            .await
            .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        adopt_pin(&h, "9.9.9.8", KEY_B, "Kasia", "2580")
            .await
            .status,
        StatusCode::OK
    );
}
