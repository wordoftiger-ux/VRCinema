//! `VRCinema.exe`: the channel engine with a window, a tray icon and one instance per user.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hooks;
mod tray;
mod window;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use semver::Version;
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::ManagerExt;
use vrcinema::app::{AppEngine, AppOptions};
use vrcinema::app_settings::AppSettings;
use vrcinema::i18n::{Lang, Msg, error_msg, system_lang, t};

const WEBVIEW2_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
const ARG_AUTOSTART: &str = "--autostart";
const ARG_AFTER_UPDATE: &str = "--after-update";
/// Tests and the manual update check: where to ask for releases instead of GitHub.
const UPDATE_API_ENV: &str = "VRCINEMA_UPDATE_API_URL";
/// How many times to wait for the old program to go away after an update (each wait ~5 s).
const AFTER_UPDATE_WAITS: usize = 4;
/// Longest wait for the engine when quitting (a folder drop in progress, a restart).
const QUIT_TIMEOUT: Duration = Duration::from_secs(20);
/// Extra time for the window shell to end before the process is ended outright.
const QUIT_GRACE: Duration = Duration::from_secs(10);

/// Command line of `VRCinema.exe`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Args {
    /// Started with Windows: stay in the tray, no window.
    pub autostart: bool,
    /// Started by the previous version after it replaced the program file.
    pub after_update: bool,
}

impl Args {
    /// Parses the arguments after the program name; unknown ones are ignored.
    pub fn parse<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut parsed = Self::default();
        for arg in args {
            match arg.as_ref() {
                ARG_AUTOSTART => parsed.autostart = true,
                ARG_AFTER_UPDATE => parsed.after_update = true,
                _ => {}
            }
        }
        parsed
    }
}

/// State of the window shell, shared by the window, the tray and the hooks.
pub struct Shell {
    /// `None` before the start and once quitting began.
    pub engine: tokio::sync::RwLock<Option<AppEngine>>,
    quitting: AtomicBool,
    /// The "still running in the background" balloon was shown.
    pub hidden_notice_shown: AtomicBool,
    /// The port of the server the window was last pointed at.
    window_port: Mutex<Option<u16>>,
    /// The engine's ports whose page may use the window commands (see `window`).
    controls_ports: Mutex<BTreeSet<u16>>,
    /// The app's data folder (`%LOCALAPPDATA%\VRCinema` or `VRCINEMA_HOME`).
    pub home: PathBuf,
}

impl Shell {
    fn new(home: PathBuf) -> Self {
        Self {
            home,
            engine: tokio::sync::RwLock::new(None),
            quitting: AtomicBool::new(false),
            hidden_notice_shown: AtomicBool::new(false),
            window_port: Mutex::new(None),
            controls_ports: Mutex::new(BTreeSet::new()),
        }
    }

    /// Runs `f` on the engine without waiting; `None` while quitting.
    pub fn with_engine<R>(&self, f: impl FnOnce(&AppEngine) -> R) -> Option<R> {
        let guard = self.engine.try_read().ok()?;
        guard.as_ref().map(f)
    }

    /// A line in the app's log (dropped while quitting).
    pub fn log(&self, msg: Msg) {
        self.with_engine(|e| e.log(msg));
    }

