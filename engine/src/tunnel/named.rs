//! The permanent link's cloudflared management commands (spec section 9): logging in to
//! Cloudflare, creating (or reusing) the tunnel `vrcinema` with a DNS route for the owner's
//! host name, and deleting it; plus [`NamedSetup`], which lets one such operation run at a time
//! and keeps its progress for the window.
//!
//! The certificate and the credentials live only in `<home>/tunnel/`. Their contents are never
//! logged, returned or put in an error message: paths at most.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{OwnedMutexGuard, mpsc, oneshot};

use super::process::forward_lines;
use crate::pipeline::clip_factory::SharedLogFn;
use crate::process::{ProcessError, run_process};

/// The tunnel's name in the owner's Cloudflare account.
pub const TUNNEL_NAME: &str = "vrcinema";
/// The origin certificate `tunnel login` produces.
pub const CERT_FILE: &str = "cert.pem";
/// Spec section 9: an account without a domain.
pub const NO_DOMAIN: &str = "Dodaj domenę do Cloudflare, zanim ustawisz stały link.";
/// How to add a domain to Cloudflare (shown with [`ErrorCode::NoDomain`]).
pub const ADD_DOMAIN_HELP: &str =
    "https://developers.cloudflare.com/fundamentals/manage-domains/add-site/";

/// Timings and places of the management commands; the defaults are the program's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedOptions {
    /// The owner has this long to log in in the browser (5 min).
    pub login_timeout: Duration,
    /// Longest run of `create`, `token`, `route dns` and `delete` (60 s).
    pub command_timeout: Duration,
    /// cloudflared's own folder, where `tunnel login` saves `cert.pem`; `None`: the real one
    /// ([`default_user_dir`]). Tests always set a temporary folder.
    pub user_dir: Option<PathBuf>,
}

impl Default for NamedOptions {
    fn default() -> Self {
        Self {
            login_timeout: Duration::from_secs(5 * 60),
            command_timeout: Duration::from_secs(60),
            user_dir: None,
        }
    }
}

/// cloudflared's default folder for `cert.pem`: `~/.cloudflared`, where `~` is `HOME` or else
/// `USERPROFILE` (the order cloudflared itself uses on Windows).
pub fn default_user_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|name| std::env::var_os(name).filter(|v| !v.is_empty()))
        .map(|home| PathBuf::from(home).join(".cloudflared"))
}

/// The files of the permanent link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedPaths {
    /// `tools\cloudflared.exe`.
    pub cloudflared: PathBuf,
    /// `<home>/tunnel`.
    pub dir: PathBuf,
    /// cloudflared's own folder (see [`NamedOptions::user_dir`]).
    pub user_dir: PathBuf,
}

impl NamedPaths {
    /// `<home>/tunnel/cert.pem`.
    pub fn cert(&self) -> PathBuf {
        self.dir.join(CERT_FILE)
    }

    /// `<home>/tunnel/<id>.json`.
    pub fn credentials(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Logged in to Cloudflare: the certificate is in `<home>/tunnel`.
    pub fn logged_in(&self) -> bool {
        non_empty(&self.cert())
    }
}

fn non_empty(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// A tunnel UUID as cloudflared names its credentials file (`8-4-4-4-12` hex digits).
pub fn is_tunnel_id(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, len)| g.len() == len && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Why an operation failed, for the window (`code`) and the owner (`message`, Polish).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Another operation of the permanent link is running.
    Busy,
    /// `tools\cloudflared.exe` is missing.
    NoCloudflared,
    /// No certificate yet: log in first.
    NotLoggedIn,
    /// The login (or a command) took too long.
    Timeout,
    /// The owner cancelled the login.
    Cancelled,
    /// Another DNS record has the host name (`overwrite` replaces it).
    DnsTaken,
    /// The host name is not in a domain of the account (or the account has none).
    NoDomain,
    /// Anything else cloudflared refused or failed at.
    Failed,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::NoCloudflared => "no_cloudflared",
            Self::NotLoggedIn => "not_logged_in",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::DnsTaken => "dns_taken",
            Self::NoDomain => "no_domain",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedError {
    pub code: ErrorCode,
    pub message: String,
}

impl NamedError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// An operation of the permanent-link wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Login,
    Create,
    Quick,
    Delete,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Create => "create",
            Self::Quick => "quick",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Nothing ran yet.
    #[default]
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Where a running operation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// cloudflared is starting.
    Starting,
    /// The login page is open: the owner logs in and picks a domain.
    WaitingForBrowser,
    CreatingTunnel,
    /// The tunnel already exists: its credentials are fetched.
    FetchingCredentials,
    RoutingDns,
    /// The settings and the running tunnel switch modes.
    Switching,
    Deleting,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::WaitingForBrowser => "waiting_for_browser",
            Self::CreatingTunnel => "creating_tunnel",
            Self::FetchingCredentials => "fetching_credentials",
            Self::RoutingDns => "routing_dns",
            Self::Switching => "switching",
            Self::Deleting => "deleting",
        }
    }
}

