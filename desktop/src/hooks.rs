//! What the engine asks of the window shell: the cookies file picker, start with Windows,
//! the program file and the restart after an update.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use vrcinema::app::DesktopHooks;

pub struct ShellHooks {
    app: AppHandle,
    /// `VRCinema.exe` as started (an update renames the running file, so it is taken at start).
    exe: Option<PathBuf>,
}

impl ShellHooks {
    pub fn new(app: AppHandle, exe: Option<PathBuf>) -> Self {
        Self { app, exe }
    }
}

impl DesktopHooks for ShellHooks {
    fn pick_cookies_file(&self) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Wybierz plik cookies (format Netscape, np. cookies.txt)")
            .add_filter("Pliki cookies (*.txt)", &["txt"])
            .add_filter("Wszystkie pliki", &["*"]);
        if let Some(window) = self.app.get_webview_window(crate::window::MAIN) {
            dialog = dialog.set_parent(&window);
        }
        dialog.pick_file()
    }

    fn pick_slate_background(&self) -> Option<PathBuf> {
        // The formats the engine accepts (it checks the file itself again).
        let mut dialog = rfd::FileDialog::new()
            .add_filter(
                "PNG, JPEG, WebP, BMP, GIF, MP4, WebM, MOV, MKV",
                &[
                    "png", "jpg", "jpeg", "webp", "bmp", "gif", "mp4", "webm", "mov", "mkv",
                ],
            )
            .add_filter("*.*", &["*"]);
        if let Some(window) = self.app.get_webview_window(crate::window::MAIN) {
            dialog = dialog.set_parent(&window);
        }
        dialog.pick_file()
    }

    fn set_autostart(&self, on: bool) -> anyhow::Result<()> {
        let autolaunch = self.app.autolaunch();
        let result = if on {
            autolaunch.enable()
        } else if autolaunch.is_enabled().unwrap_or(true) {
            autolaunch.disable()
        } else {
            Ok(())
        };
        result.map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn exe_path(&self) -> Option<PathBuf> {
        self.exe.clone()
    }

    fn restart_after_update(&self) {
        if let Some(exe) = &self.exe {
            crate::quit(&self.app, Some(exe.clone()));
        }
    }
}
