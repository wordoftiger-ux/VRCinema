//! `VRCinema.exe`: the channel engine with a window, a tray icon and one instance per user.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hooks;
mod tray;
mod window;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use vrcinema::app::{AppEngine, AppOptions};
use semver::Version;
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::ManagerExt;

const WEBVIEW2_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
const ARG_AUTOSTART: &str = "--autostart";
const ARG_AFTER_UPDATE: &str = "--after-update";
/// Tests and the manual update check: where to ask for releases instead of GitHub.
const UPDATE_API_ENV: &str = "VRCINEMA_UPDATE_API_URL";
/// How many times to wait for the old program to go away after an update (each wait ~5 s).
const AFTER_UPDATE_WAITS: usize = 4;

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
        }
    }

    /// Runs `f` on the engine without waiting; `None` while quitting.
    pub fn with_engine<R>(&self, f: impl FnOnce(&AppEngine) -> R) -> Option<R> {
        let guard = self.engine.try_read().ok()?;
        guard.as_ref().map(f)
    }

    /// A line in the app's log (dropped while quitting).
    pub fn log(&self, msg: &str) {
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
}

/// Stops the engine (the channel and its processes), then ends the program; with `relaunch`
/// the updated program is started first.
pub fn quit(app: &AppHandle, relaunch: Option<PathBuf>) {
    let shell = app.state::<Shell>();
    if shell.quitting.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(w) = app.get_webview_window(window::MAIN) {
        let _ = w.hide();
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let engine = app.state::<Shell>().engine.write().await.take();
        if let Some(engine) = engine {
            engine.shutdown().await;
        }
        match relaunch {
            Some(exe) => {
                let handle = app.clone();
                let relaunched = app.run_on_main_thread(move || {
                    // On the thread that owns the single-instance mutex: release it, so the new
                    // program does not find this one "already running". `process::exit` skips
                    // the Exit event, which would release it a second time.
                    tauri_plugin_single_instance::destroy(&handle);
                    start_updated(&exe);
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

fn message_box(text: &str, buttons: rfd::MessageButtons) -> rfd::MessageDialogResult {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("VRCinema")
        .set_description(text)
        .set_buttons(buttons)
        .show()
}

fn error_box(text: &str) {
    message_box(text, rfd::MessageButtons::Ok);
}

fn webview2_missing() {
    let text = format!(
        "VRCinema potrzebuje składnika Microsoft Edge WebView2 Runtime, \
         którego nie ma na tym komputerze.\n\n\
         Pobierz go (wersja „Evergreen”) ze strony:\n{WEBVIEW2_URL}\n\n\
         Otworzyć teraz stronę pobierania?"
    );
    if message_box(&text, rfd::MessageButtons::YesNo) == rfd::MessageDialogResult::Yes {
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", WEBVIEW2_URL])
            .spawn();
    }
}

/// After an update: waits until the old program is gone (its renamed file can be deleted
/// only then) and removes it.
fn finish_update(exe: &Path) {
    let old = exe.with_file_name("VRCinema.old.exe");
    for _ in 0..AFTER_UPDATE_WAITS {
        tauri::async_runtime::block_on(vrcinema::self_update::cleanup_after_update(exe));
        if !old.exists() {
            return;
        }
    }
}

fn engine_options() -> AppOptions {
    let mut options = AppOptions {
        version: Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0)),
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
            error_box(&format!(
                "Nie udało się uruchomić VRCinema:\n{e:#}\n\nDziennik: {}",
                home.join("logs").display()
            ));
            std::process::exit(1);
        }
    };
    if let Some(e) = contain {
        engine.log(&format!(
            "Nie udało się objąć procesów potomnych obiektem zadania: {e}"
        ));
    }
    if args.after_update {
        engine.log("Uruchomiono po aktualizacji.");
    }
    // Keeps the autostart entry pointing at this file (the program may have been moved).
    if engine.settings().autostart
        && let Err(e) = app.autolaunch().enable()
    {
        engine.log(&format!("Nie udało się odświeżyć autostartu: {e}"));
    }
    let shell = app.state::<Shell>();
    // Setup runs on the main thread, outside the async runtime: blocking is allowed here.
    *shell.engine.blocking_write() = Some(engine);
    if let Err(e) = tray::create(app) {
        shell.log(&format!("Nie udało się utworzyć ikony w zasobniku: {e}"));
    }
    tray::spawn_tooltip_refresh(app.clone());
    window::spawn_port_follow(app.clone());
    if !args.autostart {
        window::open_panel(app);
    }
}

fn main() {
    let args = Args::parse(std::env::args().skip(1));
    let exe = std::env::current_exe().ok();
    if args.after_update
        && let Some(exe) = &exe
    {
        finish_update(exe);
    }
    if tauri::webview_version().is_err() {
        webview2_missing();
        std::process::exit(1);
    }
    let home = match vrcinema::home::app_home() {
        Ok(home) => home,
        Err(e) => {
            error_box(&format!(
                "VRCinema nie może zapisywać swoich danych:\n{e:#}\n\n\
                 Sprawdź uprawnienia do tego folderu lub ustaw inny w zmiennej VRCINEMA_HOME."
            ));
            std::process::exit(1);
        }
    };
    // Every process started from now on (ffmpeg, yt-dlp, WebView2) ends with this program.
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
            error_box(&format!("Nie udało się uruchomić okna VRCinema:\n{e}"));
            std::process::exit(1);
        }
    };
    app.run(|app, event| {
        // Only "Zakończ" (or an update) ends the program; closing the window hides it.
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

    #[test]
    fn the_program_reports_the_desktop_version() {
        assert_eq!(engine_options().version, Version::new(0, 2, 0));
    }
}
