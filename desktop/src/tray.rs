//! The tray icon: its menu, the "now playing" tooltip, the copied links and the one-time
//! balloon shown when the window is first hidden. Every text comes from the dictionaries in the
//! app's language; a change of the `language` setting rebuilds the menu and the tooltip at once.

use std::time::Duration;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

use vrcinema::i18n::{Lang, Msg, t};
use vrcinema::tunnel::TunnelState;

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

    /// The dictionary key of the entry's label.
    pub fn key(self) -> &'static str {
        match self {
            TrayAction::OpenPanel => "tray.open_panel",
            TrayAction::CopyChannelLink => "tray.copy_channel_link",
            TrayAction::CopySubmitLink => "tray.copy_submit_link",
            TrayAction::Skip => "tray.skip",
            TrayAction::Quit => "tray.quit",
        }
    }

    pub fn label(self, lang: Lang) -> String {
        t(lang, self.key(), &[])
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

/// Whether the channel is reachable from the internet, as the tray tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Net {
    Online,
    Connecting,
    Offline,
    /// The tunnel is switched off and an address was typed in by hand.
    Manual,
}

impl Net {
    fn key(self) -> &'static str {
        match self {
            Net::Online => "tray.net_online",
            Net::Connecting => "tray.net_connecting",
            Net::Offline => "tray.net_offline",
            Net::Manual => "tray.net_manual",
        }
    }

    fn text(self, lang: Lang) -> String {
        t(lang, self.key(), &[])
    }
}

/// The tray's view of the connection: `None` when there is nothing to say (tunnel switched off,
/// no manual address). A tunnel that is enabled but still `Off` has just been switched on: it is
/// connecting.
pub fn net_status(tunnel_enabled: bool, state: &TunnelState, public_url: &str) -> Option<Net> {
    if !tunnel_enabled {
        return (!public_url.trim().is_empty()).then_some(Net::Manual);
    }
    Some(match state {
        TunnelState::Online { .. } => Net::Online,
        TunnelState::Connecting | TunnelState::Off => Net::Connecting,
        TunnelState::Reconnecting | TunnelState::Error { .. } => Net::Offline,
    })
}

/// Tooltip of the tray icon, short enough for Windows (a long title is cut with "…").
pub fn tooltip_text(lang: Lang, net: Option<Net>, now_playing: Option<&str>) -> String {
    let head = match net {
        Some(net) => t(lang, "tray.tooltip_head", &[("net", &net.text(lang))]),
        None => "VRCinema".to_string(),
    };
    let Some(title) = now_playing.map(str::trim).filter(|t| !t.is_empty()) else {
        return t(lang, "tray.tooltip_idle", &[("head", &head)]);
    };
    let text = t(
        lang,
        "tray.tooltip_playing",
        &[("head", &head), ("title", title)],
    );
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

/// What "Copy channel link" and "Copy request page link" copy: the link at the address in
/// effect (the tunnel's or the manual one, else the local server); with the tunnel switched on
/// but without any address (not online), a note that the link works only on this computer.
pub fn copied_link(lang: Lang, link: String, tunnel_enabled: bool, public_url: &str) -> String {
    if tunnel_enabled && public_url.trim().is_empty() {
        t(lang, "tray.link_without_internet", &[("link", &link)])
    } else {
        link
    }
}

/// The tray menu in `lang`.
fn build_menu(app: &AppHandle, lang: Lang) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for action in TrayAction::ALL {
        if action == TrayAction::Quit {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        }
        menu.append(&MenuItem::with_id(
            app,
            action.id(),
            action.label(lang),
            true,
            None::<&str>,
        )?)?;
    }
    Ok(menu)
}

/// Creates the tray icon with its menu in `lang`; left click opens the panel.
pub fn create(app: &AppHandle, lang: Lang) -> tauri::Result<()> {
    let menu = build_menu(app, lang)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip_text(lang, None, None))
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

/// Puts a new menu in `lang` on the tray icon (the tooltip follows on the next refresh).
fn rebuild_menu(app: &AppHandle, lang: Lang) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    tray.set_menu(Some(build_menu(app, lang)?))
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let Some(action) = TrayAction::from_id(event.id().as_ref()) else {
        return;
    };
    let shell = app.state::<Shell>();
    match action {
        TrayAction::OpenPanel => crate::window::open_panel(app),
        TrayAction::CopyChannelLink | TrayAction::CopySubmitLink => {
            let Some((public_url, port, tunnel_enabled, lang)) = shell.with_engine(|e| {
                let s = e.settings();
                (e.public_url(), e.port(), s.tunnel_enabled, s.language)
            }) else {
                return;
            };
            let link = if action == TrayAction::CopyChannelLink {
                channel_link(&public_url, port)
            } else {
                submit_link(&public_url, port)
            };
            let link = copied_link(lang, link, tunnel_enabled, &public_url);
            if let Err(e) = app.clipboard().write_text(link) {
                shell.log(Msg::new("log.tray_copy_failed").arg("error", e));
            }
        }
        TrayAction::Skip => {
            shell.with_engine(|e| e.skip());
        }
        TrayAction::Quit => crate::quit(app, None),
    }
}

