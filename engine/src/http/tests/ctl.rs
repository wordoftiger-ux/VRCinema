//! `vrcinema ctl` against the test server on a real loopback port: the token of the control file,
//! the password, and the token refused from elsewhere.

use super::*;
use crate::ctl::{CONTROL_FILE, ControlFile, CtlEnv};

const TOKEN: &str = "ctl-test-token-0123456789abcdef";

fn harness() -> Harness {
    Harness::with_state(config(), |s| s.with_ctl_token(TOKEN))
}

/// The harness's router served on 127.0.0.1 (a free port); its base URL.
async fn serve(h: &Harness) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = h.router.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    format!("http://{addr}")
}

/// A root folder whose `data` holds the control file (when `url` is given).
fn env(root: &Path, vars: &[(&str, &str)]) -> CtlEnv {
    CtlEnv {
        root: root.to_path_buf(),
        vars: vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        app_home: None,
        read_stdin: Box::new(|| None),
    }
}

async fn ctl(env: &CtlEnv, args: &[&str]) -> (u8, String, String) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = crate::ctl::run(&args, env, &mut out, &mut err).await;
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn json_of(out: &str) -> Value {
    serde_json::from_str(out).unwrap_or_else(|e| panic!("{e}: {out}"))
}

#[tokio::test]
async fn ctl_steers_the_server_with_the_token_of_the_control_file() {
    let mut h = harness();
    let url = serve(&h).await;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    let _file = ControlFile::write(&root.path().join("data"), &url, TOKEN).unwrap();
    let e = env(root.path(), &[]);

    let (code, out, _) = ctl(&e, &["status", "--json"]).await;
    assert_eq!(code, 0, "{out}");
    assert_eq!(json_of(&out)["status"], "idle");
    let (code, out, _) = ctl(&e, &["status"]).await;
    assert_eq!(code, 0);
    assert!(
        out.contains("Nothing is playing") || out.contains("Nic"),
        "{out}"
    );

    let (code, out, err) = ctl(&e, &["queue", "add", "https://youtu.be/a"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains('A'), "{out}");
    let queued = h.list(ItemStatus::Queued);
    assert_eq!(queued.len(), 1);
    let (code, out, _) = ctl(&e, &["queue", "list", "--json"]).await;
    assert_eq!(code, 0);
    assert_eq!(json_of(&out)["queue"][0]["title"], "A");
    let (code, out, _) = ctl(&e, &["queue", "list"]).await;
    assert_eq!(code, 0);
    assert!(out.contains(&format!("#{}", queued[0].id)), "{out}");

    let id = queued[0].id.to_string();
    assert_eq!(ctl(&e, &["queue", "remove", &id]).await.0, 0);
    assert_eq!(h.item(queued[0].id).status, ItemStatus::Removed);
    assert_eq!(ctl(&e, &["skip"]).await.0, 0);
    let commands = h.commands();
    assert!(commands.contains(&Command::QueueChanged), "{commands:?}");
    assert!(commands.contains(&Command::Skip), "{commands:?}");

    h.add("X", ItemStatus::Queued);
    let (code, out, _) = ctl(&e, &["queue", "clear", "--json"]).await;
    assert_eq!(code, 0);
    assert_eq!(json_of(&out)["removed"], 1);

    let (code, out, _) = ctl(&e, &["settings", "set", "oscEnabled", "true", "--json"]).await;
    assert_eq!(code, 0, "{out}");
    assert_eq!(json_of(&out), json!({"oscEnabled": true}));
    let stored = h.with_db(|c| SettingsRepo::new(c).get(MAIN_CHANNEL_ID).unwrap());
    assert!(stored.osc.osc_enabled);
    let (code, out, _) = ctl(&e, &["settings", "get"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("oscTemplate = "), "{out}");

    // Refusals of the server: exit 1 with its text (and code with --json).
    let (code, _, err) = ctl(&e, &["settings", "set", "oscRepeatS", "3"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("oscRepeatS"), "{err}");
    let (code, out, _) = ctl(&e, &["pause", "--json"]).await;
    assert_eq!(code, 1);
    assert_eq!(json_of(&out)["code"], "nothing_playing");
    assert_eq!(json_of(&out)["exitCode"], 1);
    assert_eq!(ctl(&e, &["play-now", "999"]).await.0, 1);
    // Wrong usage: exit 2, nothing sent.
    assert_eq!(
        ctl(&e, &["settings", "set", "oscHost", "10.0.0.1"]).await.0,
        2
    );
}

#[tokio::test]
async fn a_wrong_or_stale_control_file_and_the_password() {
    let h = harness();
    let url = serve(&h).await;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();

    // Another run's token: not authorised (4).
    let file = ControlFile::write(&data, &url, "not-the-token").unwrap();
    assert_eq!(ctl(&env(root.path(), &[]), &["status"]).await.0, 4);
    drop(file);
    assert!(!data.join(CONTROL_FILE).exists());

    // Nothing running and no password: 3.
    assert_eq!(ctl(&env(root.path(), &[]), &["status"]).await.0, 3);
    // A stale file (the program is gone): 3.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let gone = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let stale = ControlFile::write(&data, &gone, TOKEN).unwrap();
    let (code, _, err) = ctl(&env(root.path(), &[]), &["status"]).await;
    assert_eq!(code, 3, "{err}");
    drop(stale);

    // The admin password of `.env` logs in (no control file).
    let port = url.rsplit(':').next().unwrap();
    let with_password = env(root.path(), &[("ADMIN_PASSWORD", "tajne"), ("PORT", port)]);
    let (code, out, err) = ctl(&with_password, &["status", "--json"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(json_of(&out)["status"], "idle");
    let wrong = env(
        root.path(),
        &[("ADMIN_PASSWORD", "zle-haslo"), ("PORT", port)],
    );
    assert_eq!(ctl(&wrong, &["status"]).await.0, 4);
}

#[tokio::test]
async fn the_token_counts_only_from_this_computer_directly() {
    let h = harness();
    let bearer = format!("Bearer {TOKEN}");
    let panel = || get("/api/admin/channels/main").header("authorization", &bearer);
    assert_eq!(h.send(panel()).await.status, StatusCode::OK);
    assert_eq!(
        h.send(panel().ip("::ffff:127.0.0.1")).await.status,
        StatusCode::OK
    );
    for refused in [
        panel().ip("192.168.1.9"),
        panel().header("x-forwarded-for", "1.2.3.4"),
        panel().header("cf-connecting-ip", "1.2.3.4"),
        panel().header("forwarded", "for=1.2.3.4"),
        get("/api/admin/channels/main").header("authorization", "Bearer nope"),
    ] {
        assert_eq!(h.send(refused).await.status, StatusCode::UNAUTHORIZED);
    }
    // Without a token installed nothing counts.
    let plain = Harness::new();
    assert_eq!(plain.send(panel()).await.status, StatusCode::UNAUTHORIZED);
}
