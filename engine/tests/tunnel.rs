//! Tunnel manager against a fake cloudflared (`examples/fake_cloudflared.rs`). Never the real
//! cloudflared or the internet.
//!
//! The fake is an example target: a plain `cargo test` builds it, but a filtered run such as
//! `cargo test --test tunnel` does not — run `cargo build --example fake_cloudflared` first
//! (after a clean checkout or a change to the fake).
//!
//! Review Focus 1: two cloudflared processes never run at once. Every fake instance holds an
//! exclusive lock for its whole life and records `overlap` when it finds the lock taken; tests
//! also check with `tasklist` that a replaced or stopped process is gone as soon as the
//! manager's call returns.
//!
//! Windows only, like the job object the processes are started in.

#![cfg(windows)]

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vrcinema::tunnel::{
    NamedTunnel, TunnelConfig, TunnelManager, TunnelMode, TunnelState, TunnelTimings,
};
use serde_json::{Value, json};
use tokio::sync::watch;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};

const TUNNEL_ID: &str = "6ff42ae2-765d-4adf-8112-31c55c1551ef";
const HOSTNAME: &str = "kanal.example.com";

fn fake_program() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    let target = exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target folder");
    let program = target.join("examples").join("fake_cloudflared.exe");
    assert!(
        program.exists(),
        "{} is missing: run `cargo build --example fake_cloudflared` (a plain `cargo test` builds it)",
        program.display()
    );
    program
}

/// A process opened by PID, but only when its executable is `program` (this test's own fake).
/// The open handle pins the process: a PID reused by another test's fake or by any unrelated
/// program is never reported as alive, and never killed.
struct FakeProcess(HANDLE);

impl FakeProcess {
    fn open(pid: u64, program: &Path) -> Option<Self> {
        let pid = u32::try_from(pid).ok()?;
        // SAFETY: plain Win32 call; a null handle means "no such process / no access".
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return None;
        }
        let process = Self(handle);
        let mut name = vec![0u16; 32 * 1024];
        let mut len = name.len() as u32;
        // SAFETY: `name` has room for `len` UTF-16 units; the handle is valid.
        let ok = unsafe {
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, name.as_mut_ptr(), &mut len)
        };
        if ok == 0 {
            return None;
        }
        let image = PathBuf::from(OsString::from_wide(&name[..len as usize]));
        same_path(&image, program).then_some(process)
    }

    fn running(&self) -> bool {
        // SAFETY: valid handle with SYNCHRONIZE access.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_TIMEOUT }
    }

    fn kill(&self) {
        // SAFETY: valid handle with PROCESS_TERMINATE and SYNCHRONIZE access.
        unsafe {
            TerminateProcess(self.0, 1);
            WaitForSingleObject(self.0, 5000);
        }
    }
}

impl Drop for FakeProcess {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
        _ => false,
    }
}

/// A fake cloudflared in its own folder, scripted with one entry per start.
struct Fake {
    dir: tempfile::TempDir,
    program: PathBuf,
}