/// The wizard's last (or running) operation, as `GET tunnel/named/progress` shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub operation: Option<Operation>,
    pub phase: Phase,
    pub step: Option<Step>,
    /// What is happening, what went wrong or what came out (Polish).
    pub message: Option<String>,
    pub code: Option<ErrorCode>,
    /// The Cloudflare login page while the login waits for the owner (the window opens it).
    pub login_url: Option<String>,
    /// cloudflared says it opened the browser itself (`Some(false)`: the window should open
    /// `login_url`); `None` until it says either.
    pub browser_opened: Option<bool>,
}

/// One operation of the permanent link at a time, its progress and the login's cancel.
#[derive(Default)]
pub struct NamedSetup {
    busy: Arc<tokio::sync::Mutex<()>>,
    progress: Mutex<Progress>,
    cancel: Mutex<Option<oneshot::Sender<()>>>,
    /// The lock is held by a tools job replacing cloudflared.
    tools: Arc<AtomicBool>,
}

/// Held while an operation runs; others are refused meanwhile.
pub struct Busy {
    _guard: OwnedMutexGuard<()>,
    /// Set while a tools job holds it.
    tools: Option<Arc<AtomicBool>>,
}

impl Drop for Busy {
    fn drop(&mut self) {
        // Before the lock goes: whoever gets it next never sees a stale flag.
        if let Some(tools) = &self.tools {
            tools.store(false, Ordering::SeqCst);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl NamedSetup {
    /// Starts `operation` (progress `running`), or [`ErrorCode::Busy`] while another runs.
    pub fn begin(&self, operation: Operation) -> Result<Busy, NamedError> {
        let guard = Arc::clone(&self.busy).try_lock_owned().map_err(|_| {
            let why = if self.tools.load(Ordering::SeqCst) {
                TOOLS_BUSY
            } else {
                BUSY
            };
            NamedError::new(ErrorCode::Busy, why)
        })?;
        *lock(&self.progress) = Progress {
            operation: Some(operation),
            phase: Phase::Running,
            ..Progress::default()
        };
        Ok(Busy {
            _guard: guard,
            tools: None,
        })
    }

    pub fn progress(&self) -> Progress {
        lock(&self.progress).clone()
    }

    /// The running operation reached `step`.
    pub fn step(&self, step: Step, message: impl Into<String>) {
        let mut progress = lock(&self.progress);
        progress.step = Some(step);
        progress.message = Some(message.into());
    }

    pub fn set_browser_opened(&self, opened: bool) {
        lock(&self.progress).browser_opened = Some(opened);
    }

    /// Held by a tools job that replaces cloudflared (update, repair): the wizard's operations
    /// are refused meanwhile, with a message saying why. `None` while an operation runs.
    pub fn hold_for_tools(&self) -> Option<Busy> {
        let guard = Arc::clone(&self.busy).try_lock_owned().ok()?;
        self.tools.store(true, Ordering::SeqCst);
        Some(Busy {
            _guard: guard,
            tools: Some(Arc::clone(&self.tools)),
        })
    }

    pub fn set_login_url(&self, url: String) {
        lock(&self.progress).login_url = Some(url);
    }

    /// The login's cancel, kept until it ends.
    pub fn set_cancel(&self, cancel: oneshot::Sender<()>) {
        *lock(&self.cancel) = Some(cancel);
    }

    /// Cancels a running login; false when none runs.
    pub fn cancel_login(&self) -> bool {
        lock(&self.cancel)
            .take()
            .is_some_and(|cancel| cancel.send(()).is_ok())
    }

    /// The operation ended: done with `message`, or failed/cancelled. The login page goes.
    pub fn finish(&self, result: &Result<String, NamedError>) {
        lock(&self.cancel).take();
        let mut progress = lock(&self.progress);
        progress.step = None;
        progress.login_url = None;
        progress.browser_opened = None;
        match result {
            Ok(message) => {
                progress.phase = Phase::Done;
                progress.message = Some(message.clone());
                progress.code = None;
            }
            Err(e) => {
                progress.phase = if e.code == ErrorCode::Cancelled {
                    Phase::Cancelled
                } else {
                    Phase::Failed
                };
                progress.message = Some(e.message.clone());
                progress.code = Some(e.code);
            }
        }
    }
}

/// What the login did with cloudflared's certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertSource {
    /// Written by this login: moved into `<home>/tunnel` (nothing stays behind).
    Moved,
    /// Already in cloudflared's folder before the login (cloudflared then refuses to log in
    /// again): copied, and the owner's file is left alone.
    Copied,
}

/// What `tunnel login` reports while it waits for the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    /// The Cloudflare login page.
    Url(String),
    /// cloudflared opened the browser itself (`false`: it asks to open the page by hand).
    BrowserOpened(bool),
}

/// `cloudflared tunnel login`: reports the login page (and whether cloudflared opened the
/// browser) through `on_event` as soon as cloudflared prints it, waits (at most `timeout`, or
/// until `cancel` fires or is dropped) for the certificate, then puts it into
/// `<home>/tunnel/cert.pem`.
///
/// The certificate's rule: cloudflared writes it into its own folder (`user_dir`). A certificate
/// this login wrote is **moved** (no copy stays behind) — also when a cancel or the time limit
/// came just after cloudflared wrote it, so it never passes for the owner's own next time. One
/// that was already there before the login belongs to the owner — cloudflared then refuses to
/// log in again and ends at once — so it is **copied** and the owner's file is left alone.
///
/// cloudflared runs in a job of its own ([`crate::process::spawn_in_own_job`]): it dies with
/// the login (or the app), the browser it opens never does.
///
/// The login page is never logged: it leads to the certificate.
pub async fn login(
    paths: &NamedPaths,
    timeout: Duration,
    mut cancel: oneshot::Receiver<()>,
    mut on_event: impl FnMut(LoginEvent),
    log: &SharedLogFn,
) -> Result<CertSource, NamedError> {
    check_cloudflared(paths)?;
    let user_cert = paths.user_dir.join(CERT_FILE);
    let existed = non_empty(&user_cert);
    let mut command = crate::process::command(&paths.cloudflared);
    command
        .args(args(None, &["login"]))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Dropped on every way out of here: cloudflared dies with it.
    let (mut child, job) = crate::process::spawn_in_own_job(&mut command).map_err(cannot_start)?;
    if let Some(warning) = job.warning() {
        log(&format!("Stały link: {warning}."));
    }
    log(&format!(
        "Stały link: logowanie do Cloudflare (cloudflared, pid {}) — czekam na potwierdzenie w przeglądarce.",
        child.id().unwrap_or(0)
    ));
    let (tx, mut lines) = mpsc::channel(64);
    if let Some(pipe) = child.stdout.take() {
        tokio::spawn(forward_lines(pipe, tx.clone()));
    }
    if let Some(pipe) = child.stderr.take() {
        tokio::spawn(forward_lines(pipe, tx));
    }
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut url_seen = false;
    let mut last = String::new();
    let mut open = true;
    let status = loop {
        tokio::select! {
            biased;
            _ = &mut cancel => {
                kill(&mut child).await;
                keep_late_cert(paths, &user_cert, existed, log);
                log("Stały link: logowanie do Cloudflare anulowano.");
                return Err(NamedError::new(ErrorCode::Cancelled, "Logowanie do Cloudflare anulowano."));
            }
            _ = &mut deadline => {
                kill(&mut child).await;
                keep_late_cert(paths, &user_cert, existed, log);
                log(&format!(
                    "Stały link: logowanie do Cloudflare nie zakończyło się w ciągu {}.",
                    human(timeout)
                ));
                return Err(NamedError::new(
                    ErrorCode::Timeout,
                    format!(
                        "Logowanie do Cloudflare nie zostało ukończone w ciągu {}. Spróbuj ponownie. Jeśli na koncie Cloudflare nie ma jeszcze domeny, najpierw ją dodaj — bez niej logowanie się nie zakończy.",
                        human(timeout)
                    ),
                ));
            }
            line = lines.recv(), if open => match line {
                None => open = false,
                Some(line) => {
                    if let Some(opened) = browser_opened_in(&line) {
                        on_event(LoginEvent::BrowserOpened(opened));
                    }
                    if !url_seen && let Some(url) = login_url_in(&line) {
                        url_seen = true;
                        on_event(LoginEvent::Url(url));
                    }
                    remember(&mut last, &line, true);
                }
            },
            status = child.wait() => break status,
        }
    };
    // Lines written just before the exit may still be in transit.
    let late = tokio::time::Instant::now() + Duration::from_millis(200);
    while let Ok(Some(line)) = tokio::time::timeout_at(late, lines.recv()).await {
        remember(&mut last, &line, true);
    }
    let status = status.map_err(|e| failed(format!("Logowanie do Cloudflare przerwane: {e}")))?;
    if !status.success() {
        log(&format!(
            "Stały link: logowanie do Cloudflare nie powiodło się: {}",
            or_none(&last)
        ));
        return Err(failed(format!(
            "Logowanie do Cloudflare nie powiodło się: {}",
            or_none(&last)
        )));
    }
    if !non_empty(&user_cert) {
        log(&format!(
            "Stały link: cloudflared zakończył logowanie bez certyfikatu w {}.",
            user_cert.display()
        ));
        return Err(failed(
            "Logowanie do Cloudflare nie powiodło się: cloudflared nie zapisał certyfikatu. Spróbuj ponownie.",
        ));
    }
    let target = paths.cert();
    let source = if existed {
        copy_file(&user_cert, &target).map(|()| CertSource::Copied)
    } else {
        move_file(&user_cert, &target).map(|()| CertSource::Moved)
    }
    .map_err(|e| {
        log(&format!(
            "Stały link: nie można zapisać certyfikatu Cloudflare w {}: {e}",
            target.display()
        ));
        failed(format!(
            "Nie można zapisać certyfikatu Cloudflare w folderze programu: {e}"
        ))
    })?;
    log(&match source {
        CertSource::Moved => format!(
            "Stały link: połączono z Cloudflare; certyfikat przeniesiony z {} do {}.",
            user_cert.display(),
            target.display()
        ),
        CertSource::Copied => format!(
            "Stały link: użyto certyfikatu cloudflared, który już był w {} (skopiowany do {}; plik właściciela został).",
            user_cert.display(),
            target.display()
        ),
    });
    Ok(source)
}

/// After a cancel or the time limit: a certificate this login wrote just before is moved into
/// `<home>/tunnel` like a finished login's (left in cloudflared's folder, it would pass for the
/// owner's own next time).
fn keep_late_cert(paths: &NamedPaths, user_cert: &Path, existed: bool, log: &SharedLogFn) {
    if existed || !non_empty(user_cert) {
        return;
    }
    let target = paths.cert();
    match move_file(user_cert, &target) {
        Ok(()) => log(&format!(
            "Stały link: certyfikat zapisany tuż przed przerwaniem logowania przeniesiony z {} do {}.",
            user_cert.display(),
            target.display()
        )),
        Err(e) => log(&format!(
            "Stały link: nie można przenieść certyfikatu z {} do {}: {e}",
            user_cert.display(),
            target.display()
        )),
    }
}

/// Whether a line of `tunnel login` says cloudflared opened the browser (`Some(true)`) or asks
/// to open the page by hand (`Some(false)`).
fn browser_opened_in(line: &str) -> Option<bool> {
    if line.contains("A browser window should have opened") {
        Some(true)
    } else if line.contains("Please open the following URL") {
        Some(false)
    } else {
        None
    }
}

/// `cloudflared tunnel create vrcinema` (an existing `vrcinema` is reused, its credentials fetched
/// with `tunnel token --cred-file`), the credentials stored as `<home>/tunnel/<id>.json`, then
/// `tunnel route dns [--overwrite-dns] <id> <hostname>`. Returns the tunnel's id.
///
/// cloudflared names the credentials file only after the tunnel exists, so they are written to
/// `vrcinema-new.json` first and renamed after the id inside (the only field ever read).
pub async fn create(
    paths: &NamedPaths,
    hostname: &str,
    overwrite: bool,
    timeout: Duration,
    mut on_step: impl FnMut(Step),
    log: &SharedLogFn,
) -> Result<String, NamedError> {
    check_cloudflared(paths)?;
    let cert = check_cert(paths)?;
    std::fs::create_dir_all(&paths.dir)
        .map_err(|e| failed(format!("Nie można utworzyć folderu tunelu: {e}")))?;
    let new = paths.dir.join(NEW_CREDENTIALS);
    let _ = remove_file(&new);

    on_step(Step::CreatingTunnel);
    let out = run(
        paths,
        args(
            Some(&cert),
            &[
                OsString::from("create"),
                "--credentials-file".into(),
                new.clone().into(),
                TUNNEL_NAME.into(),
            ],
        ),
        timeout,
    )
    .await?;
    if out.code != Some(0) {
        if !tunnel_exists(&out.text) {
            log(&format!(
                "Stały link: tworzenie tunelu nie powiodło się: {}",
                out.last
            ));
            return Err(failed(format!(
                "Nie udało się utworzyć tunelu {TUNNEL_NAME} w Cloudflare: {}",
                out.last
            )));
        }
        log(&format!(
            "Stały link: tunel {TUNNEL_NAME} już istnieje w Cloudflare — używam go."
        ));
        on_step(Step::FetchingCredentials);
        let out = run(
            paths,
            args(
                Some(&cert),
                &[
                    OsString::from("token"),
                    "--cred-file".into(),
                    new.clone().into(),
                    TUNNEL_NAME.into(),
                ],
            ),
            timeout,
        )
        .await?;
        if out.code != Some(0) {
            log(&format!(
                "Stały link: pobieranie poświadczeń nie powiodło się: {}",
                out.last
            ));
            return Err(failed(format!(
                "Nie udało się pobrać poświadczeń tunelu {TUNNEL_NAME}: {}",
                out.last
            )));
        }
    }
    let Some(id) = credentials_id(&new) else {
        let _ = remove_file(&new);
        return Err(failed(
            "cloudflared nie zapisał poprawnych poświadczeń tunelu. Spróbuj ponownie.",
        ));
    };
    let credentials = paths.credentials(&id);
    replace_file(&new, &credentials).map_err(|e| {
        failed(format!(
            "Nie można zapisać poświadczeń tunelu w folderze programu: {e}"
        ))
    })?;
    log(&format!(
        "Stały link: tunel {TUNNEL_NAME} ({id}), poświadczenia w {}.",
        credentials.display()
    ));

    on_step(Step::RoutingDns);
    let mut rest = vec!["route", "dns"];
    if overwrite {
        rest.push("--overwrite-dns");
    }
    rest.extend([id.as_str(), hostname]);
    let out = run(paths, args(Some(&cert), &rest), timeout).await?;
    if out.code != Some(0) {
        log(&format!(
            "Stały link: ustawianie {hostname} w DNS nie powiodło się: {}",
            out.last
        ));
        return Err(if dns_taken(&out.text) {
            NamedError::new(
                ErrorCode::DnsTaken,
                format!(
                    "Adres {hostname} jest już zajęty w DNS Cloudflare (ma go inny rekord). Wybierz inną nazwę albo zastąp istniejący rekord."
                ),
            )
        } else if zone_missing(&out.text) {
            no_domain(hostname, None)
        } else {
            failed(format!(
                "Nie udało się ustawić adresu {hostname} w DNS Cloudflare: {}",
                out.last
            ))
        });
    }
    // A name outside the certificate's domain gets that domain appended by Cloudflare.
    if let Some(record) = routed_record(&out.text)
        && !record.eq_ignore_ascii_case(hostname)
    {
        log(&format!(
            "Stały link: {hostname} nie należy do domeny z logowania — Cloudflare ustawił rekord {record}."
        ));
        return Err(no_domain(hostname, Some(&record)));
    }
    log(&format!(
        "Stały link: {hostname} prowadzi do tunelu {id}{}.",
        if overwrite {
            " (poprzedni rekord DNS zastąpiony)"
        } else {
            ""
        }
    ));
    Ok(id)
}

/// `cloudflared tunnel delete -f vrcinema` (a tunnel already gone counts as deleted).
pub async fn delete(
    paths: &NamedPaths,
    timeout: Duration,
    log: &SharedLogFn,
) -> Result<(), NamedError> {
    check_cloudflared(paths)?;
    let cert = check_cert(paths)?;
    let out = run(
        paths,
        args(Some(&cert), &["delete", "-f", TUNNEL_NAME]),
        timeout,
    )
    .await?;
    if out.code == Some(0) {
        log(&format!(
            "Stały link: usunięto tunel {TUNNEL_NAME} w Cloudflare."
        ));
    } else if tunnel_missing(&out.text) {
        log(&format!(
            "Stały link: tunelu {TUNNEL_NAME} już nie ma w Cloudflare."
        ));
    } else {
        log(&format!(
            "Stały link: usuwanie tunelu nie powiodło się: {}",
            out.last
        ));
        return Err(failed(format!(
            "Nie udało się usunąć tunelu {TUNNEL_NAME} w Cloudflare: {}",
            out.last
        )));
    }
    Ok(())
}

/// Removes the permanent link's files: the certificate, every credentials file and anything
/// half written (`*.tmp`). Other files stay; a missing folder is nothing to remove.
pub fn remove_files(dir: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        entries => entries?,
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let ours = name == CERT_FILE || name.ends_with(".json") || name.ends_with(".tmp");
        if ours && entry.file_type()?.is_file() {
            remove_file(&entry.path())?;
        }
    }
    Ok(())
}

