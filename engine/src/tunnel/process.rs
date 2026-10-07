//! One cloudflared process: its command line, reading its stderr, and asking its metrics
//! server for the public address.

use std::ffi::OsString;
use std::io;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc;

use super::{TunnelConfig, TunnelMode};

const QUICK_SUFFIX: &str = ".trycloudflare.com";
/// Longest stderr line kept; the rest of a longer line is dropped (bounded memory).
const MAX_LINE: usize = 64 * 1024;

/// A running cloudflared. Dropping it kills the process (`kill_on_drop`) without waiting;
/// [`Cloudflared::kill`] also waits for the exit.
pub struct Cloudflared {
    pub child: Child,
    /// Lines cloudflared writes to stderr (its log), read continuously so the pipe never fills.
    pub stderr: mpsc::Receiver<String>,
    pub metrics_port: u16,
}

impl Cloudflared {
    /// Starts cloudflared through [`crate::process::spawn`] (Windows job object).
    pub fn spawn(config: &TunnelConfig) -> io::Result<Self> {
        let metrics_port = free_port()?;
        let mut command = crate::process::command(&config.cloudflared);
        command
            .args(args(config, metrics_port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = crate::process::spawn(&mut command)?;
        let (tx, stderr) = mpsc::channel(64);
        if let Some(pipe) = child.stderr.take() {
            tokio::spawn(forward_lines(pipe, tx));
        }
        Ok(Self {
            child,
            stderr,
            metrics_port,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Kills the process and waits until it has exited.
    pub async fn kill(&mut self) {
        // Fails only when the process has already exited; the wait below reaps it either way.
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }

    /// The stderr lines still in transit after the process exited (bounded by `within`).
    pub async fn remaining_lines(&mut self, within: Duration) -> Vec<String> {
        let mut lines = Vec::new();
        let deadline = tokio::time::Instant::now() + within;
        while let Ok(Some(line)) = tokio::time::timeout_at(deadline, self.stderr.recv()).await {
            lines.push(line);
        }
        lines
    }
}

/// Sends every line `pipe` delivers to `tx` (at most [`MAX_LINE`] bytes of each).
pub(super) async fn forward_lines<R: AsyncRead + Unpin>(pipe: R, tx: mpsc::Sender<String>) {
    let mut reader = BufReader::new(pipe);
    let mut line = Vec::new();
    loop {
        let chunk = match reader.fill_buf().await {
            Ok([]) | Err(_) => break,
            Ok(chunk) => chunk,
        };
        let newline = chunk.iter().position(|b| *b == b'\n');
        let part = &chunk[..newline.unwrap_or(chunk.len())];
        // Keep at most MAX_LINE bytes of a line; the rest is read and dropped.
        let room = MAX_LINE.saturating_sub(line.len());
        line.extend_from_slice(&part[..part.len().min(room)]);
        let used = newline.map_or(chunk.len(), |i| i + 1);
        reader.consume(used);
        if newline.is_some() {
            send_line(&tx, &mut line).await;
        }
    }
    if !line.is_empty() {
        send_line(&tx, &mut line).await;
    }
}

async fn send_line(tx: &mpsc::Sender<String>, line: &mut Vec<u8>) {
    let text = String::from_utf8_lossy(line).trim_end().to_string();
    line.clear();
    // Nobody listens any more: keep draining until the process is gone.
    let _ = tx.send(text).await;
}

/// A port that is free on 127.0.0.1 right now (for the metrics server).
fn free_port() -> io::Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Command-line arguments for `config`, with the metrics server on `127.0.0.1:<metrics_port>`.
///
/// Quick: `tunnel --no-autoupdate --metrics … --url http://127.0.0.1:<port>`;
/// named: `tunnel --no-autoupdate --metrics … run --credentials-file <file> --url … <id>`.
pub fn args(config: &TunnelConfig, metrics_port: u16) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "tunnel".into(),
        "--no-autoupdate".into(),
        "--metrics".into(),
        format!("127.0.0.1:{metrics_port}").into(),
    ];
    let local = format!("http://127.0.0.1:{}", config.port);
    match &config.mode {
        TunnelMode::Quick => {
            args.extend(["--url".into(), local.into()]);
        }
        TunnelMode::Named(named) => {
            args.extend([
                "run".into(),
                "--credentials-file".into(),
                named.credentials_file.clone().into_os_string(),
                "--url".into(),
                local.into(),
                named.tunnel_id.clone().into(),
            ]);
        }
    }
    args
}

/// What cloudflared's metrics server said, or that it could not be reached at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Metrics<T> {
    Answered(T),
    Unreachable,
}

async fn get(client: &reqwest::Client, metrics_port: u16, path: &str) -> Option<reqwest::Response> {
    client
        .get(format!("http://127.0.0.1:{metrics_port}{path}"))
        .send()
        .await
        .ok()
}

/// `/quicktunnel`: the quick tunnel's address (`None` while it has none yet).
pub async fn quick_address(client: &reqwest::Client, metrics_port: u16) -> Metrics<Option<String>> {
    let Some(response) = get(client, metrics_port, "/quicktunnel").await else {
        return Metrics::Unreachable;
    };
    if !response.status().is_success() {
        return Metrics::Answered(None);
    }
    let body: Option<serde_json::Value> = response.json().await.ok();
    let host = body
        .as_ref()
        .and_then(|b| b.get("hostname"))
        .and_then(|h| h.as_str())
        .map(str::trim)
        .unwrap_or("");
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    Metrics::Answered(valid.then(|| format!("https://{host}")))
}

/// `/ready`: 200 while cloudflared has at least one connection to Cloudflare, 503 without.
pub async fn ready(client: &reqwest::Client, metrics_port: u16) -> Metrics<bool> {
    match get(client, metrics_port, "/ready").await {
        Some(response) => Metrics::Answered(response.status() == reqwest::StatusCode::OK),
        None => Metrics::Unreachable,
    }
}

/// The public address a stderr line reveals, if any.
pub fn address_in_line(line: &str, mode: &TunnelMode) -> Option<String> {
    match mode {
        TunnelMode::Quick => quick_url_in(line),
        TunnelMode::Named(named) => {
            is_connection_registered(line).then(|| named_url(&named.hostname))
        }
    }
}

/// Public address of a named tunnel.
pub fn named_url(hostname: &str) -> String {
    format!("https://{hostname}")
}

/// The quick-tunnel address printed in a stderr line (`https://<name>.trycloudflare.com`).
pub fn quick_url_in(line: &str) -> Option<String> {
    let mut rest = line;
    while let Some(start) = rest.find("https://") {
        rest = &rest[start + "https://".len()..];
        let name_len = rest
            .bytes()
            .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            .count();
        let name = &rest[..name_len];
        let Some(after) = rest[name_len..].strip_prefix(QUICK_SUFFIX) else {
            continue;
        };
        let host_ends = !after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        // `api.trycloudflare.com` is the service that hands out quick tunnels (error messages).
        if !name.is_empty() && name != "api" && host_ends {
            return Some(format!("https://{name}{QUICK_SUFFIX}"));
        }
    }
    None
}

/// Cloudflare refused a quick tunnel because of too many requests (HTTP 429).
pub fn is_rate_limited(line: &str) -> bool {
    if line.to_ascii_lowercase().contains("too many requests") {
        return true;
    }
    // "429" as a word of its own, not part of a number or a timestamp.
    let bytes = line.as_bytes();
    line.match_indices("429").any(|(i, _)| {
        let before = i.checked_sub(1).map(|j| bytes[j]);
        let after = bytes.get(i + 3).copied();
        let word = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_alphanumeric());
        !word(before) && !word(after)
    })
}

/// A named tunnel registered a connection with Cloudflare's edge.
pub fn is_connection_registered(line: &str) -> bool {
    line.contains("Registered tunnel connection")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::NamedTunnel;
    use std::path::PathBuf;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.into_string().unwrap()).collect()
    }

    #[test]
    fn quick_tunnel_arguments() {
        let config = TunnelConfig {
            cloudflared: PathBuf::from("cloudflared.exe"),
            port: 8123,
            mode: TunnelMode::Quick,
        };
        assert_eq!(
            strings(args(&config, 40001)),
            [
                "tunnel",
                "--no-autoupdate",
                "--metrics",
                "127.0.0.1:40001",
                "--url",
                "http://127.0.0.1:8123"
            ]
        );
    }

    #[test]
    fn named_tunnel_arguments() {
        let config = TunnelConfig {
            cloudflared: PathBuf::from("cloudflared.exe"),
            port: 8124,
            mode: TunnelMode::Named(NamedTunnel {
                hostname: "kanal.example.com".into(),
                tunnel_id: "6ff42ae2-765d-4adf-8112-31c55c1551ef".into(),
                credentials_file: PathBuf::from(r"C:\home\tunnel\6ff42ae2.json"),
                origin_cert: PathBuf::from(r"C:\home\tunnel\cert.pem"),
            }),
        };
        assert_eq!(
            strings(args(&config, 40002)),
            [
                "tunnel",
                "--no-autoupdate",
                "--metrics",
                "127.0.0.1:40002",
                "run",
                "--credentials-file",
                r"C:\home\tunnel\6ff42ae2.json",
                "--url",
                "http://127.0.0.1:8124",
                "6ff42ae2-765d-4adf-8112-31c55c1551ef"
            ]
        );
    }

    #[test]
    fn finds_the_quick_tunnel_address_in_the_banner() {
        let line =
            "2026-10-07T10:00:00Z INF |  https://brave-lion-sea-moon.trycloudflare.com          |";
        assert_eq!(
            quick_url_in(line).as_deref(),
            Some("https://brave-lion-sea-moon.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_the_quick_tunnel_api_and_other_hosts() {
        let api = r#"ERR failed to request quick Tunnel: Post "https://api.trycloudflare.com/tunnel": dial tcp"#;
        assert_eq!(quick_url_in(api), None);
        assert_eq!(
            quick_url_in("INF Requesting new quick Tunnel on trycloudflare.com..."),
            None
        );
        assert_eq!(quick_url_in("see https://example.com/x"), None);
        assert_eq!(quick_url_in("https://.trycloudflare.com"), None);
        assert_eq!(
            quick_url_in("x https://a-b.trycloudflare.community"),
            None,
            "the host must end at .trycloudflare.com"
        );
    }

    #[test]
    fn recognises_rate_limiting() {
        assert!(is_rate_limited(
            "ERR failed to unmarshal quick Tunnel: status code 429 Too Many Requests"
        ));
        assert!(is_rate_limited("error: too many requests"));
        assert!(is_rate_limited("http status 429"));
        assert!(!is_rate_limited(
            "INF Starting metrics server on 127.0.0.1:4290/metrics"
        ));
        assert!(!is_rate_limited(
            "2026-10-07T10:00:00.429Z INF Version 2026.9.0"
        ));
    }

    #[tokio::test]
    async fn an_overlong_stderr_line_is_truncated() {
        let mut input = vec![b'x'; 200 * 1024];
        input.extend_from_slice(b"\r\nnext line\nlast without newline");
        let (tx, mut rx) = mpsc::channel(8);
        forward_lines(&input[..], tx).await;
        let first = rx.recv().await.unwrap();
        assert_eq!(first.len(), MAX_LINE);
        assert!(first.bytes().all(|b| b == b'x'));
        assert_eq!(rx.recv().await.as_deref(), Some("next line"));
        assert_eq!(rx.recv().await.as_deref(), Some("last without newline"));
        assert_eq!(rx.recv().await, None);
    }

    #[test]
    fn recognises_a_registered_connection() {
        assert!(is_connection_registered(
            "2026-10-07T10:00:01Z INF Registered tunnel connection connIndex=0 connection=abc location=waw01"
        ));
        assert!(!is_connection_registered(
            "INF Starting tunnel tunnelID=abc"
        ));
    }
}