impl Fake {
    fn new(runs: Value) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let program = dir.path().join("cloudflared.exe");
        // A hard link starts as fast as the original (a fresh copy may be scanned first).
        let fake = fake_program();
        if std::fs::hard_link(&fake, &program).is_err() {
            std::fs::copy(&fake, &program).expect("copy the fake");
        }
        std::fs::write(dir.path().join("fake_cloudflared.json"), runs.to_string())
            .expect("write the script");
        Self { dir, program }
    }

    fn quick(&self, port: u16) -> TunnelConfig {
        TunnelConfig {
            cloudflared: self.program.clone(),
            port,
            mode: TunnelMode::Quick,
        }
    }

    fn named_mode(&self) -> TunnelMode {
        TunnelMode::Named(NamedTunnel {
            hostname: HOSTNAME.into(),
            tunnel_id: TUNNEL_ID.into(),
            credentials_file: self.dir.path().join(format!("{TUNNEL_ID}.json")),
            origin_cert: self.dir.path().join("cert.pem"),
        })
    }

    fn events(&self, kind: &str) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("fake_cloudflared.log"))
            .unwrap_or_default()
            .lines()
            // The last line may still be being written by a fake that is starting.
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["event"] == kind)
            .collect()
    }

    fn starts(&self) -> Vec<Value> {
        self.events("start")
    }

    fn pids(&self) -> Vec<u64> {
        self.starts()
            .iter()
            .map(|s| s["pid"].as_u64().expect("pid"))
            .collect()
    }

    fn args(&self, start: usize) -> Vec<String> {
        self.starts()[start]["args"]
            .as_array()
            .expect("args")
            .iter()
            .map(|a| a.as_str().expect("arg").to_string())
            .collect()
    }

    async fn wait_starts(&self, count: usize, within: Duration) -> Vec<Value> {
        let deadline = Instant::now() + within;
        loop {
            let starts = self.starts();
            if starts.len() >= count {
                return starts;
            }
            assert!(
                Instant::now() < deadline,
                "expected {count} starts, got {}",
                starts.len()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// No instance ever found another one running.
    fn assert_single_instance(&self) {
        let overlapping: Vec<Value> = self
            .starts()
            .into_iter()
            .filter(|s| s["overlap"] != false)
            .collect();
        assert!(
            overlapping.is_empty(),
            "two cloudflared processes ran at once: {overlapping:?}"
        );
    }

    /// `pid` is a running instance of this test's fake (a reused PID is not).
    fn is_alive(&self, pid: u64) -> bool {
        FakeProcess::open(pid, &self.program).is_some_and(|p| p.running())
    }

    fn assert_all_dead(&self) {
        for pid in self.pids() {
            assert!(!self.is_alive(pid), "cloudflared {pid} is still running");
        }
    }
}

impl Drop for Fake {
    /// A failed test must not leave fake processes behind. Only processes whose executable is
    /// this test's fake are killed, through a handle that pins them.
    fn drop(&mut self) {
        for pid in self.pids() {
            if let Some(process) = FakeProcess::open(pid, &self.program)
                && process.running()
            {
                process.kill();
            }
        }
    }
}

fn timings() -> TunnelTimings {
    TunnelTimings {
        address_timeout: Duration::from_secs(10),
        poll_interval: Duration::from_millis(50),
        backoff_initial: Duration::from_millis(100),
        backoff_max: Duration::from_secs(5),
        stable_after: Duration::from_secs(60),
        rate_limit_wait: Duration::from_secs(60),
        health_grace: Duration::from_millis(500),
        reconnect_limit: Duration::from_secs(60),
    }
}

/// The helpers act on a PID only when its executable is this test's fake: a live process of
/// another program (here: this test binary) is neither "alive" as a fake nor ever killed.
#[test]
fn a_pid_of_another_program_is_never_treated_as_the_fake() {
    let fake = Fake::new(json!([{}]));
    let me = u64::from(std::process::id());
    assert!(FakeProcess::open(me, &fake.program).is_none());
    assert!(!fake.is_alive(me));
}

/// cloudflared alive but without a connection for longer than `reconnect_limit`: killed and
/// started again through the usual waits.
#[tokio::test]
async fn a_connection_lost_for_too_long_restarts_cloudflared() {
    let fake = Fake::new(json!([
        { "hostname": "stuck.trycloudflare.com", "ready_down": [[300, 600000]] },
        { "hostname": "fresh.trycloudflare.com" }
    ]));
    let tunnel = manager(TunnelTimings {
        health_grace: Duration::from_millis(300),
        reconnect_limit: Duration::from_millis(1500),
        ..timings()
    });
    let seen = record(tunnel.state());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18190)).await;
    wait_online(&mut rx, "https://fresh.trycloudflare.com").await;

    let starts = fake.starts();
    assert_eq!(starts.len(), 2);
    assert!(
        !fake.is_alive(fake.pids()[0]),
        "the stuck process was not killed"
    );
    // Connection lost at 300 ms, limit 1.5 s, wait 100 ms.
    let gap = start_ms(&starts[1]) - start_ms(&starts[0]);
    assert!(gap >= 1800, "restarted {gap} ms after the start");
    assert_sequence(
        &seen,
        &[
            TunnelState::Online {
                url: "https://stuck.trycloudflare.com".into(),
            },
            TunnelState::Reconnecting,
            TunnelState::Online {
                url: "https://fresh.trycloudflare.com".into(),
            },
        ],
    );
    fake.assert_single_instance();
    tunnel.stop().await;
    fake.assert_all_dead();
}

