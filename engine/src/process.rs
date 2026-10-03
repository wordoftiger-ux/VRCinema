//! Child processes: run-and-collect helper plus a shared way to spawn long-lived children
//! (ffmpeg) that die together with this program on Windows (job object) and when dropped.
//!
//! Port of `src/util/process.ts`.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

/// Upper bound for collected output (stdout and stderr together); anything beyond is dropped.
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("Przekroczono limit czasu ({secs} s): {cmd}")]
    Timeout { secs: u64, cmd: String },
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Builds a command: no console window on Windows and `kill_on_drop(true)` (the child is killed
/// when its `Child` handle is dropped).
///
/// **Callers must start the command with [`spawn`]** (or [`run_process`]), not with
/// `Command::spawn`; only `spawn` attaches the child to the Windows job object. Without it a
/// child outlives this process if the program dies abruptly.
///
/// On Linux there is deliberately no parent-death signal (`PR_SET_PDEATHSIG` is tied to the
/// spawning thread, and tokio blocking threads come and go); we rely on `kill_on_drop` and the
/// container lifecycle there.
pub fn command(cmd: &Path) -> Command {
    let mut command = Command::new(cmd);
    command.kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Spawns `cmd` (built with [`command`]) and, on Windows, attaches the child to the
/// program-wide job object so it cannot outlive this process even if we are killed abruptly.
/// On other platforms this is equivalent to `cmd.spawn()` (`kill_on_drop` only).
pub fn spawn(cmd: &mut Command) -> io::Result<Child> {
    let child = cmd.spawn()?;
    assign_to_job(&child);
    Ok(child)
}

/// Attaches an already spawned child to the program-wide job object (Windows,
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`). Best effort; a no-op on other platforms.
pub fn assign_to_job(child: &Child) {
    #[cfg(windows)]
    job::assign(child);
    #[cfg(not(windows))]
    let _ = child;
}

#[cfg(windows)]
mod job {
    use std::io::Error;
    use std::sync::OnceLock;

    use tokio::process::Child;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// The job handle as an address (`HANDLE` is a raw pointer, not `Send`/`Sync`); 0 = unavailable.
    /// The handle is intentionally never closed: it lives as long as the program, and the OS
    /// closing it at process exit is exactly what kills the children.
    static JOB: OnceLock<usize> = OnceLock::new();

    fn create() -> Option<usize> {
        // SAFETY: plain Win32 calls with valid arguments; `info` outlives the call.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                eprintln!(
                    "job object: CreateJobObjectW failed: {}",
                    Error::last_os_error()
                );
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                eprintln!(
                    "job object: SetInformationJobObject failed: {}",
                    Error::last_os_error()
                );
                return None;
            }
            Some(handle as usize)
        }
    }

    pub fn assign(child: &Child) {
        let job = *JOB.get_or_init(|| create().unwrap_or(0));
        if job == 0 {
            return;
        }
        let Some(process) = child.raw_handle() else {
            return; // already reaped
        };
        // SAFETY: both handles are valid for the duration of the call.
        let ok = unsafe { AssignProcessToJobObject(job as HANDLE, process as HANDLE) };
        if ok == 0 {
            eprintln!(
                "job object: AssignProcessToJobObject failed: {}",
                Error::last_os_error()
            );
        }
    }
}

/// Reads `reader` to the end, keeping chunks only while the shared byte total stays within
/// [`MAX_OUTPUT`] (like the TypeScript original, which still drains the pipe afterwards).
async fn collect<R: AsyncRead + Unpin>(mut reader: R, total: &AtomicUsize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let seen = total.fetch_add(n, Ordering::Relaxed) + n;
                if seen <= MAX_OUTPUT {
                    out.extend_from_slice(&buf[..n]);
                }
            }
        }
    }
    out
}

async fn collect_opt<R: AsyncRead + Unpin>(reader: Option<R>, total: &AtomicUsize) -> Vec<u8> {
    match reader {
        Some(reader) => collect(reader, total).await,
        None => Vec::new(),
    }
}

/// Runs a program and collects its output. When `timeout` elapses the process is killed and
/// [`ProcessError::Timeout`] is returned.
pub async fn run_process(
    cmd: &Path,
    args: &[OsString],
    timeout: Duration,
) -> Result<ProcessOutput, ProcessError> {
    let mut command = command(cmd);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn(&mut command)?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let total = AtomicUsize::new(0);
    let work = async {
        let (status, out, err) = tokio::join!(
            child.wait(),
            collect_opt(stdout, &total),
            collect_opt(stderr, &total)
        );
        status.map(|status| ProcessOutput {
            code: status.code(),
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        })
    };

    match tokio::time::timeout(timeout, work).await {
        Ok(result) => Ok(result?),
        Err(_) => {
            // The timeout dropped `work`, releasing its borrow of `child`.
            let _ = child.kill().await;
            Err(ProcessError::Timeout {
                secs: timeout.as_secs_f64().round() as u64,
                cmd: cmd.display().to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[cfg(windows)]
    fn shell(script: &str) -> (std::path::PathBuf, Vec<OsString>) {
        ("cmd".into(), vec!["/C".into(), script.into()])
    }

    #[cfg(unix)]
    fn shell(script: &str) -> (std::path::PathBuf, Vec<OsString>) {
        ("sh".into(), vec!["-c".into(), script.into()])
    }

    #[cfg(windows)]
    fn sleeper() -> (std::path::PathBuf, Vec<OsString>) {
        (
            "ping".into(),
            vec!["-n".into(), "60".into(), "127.0.0.1".into()],
        )
    }

    #[cfg(unix)]
    fn sleeper() -> (std::path::PathBuf, Vec<OsString>) {
        ("sleep".into(), vec!["60".into()])
    }

    #[tokio::test]
    async fn collects_stdout_and_exit_code() {
        let (cmd, args) = shell("echo hello");
        let out = run_process(&cmd, &args, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(out.code, Some(0));
        assert_eq!(out.stdout.trim(), "hello");
        assert_eq!(out.stderr, "");
    }

    #[tokio::test]
    async fn reports_nonzero_exit_code_and_stderr() {
        #[cfg(windows)]
        let script = "echo oops 1>&2 & exit 3";
        #[cfg(unix)]
        let script = "echo oops 1>&2; exit 3";
        let (cmd, args) = shell(script);
        let out = run_process(&cmd, &args, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stderr.trim(), "oops");
    }

    #[tokio::test]
    async fn missing_program_is_an_io_error() {
        let err = run_process(
            Path::new("definitely-not-a-real-program-xyz"),
            &[],
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProcessError::Io(_)), "{err:?}");
    }

    #[tokio::test]
    async fn timeout_kills_the_process_and_fails_in_polish() {
        let (cmd, args) = sleeper();
        let started = Instant::now();
        let err = run_process(&cmd, &args, Duration::from_millis(700))
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        assert!(matches!(err, ProcessError::Timeout { .. }), "{err:?}");
        let text = err.to_string();
        assert!(
            text.starts_with("Przekroczono limit czasu (1 s): "),
            "{text}"
        );
        assert!(text.ends_with(&cmd.display().to_string()), "{text}");
    }

    /// Compile-time proof that the future is `Send` (needed for `tokio::spawn` / axum handlers).
    #[tokio::test(flavor = "multi_thread")]
    async fn run_process_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let (cmd, args) = shell("echo hello");
        let fut = run_process(&cmd, &args, Duration::from_secs(10));
        assert_send(&fut);
        drop(fut);

        let handle =
            tokio::spawn(async move { run_process(&cmd, &args, Duration::from_secs(10)).await });
        let out = handle.await.unwrap().unwrap();
        assert_eq!(out.stdout.trim(), "hello");
    }

    #[test]
    fn timeout_message_format() {
        let err = ProcessError::Timeout {
            secs: 2,
            cmd: "x".into(),
        };
        assert_eq!(err.to_string(), "Przekroczono limit czasu (2 s): x");
    }
}
