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
/// child outlives this process if the program dies abruptly. (The server additionally puts
/// itself into the job at startup, see [`contain_this_process`]; then children inherit it.)
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

/// Puts this process itself into the program-wide job object (Windows), so every process it
/// starts from now on — and every process those start — is born inside the job and dies with
/// this program, even a grandchild started before [`spawn`] could assign its parent.
///
/// Call it once at server startup from `main`. Never from tests or library code run inside
/// the test harness: the job kills all its members when this process exits.
/// A no-op on other platforms.
pub fn contain_this_process() -> io::Result<()> {
    #[cfg(windows)]
    return job::contain_self();
    #[cfg(not(windows))]
    Ok(())
}

/// The job of one process started by [`spawn_in_own_job`]: dropping it (or this program
/// ending, which closes the handle) kills that process.
pub struct OwnJob {
    #[cfg(windows)]
    handle: usize,
    warning: Option<String>,
}

impl OwnJob {
    /// What did not go as planned (Polish, for the app's log): the process could not leave
    /// the program-wide job, or got no job of its own (it still dies with its handle).
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }
}

impl Drop for OwnJob {
    fn drop(&mut self) {
        #[cfg(windows)]
        job::close(self.handle);
    }
}

/// Spawns `cmd` (built with [`command`]) in a job of its own instead of the program-wide one,
/// for a program that starts processes which must outlive it and us — `cloudflared tunnel
/// login` opens the owner's browser, which must never die with this program. Windows: the
/// child is started outside the program-wide job (`CREATE_BREAKAWAY_FROM_JOB`, which that job
/// allows) and put alone into a new job with `KILL_ON_JOB_CLOSE` (it dies with the returned
/// [`OwnJob`] or with this program) and `SILENT_BREAKAWAY_OK` (whatever it starts is in no job
/// of ours). Elsewhere: a plain spawn (`kill_on_drop`).
pub fn spawn_in_own_job(cmd: &mut Command) -> io::Result<(Child, OwnJob)> {
    #[cfg(windows)]
    return job::spawn_own(cmd);
    #[cfg(not(windows))]
    Ok((cmd.spawn()?, OwnJob { warning: None }))
}