const BUSY: &str = "Trwa już inna operacja stałego linku — poczekaj, aż się skończy.";
const TOOLS_BUSY: &str =
    "Trwa aktualizacja lub naprawa programu cloudflared — spróbuj ponownie, gdy się skończy.";
/// Credentials of a tunnel whose id is not known yet.
const NEW_CREDENTIALS: &str = "vrcinema-new.json";
/// The longest part of cloudflared's output put into a message.
const MAX_DETAIL: usize = 300;

/// No cloudflared in the tools (the same words as the tunnel's own error).
pub const NO_CLOUDFLARED: &str =
    "Brak programu cloudflared — pobierz brakujące narzędzia w ustawieniach (Narzędzia).";

impl Step {
    /// What the window says during `step` (`hostname`: the address being set up).
    pub fn message(self, hostname: &str) -> String {
        match self {
            Self::Starting => "Uruchamianie logowania do Cloudflare…".into(),
            Self::WaitingForBrowser => {
                "Zaloguj się do Cloudflare w przeglądarce i wybierz domenę dla stałego linku."
                    .into()
            }
            Self::CreatingTunnel => format!("Tworzenie tunelu {TUNNEL_NAME} w Cloudflare…"),
            Self::FetchingCredentials => {
                format!("Tunel {TUNNEL_NAME} już istnieje — pobieranie jego poświadczeń…")
            }
            Self::RoutingDns => format!("Ustawianie adresu {hostname} w DNS Cloudflare…"),
            Self::Switching => "Przełączanie tunelu…".into(),
            Self::Deleting => format!("Usuwanie tunelu {TUNNEL_NAME} w Cloudflare…"),
        }
    }
}

