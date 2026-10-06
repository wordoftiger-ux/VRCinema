//! Smoke test: the whole server (engine, slates, HTTP) on a free port with a fake resolver.
//! Skips when `bin/ffmpeg` is missing.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use vrcinema::config::load_config;
use vrcinema::server::{RunOptions, run};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

/// Minimal HTTP/1.0 GET (no extra client dependency): returns (status, body).
async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split(' ').nth(1).unwrap().parse().unwrap();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

#[tokio::test]
async fn serves_playlist_with_three_segments() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let exe = if cfg!(windows) { ".exe" } else { "" };
    if !root.join("bin").join(format!("ffmpeg{exe}")).exists() {
        eprintln!("pomijam: brak bin/ffmpeg");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let fake = data.path().join("fake.json");
    std::fs::write(&fake, "{}").unwrap();

    let env: HashMap<String, String> = [
        ("ADMIN_PASSWORD", "haslo-testowe-123"),
        ("DATA_DIR", data.path().join("data").to_str().unwrap()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let mut config = load_config(&env, &root, cfg!(windows)).unwrap();
    // load_config rejects port 0; override it for the test.
    config.port = 0;
    config.host = "127.0.0.1".into();
    config.data_dir = data.path().join("data");

    let (ready_tx, ready_rx) = oneshot::channel();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let opts = RunOptions {
        fake_resolver: Some(fake),
        ready: Some(ready_tx),
        ..RunOptions::new(root)
    };
    let server = tokio::spawn(run(config, opts, async {
        let _ = stop_rx.await;
    }));

    let addr = tokio::time::timeout(Duration::from_secs(60), ready_rx)
        .await
        .expect("serwer nie wystartował w 60 s")
        .unwrap();

    let (status, body) = get(addr, "/live/main.m3u8").await;
    assert_eq!(status, 200, "{body}");
    let segments = body.lines().filter(|l| l.ends_with(".ts")).count();
    assert!(segments >= 3, "oczekiwano >= 3 fragmentów:\n{body}");

    let first = body.lines().find(|l| l.ends_with(".ts")).unwrap();
    let (status, _) = get(addr, &format!("/live/{first}")).await;
    assert_eq!(status, 200);

    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("zamykanie trwa za długo")
        .unwrap()
        .unwrap();
}