/// Diagnostics and tests: whether process `pid` is in the program-wide job (`None`: cannot
/// tell). Windows only.
#[cfg(windows)]
pub fn in_program_job(pid: u32) -> Option<bool> {
    job::contains(pid)
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::process::{Child, Command};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// The job handle as an address (`HANDLE` is a raw pointer, not `Send`/`Sync`); 0 = unavailable.
    /// The handle is intentionally never closed: it lives as long as the program, and the OS
    /// closing it at process exit is exactly what kills the children.
    static JOB: OnceLock<usize> = OnceLock::new();
    /// This process is a member of `JOB`: children inherit the membership at creation.
    static SELF_CONTAINED: AtomicBool = AtomicBool::new(false);

    fn job() -> usize {
        *JOB.get_or_init(|| create().unwrap_or(0))
    }

    pub fn contain_self() -> std::io::Result<()> {
        if SELF_CONTAINED.load(Ordering::SeqCst) {
            return Ok(());
        }
        let job = job();
        if job == 0 {
            return Err(Error::other(
                "nie można utworzyć obiektu zadania (job object)",
            ));
        }
        // SAFETY: `job` is a valid job handle; GetCurrentProcess returns a pseudo-handle that
        // needs no closing.
        let ok = unsafe { AssignProcessToJobObject(job as HANDLE, GetCurrentProcess()) };
        if ok == 0 {
            return Err(Error::last_os_error());
        }
        SELF_CONTAINED.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn create() -> Option<usize> {
        // Members die with the job; only a process started with CREATE_BREAKAWAY_FROM_JOB
        // (the updated desktop app, the login's cloudflared) may leave it.
        create_with(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK)
            .inspect_err(|e| eprintln!("job object: {e}"))
            .ok()
    }

    /// A new job with these limit flags, or which call failed and why.
    fn create_with(flags: u32) -> Result<usize, String> {
        // SAFETY: plain Win32 calls with valid arguments; `info` outlives the call; a handle
        // that cannot be set up is closed once.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(format!("CreateJobObjectW: {}", Error::last_os_error()));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = flags;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let e = Error::last_os_error();
                CloseHandle(handle);
                return Err(format!("SetInformationJobObject: {e}"));
            }
            Ok(handle as usize)
        }
    }

    /// See [`super::spawn_in_own_job`]. What did not go as planned is in
    /// [`super::OwnJob::warning`] (Polish, for the app's log).
    pub fn spawn_own(cmd: &mut Command) -> std::io::Result<(Child, super::OwnJob)> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const ERROR_ACCESS_DENIED: i32 = 5;
        let mut warnings = Vec::new();
        // Out of the program-wide job (which allows that), so its own children are born
        // outside every job of ours.
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        let child = match cmd.spawn() {
            Ok(child) => child,
            // This process sits in a job (not ours) that allows no breakaway: start it inside.
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
                warnings.push(
                    "program działa w obiekcie zadania Windows, który nie pozwala wydzielić procesu — przeglądarka otwarta przez cloudflared może zostać zamknięta razem z programem"
                        .to_string(),
                );
                cmd.creation_flags(CREATE_NO_WINDOW);
                cmd.spawn()?
            }
            Err(e) => return Err(e),
        };
        // Killed when the job closes; whatever it starts is silently left out of the job.
        let handle = match create_with(
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
        ) {
            Ok(handle) => handle,
            Err(e) => {
                warnings.push(format!(
                    "nie można utworzyć osobnego obiektu zadania Windows dla cloudflared ({e}) — zatrzyma się przy anulowaniu, ale może przetrwać nagłe zamknięcie programu"
                ));
                0
            }
        };
        if handle != 0
            && let Some(process) = child.raw_handle()
        {
            // SAFETY: both handles are valid for the duration of the call.
            if unsafe { AssignProcessToJobObject(handle as HANDLE, process as HANDLE) } == 0 {
                warnings.push(format!(
                    "nie można przypisać cloudflared do jego obiektu zadania Windows ({}) — zatrzyma się przy anulowaniu, ale może przetrwać nagłe zamknięcie programu",
                    Error::last_os_error()
                ));
            }
        }
        // Without its own job it still dies with its handle (`kill_on_drop`).
        let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
        Ok((child, super::OwnJob { handle, warning }))
    }

    /// Closes a job made by [`spawn_own`] (its process dies with it).
    pub fn close(handle: usize) {
        if handle != 0 {
            // SAFETY: the handle came from CreateJobObjectW and is closed once.
            unsafe { CloseHandle(handle as HANDLE) };
        }
    }

    /// The limit flags of the program-wide job (creating it if needed).
    #[cfg(test)]
    pub fn limit_flags() -> Option<u32> {
        use windows_sys::Win32::System::JobObjects::QueryInformationJobObject;
        let job = job();
        if job == 0 {
            return None;
        }
        // SAFETY: `job` is a valid job handle; `info` outlives the call and has the queried size.
        unsafe {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                job as HANDLE,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of_mut!(info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            (ok != 0).then_some(info.BasicLimitInformation.LimitFlags)
        }
    }

    /// Whether process `pid` is in the program-wide job (`None`: cannot tell).
    pub fn contains(pid: u32) -> Option<bool> {
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let job = job();
        if job == 0 {
            return None;
        }
        // SAFETY: plain Win32 calls; the process handle is closed once.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut result = 0;
            let ok = IsProcessInJob(process, job as HANDLE, &mut result);
            CloseHandle(process);
            (ok != 0).then_some(result != 0)
        }
    }

    /// Fallback for when this process is not itself in the job (tests, or `contain_self`
    /// failed): attaches the child after it started. A child that starts its own child at once
    /// can escape this way, hence `contain_self` at startup.
    pub fn assign(child: &Child) {
        if SELF_CONTAINED.load(Ordering::SeqCst) {
            return; // born inside the job already
        }
        let job = job();
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

    /// The desktop app starts its updated self with `CREATE_BREAKAWAY_FROM_JOB`; without
    /// `BREAKAWAY_OK` the new program would die together with the old one.
    #[cfg(windows)]
    #[test]
    fn job_kills_its_members_but_allows_an_explicit_breakaway() {
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
        };
        let flags = job::limit_flags().expect("job object");
        assert_ne!(flags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, 0);
        assert_ne!(flags & JOB_OBJECT_LIMIT_BREAKAWAY_OK, 0);
        assert_eq!(flags & JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, 0);
    }

    /// A process in a job of its own dies when that job is dropped (its handle is still held
    /// here, so nothing else killed it).
    #[cfg(windows)]
    #[tokio::test]
    async fn closing_its_own_job_kills_the_process() {
        let (cmd, args) = sleeper();
        let mut command = command(&cmd);
        command.args(&args);
        let (mut child, job) = spawn_in_own_job(&mut command).unwrap();
        let pid = child.id().unwrap();
        assert_eq!(in_program_job(pid), Some(false), "not in the program's job");
        assert!(
            tokio::time::timeout(Duration::from_millis(300), child.wait())
                .await
                .is_err(),
            "still running"
        );
        drop(job);
        // The sleeper runs 60 s; the job's close ends it at once (exit code 0: the job's).
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("the job's close killed it")
            .unwrap();
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