    pub fn quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    pub fn window_port(&self) -> MutexGuard<'_, Option<u16>> {
        self.window_port
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn controls_ports(&self) -> MutexGuard<'_, BTreeSet<u16>> {
        self.controls_ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Stops the engine (the channel and its processes), then ends the program; with `relaunch`
/// the updated program is started first.
pub fn quit(app: &AppHandle, relaunch: Option<PathBuf>) {
    let shell = app.state::<Shell>();
    if shell.quitting.swap(true, Ordering::SeqCst) {
        return;
    }
    // A move just before quitting may not be saved yet.
    window::save_place(app);
    if let Some(w) = app.get_webview_window(window::MAIN) {
        let _ = w.hide();
    }
    // The window is hidden and second launches only poke this process: it must end. Should
    // the orderly way below get stuck anywhere, this ends it (the job object takes ffmpeg and
    // yt-dlp along) — after starting the updated program, if that was the point.
    let started_new = Arc::new(AtomicBool::new(false));
    {
        let relaunch = relaunch.clone();
        let started_new = Arc::clone(&started_new);
        spawn_watchdog(QUIT_TIMEOUT + QUIT_GRACE, move || {
            if let Some(exe) = relaunch
                && !started_new.swap(true, Ordering::SeqCst)
            {
                start_updated(&exe);
            }
            std::process::exit(0);
        });
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shell = app.state::<Shell>();
        let stopped = stop_engine(&shell.engine, QUIT_TIMEOUT, |engine, left| {
            engine.shutdown_within(left)
        })
        .await;
        if !stopped {
            // The log is closed with the engine (or the engine is still busy): stderr only.
            eprintln!("{}", t(startup_lang(&shell.home), "app.stop_too_slow", &[]));
        }
        match relaunch {
            Some(exe) => {
                let handle = app.clone();
                let relaunched = app.run_on_main_thread(move || {
                    // On the thread that owns the single-instance mutex: release it, so the new
                    // program does not find this one "already running". `process::exit` skips
                    // the Exit event, which would release it a second time.
                    tauri_plugin_single_instance::destroy(&handle);
                    if !started_new.swap(true, Ordering::SeqCst) {
                        start_updated(&exe);
                    }
                    handle.cleanup_before_exit();
                    std::process::exit(0);
                });
                if relaunched.is_err() {
                    app.exit(0);
                }
            }
            None => app.exit(0),
        }
    });
}

/// Starts the new program outside this program's job object (which kills its members when
/// this process ends).
fn start_updated(exe: &Path) {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(ARG_AFTER_UPDATE);
    spawn_outside_job(cmd);
}

/// Starts `cmd` outside this program's job object when possible (it must outlive us: the
/// updated program, a browser started for a link), else as a plain child.
fn spawn_outside_job(mut cmd: std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return;
        }
        cmd.creation_flags(0);
    }
    let _ = cmd.spawn();
}

/// Opens an http(s) address in the user's default browser (Linux: `xdg-open`).
pub fn open_in_browser(url: &str) {
    let mut cmd = if cfg!(windows) {
        let mut cmd = std::process::Command::new("rundll32");
        cmd.arg("url.dll,FileProtocolHandler");
        cmd
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url);
    spawn_outside_job(cmd);
}

fn message_box(text: &str, buttons: rfd::MessageButtons) -> rfd::MessageDialogResult {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("VRCinema")
        .set_description(text)
        .set_buttons(buttons)
        .show()
}
/// The language for texts shown before the engine is up (or after it stopped): the saved one,
/// else the language of Windows.
fn startup_lang(home: &std::path::Path) -> Lang {
    AppSettings::saved_language(home).unwrap_or_else(system_lang)
}

fn error_box(text: &str) {
    message_box(text, rfd::MessageButtons::Ok);
}

fn webview2_missing(lang: Lang) {
    let text = t(lang, "app.webview2_missing", &[("url", WEBVIEW2_URL)]);
    if message_box(&text, rfd::MessageButtons::YesNo) == rfd::MessageDialogResult::Yes {
        open_in_browser(WEBVIEW2_URL);
    }
}

/// Takes the engine out of `slot` and stops it with `stop(engine, time left)`, all within
/// `limit`. False when it gave up (the slot stayed locked, e.g. by a long folder drop, or the
/// engine did not stop in time): the program then ends anyway.
pub async fn stop_engine<E, F, Fut>(
    slot: &tokio::sync::RwLock<Option<E>>,
    limit: Duration,
    stop: F,
) -> bool
where
    F: FnOnce(E, Duration) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + limit;
    let Ok(mut guard) = tokio::time::timeout_at(deadline, slot.write()).await else {
        return false;
    };
    let Some(engine) = guard.take() else {
        return true;
    };
    drop(guard);
    let left = deadline.saturating_duration_since(tokio::time::Instant::now());
    tokio::time::timeout_at(deadline, stop(engine, left))
        .await
        .unwrap_or(false)
}