/// "429" / "Too Many Requests" is about quick tunnels only; a named tunnel printing it keeps
/// running and goes online as usual.
#[tokio::test]
async fn a_named_tunnel_ignores_quick_tunnel_rate_limiting() {
    let fake = Fake::new(json!([{
        "stderr": ["2026-10-07T10:00:00Z WRN edge answered 429 Too Many Requests, retrying"],
        "ready_after_ms": 600,
    }]));
    let tunnel = manager(timings());
    let seen = record(tunnel.state());
    let mut rx = tunnel.state();
    let mut config = fake.quick(18191);
    config.mode = fake.named_mode();
    tunnel.start(config).await;
    wait_online(&mut rx, "https://kanal.example.com").await;

    assert_eq!(fake.starts().len(), 1);
    assert!(
        !seen
            .lock()
            .unwrap()
            .iter()
            .any(|s| matches!(s, TunnelState::Error { .. })),
        "{:?}",
        seen.lock().unwrap()
    );
    tunnel.stop().await;
    fake.assert_all_dead();
}

/// Without a metrics server there is nothing to watch: an address found on stderr stays
/// online (well past the health grace).
async fn assert_stays_online(rx: &watch::Receiver<TunnelState>, url: &str) {
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(*rx.borrow(), TunnelState::Online { url: url.into() });
}