fn failed(message: impl Into<String>) -> NamedError {
    NamedError::new(ErrorCode::Failed, message)
}

fn cannot_start(e: std::io::Error) -> NamedError {
    failed(format!("Nie można uruchomić cloudflared: {e}"))
}

fn no_domain(hostname: &str, record: Option<&str>) -> NamedError {
    let mut message = format!(
        "{NO_DOMAIN} Adres {hostname} musi należeć do domeny wybranej przy logowaniu do Cloudflare."
    );
    if let Some(record) = record {
        message.push_str(&format!(
            " Cloudflare ustawił zamiast niego rekord {record} — usuń go w panelu Cloudflare (DNS)."
        ));
    }
    NamedError::new(ErrorCode::NoDomain, message)
}

fn check_cloudflared(paths: &NamedPaths) -> Result<(), NamedError> {
    if paths.cloudflared.is_file() {
        Ok(())
    } else {
        Err(NamedError::new(ErrorCode::NoCloudflared, NO_CLOUDFLARED))
    }
}

/// The certificate the commands need (`--origincert`).
fn check_cert(paths: &NamedPaths) -> Result<PathBuf, NamedError> {
    if paths.logged_in() {
        Ok(paths.cert())
    } else {
        Err(NamedError::new(
            ErrorCode::NotLoggedIn,
            "Najpierw połącz się z Cloudflare (krok „Połącz z Cloudflare”).",
        ))
    }
}

