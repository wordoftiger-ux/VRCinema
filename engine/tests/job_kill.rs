//! Review Focus 3: a child started through `process::spawn` must die together with its parent,
//! even when the parent exits abruptly (`std::process::exit`, no destructors, no explicit kill).
//!
//! The test re-invokes this very test binary as the "parent" (selected by an environment
//! variable). The parent starts a long-running process, prints its PID and exits. The outer
//! test then checks that the grandchild is gone.
//!
//! Windows only: the guarantee comes from the Windows Job Object. On Linux the children rely on
//! `kill_on_drop` (no PDEATHSIG, which is tied to the spawning thread and unsafe with tokio).

#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use vrcinema::process;

const HELPER_ENV: &str = "VRCINEMA_JOB_HELPER";
const PID_PREFIX: &str = "CHILD_PID=";

fn long_running() -> (&'static str, Vec<&'static str>) {
    ("ping", vec!["-n", "60", "127.0.0.1"])
}

fn is_alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
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

/// Helper "parent" that puts itself into the job first (like `main` does) and then starts a
/// child WITHOUT `process::spawn`. This stands for a grandchild that a child (the PyInstaller
/// yt-dlp bootloader) starts immediately, before any per-child assignment could catch it:
/// only membership inherited from this process keeps it in the job.
#[test]
fn job_helper_contained_parent() {
    if std::env::var_os(CONTAINED_HELPER_ENV).is_none() {
        return;
    }
    process::contain_this_process().expect("join the job object");
    let (program, args) = long_running();
    let child = Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");
    println!("{PID_PREFIX}{}", child.id());
    std::thread::sleep(Duration::from_millis(1500));
    std::process::exit(0);
}

const CONTAINED_HELPER_ENV: &str = "VRCINEMA_JOB_CONTAINED_HELPER";

#[test]
fn child_dies_when_parent_exits_abruptly() {
    assert_child_dies_with_parent("job_helper_parent", HELPER_ENV);
}

#[test]
fn an_unassigned_grandchild_dies_when_the_contained_parent_exits_abruptly() {
    assert_child_dies_with_parent("job_helper_contained_parent", CONTAINED_HELPER_ENV);
}

fn assert_child_dies_with_parent(helper: &str, env: &str) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut parent = Command::new(exe)
        .args([helper, "--exact", "--nocapture", "--test-threads=1"])
        .env(env, "1")
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
            let _ = Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
            panic!("child {pid} survived the exit of its parent");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ---------- The login's cloudflared and the browser it opens ----------

/// Folder of the fake cloudflared (as `cloudflared.exe`) for the login helper.
const LOGIN_HELPER_ENV: &str = "VRCINEMA_JOB_LOGIN_HELPER";

/// Helper "app": contains itself in the program-wide job like `main` does, then starts the fake
/// `cloudflared tunnel login` the way the wizard does ([`process::spawn_in_own_job`]); the fake
/// starts an ordinary child of its own (the browser). Reports where both are, then exits
/// abruptly: the OS closes every job handle of this process.
#[test]
fn job_helper_contained_login() {
    let Some(dir) = std::env::var_os(LOGIN_HELPER_ENV) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    process::contain_this_process().expect("join the job object");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (login, job) = runtime.block_on(async {
        let mut cmd = process::command(&dir.join("cloudflared.exe"));
        cmd.args(["tunnel", "--no-autoupdate", "login"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        process::spawn_in_own_job(&mut cmd).expect("start the login")
    });
    let login_pid = login.id().expect("login pid");
    println!(
        "WARNING={}",
        job.warning().map(ToString::to_string).unwrap_or_default()
    );
    println!(
        "LOGIN_IN_PROGRAM_JOB={:?}",
        process::in_program_job(login_pid)
    );
    let pid_file = dir.join("child.pid");
    let deadline = Instant::now() + Duration::from_secs(10);
    let child_pid: u32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|t| t.trim().parse().ok())
        {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "the browser stand-in did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    println!(
        "CHILD_IN_PROGRAM_JOB={:?}",
        process::in_program_job(child_pid)
    );
    println!("READY");
    std::thread::sleep(Duration::from_millis(300));
    // Abrupt exit: the job handles close with the process, nothing else.
    std::process::exit(0);
}

/// `path` is held open exclusively by a live fake (a lock a dead process cannot hold).
fn held(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    path.exists()
        && std::fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(path)
            .is_err()
}

fn fake_program() -> std::path::PathBuf {
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

/// Waits (up to `secs`) until `f` holds.
fn wait_until(secs: u64, f: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !f() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Review item: in the real program (contained in the program-wide job), the login's
/// cloudflared is outside that job, the browser it opens is in no job of ours, and when the
/// program ends abruptly the login dies while the browser lives on.
#[test]
fn the_login_dies_with_the_contained_app_but_the_browser_it_opens_lives_on() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("tools");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(fake_program(), dir.join("cloudflared.exe")).expect("copy the fake");
    let script = serde_json::json!({"login": {
        "wait_forever": true,
        "spawn_child_ms": 6000,
        "cert_dir": tmp.path().join("user").join(".cloudflared"),
    }});
    std::fs::write(
        dir.join("fake_cloudflared_commands.json"),
        script.to_string(),
    )
    .unwrap();
    let login_lock = dir.join("login.lock");
    let child_lock = dir.join("child.lock");

    let exe = std::env::current_exe().expect("current_exe");
    let mut helper = Command::new(exe)
        .args([
            "job_helper_contained_login",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(LOGIN_HELPER_ENV, &dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the helper");
    let mut report = std::collections::HashMap::new();
    for line in BufReader::new(helper.stdout.take().expect("stdout")).lines() {
        let line = line.expect("read line");
        if line.trim() == "READY" {
            break;
        }
        // The harness prints `test <name> ... ` on the same line as the first report.
        for key in ["WARNING", "LOGIN_IN_PROGRAM_JOB", "CHILD_IN_PROGRAM_JOB"] {
            if let Some((_, value)) = line.split_once(&format!("{key}=")) {
                report.insert(key.to_string(), value.trim().to_string());
            }
        }
    }
    let login_alive = held(&login_lock);
    let child_alive = held(&child_lock);
    let status = helper.wait().expect("wait for the helper");
    assert!(status.success(), "helper failed: {status:?} ({report:?})");

    if report.get("LOGIN_IN_PROGRAM_JOB").map(String::as_str) != Some("Some(false)") {
        // The test runner's own job forbids leaving it: the program cannot show here what it
        // does when started normally (Task 8 checks it live).
        eprintln!(
            "pomijam: proces testowy jest w obiekcie zadania, który nie pozwala wydzielić procesu ({report:?})"
        );
        // Before any assertion: the temporary folder goes only once they are gone.
        wait_until(20, || !held(&child_lock) && !held(&login_lock));
        assert!(
            report["WARNING"].contains("nie pozwala"),
            "the fallback must be reported: {report:?}"
        );
        return;
    }
    assert_eq!(report["WARNING"], "", "{report:?}");
    assert!(login_alive && child_alive, "both ran: {report:?}");
    assert_eq!(
        report["CHILD_IN_PROGRAM_JOB"], "Some(false)",
        "the browser must not be in the program's job"
    );
    assert!(
        wait_until(5, || !held(&login_lock)),
        "the login's cloudflared outlived the app"
    );
    assert!(held(&child_lock), "the browser stand-in died with the app");
    // It ends on its own before the temporary folder goes.
    assert!(wait_until(20, || !held(&child_lock)));
}
