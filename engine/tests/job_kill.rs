//! Review Focus 3: a child started through `process::spawn` must die together with its parent,
//! even when the parent exits abruptly (`std::process::exit`, no destructors, no explicit kill).
//!
//! The test re-invokes this very test binary as the "parent" (selected by an environment
//! variable). The parent starts a long-running process, prints its PID and exits. The outer
//! test then checks that the grandchild is gone.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use vrcinema::process;

const HELPER_ENV: &str = "VRCINEMA_JOB_HELPER";
const PID_PREFIX: &str = "CHILD_PID=";

#[cfg(windows)]
fn long_running() -> (&'static str, Vec<&'static str>) {
    ("ping", vec!["-n", "60", "127.0.0.1"])
}

#[cfg(unix)]
fn long_running() -> (&'static str, Vec<&'static str>) {
    ("sleep", vec!["60"])
}

#[cfg(windows)]
fn is_alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Entry point of the helper "parent" process. A no-op in a normal test run.
#[test]
fn job_helper_parent() {
    if std::env::var_os(HELPER_ENV).is_none() {
        return;
    }
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let child = runtime.block_on(async {
        let (program, args) = long_running();
        let mut cmd = process::command(Path::new(program));
        cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
        process::spawn(&mut cmd).expect("spawn child")
    });
    println!("{PID_PREFIX}{}", child.id().expect("child pid"));
    // Give the controlling test time to observe that the child is alive.
    std::thread::sleep(Duration::from_millis(1500));
    // Abrupt exit: no destructors, so `kill_on_drop` cannot help; only the OS-level job can.
    std::process::exit(0);
}

#[test]
fn child_dies_when_parent_exits_abruptly() {
    let exe = std::env::current_exe().expect("current_exe");
    let mut parent = Command::new(exe)
        .args([
            "job_helper_parent",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HELPER_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start helper parent");

    let mut lines = BufReader::new(parent.stdout.take().expect("stdout")).lines();
    let pid: u32 = loop {
        let line = lines
            .next()
            .expect("helper ended without printing the child PID")
            .expect("read line");
        if let Some((_, rest)) = line.split_once(PID_PREFIX) {
            break rest.trim().parse().expect("pid number");
        }
    };

    assert!(
        is_alive(pid),
        "child should be running while the parent lives"
    );

    let status = parent.wait().expect("wait for helper parent");
    assert!(status.success(), "helper parent failed: {status:?}");

    let deadline = Instant::now() + Duration::from_secs(5);
    while is_alive(pid) {
        if Instant::now() > deadline {
            // Do not leave the 60 s process behind if the assertion fails.
            #[cfg(windows)]
            let _ = Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
            #[cfg(unix)]
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
            panic!("child {pid} survived the exit of its parent");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