/// A finished management command.
struct Output {
    code: Option<i32>,
    /// stdout and stderr.
    text: String,
    /// Its last line, for a message.
    last: String,
}

async fn run(
    paths: &NamedPaths,
    args: Vec<OsString>,
    timeout: Duration,
) -> Result<Output, NamedError> {
    match run_process(&paths.cloudflared, &args, timeout).await {
        Ok(out) => {
            let text = format!("{}\n{}", out.stdout, out.stderr);
            let mut last = String::new();
            for line in text.lines() {
                remember(&mut last, line, false);
            }
            Ok(Output {
                code: out.code,
                last: or_none(&last).to_string(),
                text,
            })
        }
        Err(ProcessError::Timeout { secs, .. }) => Err(NamedError::new(
            ErrorCode::Timeout,
            format!(
                "cloudflared nie odpowiedział w ciągu {secs} s. Sprawdź połączenie z internetem i spróbuj ponownie."
            ),
        )),
        Err(ProcessError::Io(e)) => Err(cannot_start(e)),
    }
}

/// Keeps `line` as the last one worth showing (`no_urls`: never one with an address — the login
/// page leads to the certificate).
fn remember(last: &mut String, line: &str, no_urls: bool) {
    let line = line.trim();
    if line.is_empty() || (no_urls && line.contains("https://")) {
        return;
    }
    *last = line.chars().take(MAX_DETAIL).collect();
}