/// Keeps the tray in step with the app for the life of the app: the tooltip with the film on
/// air every few seconds, and — at once — the menu and the tooltip when the language changes.
pub fn spawn_tooltip_refresh(app: AppHandle, created_in: Lang) {
    tauri::async_runtime::spawn(async move {
        let mut last = String::new();
        let mut shown_lang = Some(created_in);
        let mut changes = {
            let shell = app.state::<Shell>();
            let engine = shell.engine.read().await;
            match engine.as_ref() {
                Some(engine) => engine.language_changes(),
                None => return, // quitting
            }
        };
        loop {
            let (lang, text) = {
                let shell = app.state::<Shell>();
                let engine = shell.engine.read().await;
                match engine.as_ref() {
                    Some(engine) => {
                        let lang = engine.language();
                        let text = tooltip_text(
                            lang,
                            net_status(
                                engine.settings().tunnel_enabled,
                                &engine.tunnel_state(),
                                &engine.public_url(),
                            ),
                            engine.now_playing().as_deref(),
                        );
                        (lang, text)
                    }
                    None => return, // quitting
                }
            };
            if shown_lang.is_some_and(|shown| shown != lang) {
                // Only a change rebuilds the menu: the first pass shows what `create` made.
                if let Err(e) = rebuild_menu(&app, lang) {
                    app.state::<Shell>()
                        .log(Msg::new("log.tray_menu_failed").arg("error", e));
                }
            }
            shown_lang = Some(lang);
            if text != last
                && let Some(tray) = app.tray_by_id(TRAY_ID)
                && tray.set_tooltip(Some(&text)).is_ok()
            {
                last = text;
            }
            tokio::select! {
                () = tokio::time::sleep(TOOLTIP_REFRESH) => {}
                changed = changes.changed() => {
                    if changed.is_err() {
                        return; // the engine is gone
                    }
                }
            }
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

    const PL: Lang = Lang::Pl;
    const EN: Lang = Lang::En;

    #[test]
    fn menu_ids_map_back_to_their_actions() {
        for action in TrayAction::ALL {
            assert_eq!(TrayAction::from_id(action.id()), Some(action));
        }
        assert_eq!(TrayAction::from_id("cos-innego"), None);
        assert_eq!(TrayAction::from_id(""), None);
    }

    fn labels(lang: Lang) -> Vec<String> {
        TrayAction::ALL.iter().map(|a| a.label(lang)).collect()
    }

    #[test]
    fn menu_labels_come_from_the_keys_in_each_language_in_order() {
        assert_eq!(
            labels(PL),
            [
                "Otwórz panel",
                "Kopiuj link kanału",
                "Kopiuj link strony zgłoszeń",
                "Pomiń film",
                "Zakończ"
            ]
        );
        assert_eq!(
            labels(EN),
            [
                "Open panel",
                "Copy channel link",
                "Copy request page link",
                "Skip video",
                "Quit"
            ]
        );
        // Every language shows a real text for every entry (a missing key would show the key).
        for lang in Lang::ALL {
            for (action, label) in TrayAction::ALL.iter().zip(labels(lang)) {
                assert_ne!(label, action.key(), "{lang:?}");
                assert!(!label.is_empty());
            }
        }
    }

    #[test]
    fn every_text_of_the_tray_exists_in_english_and_polish() {
        for lang in [EN, PL] {
            for key in [
                "tray.net_online",
                "tray.net_connecting",
                "tray.net_offline",
                "tray.net_manual",
                "tray.tooltip_head",
                "tray.tooltip_idle",
                "tray.tooltip_playing",
                "tray.link_without_internet",
                "tray.hidden_title",
                "tray.hidden_text",
            ] {
                assert_ne!(t(lang, key, &[]), key, "{lang:?} {key}");
            }
        }
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
    fn tooltip_shows_the_film_or_the_break_in_the_language_of_the_app() {
        assert_eq!(tooltip_text(PL, None, None), "VRCinema — przerwa");
        assert_eq!(tooltip_text(PL, None, Some("")), "VRCinema — przerwa");
        assert_eq!(
            tooltip_text(PL, None, Some("Odcinek 1")),
            "VRCinema — teraz leci: Odcinek 1"
        );
        assert_eq!(tooltip_text(EN, None, None), "VRCinema — break");
        assert_eq!(
            tooltip_text(EN, None, Some("Episode 1")),
            "VRCinema — now playing: Episode 1"
        );
    }

    #[test]
    fn long_titles_are_cut_to_fit_windows() {
        let title = "ż".repeat(300);
        for (lang, start) in [
            (PL, "VRCinema — W sieci — teraz leci: żż"),
            (EN, "VRCinema — Online — now playing: żż"),
        ] {
            let text = tooltip_text(lang, Some(Net::Online), Some(&title));
            assert_eq!(text.encode_utf16().count(), TOOLTIP_MAX_UNITS);
            assert!(text.starts_with(start), "{text}");
            assert!(text.ends_with('…'));
        }
        // Characters outside the BMP (two UTF-16 units) are never split.
        let emoji = "🎬".repeat(100);
        let text = tooltip_text(PL, Some(Net::Offline), Some(&emoji));
        assert!(text.encode_utf16().count() <= TOOLTIP_MAX_UNITS);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn tooltip_tells_whether_the_channel_is_online() {
        assert_eq!(
            tooltip_text(PL, Some(Net::Online), None),
            "VRCinema — W sieci — przerwa"
        );
        assert_eq!(
            tooltip_text(PL, Some(Net::Offline), Some("Odcinek 1")),
            "VRCinema — Brak internetu — teraz leci: Odcinek 1"
        );
        assert_eq!(
            tooltip_text(PL, Some(Net::Connecting), None),
            "VRCinema — Łączenie z internetem — przerwa"
        );
        assert_eq!(
            tooltip_text(EN, Some(Net::Offline), Some("Episode 1")),
            "VRCinema — No internet — now playing: Episode 1"
        );
        assert_eq!(
            tooltip_text(EN, Some(Net::Connecting), None),
            "VRCinema — Connecting to the internet — break"
        );
    }

    #[test]
    fn the_net_status_follows_the_tunnel() {
        let online = TunnelState::Online {
            url: "https://a.trycloudflare.com".into(),
        };
        let error = TunnelState::Error {
            message: vrcinema::i18n::Msg::error("no_cloudflared"),
        };
        assert_eq!(net_status(true, &online, ""), Some(Net::Online));
        assert_eq!(
            net_status(true, &TunnelState::Connecting, ""),
            Some(Net::Connecting)
        );
        assert_eq!(
            net_status(true, &TunnelState::Reconnecting, ""),
            Some(Net::Offline)
        );
        assert_eq!(net_status(true, &error, ""), Some(Net::Offline));
    }

    #[test]
    fn an_enabled_tunnel_that_is_momentarily_off_is_connecting() {
        // Just after switching it on the manager has not started yet.
        assert_eq!(
            net_status(true, &TunnelState::Off, ""),
            Some(Net::Connecting)
        );
        assert_eq!(
            tooltip_text(PL, Some(Net::Connecting), None),
            "VRCinema — Łączenie z internetem — przerwa"
        );
    }

    #[test]
    fn without_a_tunnel_only_a_manual_address_is_mentioned() {
        let online = TunnelState::Online {
            url: "https://a.trycloudflare.com".into(),
        };
        assert_eq!(net_status(false, &online, ""), None);
        assert_eq!(net_status(false, &TunnelState::Off, "  "), None);
        assert_eq!(
            net_status(false, &TunnelState::Off, "https://tv.example.com"),
            Some(Net::Manual)
        );
        assert_eq!(
            tooltip_text(PL, Some(Net::Manual), None),
            "VRCinema — Adres ręczny — przerwa"
        );
        assert_eq!(
            tooltip_text(EN, Some(Net::Manual), None),
            "VRCinema — Manual address — break"
        );
    }

    #[test]
    fn a_link_without_an_address_is_marked_when_the_tunnel_is_on() {
        let local = channel_link("", 8181);
        assert_eq!(
            copied_link(PL, local.clone(), true, ""),
            "http://127.0.0.1:8181/live/main.m3u8 (brak internetu)"
        );
        assert_eq!(
            copied_link(EN, local.clone(), true, ""),
            "http://127.0.0.1:8181/live/main.m3u8 (no internet)"
        );
        // Tunnel off: the local link as before; with an address: the link as it is.
        assert_eq!(copied_link(PL, local.clone(), false, ""), local);
        let public = channel_link("https://a.trycloudflare.com", 8181);
        assert_eq!(
            copied_link(PL, public.clone(), true, "https://a.trycloudflare.com"),
            public
        );
    }
}