/// Runs `on_expire` after `after` on its own thread, unless the process ended before.
fn spawn_watchdog(after: Duration, on_expire: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(after);
        on_expire();
    });
}

/// Removes `VRCinema.old.exe` left by an update, in the background (it may take a while for
/// the old program to go away).
fn spawn_update_cleanup(exe: PathBuf) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || finish_update(&exe))
}

/// After an update: waits until the old program is gone (its renamed file can be deleted
/// only then) and removes it.
fn finish_update(exe: &Path) {
    let leftover = vrcinema::self_update::leftover_path(exe);
    for _ in 0..AFTER_UPDATE_WAITS {
        tauri::async_runtime::block_on(vrcinema::self_update::cleanup_after_update(exe));
        if !leftover.exists() {
            return;
        }
    }
}

fn engine_options() -> AppOptions {
    let mut options = AppOptions {
        version: Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0)),
        // Releases carry only the Windows program.
        self_update: cfg!(windows),
        ..AppOptions::default()
    };
    if let Some(url) = std::env::var_os(UPDATE_API_ENV).and_then(|v| v.into_string().ok())
        && !url.is_empty()
    {
        options.update_api_url = url;
    }
    options
}

/// Starts the engine and shows the window (unless started with Windows). Runs in Tauri's
/// setup, once the plugins (the single-instance check among them) are in place.
fn start(
    app: &AppHandle,
    home: PathBuf,
    exe: Option<PathBuf>,
    args: &Args,
    contain: Option<String>,
) {
    let hooks = Arc::new(hooks::ShellHooks::new(app.clone(), exe));
    let engine = match tauri::async_runtime::block_on(AppEngine::start_with(
        home.clone(),
        hooks,
        engine_options(),
    )) {
        Ok(engine) => engine,
        Err(e) => {
            let lang = startup_lang(&home);
            error_box(&t(
                lang,
                "app.start_failed",
                &[
                    ("error", &error_msg(&e).render(lang)),
                    ("path", &home.join("logs").display().to_string()),
                ],
            ));
            std::process::exit(1);
        }
    };
    if let Some(e) = contain {
        engine.log(Msg::new("log.job_containment_failed").arg("error", e));
    }
    if args.after_update {
        engine.log(Msg::new("log.started_after_update"));
    }
    // Keeps the autostart entry pointing at this file (the program may have been moved).
    if engine.settings().autostart
        && let Err(e) = app.autolaunch().enable()
    {
        engine.log(Msg::new("log.autostart_refresh_failed").arg("error", e));
    }
    let shell = app.state::<Shell>();
    // Setup runs on the main thread, outside the async runtime: blocking is allowed here.
    *shell.engine.blocking_write() = Some(engine);
    let lang = shell.with_engine(|e| e.language()).unwrap_or_default();
    if let Err(e) = tray::create(app, lang) {
        shell.log(Msg::new("log.tray_create_failed").arg("error", e));
    }
    tray::spawn_tooltip_refresh(app.clone(), lang);
    window::spawn_port_follow(app.clone());
    if !args.autostart {
        window::open_panel(app);
    }
}