fn or_none(last: &str) -> &str {
    if last.is_empty() {
        "cloudflared nie podał przyczyny"
    } else {
        last
    }
}

/// `5 min`, `90 s`.
fn human(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

async fn kill(child: &mut tokio::process::Child) {
    // Fails only when the process has already exited; the wait reaps it either way.
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Copies through a temporary file next to `to`, so a half-written certificate never appears.
fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = to.with_extension("pem.tmp");
    std::fs::copy(from, &tmp)?;
    std::fs::rename(&tmp, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Moves `from` to `to`; across volumes it copies and then removes `from`.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_file(from, to)?;
    std::fs::remove_file(from)
}

/// `tunnel --no-autoupdate [--origincert <cert>] <rest…>`.
fn args<S: AsRef<OsStr>>(cert: Option<&Path>, rest: &[S]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["tunnel".into(), "--no-autoupdate".into()];
    if let Some(cert) = cert {
        args.push("--origincert".into());
        args.push(cert.into());
    }
    args.extend(rest.iter().map(|a| a.as_ref().to_os_string()));
    args
}

/// The Cloudflare login page in a line of `tunnel login`'s output (`https://…cloudflare.com/…`).
fn login_url_in(line: &str) -> Option<String> {
    line.split_whitespace()
        .filter(|word| word.starts_with("https://"))
        .find(|word| {
            url::Url::parse(word).is_ok_and(|url| {
                url.host_str()
                    .is_some_and(|h| h == "cloudflare.com" || h.ends_with(".cloudflare.com"))
            })
        })
        .map(str::to_string)
}

/// `create` failed because a tunnel of that name exists.
fn tunnel_exists(output: &str) -> bool {
    output.to_ascii_lowercase().contains("already exists")
}

/// `route dns` failed because another record has the name.
fn dns_taken(output: &str) -> bool {
    let output = output.to_ascii_lowercase();
    output.contains("already exists") || output.contains("code: 1003")
}

/// `route dns` failed because the account has no zone for the name.
fn zone_missing(output: &str) -> bool {
    let output = output.to_ascii_lowercase();
    output.contains("zone")
        && [
            "not found",
            "could not find",
            "no zone",
            "does not exist",
            "invalid zone",
        ]
        .iter()
        .any(|p| output.contains(p))
}

/// The record a successful `route dns` names, from any of cloudflared's three summaries
/// (`DNSRouteResult::SuccessSummary`): `Added CNAME <name> which will route to this tunnel`,
/// `<name> is already configured to route to your tunnel`, `<name> updated to route to your
/// tunnel`.
fn routed_record(output: &str) -> Option<String> {
    const AFTER: [&str; 2] = [
        " is already configured to route to your tunnel",
        " updated to route to your tunnel",
    ];
    output.lines().find_map(|line| {
        if let Some((_, rest)) = line.split_once("Added CNAME ") {
            return rest.split_whitespace().next().map(str::to_string);
        }
        AFTER.iter().find_map(|after| {
            let (before, _) = line.split_once(after)?;
            before.split_whitespace().last().map(str::to_string)
        })
    })
}

/// Moves `from` over `to`, also when `to` is read-only (cloudflared writes credentials
/// read-only, so a second attempt meets the first one's file).
///
/// The rename replaces `to` in one step; if it fails, `to` stays as it was (read-only again), so
/// a running named tunnel never loses its credentials to a failed attempt.
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let was_read_only = set_read_only(to, false)?;
    std::fs::rename(from, to).inspect_err(|_| {
        if was_read_only {
            let _ = set_read_only(to, true);
        }
    })
}

/// Sets or clears the read-only attribute of `path` if it exists; returns whether it was
/// read-only before (false for a missing file).
fn set_read_only(path: &Path, read_only: bool) -> std::io::Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        metadata => metadata?,
    };
    let mut permissions = metadata.permissions();
    let was = permissions.readonly();
    if was != read_only {
        #[allow(clippy::permissions_set_readonly_false)] // Windows: the attribute only
        permissions.set_readonly(read_only);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(was)
}

