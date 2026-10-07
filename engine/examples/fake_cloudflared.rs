//! Fake `cloudflared` for the tunnel manager tests (`tests/tunnel.rs`). Not part of the product.
//!
//! Tests copy this program into a temporary folder as `cloudflared.exe`, next to a script
//! `fake_cloudflared.json`: a JSON array with one entry per start (the last entry repeats),
//! so tests running in parallel never share settings. Fields of an entry (all optional):
//!
//! - `delay_ms`: wait before anything else (a start still in progress);
//! - `serve_metrics` (default true): serve `/quicktunnel` and `/ready` on the `--metrics` address;
//! - `hostname`: what `/quicktunnel` reports (`""` when absent);
//! - `ready_after_ms` (default 0): `/ready` answers 200 this long after the start, 503 before;
//! - `ready_down`: `[[from_ms, to_ms], …]` after the start during which `/ready` answers 503
//!   again (connection to Cloudflare lost while the process keeps running);
//! - `stderr`: lines printed to stderr;
//! - `exit_after_ms` + `exit_code`: exit on its own (runs until killed when absent).
//!
//! Every start appends `{"event":"start","pid","args","ms","overlap"}` to `fake_cloudflared.log`
//! in the same folder (and `{"event":"exit",…}` before exiting on its own). `overlap` is true
//! when another instance from that folder was still running: each instance holds an exclusive
//! lock on `instance.lock` for its whole life (Windows). The path of every request to the
//! metrics server is appended to `requests.log`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct Run {
    delay_ms: u64,
    serve_metrics: bool,
    hostname: Option<String>,
    ready_after_ms: u64,
    ready_down: Vec<[u64; 2]>,
    stderr: Vec<String>,
    exit_after_ms: Option<u64>,
    exit_code: i32,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            delay_ms: 0,
            serve_metrics: true,
            hostname: None,
            ready_after_ms: 0,
            ready_down: Vec::new(),
            stderr: Vec::new(),
            exit_after_ms: None,
            exit_code: 0,
        }
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn append(log: &Path, value: serde_json::Value) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .expect("open fake_cloudflared.log");
    // One write per entry: the manager may kill this process at any moment, and a partial
    // line would corrupt the next one.
    file.write_all(format!("{value}\n").as_bytes())
        .expect("write fake_cloudflared.log");
}

/// The exclusive instance lock, or `None` when another instance holds it.
#[cfg(windows)]
fn lock(dir: &Path) -> Option<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .share_mode(0)
        .open(dir.join("instance.lock"))
        .ok()
}

#[cfg(not(windows))]
fn lock(dir: &Path) -> Option<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))
        .ok()
}

fn serve(listener: TcpListener, run: Run, started: Instant, requests: PathBuf) {
    for stream in listener.incoming().flatten() {
        let run = run.clone();
        let requests = requests.clone();
        std::thread::spawn(move || answer(stream, &run, started, &requests));
    }
}

fn is_ready(run: &Run, started: Instant) -> bool {
    let ms = started.elapsed().as_millis() as u64;
    ms >= run.ready_after_ms
        && !run
            .ready_down
            .iter()
            .any(|[from, to]| (*from..*to).contains(&ms))
}

fn answer(mut stream: TcpStream, run: &Run, started: Instant, requests: &Path) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request.split_whitespace().nth(1).unwrap_or("");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(requests) {
        let _ = file.write_all(format!("{path}\n").as_bytes());
    }
    let (status, body) = match path {
        "/quicktunnel" => (
            "200 OK",
            serde_json::json!({ "hostname": run.hostname.clone().unwrap_or_default() }).to_string(),
        ),
        "/ready" => {
            if is_ready(run, started) {
                (
                    "200 OK",
                    r#"{"status":200,"readyConnections":1}"#.to_string(),
                )
            } else {
                (
                    "503 Service Unavailable",
                    r#"{"status":503,"readyConnections":0}"#.to_string(),
                )
            }
        }
        _ => ("404 Not Found", String::new()),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn main() {
    let started = Instant::now();
    let exe = std::env::current_exe().expect("current_exe");
    let dir = exe.parent().expect("folder of the fake").to_path_buf();
    let log = dir.join("fake_cloudflared.log");
    let args: Vec<String> = std::env::args().skip(1).collect();

    let instance = lock(&dir);
    let previous_starts = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(r#""event":"start""#))
        .count();
    append(
        &log,
        serde_json::json!({
            "event": "start",
            "pid": std::process::id(),
            "args": args,
            "ms": now_ms(),
            "overlap": instance.is_none(),
        }),
    );

    let script: Vec<Run> = std::fs::read_to_string(dir.join("fake_cloudflared.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let run = script
        .get(previous_starts)
        .or(script.last())
        .cloned()
        .unwrap_or_default();

    std::thread::sleep(Duration::from_millis(run.delay_ms));

    if run.serve_metrics {
        let metrics = args
            .iter()
            .position(|a| a == "--metrics")
            .and_then(|i| args.get(i + 1))
            .expect("--metrics <address>");
        let listener = TcpListener::bind(metrics).expect("bind the metrics address");
        let served = run.clone();
        let requests = dir.join("requests.log");
        std::thread::spawn(move || serve(listener, served, started, requests));
    }

    for line in &run.stderr {
        eprintln!("{line}");
    }

    match run.exit_after_ms {
        Some(ms) => {
            std::thread::sleep(Duration::from_millis(ms));
            append(
                &log,
                serde_json::json!({ "event": "exit", "pid": std::process::id(), "ms": now_ms() }),
            );
            // The lock stays held until the OS ends the process (no destructors run).
            std::process::exit(run.exit_code);
        }
        None => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
    }
}
