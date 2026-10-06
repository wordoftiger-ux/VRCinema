//! The tray icon: its menu, the "now playing" tooltip, the copied links and the one-time
//! balloon shown when the window is first hidden.

use std::time::Duration;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::Shell;

const TRAY_ID: &str = "main";
const TOOLTIP_REFRESH: Duration = Duration::from_secs(5);
/// Windows keeps 128 UTF-16 units of a tray tooltip, the terminating zero included.
const TOOLTIP_MAX_UNITS: usize = 127;

/// What a tray menu entry does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenPanel,
    CopyChannelLink,
    CopySubmitLink,
    Skip,
    Quit,
}

impl TrayAction {
    /// Menu order.
    pub const ALL: [TrayAction; 5] = [
        TrayAction::OpenPanel,
        TrayAction::CopyChannelLink,
        TrayAction::CopySubmitLink,
        TrayAction::Skip,
        TrayAction::Quit,
    ];

    pub fn id(self) -> &'static str {
        match self {
            TrayAction::OpenPanel => "open",
            TrayAction::CopyChannelLink => "copy-channel",
            TrayAction::CopySubmitLink => "copy-submit",
            TrayAction::Skip => "skip",
            TrayAction::Quit => "quit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TrayAction::OpenPanel => "Otwórz panel",
            TrayAction::CopyChannelLink => "Kopiuj link kanału",
            TrayAction::CopySubmitLink => "Kopiuj link strony zgłoszeń",
            TrayAction::Skip => "Pomiń film",
            TrayAction::Quit => "Zakończ",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// Address the links start with: the public address when set, else the local server.
pub fn base_url(public_url: &str, port: u16) -> String {
    let public = public_url.trim().trim_end_matches('/');
    if public.is_empty() {
        format!("http://127.0.0.1:{port}")
    } else {
        public.to_string()
    }
}

/// The channel link for VRChat (`/live/main.m3u8`).
pub fn channel_link(public_url: &str, port: u16) -> String {
    format!("{}/live/main.m3u8", base_url(public_url, port))
}

/// The submission page (`/`).
pub fn submit_link(public_url: &str, port: u16) -> String {
    format!("{}/", base_url(public_url, port))
}

/// Tooltip of the tray icon, short enough for Windows (a long title is cut with "…").
pub fn tooltip_text(now_playing: Option<&str>) -> String {
    let Some(title) = now_playing.map(str::trim).filter(|t| !t.is_empty()) else {
        return "VRCinema — przerwa".to_string();
    };
    let text = format!("VRCinema — teraz leci: {title}");
    if text.encode_utf16().count() <= TOOLTIP_MAX_UNITS {
        return text;
    }
    let mut cut = String::new();
    let mut units = 1; // the final "…"
    for c in text.chars() {
        units += c.len_utf16();
        if units > TOOLTIP_MAX_UNITS {
            break;
        }
        cut.push(c);
    }
    cut.push('…');
    cut
}

/// Creates the tray icon with its menu; left click opens the panel.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::new(app)?;
    for action in TrayAction::ALL {
        if action == TrayAction::Quit {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        }
        menu.append(&MenuItem::with_id(
            app,
            action.id(),
            action.label(),
            true,
            None::<&str>,
        )?)?;
    }
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip_text(None))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::window::open_panel(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let Some(action) = TrayAction::from_id(event.id().as_ref()) else {
        return;
    };
    let shell = app.state::<Shell>();
    match action {
        TrayAction::OpenPanel => crate::window::open_panel(app),
        TrayAction::CopyChannelLink | TrayAction::CopySubmitLink => {
            let Some((public_url, port)) =
                shell.with_engine(|e| (e.settings().public_url, e.port()))
            else {
                return;
            };
            let link = if action == TrayAction::CopyChannelLink {
                channel_link(&public_url, port)
            } else {
                submit_link(&public_url, port)
            };
            if let Err(e) = app.clipboard().write_text(link) {
                shell.log(&format!("Nie udało się skopiować linku: {e}"));
            }
        }
        TrayAction::Skip => {
            shell.with_engine(|e| e.skip());
        }
        TrayAction::Quit => crate::quit(app, None),
    }
}

/// Refreshes the tooltip with the film on air every few seconds, for the life of the app.
pub fn spawn_tooltip_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last = String::new();
        loop {
            let text = {
                let shell = app.state::<Shell>();
                let engine = shell.engine.read().await;
                match engine.as_ref() {
                    Some(engine) => tooltip_text(engine.now_playing().as_deref()),
                    None => return, // quitting
                }
            };
            if text != last
                && let Some(tray) = app.tray_by_id(TRAY_ID)
                && tray.set_tooltip(Some(&text)).is_ok()
            {
                last = text;
            }
            tokio::time::sleep(TOOLTIP_REFRESH).await;
        }
    });
}