impl Fake {
    /// Paths the manager requested from the fake's metrics server.
    fn requests(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("requests.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// The states in `seen` include `expected` in this order (other states may come between).
fn assert_sequence(seen: &Mutex<Vec<TunnelState>>, expected: &[TunnelState]) {
    let seen = seen.lock().unwrap();
    let mut wanted = expected.iter();
    let mut next = wanted.next();
    for state in seen.iter() {
        if Some(state) == next {
            next = wanted.next();
        }
    }
    assert!(
        next.is_none(),
        "expected {expected:?} in order, saw {seen:?}"
    );
}

fn manager(timings: TunnelTimings) -> TunnelManager {
    TunnelManager::new()
        .with_timings(timings)
        .with_log(Arc::new(|_: &str| {}))
}

async fn wait_for(
    rx: &mut watch::Receiver<TunnelState>,
    what: &str,
    within: Duration,
    accept: impl Fn(&TunnelState) -> bool,
) -> TunnelState {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let state = rx.borrow_and_update().clone();
        if accept(&state) {
            return state;
        }
        match tokio::time::timeout_at(deadline, rx.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("state channel closed while waiting for {what}"),
            Err(_) => panic!("no {what} within {within:?}; last state {state:?}"),
        }
    }
}

async fn wait_online(rx: &mut watch::Receiver<TunnelState>, url: &str) {
    wait_online_within(rx, url, Duration::from_secs(10)).await;
}

async fn wait_online_within(rx: &mut watch::Receiver<TunnelState>, url: &str, within: Duration) {
    let want = TunnelState::Online { url: url.into() };
    wait_for(rx, url, within, |s| *s == want).await;
}

/// Every state the manager publishes (fast transitions may be merged by `watch`).
fn record(mut rx: watch::Receiver<TunnelState>) -> Arc<Mutex<Vec<TunnelState>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let out = seen.clone();
    tokio::spawn(async move {
        loop {
            out.lock().unwrap().push(rx.borrow_and_update().clone());
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
    seen
}

fn metrics_port(args: &[String]) -> u16 {
    let i = args
        .iter()
        .position(|a| a == "--metrics")
        .expect("--metrics");
    let address = &args[i + 1];
    let port = address
        .strip_prefix("127.0.0.1:")
        .unwrap_or_else(|| panic!("metrics on loopback: {address}"));
    port.parse().expect("metrics port")
}

fn start_ms(start: &Value) -> i64 {
    start["ms"].as_i64().expect("ms")
}

/// When the process of `start` exited on its own.
fn exit_ms(fake: &Fake, start: &Value) -> i64 {
    let exit = fake
        .events("exit")
        .into_iter()
        .find(|e| e["pid"] == start["pid"])
        .unwrap_or_else(|| panic!("no exit of {start}"));
    start_ms(&exit)
}

#[tokio::test]
async fn quick_tunnel_goes_online_with_the_address_from_the_metrics_server() {
    let fake = Fake::new(json!([{ "hostname": "brave-lion.trycloudflare.com" }]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    assert_eq!(*rx.borrow(), TunnelState::Off);

    tunnel.start(fake.quick(18101)).await;
    assert_eq!(*rx.borrow(), TunnelState::Connecting);
    wait_online(&mut rx, "https://brave-lion.trycloudflare.com").await;

    let args = fake.args(0);
    let metrics = metrics_port(&args);
    assert_ne!(metrics, 0);
    assert_eq!(
        args,
        [
            "tunnel".to_string(),
            "--no-autoupdate".into(),
            "--metrics".into(),
            format!("127.0.0.1:{metrics}"),
            "--url".into(),
            "http://127.0.0.1:18101".into(),
        ]
    );

    tunnel.stop().await;
    assert_eq!(*rx.borrow(), TunnelState::Off);
    fake.assert_all_dead();
    assert_eq!(fake.starts().len(), 1);
}

#[tokio::test]
async fn quick_tunnel_address_falls_back_to_stderr() {
    let fake = Fake::new(json!([{
        "serve_metrics": false,
        "stderr": [
            "2026-10-07T10:00:00Z INF Requesting new quick Tunnel on trycloudflare.com...",
            "2026-10-07T10:00:00Z ERR Post \"https://api.trycloudflare.com/tunnel\": retrying",
            "2026-10-07T10:00:01Z INF |  https://quiet-river.trycloudflare.com                |",
        ],
    }]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18102)).await;
    wait_online(&mut rx, "https://quiet-river.trycloudflare.com").await;
    assert_stays_online(&rx, "https://quiet-river.trycloudflare.com").await;
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn named_tunnel_is_online_once_ready() {
    let fake = Fake::new(json!([{ "ready_after_ms": 700 }]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    let mut config = fake.quick(18103);
    config.mode = fake.named_mode();

    let started = Instant::now();
    tunnel.start(config).await;
    wait_online(&mut rx, "https://kanal.example.com").await;
    assert!(
        started.elapsed() >= Duration::from_millis(700),
        "online before /ready answered 200: {:?}",
        started.elapsed()
    );

    let args = fake.args(0);
    let metrics = metrics_port(&args);
    let credentials = fake.dir.path().join(format!("{TUNNEL_ID}.json"));
    assert_eq!(
        args,
        [
            "tunnel".to_string(),
            "--no-autoupdate".into(),
            "--metrics".into(),
            format!("127.0.0.1:{metrics}"),
            "run".into(),
            "--credentials-file".into(),
            credentials.display().to_string(),
            "--url".into(),
            "http://127.0.0.1:18103".into(),
            TUNNEL_ID.into(),
        ]
    );
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn named_tunnel_falls_back_to_a_registered_connection_in_stderr() {
    let fake = Fake::new(json!([{
        "serve_metrics": false,
        "stderr": ["2026-10-07T10:00:01Z INF Registered tunnel connection connIndex=0 location=waw01"],
    }]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    let mut config = fake.quick(18104);
    config.mode = fake.named_mode();
    tunnel.start(config).await;
    wait_online(&mut rx, "https://kanal.example.com").await;
    assert_stays_online(&rx, "https://kanal.example.com").await;
    tunnel.stop().await;
    fake.assert_all_dead();
}

/// Connection to Cloudflare lost while cloudflared keeps running (`/ready` 503 longer than the
/// grace): `Reconnecting`, then `Online` with the same address once `/ready` is 200 again —
/// the same process, no restart.
async fn lost_connection_and_back(mode: Option<TunnelMode>, url: &str, port: u16) {
    let fake = Fake::new(json!([{
        "hostname": url.trim_start_matches("https://"),
        "ready_down": [[1000, 2800]],
    }]));
    let tunnel = manager(timings());
    let seen = record(tunnel.state());
    let mut rx = tunnel.state();
    let mut config = fake.quick(port);
    if let Some(mode) = mode {
        config.mode = mode;
    }
    tunnel.start(config).await;
    wait_online(&mut rx, url).await;
    wait_for(&mut rx, "reconnecting", Duration::from_secs(5), |s| {
        *s == TunnelState::Reconnecting
    })
    .await;
    let pid = fake.pids()[0];
    assert!(fake.is_alive(pid), "the process must keep running");
    wait_online(&mut rx, url).await;

    assert_eq!(fake.starts().len(), 1, "no restart");
    assert!(fake.is_alive(pid));
    let online = TunnelState::Online { url: url.into() };
    assert_sequence(
        &seen,
        &[online.clone(), TunnelState::Reconnecting, online.clone()],
    );
    let polls = fake.requests().iter().filter(|p| *p == "/ready").count();
    assert!(polls > 10, "/ready polled {polls} times");
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn named_tunnel_without_connection_is_reconnecting_until_ready_again() {
    let fake_mode = TunnelMode::Named(NamedTunnel {
        hostname: HOSTNAME.into(),
        tunnel_id: TUNNEL_ID.into(),
        credentials_file: PathBuf::from("unused.json"),
        origin_cert: PathBuf::from("cert.pem"),
    });
    lost_connection_and_back(Some(fake_mode), "https://kanal.example.com", 18180).await;
}

#[tokio::test]
async fn quick_tunnel_without_connection_is_reconnecting_until_ready_again() {
    lost_connection_and_back(None, "https://lost-link.trycloudflare.com", 18181).await;
}

/// Fix round 1: the moment the connection was lost is known (the first failed `/ready`, not
/// the end of the grace), for the automatic pause; none while online or stopped.
#[tokio::test]
async fn the_loss_is_dated_from_the_first_failed_ready_check() {
    let fake = Fake::new(json!([{
        "hostname": "dated.trycloudflare.com",
        "ready_down": [[1000, 2800]],
    }]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18192)).await;
    // Starting: lost (never connected) since the start.
    assert!(tunnel.lost_since().is_some());
    wait_online(&mut rx, "https://dated.trycloudflare.com").await;
    assert_eq!(tunnel.lost_since(), None);
    wait_for(&mut rx, "reconnecting", Duration::from_secs(5), |s| {
        *s == TunnelState::Reconnecting
    })
    .await;
    let lost = tunnel.lost_since().expect("the loss is dated");
    // Fix round 2: the same process lives on (its address still leads to the channel).
    assert_eq!(tunnel.runs_ended(), 0);
    // Reconnecting comes `health_grace` (500 ms) after the first failed check.
    assert!(
        lost.elapsed() >= Duration::from_millis(400),
        "dated only {:?} ago",
        lost.elapsed()
    );
    wait_online(&mut rx, "https://dated.trycloudflare.com").await;
    assert_eq!(tunnel.lost_since(), None);
    tunnel.stop().await;
    assert_eq!(tunnel.lost_since(), None);
    fake.assert_all_dead();
}

/// A process that exits while online is a loss from that moment, kept through the restarts.
#[tokio::test]
async fn an_exit_while_online_is_dated_from_the_exit_and_kept_until_online() {
    let fake = Fake::new(json!([
        { "hostname": "first.trycloudflare.com", "exit_after_ms": 600, "exit_code": 1 },
        { "hostname": "", "serve_metrics": false, "exit_after_ms": 100, "exit_code": 1 },
        { "hostname": "second.trycloudflare.com" }
    ]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18193)).await;
    wait_online(&mut rx, "https://first.trycloudflare.com").await;
    wait_for(&mut rx, "not online", Duration::from_secs(5), |s| {
        s.url().is_none()
    })
    .await;
    let lost = tunnel.lost_since().expect("the loss is dated");
    // Fix round 2: the process that learnt the address has ended (its address is dead).
    assert!(tunnel.runs_ended() >= 1);
    fake.wait_starts(2, Duration::from_secs(10)).await;
    assert_eq!(
        tunnel.lost_since(),
        Some(lost),
        "a restart must not move it"
    );
    wait_online(&mut rx, "https://second.trycloudflare.com").await;
    assert_eq!(tunnel.lost_since(), None);
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn a_short_ready_blip_below_the_grace_stays_online() {
    let fake = Fake::new(json!([{
        "hostname": "steady.trycloudflare.com",
        "ready_down": [[800, 1000]],
    }]));
    let tunnel = manager(TunnelTimings {
        health_grace: Duration::from_millis(900),
        ..timings()
    });
    let seen = record(tunnel.state());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18182)).await;
    wait_online(&mut rx, "https://steady.trycloudflare.com").await;
    tokio::time::sleep(Duration::from_millis(2200)).await;

    assert_eq!(
        *rx.borrow(),
        TunnelState::Online {
            url: "https://steady.trycloudflare.com".into()
        }
    );
    assert!(
        !seen.lock().unwrap().contains(&TunnelState::Reconnecting),
        "{:?}",
        seen.lock().unwrap()
    );
    let polls = fake.requests().iter().filter(|p| *p == "/ready").count();
    assert!(polls > 10, "/ready polled {polls} times");
    tunnel.stop().await;
    fake.assert_all_dead();
}

/// Rule: the backoff starts over only after `stable_after` of continuous `Online`. Time spent
/// `Reconnecting` while the process is alive does not count.
#[tokio::test]
async fn time_without_connection_does_not_count_as_stable() {
    let fail = json!({ "exit_after_ms": 0, "exit_code": 1 });
    let fake = Fake::new(json!([
        fail,
        fail,
        // Online at once, connection lost at 300 ms (Reconnecting about 300 ms later), alive
        // for 2.5 s in total: longer than `stable_after`, but only ~0.6 s of it online.
        {
            "hostname": "flaky.trycloudflare.com",
            "ready_down": [[300, 60000]],
            "exit_after_ms": 2500,
            "exit_code": 1
        },
        fail,
        { "hostname": "final-flaky.trycloudflare.com" }
    ]));
    let tunnel = manager(TunnelTimings {
        backoff_initial: Duration::from_millis(300),
        stable_after: Duration::from_millis(1000),
        health_grace: Duration::from_millis(300),
        ..timings()
    });
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18183)).await;
    // About 7 s of scripted waits and runs before the last start.
    wait_online_within(
        &mut rx,
        "https://final-flaky.trycloudflare.com",
        Duration::from_secs(20),
    )
    .await;

    let starts = fake.starts();
    assert_eq!(starts.len(), 5);
    // Waits 300, 600, then 1200 (no reset); a reset would make it 300.
    let after_flaky = start_ms(&starts[3]) - exit_ms(&fake, &starts[2]);
    assert!(
        after_flaky >= 1200,
        "wait after a run that was mostly disconnected: {after_flaky} ms, expected about 1200"
    );
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn no_address_in_time_is_a_failed_start() {
    let fake = Fake::new(json!([{}, { "hostname": "second-try.trycloudflare.com" }]));
    let tunnel = manager(TunnelTimings {
        address_timeout: Duration::from_millis(1500),
        ..timings()
    });
    let seen = record(tunnel.state());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18105)).await;
    wait_online(&mut rx, "https://second-try.trycloudflare.com").await;

    let starts = fake.starts();
    assert_eq!(starts.len(), 2);
    let gap = start_ms(&starts[1]) - start_ms(&starts[0]);
    // Timeout 1500 + wait 100, minus slack: the timeout runs from the spawn, but the first fake
    // stamps its start only after its own startup (a few hundred ms under load). Without the
    // timeout the gap would be about the 100 ms wait.
    assert!(
        gap >= 1200,
        "restarted after {gap} ms (timeout 1500 + wait 100)"
    );
    assert!(
        !fake.is_alive(fake.pids()[0]),
        "the silent process was not killed"
    );
    assert!(
        seen.lock().unwrap().contains(&TunnelState::Reconnecting),
        "{:?}",
        seen.lock().unwrap()
    );
    fake.assert_single_instance();
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn an_exited_process_is_restarted_with_doubling_waits() {
    let fail = json!({ "exit_after_ms": 0, "exit_code": 1 });
    let fake = Fake::new(json!([
        fail,
        fail,
        fail,
        fail,
        { "hostname": "back-again.trycloudflare.com" }
    ]));
    let tunnel = manager(TunnelTimings {
        backoff_initial: Duration::from_millis(150),
        ..timings()
    });
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18106)).await;
    wait_for(&mut rx, "reconnecting", Duration::from_secs(5), |s| {
        *s == TunnelState::Reconnecting
    })
    .await;
    wait_online(&mut rx, "https://back-again.trycloudflare.com").await;

    let starts = fake.starts();
    assert_eq!(starts.len(), 5);
    // From each exit to the next start (the fake logs its exit just before it).
    let gaps: Vec<i64> = starts
        .windows(2)
        .map(|w| start_ms(&w[1]) - exit_ms(&fake, &w[0]))
        .collect();
    for (gap, wait) in gaps.iter().zip([150, 300, 600, 1200]) {
        assert!(
            *gap >= wait && *gap < wait + 900,
            "gaps {gaps:?}, expected about 150, 300, 600, 1200 ms"
        );
    }
    fake.assert_single_instance();
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn waits_start_over_after_a_long_enough_time_online() {
    let fail = json!({ "exit_after_ms": 0, "exit_code": 1 });
    let fake = Fake::new(json!([
        fail,
        fail,
        // Online for about a second, longer than `stable_after`.
        { "hostname": "stable.trycloudflare.com", "exit_after_ms": 1500, "exit_code": 1 },
        fail,
        { "hostname": "final.trycloudflare.com" }
    ]));
    let tunnel = manager(TunnelTimings {
        backoff_initial: Duration::from_millis(300),
        stable_after: Duration::from_millis(400),
        ..timings()
    });
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18107)).await;
    wait_online(&mut rx, "https://final.trycloudflare.com").await;

    let starts = fake.starts();
    assert_eq!(starts.len(), 5);
    // Without the reset the wait would be 1200 ms (300, 600, 1200).
    let after_stable = start_ms(&starts[3]) - exit_ms(&fake, &starts[2]);
    assert!(
        (300..1000).contains(&after_stable),
        "wait after a stable run: {after_stable} ms, expected about 300"
    );
    let next = start_ms(&starts[4]) - exit_ms(&fake, &starts[3]);
    assert!(next >= 600, "the wait doubles again: {next} ms");
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn a_rate_limited_quick_tunnel_is_an_error_with_a_long_wait() {
    let refused = "2026-10-07T10:00:00Z ERR failed to request quick Tunnel: status code 429 Too Many Requests";
    let fake = Fake::new(json!([
        // Keeps running after the refusal: the manager must kill it.
        { "stderr": [refused] },
        // Exits right after the refusal, like the real cloudflared.
        { "stderr": [refused], "exit_after_ms": 0, "exit_code": 1 },
        { "hostname": "allowed.trycloudflare.com" }
    ]));
    let tunnel = manager(TunnelTimings {
        rate_limit_wait: Duration::from_millis(1500),
        ..timings()
    });
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18108)).await;

    let error = wait_for(&mut rx, "error", Duration::from_secs(5), |s| {
        matches!(s, TunnelState::Error { .. })
    })
    .await;
    let TunnelState::Error { message } = error else {
        unreachable!()
    };
    assert!(message.contains("Cloudflare"), "{message}");
    assert!(message.contains("zbyt wiele"), "{message}");
    assert!(
        !fake.is_alive(fake.pids()[0]),
        "the refused process was not killed"
    );

    wait_online(&mut rx, "https://allowed.trycloudflare.com").await;
    let starts = fake.starts();
    assert_eq!(starts.len(), 3);
    for w in starts.windows(2) {
        let gap = start_ms(&w[1]) - start_ms(&w[0]);
        assert!(gap >= 1500, "restarted {gap} ms after a 429");
    }
    fake.assert_single_instance();
    tunnel.stop().await;
    fake.assert_all_dead();
}

/// Review Focus 1.
#[tokio::test]
async fn restart_on_a_new_port_replaces_the_process() {
    let fake = Fake::new(json!([
        { "hostname": "first-port.trycloudflare.com" },
        { "hostname": "second-port.trycloudflare.com" }
    ]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18110)).await;
    wait_online(&mut rx, "https://first-port.trycloudflare.com").await;
    let old = fake.pids()[0];

    tunnel.restart(18111).await;
    assert!(
        !fake.is_alive(old),
        "the old process must be gone when restart returns"
    );
    assert_eq!(*rx.borrow(), TunnelState::Connecting);
    wait_online(&mut rx, "https://second-port.trycloudflare.com").await;

    assert_eq!(fake.starts().len(), 2);
    let args = fake.args(1);
    assert_eq!(
        args.last().map(String::as_str),
        Some("http://127.0.0.1:18111")
    );
    fake.assert_single_instance();
    tunnel.stop().await;
    fake.assert_all_dead();
}

/// Review Focus 1: restart and stop while cloudflared is still starting.
#[tokio::test]
async fn restart_and_stop_during_a_start_in_progress() {
    let slow = json!({ "delay_ms": 5000, "hostname": "too-late.trycloudflare.com" });
    let fake = Fake::new(json!([slow, slow, { "hostname": "third.trycloudflare.com" }]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();

    tunnel.start(fake.quick(18120)).await;
    fake.wait_starts(1, Duration::from_secs(5)).await;
    tunnel.restart(18121).await;
    assert!(
        !fake.is_alive(fake.pids()[0]),
        "restart left the old process"
    );

    fake.wait_starts(2, Duration::from_secs(5)).await;
    tunnel.stop().await;
    assert!(!fake.is_alive(fake.pids()[1]), "stop left the process");
    assert_eq!(*rx.borrow(), TunnelState::Off);

    tunnel.start(fake.quick(18122)).await;
    wait_online(&mut rx, "https://third.trycloudflare.com").await;
    assert_eq!(
        fake.args(2).last().map(String::as_str),
        Some("http://127.0.0.1:18122")
    );
    fake.assert_single_instance();
    tunnel.stop().await;
    fake.assert_all_dead();
}

/// Review Focus 1: overlapping calls are serialised and leave no process behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_calls_never_run_two_processes() {
    let fake = Fake::new(json!([{ "delay_ms": 300, "hostname": "busy.trycloudflare.com" }]));
    let tunnel = Arc::new(manager(timings()));
    tunnel.start(fake.quick(18130)).await;

    let mut calls = Vec::new();
    for i in 0..6u16 {
        let tunnel = tunnel.clone();
        let config = fake.quick(18131 + i);
        calls.push(tokio::spawn(async move {
            match i % 3 {
                0 => tunnel.restart(18131 + i).await,
                1 => tunnel.start(config).await,
                _ => tunnel.set_mode(TunnelMode::Quick).await,
            }
        }));
    }
    for call in calls {
        call.await.unwrap();
    }
    let mut rx = tunnel.state();
    wait_online(&mut rx, "https://busy.trycloudflare.com").await;
    let pids = fake.pids();
    let alive: Vec<u64> = pids.iter().copied().filter(|p| fake.is_alive(*p)).collect();
    assert_eq!(alive.len(), 1, "running: {alive:?} of {pids:?}");

    tunnel.stop().await;
    fake.assert_single_instance();
    fake.assert_all_dead();
}

#[tokio::test]
async fn dropping_the_manager_kills_cloudflared() {
    let fake = Fake::new(json!([{ "hostname": "dropped.trycloudflare.com" }]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18140)).await;
    wait_online(&mut rx, "https://dropped.trycloudflare.com").await;
    let pid = fake.pids()[0];

    drop(tunnel);
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.is_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "cloudflared outlived the manager"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn set_mode_switches_between_quick_and_named() {
    let fake = Fake::new(json!([
        { "hostname": "quick-one.trycloudflare.com" },
        { "ready_after_ms": 0 },
        { "hostname": "quick-two.trycloudflare.com" }
    ]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18150)).await;
    wait_online(&mut rx, "https://quick-one.trycloudflare.com").await;

    tunnel.set_mode(fake.named_mode()).await;
    wait_online(&mut rx, "https://kanal.example.com").await;
    let named = fake.args(1);
    assert!(named.contains(&"run".to_string()), "{named:?}");
    assert_eq!(named.last().map(String::as_str), Some(TUNNEL_ID));

    tunnel.set_mode(TunnelMode::Quick).await;
    wait_online(&mut rx, "https://quick-two.trycloudflare.com").await;
    assert!(!fake.args(2).contains(&"run".to_string()));
    fake.assert_single_instance();
    tunnel.stop().await;
    fake.assert_all_dead();
}

#[tokio::test]
async fn restart_and_set_mode_keep_a_stopped_tunnel_stopped() {
    let fake = Fake::new(json!([{ "hostname": "once.trycloudflare.com" }]));
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    tunnel.start(fake.quick(18160)).await;
    wait_online(&mut rx, "https://once.trycloudflare.com").await;
    tunnel.stop().await;

    tunnel.restart(18161).await;
    tunnel.set_mode(fake.named_mode()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(*rx.borrow(), TunnelState::Off);
    assert_eq!(fake.starts().len(), 1);

    // Never started at all: nothing to restart either.
    let idle = manager(timings());
    idle.restart(18162).await;
    assert_eq!(*idle.state().borrow(), TunnelState::Off);
}

#[tokio::test]
async fn a_missing_program_is_an_error_and_stop_ends_the_retries() {
    let dir = tempfile::tempdir().unwrap();
    let tunnel = manager(timings());
    let mut rx = tunnel.state();
    tunnel
        .start(TunnelConfig {
            cloudflared: dir.path().join("cloudflared.exe"),
            port: 18170,
            mode: TunnelMode::Quick,
        })
        .await;
    let error = wait_for(&mut rx, "error", Duration::from_secs(5), |s| {
        matches!(s, TunnelState::Error { .. })
    })
    .await;
    let TunnelState::Error { message } = error else {
        unreachable!()
    };
    assert!(message.contains("cloudflared"), "{message}");
    tunnel.stop().await;
    assert_eq!(*rx.borrow(), TunnelState::Off);
}