fn main() {
    let args = Args::parse(std::env::args().skip(1));
    // The program file an update replaces: Windows only (releases carry only `VRCinema.exe`).
    let exe = std::env::current_exe().ok().filter(|_| cfg!(windows));
    // Also on a normal start: a previous cleanup may have failed (the old program was still
    // running then). Never delays the start.
    if let Some(exe) = &exe {
        spawn_update_cleanup(exe.clone());
    }
    // WebView2 is a separate install only on Windows (Linux: WebKitGTK comes with the package).
    if cfg!(windows) && tauri::webview_version().is_err() {
        webview2_missing(system_lang());
        std::process::exit(1);
    }
    let home = match vrcinema::home::app_home() {
        Ok(home) => home,
        Err(e) => {
            let lang = system_lang();
            error_box(&t(
                lang,
                "app.home_unwritable",
                &[("error", &error_msg(&e).render(lang))],
            ));
            std::process::exit(1);
        }
    };
    // Every process started from now on (ffmpeg, yt-dlp, WebView2) ends with this program.
    let lang = startup_lang(&home);
    let contain = vrcinema::process::contain_this_process()
        .err()
        .map(|e| e.to_string());

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !Args::parse(argv.iter().skip(1)).autostart {
                window::open_panel(app);
            }
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .arg(ARG_AUTOSTART)
                .build(),
        )
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(Shell::new(home.clone()))
        .setup(move |app| {
            start(app.handle(), home, exe, &args, contain);
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(e) => {
            error_box(&t(lang, "app.window_failed", &[("error", &e.to_string())]));
            std::process::exit(1);
        }
    };
    app.run(|app, event| {
        // Only "Quit" (or an update) ends the program; closing the window hides it.
        if let RunEvent::ExitRequested { api, .. } = event
            && !app.state::<Shell>().quitting()
        {
            api.prevent_exit();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_mean_a_normal_start() {
        assert_eq!(Args::parse(Vec::<String>::new()), Args::default());
    }

    #[test]
    fn autostart_and_after_update_are_recognised() {
        assert_eq!(
            Args::parse(["--autostart"]),
            Args {
                autostart: true,
                after_update: false
            }
        );
        assert_eq!(
            Args::parse(["--after-update", "--autostart"]),
            Args {
                autostart: true,
                after_update: true
            }
        );
    }

    #[test]
    fn unknown_arguments_are_ignored() {
        assert_eq!(
            Args::parse(["--cos", "C:\\Filmy\\a.mp4", "--after-update"]),
            Args {
                autostart: false,
                after_update: true
            }
        );
        assert_eq!(Args::parse(["--AUTOSTART", "autostart"]), Args::default());
    }

    /// A folder drop holds the engine (read lock) for as long as it takes: quitting must not
    /// wait for it without end.
    #[test]
    fn quitting_gives_up_on_an_engine_held_by_a_long_drop() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let slot = Arc::new(tokio::sync::RwLock::new(Some("engine")));
            let held = Arc::clone(&slot).read_owned().await;
            let started = std::time::Instant::now();
            let stopped = tokio::time::timeout(
                Duration::from_secs(5),
                stop_engine(&slot, Duration::from_millis(300), |_, _| async { true }),
            )
            .await
            .expect("quitting waited for the drop");
            assert!(!stopped);
            assert!(started.elapsed() < Duration::from_secs(2));
            drop(held);
        });
    }

    #[test]
    fn quitting_gives_up_on_an_engine_that_does_not_stop() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let slot = tokio::sync::RwLock::new(Some("engine"));
            let stopped = tokio::time::timeout(
                Duration::from_secs(5),
                stop_engine(&slot, Duration::from_millis(300), |_, _| {
                    std::future::pending::<bool>()
                }),
            )
            .await
            .expect("quitting waited for the engine");
            assert!(!stopped);
            assert!(slot.read().await.is_none(), "the engine was taken out");
        });
    }

    #[test]
    fn quitting_stops_the_engine_with_the_time_left() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let slot = tokio::sync::RwLock::new(Some("engine"));
            let mut given = None;
            let stopped = stop_engine(&slot, Duration::from_secs(20), |e, left| {
                given = Some((e, left));
                async { true }
            })
            .await;
            assert!(stopped);
            let (engine, left) = given.unwrap();
            assert_eq!(engine, "engine");
            assert!(left <= Duration::from_secs(20) && left > Duration::from_secs(19));
        });
    }

    #[test]
    fn the_watchdog_fires_after_its_delay() {
        let (tx, rx) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        spawn_watchdog(Duration::from_millis(200), move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the watchdog did not fire");
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn a_leftover_old_program_is_removed_on_any_start() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("VRCinema.exe");
        std::fs::write(&exe, b"new").unwrap();
        std::fs::write(tmp.path().join("VRCinema.old.exe"), b"old").unwrap();
        spawn_update_cleanup(exe.clone()).join().unwrap();
        assert!(!tmp.path().join("VRCinema.old.exe").exists());
        assert!(exe.exists());
    }

    #[test]
    fn the_program_reports_the_desktop_version() {
        assert_eq!(engine_options().version, Version::new(0, 3, 0));
    }
}