/// A balloon from the tray icon (Windows shows it as a notification).
pub fn show_balloon(title: &str, text: &str) {
    #[cfg(windows)]
    balloon::show(title, text);
    #[cfg(not(windows))]
    let _ = (title, text);
}

/// Tauri does not expose balloons of its tray icon, so this finds the icon's window (class
/// `tray_icon_app` of the `tray-icon` crate, in this process) and adds the balloon with
/// `Shell_NotifyIconW(NIM_MODIFY)`. Best effort: nothing happens when it is not found.
#[cfg(windows)]
mod balloon {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::Shell::{
        NIF_INFO, NIIF_INFO, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId,
    };
    use windows_sys::core::BOOL;

    const CLASS: &str = "tray_icon_app";
    /// `tray-icon` numbers its icons from 1; the app has one, a few spare tries cost nothing.
    const MAX_ICON_ID: u32 = 8;

    fn copy_into(dst: &mut [u16], text: &str) {
        let units: Vec<u16> = text.encode_utf16().take(dst.len() - 1).collect();
        dst[..units.len()].copy_from_slice(&units);
        dst[units.len()] = 0;
    }

    unsafe extern "system" fn find(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0u32;
        // SAFETY: `hwnd` comes from EnumWindows; `found` points at the caller's `HWND`.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid != GetCurrentProcessId() {
                return 1;
            }
            let mut name = [0u16; 64];
            let len = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32);
            if len > 0 && String::from_utf16_lossy(&name[..len as usize]) == CLASS {
                *(found as *mut HWND) = hwnd;
                return 0;
            }
        }
        1
    }

    pub fn show(title: &str, text: &str) {
        let mut hwnd: HWND = std::ptr::null_mut();
        // SAFETY: the callback writes only through the pointer to `hwnd`, which outlives the call.
        unsafe { EnumWindows(Some(find), &mut hwnd as *mut HWND as LPARAM) };
        if hwnd.is_null() {
            return;
        }
        // SAFETY: plain struct of integers and arrays; all-zero is a valid value.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        copy_into(&mut data.szInfoTitle, title);
        copy_into(&mut data.szInfo, text);
        for id in 1..=MAX_ICON_ID {
            data.uID = id;
            // SAFETY: `data` is fully initialised and lives for the call.
            if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } != 0 {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_back_to_their_actions() {
        for action in TrayAction::ALL {
            assert_eq!(TrayAction::from_id(action.id()), Some(action));
        }
        assert_eq!(TrayAction::from_id("cos-innego"), None);
        assert_eq!(TrayAction::from_id(""), None);
    }

    #[test]
    fn menu_labels_are_the_polish_ones_in_order() {
        let labels: Vec<_> = TrayAction::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(
            labels,
            [
                "Otwórz panel",
                "Kopiuj link kanału",
                "Kopiuj link strony zgłoszeń",
                "Pomiń film",
                "Zakończ"
            ]
        );
    }

    #[test]
    fn links_use_the_local_server_without_a_public_address() {
        assert_eq!(
            channel_link("", 8181),
            "http://127.0.0.1:8181/live/main.m3u8"
        );
        assert_eq!(submit_link("  ", 8181), "http://127.0.0.1:8181/");
    }

    #[test]
    fn links_use_the_public_address_when_set() {
        assert_eq!(
            channel_link("https://tv.example.com/", 8181),
            "https://tv.example.com/live/main.m3u8"
        );
        assert_eq!(
            submit_link(" https://tv.example.com ", 8181),
            "https://tv.example.com/"
        );
    }

    #[test]
    fn tooltip_shows_the_film_or_the_break() {
        assert_eq!(tooltip_text(None), "VRCinema — przerwa");
        assert_eq!(tooltip_text(Some("")), "VRCinema — przerwa");
        assert_eq!(
            tooltip_text(Some("Odcinek 1")),
            "VRCinema — teraz leci: Odcinek 1"
        );
    }

    #[test]
    fn long_titles_are_cut_to_fit_windows() {
        let title = "ż".repeat(300);
        let text = tooltip_text(Some(&title));
        assert_eq!(text.encode_utf16().count(), TOOLTIP_MAX_UNITS);
        assert!(text.starts_with("VRCinema — teraz leci: żż"));
        assert!(text.ends_with('…'));
        // Characters outside the BMP (two UTF-16 units) are never split.
        let emoji = "🎬".repeat(100);
        let text = tooltip_text(Some(&emoji));
        assert!(text.encode_utf16().count() <= TOOLTIP_MAX_UNITS);
        assert!(text.ends_with('…'));
    }
}