/// Removes `path`, also when it is read-only; a missing file is fine.
fn remove_file(path: &Path) -> std::io::Result<()> {
    set_read_only(path, false)?;
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `delete` failed because there is no such tunnel (and not because something else was not
/// found: then the files stay).
fn tunnel_missing(output: &str) -> bool {
    let output = output.to_ascii_lowercase();
    output.contains("neither the id nor the name")
        || output.contains("tunnel not found")
        || output.contains(&format!("tunnel {TUNNEL_NAME} not found"))
}

/// The tunnel id in a credentials file (read for that field only, never shown).
fn credentials_id(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("TunnelID")?
        .as_str()
        .filter(|id| is_tunnel_id(id))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.into_string().unwrap()).collect()
    }

    #[test]
    fn tunnel_ids_are_uuids() {
        assert!(is_tunnel_id("6ff42ae2-765d-4adf-8112-31c55c1551ef"));
        assert!(!is_tunnel_id("cert"));
        assert!(!is_tunnel_id("6ff42ae2-765d-4adf-8112-31c55c1551eZ"));
        assert!(!is_tunnel_id("6ff42ae2765d-4adf-8112-31c55c1551ef0"));
    }

    #[test]
    fn management_commands_never_update_and_name_the_certificate() {
        assert_eq!(
            strings(args(None, &["login"])),
            ["tunnel", "--no-autoupdate", "login"]
        );
        assert_eq!(
            strings(args(
                Some(Path::new(r"C:\h\tunnel\cert.pem")),
                &["delete", "-f", "vrcinema"]
            )),
            [
                "tunnel",
                "--no-autoupdate",
                "--origincert",
                r"C:\h\tunnel\cert.pem",
                "delete",
                "-f",
                "vrcinema"
            ]
        );
    }

    #[test]
    fn finds_the_login_page_and_nothing_else() {
        assert_eq!(
            login_url_in(
                "https://dash.cloudflare.com/argotunnel?aud=&callback=https%3A%2F%2Flogin.cloudflareaccess.org%2Fabc"
            )
            .as_deref(),
            Some(
                "https://dash.cloudflare.com/argotunnel?aud=&callback=https%3A%2F%2Flogin.cloudflareaccess.org%2Fabc"
            )
        );
        assert_eq!(
            login_url_in("  https://dash.cloudflare.com/argotunnel?x=1  ").as_deref(),
            Some("https://dash.cloudflare.com/argotunnel?x=1")
        );
        assert_eq!(
            login_url_in("Please open the following URL and log in with your Cloudflare account:"),
            None
        );
        assert_eq!(
            login_url_in("https://evil.example.com/cloudflare.com"),
            None
        );
        assert_eq!(login_url_in("https://cloudflare.com.evil.example/"), None);
        assert_eq!(login_url_in("http://dash.cloudflare.com/argotunnel"), None);
    }

    #[test]
    fn recognises_cloudflared_failures() {
        assert!(tunnel_exists(
            "failed to create tunnel: Create Tunnel API call failed: tunnel with name already exists"
        ));
        assert!(!tunnel_exists("Unauthorized: Invalid origin certificate"));
        assert!(dns_taken(
            "Failed to add route: code: 1003, reason: Failed to create record kanal.example.com with err An A, AAAA, or CNAME record with that host already exists."
        ));
        assert!(!dns_taken(
            "Failed to add route: code: 1000, reason: internal error"
        ));
        assert!(zone_missing("Failed to add route: zone not found"));
        assert!(!zone_missing(
            "Failed to add route: code: 1000, reason: internal error"
        ));
        assert!(tunnel_missing(
            "vrcinema is neither the ID nor the name of any of your tunnels"
        ));
        assert!(tunnel_missing("ERR tunnel vrcinema not found"));
        assert!(!tunnel_missing("Unauthorized"));
        // Something else not found is not a missing tunnel: the files must stay.
        for other in [
            "Cannot determine default origin certificate path. No file cert.pem found",
            "Error: TUNNEL_ORIGIN_CERT not found",
            "zone not found",
            "404 Not Found",
            "the system cannot find the file: credentials do not exist",
        ] {
            assert!(!tunnel_missing(other), "{other}");
        }
    }

    #[test]
    fn reads_the_record_from_every_dns_route_summary() {
        for (line, record) in [
            (
                "2026-10-07T10:00:01Z INF Added CNAME kanal.example.com.inna.pl which will route to this tunnel tunnelID=x",
                "kanal.example.com.inna.pl",
            ),
            (
                "2026-10-07T10:00:01Z INF kanal.example.com.inna.pl is already configured to route to your tunnel tunnelID=x",
                "kanal.example.com.inna.pl",
            ),
            (
                "2026-10-07T10:00:01Z INF kanal.example.com updated to route to your tunnel tunnelID=x",
                "kanal.example.com",
            ),
        ] {
            let output = format!("\n{line}\n");
            assert_eq!(routed_record(&output).as_deref(), Some(record), "{line}");
        }
        assert_eq!(routed_record("INF Starting\n"), None);
    }

    #[test]
    fn credentials_replace_a_read_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("6ff42ae2-765d-4adf-8112-31c55c1551ef.json");
        let new = dir.path().join("vrcinema-new.json");
        for (path, text) in [(&old, "stare"), (&new, "nowe")] {
            std::fs::write(path, text).unwrap();
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(path, permissions).unwrap();
        }
        replace_file(&new, &old).unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "nowe");
        assert!(!new.exists());
        // Nothing to replace.
        std::fs::write(&new, "jeszcze").unwrap();
        let fresh = dir.path().join("0aa42ae2-765d-4adf-8112-31c55c1551ef.json");
        replace_file(&new, &fresh).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "jeszcze");
        remove_file(&new).unwrap();
        remove_file(&old).unwrap();
        assert!(!old.exists());
    }

    /// A rename that fails (here: the new file is held open exclusively, like an antivirus scan
    /// can) keeps the old credentials — the running named tunnel needs them.
    #[cfg(windows)]
    #[test]
    fn a_failed_replace_keeps_the_old_credentials() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("6ff42ae2-765d-4adf-8112-31c55c1551ef.json");
        let new = dir.path().join("vrcinema-new.json");
        std::fs::write(&old, "stare").unwrap();
        let mut permissions = std::fs::metadata(&old).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&old, permissions).unwrap();
        std::fs::write(&new, "nowe").unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&new)
            .unwrap();
        assert!(replace_file(&new, &old).is_err());
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "stare");
        assert!(
            std::fs::metadata(&old).unwrap().permissions().readonly(),
            "still read-only"
        );
        drop(held);
        replace_file(&new, &old).unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "nowe");
    }

    #[test]
    fn the_credentials_id_is_read_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("c.json");
        std::fs::write(
            &file,
            r#"{"AccountTag":"a","TunnelSecret":"s","TunnelID":"6ff42ae2-765d-4adf-8112-31c55c1551ef"}"#,
        )
        .unwrap();
        assert_eq!(
            credentials_id(&file).as_deref(),
            Some("6ff42ae2-765d-4adf-8112-31c55c1551ef")
        );
        std::fs::write(&file, r#"{"TunnelID":"..\\x"}"#).unwrap();
        assert_eq!(credentials_id(&file), None);
        assert_eq!(credentials_id(&dir.path().join("brak.json")), None);
    }

    #[test]
    fn removing_the_files_keeps_other_things() {
        let dir = tempfile::tempdir().unwrap();
        let id = "6ff42ae2-765d-4adf-8112-31c55c1551ef";
        std::fs::write(dir.path().join(CERT_FILE), "c").unwrap();
        std::fs::write(dir.path().join(format!("{id}.json")), "{}").unwrap();
        std::fs::write(dir.path().join("vrcinema-new.json"), "{}").unwrap();
        std::fs::write(dir.path().join("notatka.txt"), "x").unwrap();
        // cloudflared writes credentials read-only.
        let credentials = dir.path().join(format!("{id}.json"));
        let mut permissions = std::fs::metadata(&credentials).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&credentials, permissions).unwrap();
        remove_files(dir.path()).unwrap();
        let left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left, ["notatka.txt"]);
        // A missing folder is nothing to remove.
        remove_files(&dir.path().join("brak")).unwrap();
    }

    #[test]
    fn one_operation_at_a_time_with_its_progress() {
        let setup = NamedSetup::default();
        assert_eq!(setup.progress().phase, Phase::Idle);
        let busy = setup.begin(Operation::Login).unwrap();
        let refused = setup.begin(Operation::Create).err().unwrap();
        assert_eq!(refused.code, ErrorCode::Busy);
        setup.set_login_url("https://dash.cloudflare.com/x".into());
        setup.step(Step::WaitingForBrowser, "Zaloguj się");
        let p = setup.progress();
        assert_eq!(
            (p.operation, p.phase, p.step),
            (
                Some(Operation::Login),
                Phase::Running,
                Some(Step::WaitingForBrowser)
            )
        );
        assert_eq!(
            p.login_url.as_deref(),
            Some("https://dash.cloudflare.com/x")
        );
        assert_eq!(p.browser_opened, None);
        setup.set_browser_opened(true);
        assert_eq!(setup.progress().browser_opened, Some(true));
        assert!(setup.hold_for_tools().is_none(), "a login runs");
        let (tx, mut rx) = oneshot::channel();
        setup.set_cancel(tx);
        assert!(setup.cancel_login());
        assert!(rx.try_recv().is_ok());
        setup.finish(&Err(NamedError::new(ErrorCode::Cancelled, "Anulowano")));
        drop(busy);
        let p = setup.progress();
        assert_eq!(
            (p.phase, p.code),
            (Phase::Cancelled, Some(ErrorCode::Cancelled))
        );
        assert_eq!((p.step, p.login_url, p.browser_opened), (None, None, None));
        assert!(!setup.cancel_login());
        let again = setup.begin(Operation::Create).unwrap();
        assert_eq!(setup.progress().phase, Phase::Running);
        assert_eq!(setup.progress().code, None);
        drop(again);
    }

    #[test]
    fn a_cloudflared_update_and_the_wizard_exclude_each_other() {
        let setup = NamedSetup::default();
        let held = setup.hold_for_tools().unwrap();
        let refused = setup.begin(Operation::Login).err().unwrap();
        assert_eq!(refused.code, ErrorCode::Busy);
        assert!(
            refused.message.contains("cloudflared"),
            "{}",
            refused.message
        );
        assert_eq!(setup.progress().phase, Phase::Idle, "nothing started");
        drop(held);
        let busy = setup.begin(Operation::Login).unwrap();
        assert!(setup.hold_for_tools().is_none());
        drop(busy);
        assert!(setup.hold_for_tools().is_some());
    }
}
