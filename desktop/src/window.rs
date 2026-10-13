//! The panel window: opened on the engine's one-time login link, hidden instead of closed,
//! files dropped on it go to the queue, and it follows the server to a new port. The new
//! interface draws its own title bar (no frame); its window buttons may use Tauri's window
//! commands from the engine's own address only. The window's place is kept in `config.json`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tauri::ipc::CapabilityBuilder;
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{
    AppHandle, DragDropEvent, Manager, PhysicalPosition, PhysicalSize, Runtime, Url, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use vrcinema::app_settings::WindowState;
use vrcinema::i18n::{Msg, error_msg, t};

use crate::Shell;

pub const MAIN: &str = "main";
/// Subfolder of the app's home with the WebView2 profile.
const WEBVIEW_DIR: &str = "webview";
const PORT_CHECK: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// Spec section 10: the smallest window the interface is laid out for (logical pixels).
const MIN_SIZE: (f64, f64) = (960.0, 600.0);
/// The size of a window with no saved place (logical pixels), centred.
const DEFAULT_SIZE: (f64, f64) = (1180.0, 760.0);
/// A moved or resized window is saved once it has been still for this long.
const SAVE_DELAY: Duration = Duration::from_millis(600);

/// Tauri's window commands the title bar of the new interface uses: its buttons, dragging
/// and double-click maximising (`data-tauri-drag-region`) and the maximised state for the
/// button's icon. Closing only hides the window (see [`on_event`]).
const WINDOW_CONTROLS: [&str; 6] = [
    "core:window:allow-minimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-internal-toggle-maximize",
    "core:window:allow-start-dragging",
    "core:window:allow-is-maximized",
    "core:window:allow-close",
];

/// Lets the page of the main window at `http://127.0.0.1:<port>` (the engine's own address)
/// use [`WINDOW_CONTROLS`]. Tauri checks every request's origin against this: no other
/// address, no other window, no other command. Permissions cannot be taken back, so after a
/// port change the old port keeps them — the window never shows another server's page
/// (see [`link_policy`]).
pub fn grant_window_controls<R: Runtime>(app: &AppHandle<R>, port: u16) -> tauri::Result<()> {
    let capability = WINDOW_CONTROLS.iter().fold(
        CapabilityBuilder::new(format!("window-controls-{port}"))
            .remote(format!("http://127.0.0.1:{port}"))
            .window(MAIN),
        |c, permission| c.permission(*permission),
    );
    app.add_capability(capability)
}

/// [`grant_window_controls`] once per port.
fn allow_window_controls(app: &AppHandle, port: u16) {
    let shell = app.state::<Shell>();
    if !shell.controls_ports().insert(port) {
        return;
    }
    if let Err(e) = grant_window_controls(app, port) {
        shell.controls_ports().remove(&port);
        shell.log(Msg::new("log.window_controls_failed").arg("error", e));
    }
}

/// The new interface (`/app`) draws its own title bar; the older pages need the window's.
pub fn frameless_page(url: &Url) -> bool {
    url.path() == "/app"
}

// ---------- The window's place ----------

/// A rectangle on the desktop, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// What the window looks like now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    /// Outer position and inner size.
    pub place: Area,
    pub maximized: bool,
    pub minimized: bool,
}

/// The height of the title bar strip that must be on a monitor, and how much of it.
const TITLE_STRIP: i64 = 32;
const GRAB_W: i64 = 100;
const GRAB_H: i64 = 24;
/// Sizes beyond this come from a damaged or hand-edited file.
const MAX_SIDE: u32 = 32_768;

/// The saved place to put the window at, if it can still be grabbed: enough of its title bar
/// on one of the `monitors`. `None`: the default place (a monitor was unplugged, the
/// resolution changed, nothing saved).
pub fn restore_place(saved: Option<WindowState>, monitors: &[Area]) -> Option<WindowState> {
    let s = saved?;
    if s.w == 0 || s.h == 0 || s.w > MAX_SIDE || s.h > MAX_SIDE {
        return None;
    }
    let (x, y, w) = (i64::from(s.x), i64::from(s.y), i64::from(s.w));
    let grabbable = monitors.iter().any(|m| {
        let (mx, my) = (i64::from(m.x), i64::from(m.y));
        let across = (x + w).min(mx + i64::from(m.w)) - x.max(mx);
        let down = (y + TITLE_STRIP).min(my + i64::from(m.h)) - y.max(my);
        across >= GRAB_W.min(w) && down >= GRAB_H
    });
    grabbable.then_some(s)
}

/// The place to keep for the window as it is `now`, given the `previous` one: a maximised
/// window keeps its normal bounds (so un-maximising after a restart restores them), a
/// minimised one changes nothing.
pub fn observed_state(previous: Option<WindowState>, now: Observed) -> Option<WindowState> {
    if now.minimized {
        return previous;
    }
    let Area { x, y, w, h } = now.place;
    match (now.maximized, previous) {
        (true, Some(p)) => Some(WindowState {
            maximized: true,
            ..p
        }),
        (maximized, _) => Some(WindowState {
            x,
            y,
            w,
            h,
            maximized,
        }),
    }
}

fn monitors(window: &WebviewWindow) -> Vec<Area> {
    window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| Area {
            x: m.position().x,
            y: m.position().y,
            w: m.size().width,
            h: m.size().height,
        })
        .collect()
}

fn observe(window: &WebviewWindow) -> Option<Observed> {
    let position = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    Some(Observed {
        place: Area {
            x: position.x,
            y: position.y,
            w: size.width,
            h: size.height,
        },
        maximized: window.is_maximized().unwrap_or(false),
        minimized: window.is_minimized().unwrap_or(false),
    })
}

/// Saves where the window is now (nothing when it did not change). A failure only loses the
/// window's place: not worth a message.
pub fn save_place(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    let Some(now) = observe(&window) else {
        return;
    };
    app.state::<Shell>().with_engine(|engine| {
        if let Some(state) = observed_state(engine.window_state(), now) {
            let _ = engine.save_window_state(state);
        }
    });
}

static SAVE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Saves the window's place once it has stopped moving.
fn schedule_save(app: &AppHandle) {
    let generation = SAVE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SAVE_DELAY).await;
        if SAVE_GENERATION.load(Ordering::SeqCst) == generation {
            save_place(&app);
        }
    });
}

/// Where an address opened from the panel goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPolicy {
    /// The app's own server (`http://127.0.0.1` on its port, or on the port the window is
    /// moving to after a port change): in the window.
    InApp,
    /// Any other http(s) address — `localhost` and other local ports too, whose cookies are
    /// not the window's session: the default browser.
    Browser,
    /// Everything else (`file:`, `data:`, `javascript:`, `about:`…): nowhere.
    Block,
}

pub fn link_policy(url: &Url, app_ports: &[u16]) -> LinkPolicy {
    let own_port = url
        .port_or_known_default()
        .is_some_and(|p| app_ports.contains(&p));
    match url.scheme() {
        "http" if url.host_str() == Some("127.0.0.1") && own_port => LinkPolicy::InApp,
        "http" | "https" => LinkPolicy::Browser,
        _ => LinkPolicy::Block,
    }
}

/// The ports of the app's own server: the engine's and the one the window was last sent to
/// (they differ while the window follows a port change).
fn app_ports(app: &AppHandle) -> Vec<u16> {
    let shell = app.state::<Shell>();
    let mut ports = Vec::with_capacity(2);
    if let Some(port) = shell.with_engine(|e| e.port()) {
        ports.push(port);
    }
    if let Some(port) = *shell.window_port() {
        ports.push(port);
    }
    ports
}

/// Top-level navigation of the window: only the app's own server; other web addresses go to
/// the browser instead.
fn allow_navigation(app: &AppHandle, url: &Url) -> bool {
    match link_policy(url, &app_ports(app)) {
        LinkPolicy::InApp => true,
        LinkPolicy::Browser => {
            crate::open_in_browser(url.as_str());
            false
        }
        LinkPolicy::Block => false,
    }
}

/// `target="_blank"` and `window.open`: never a second app window; web addresses (the
/// app's own pages included) open in the browser.
fn new_window(app: &AppHandle, url: Url) -> NewWindowResponse<tauri::Wry> {
    if link_policy(&url, &app_ports(app)) != LinkPolicy::Block {
        crate::open_in_browser(url.as_str());
    }
    NewWindowResponse::Deny
}

/// `http://127.0.0.1:<port>/desktop-login?key=<key>`
pub fn login_url(port: u16, key: &str) -> Url {
    let mut url = Url::parse(&format!("http://127.0.0.1:{port}/desktop-login"))
        .expect("a valid local address");
    url.query_pairs_mut().append_pair("key", key);
    url
}

/// Tells the page how a drop went (`vrcinema:files-added`, `detail`: added, skipped, error).
pub fn files_added_script(result: Result<(usize, usize), String>) -> String {
    let detail = match result {
        Ok((added, skipped)) => {
            serde_json::json!({"added": added, "skipped": skipped, "error": null})
        }
        Err(error) => serde_json::json!({"added": 0, "skipped": 0, "error": error}),
    };
    format!("window.dispatchEvent(new CustomEvent('vrcinema:files-added', {{detail: {detail}}}));")
}

/// Tells the page that files are dragged over the window (`vrcinema:drag-enter`) or no
/// longer (`vrcinema:drag-leave`): with the native drop handler on, WebView2 gives the page
/// no HTML5 drag events for files.
pub fn drag_script(over: bool) -> String {
    let event = if over {
        "vrcinema:drag-enter"
    } else {
        "vrcinema:drag-leave"
    };
    format!("window.dispatchEvent(new CustomEvent('{event}'));")
}

/// The port the window should move to: the engine's, when the window shows another one.
pub fn port_to_follow(window_port: Option<u16>, engine_port: u16) -> Option<u16> {
    window_port
        .filter(|&p| p != engine_port)
        .map(|_| engine_port)
}

/// Shows the panel: focuses the window when it is on screen, otherwise logs it in again
/// with a fresh one-time key (creating the window if needed).
pub fn open_panel(app: &AppHandle) {
    let shell = app.state::<Shell>();
    if shell.quitting() {
        return;
    }
    let window = app.get_webview_window(MAIN);
    if let Some(w) = &window
        && w.is_visible().unwrap_or(false)
    {
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let Some((port, key)) = shell.with_engine(|e| (e.port(), e.new_desktop_key())) else {
        return;
    };
    let url = login_url(port, &key);
    *shell.window_port() = Some(port);
    allow_window_controls(app, port);
    match window {
        Some(w) => {
            if let Err(e) = w.navigate(url) {
                shell.log(Msg::new("log.panel_open_failed").arg("error", e));
            }
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        None => {
            if let Err(e) = create(app, url) {
                shell.log(Msg::new("log.window_create_failed").arg("error", e));
            }
        }
    }
}

fn create(app: &AppHandle, url: Url) -> tauri::Result<()> {
    let nav_app = app.clone();
    let new_window_app = app.clone();
    let frameless = frameless_page(&url);
    let window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(url))
        .title("VRCinema")
        .inner_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1)
        .min_inner_size(MIN_SIZE.0, MIN_SIZE.1)
        .center()
        .decorations(!frameless)
        .shadow(true)
        // Shown once it is in its saved place.
        .visible(false)
        .on_navigation(move |url| allow_navigation(&nav_app, url))
        .on_new_window(move |url, _features| new_window(&new_window_app, url))
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Started {
                let _ = window.set_decorations(!frameless_page(payload.url()));
            }
        })
        // The WebView2 profile stays with the rest of the app's data.
        .data_directory(app.state::<Shell>().home.join(WEBVIEW_DIR))
        .build()?;
    let saved = app
        .state::<Shell>()
        .with_engine(|e| e.window_state())
        .flatten();
    if let Some(place) = restore_place(saved, &monitors(&window)) {
        let _ = window.set_size(PhysicalSize::new(place.w, place.h));
        let _ = window.set_position(PhysicalPosition::new(place.x, place.y));
        if place.maximized {
            let _ = window.maximize();
        }
    }
    let _ = window.show();
    let _ = window.set_focus();
    let app = app.clone();
    window.on_window_event(move |event| on_event(&app, event));
    Ok(())
}

fn eval(app: &AppHandle, js: String) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.eval(js);
    }
}

fn on_event(app: &AppHandle, event: &WindowEvent) {
    match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => schedule_save(app),
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            save_place(app);
            if let Some(w) = app.get_webview_window(MAIN) {
                let _ = w.hide();
            }
            let shell = app.state::<Shell>();
            if !shell.quitting() && !shell.hidden_notice_shown.swap(true, Ordering::SeqCst) {
                let lang = shell.with_engine(|e| e.language()).unwrap_or_default();
                crate::tray::show_balloon(
                    &t(lang, "tray.hidden_title", &[]),
                    &t(lang, "tray.hidden_text", &[]),
                );
            }
        }
        WindowEvent::DragDrop(DragDropEvent::Enter { .. }) => eval(app, drag_script(true)),
        WindowEvent::DragDrop(DragDropEvent::Leave) => eval(app, drag_script(false)),
        WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
            eval(app, drag_script(false));
            add_dropped(app.clone(), paths.clone());
        }
        _ => {}
    }
}

/// Queues the dropped files and reports the result to the page.
fn add_dropped(app: AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let result = {
            let shell = app.state::<Shell>();
            let engine = shell.engine.read().await;
            let Some(engine) = engine.as_ref() else {
                return; // quitting
            };
            match engine.add_files(paths).await {
                Ok(report) => Ok((report.added.len(), report.skipped)),
                Err(e) => {
                    let error = error_msg(&e);
                    engine.log(Msg::new("log.files_add_failed").arg_msg("error", error.clone()));
                    Err(error.render(engine.language()))
                }
            }
        };
        eval(&app, files_added_script(result));
    });
}

async fn server_answers(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio::net::TcpStream::connect(("127.0.0.1", port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// After a port change the server runs elsewhere: once it answers there, the window logs in
/// on the new port.
pub fn spawn_port_follow(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(PORT_CHECK).await;
            let shell = app.state::<Shell>();
            let engine_port = match shell.engine.read().await.as_ref() {
                Some(engine) => engine.port(),
                None => return, // quitting
            };
            let window_port = *shell.window_port();
            let Some(port) = port_to_follow(window_port, engine_port) else {
                continue;
            };
            if !server_answers(port).await {
                continue;
            }
            let Some(window) = app.get_webview_window(MAIN) else {
                continue;
            };
            let Some(key) = shell.with_engine(|e| e.new_desktop_key()) else {
                continue;
            };
            shell.log(Msg::new("log.window_follows_port").arg("port", port));
            *shell.window_port() = Some(port);
            allow_window_controls(&app, port);
            if let Err(e) = window.navigate(login_url(port, &key)) {
                shell.log(Msg::new("log.window_switch_failed").arg("error", e));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's server listens on 8181 (and the window is moving to 9000 in one test).
    fn policy(url: &str) -> LinkPolicy {
        link_policy(&Url::parse(url).unwrap(), &[8181])
    }

    #[test]
    fn the_apps_own_server_stays_in_the_window() {
        assert_eq!(policy("http://127.0.0.1:8181/admin"), LinkPolicy::InApp);
        assert_eq!(policy("http://127.0.0.1:8181/"), LinkPolicy::InApp);
        // During a port change: the port the window is following, too.
        assert_eq!(
            link_policy(
                &Url::parse("http://127.0.0.1:9000/desktop-login?key=x").unwrap(),
                &[8181, 9000]
            ),
            LinkPolicy::InApp
        );
    }

    #[test]
    fn other_local_ports_open_in_the_browser() {
        // Another program's local server: not the window's session, not the window's content.
        assert_eq!(policy("http://127.0.0.1:8080/admin"), LinkPolicy::Browser);
        assert_eq!(
            policy("http://127.0.0.1:9000/desktop-login?key=x"),
            LinkPolicy::Browser
        );
        assert_eq!(policy("http://127.0.0.1/"), LinkPolicy::Browser);
        // Nothing known yet (quitting): nothing local stays in the window.
        assert_eq!(
            link_policy(&Url::parse("http://127.0.0.1:8181/").unwrap(), &[]),
            LinkPolicy::Browser
        );
    }

    #[test]
    fn other_web_addresses_open_in_the_browser() {
        assert_eq!(policy("https://evil.example/"), LinkPolicy::Browser);
        assert_eq!(
            policy("https://tv.example.com/live/main.m3u8"),
            LinkPolicy::Browser
        );
        assert_eq!(policy("https://127.0.0.1:8080/"), LinkPolicy::Browser);
        assert_eq!(policy("http://localhost:8080/admin"), LinkPolicy::Browser);
        assert_eq!(policy("http://127.0.0.2:8080/"), LinkPolicy::Browser);
        assert_eq!(
            policy("http://127.0.0.1.evil.example/"),
            LinkPolicy::Browser
        );
    }

    #[test]
    fn non_web_addresses_are_blocked() {
        assert_eq!(policy("file:///C:/Windows/win.ini"), LinkPolicy::Block);
        assert_eq!(policy("data:text/html,hi"), LinkPolicy::Block);
        assert_eq!(policy("javascript:alert(1)"), LinkPolicy::Block);
        assert_eq!(policy("about:blank"), LinkPolicy::Block);
        assert_eq!(policy("ms-settings:privacy"), LinkPolicy::Block);
    }

    #[test]
    fn login_url_points_at_the_local_server_with_the_key() {
        assert_eq!(
            login_url(8181, "abc123").as_str(),
            "http://127.0.0.1:8181/desktop-login?key=abc123"
        );
        // Whatever the key contains, it stays one query value.
        let url = login_url(8181, "a&b=c d");
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "key");
        assert_eq!(pairs[0].1, "a&b=c d");
    }

    fn detail(script: &str, event: &str) -> serde_json::Value {
        let prefix = format!("window.dispatchEvent(new CustomEvent('{event}', {{detail: ");
        let json = script
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix("}));"))
            .unwrap_or_else(|| panic!("unexpected script: {script}"));
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn drop_result_reaches_the_page_as_an_event() {
        let ok = files_added_script(Ok((3, 1)));
        assert_eq!(
            detail(&ok, "vrcinema:files-added"),
            serde_json::json!({"added": 3, "skipped": 1, "error": null})
        );
        let err = files_added_script(Err("Serwer jest \"zajęty\"</script>\n".into()));
        assert_eq!(
            detail(&err, "vrcinema:files-added"),
            serde_json::json!({"added": 0, "skipped": 0, "error": "Serwer jest \"zajęty\"</script>\n"})
        );
        assert!(!err.contains('\n'), "one line of JS: {err}");
    }

    #[test]
    fn drag_state_reaches_the_page_as_events() {
        assert_eq!(
            drag_script(true),
            "window.dispatchEvent(new CustomEvent('vrcinema:drag-enter'));"
        );
        assert_eq!(
            drag_script(false),
            "window.dispatchEvent(new CustomEvent('vrcinema:drag-leave'));"
        );
    }

    // ---------- The window's place ----------

    const FULL_HD: Area = Area {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    /// A second monitor on the left of the first.
    const LEFT: Area = Area {
        x: -1280,
        y: 0,
        w: 1280,
        h: 1024,
    };

    fn place(x: i32, y: i32, w: u32, h: u32, maximized: bool) -> WindowState {
        WindowState {
            x,
            y,
            w,
            h,
            maximized,
        }
    }

    #[test]
    fn a_saved_place_on_a_monitor_is_used() {
        let saved = place(100, 80, 1200, 760, false);
        assert_eq!(restore_place(Some(saved), &[FULL_HD]), Some(saved));
        // On the second monitor, maximised there.
        let left = place(-1200, 20, 1100, 700, true);
        assert_eq!(restore_place(Some(left), &[FULL_HD, LEFT]), Some(left));
        // Mostly off the right edge, but enough of the title bar to grab: kept.
        let edge = place(1700, 300, 1200, 760, false);
        assert_eq!(restore_place(Some(edge), &[FULL_HD]), Some(edge));
    }

    #[test]
    fn a_place_off_every_monitor_gives_the_default() {
        assert_eq!(restore_place(None, &[FULL_HD]), None);
        // The second monitor was unplugged.
        let left = place(-1200, 20, 1100, 700, false);
        assert_eq!(restore_place(Some(left), &[FULL_HD]), None);
        // Far away, below the screen, or only a sliver of the title bar visible.
        for gone in [
            place(5000, 5000, 1200, 760, false),
            place(100, 1075, 1200, 760, false),
            place(1880, 100, 1200, 760, false),
            place(100, -40, 1200, 760, false),
        ] {
            assert_eq!(restore_place(Some(gone), &[FULL_HD]), None, "{gone:?}");
        }
        // Nonsense sizes from a hand-edited file.
        assert_eq!(
            restore_place(Some(place(10, 10, 0, 760, false)), &[FULL_HD]),
            None
        );
        assert_eq!(
            restore_place(Some(place(10, 10, 1200, 90_000, false)), &[FULL_HD]),
            None
        );
        // No monitor known (should not happen): nothing to check against, the default.
        assert_eq!(
            restore_place(Some(place(10, 10, 1200, 760, false)), &[]),
            None
        );
    }

    fn seen(x: i32, y: i32, w: u32, h: u32, maximized: bool, minimized: bool) -> Observed {
        Observed {
            place: Area { x, y, w, h },
            maximized,
            minimized,
        }
    }

    #[test]
    fn the_place_to_keep_follows_the_window_but_not_its_maximised_or_minimised_bounds() {
        let normal = place(100, 80, 1200, 760, false);
        // Moved or resized: the new place.
        assert_eq!(
            observed_state(Some(normal), seen(300, 90, 1300, 800, false, false)),
            Some(place(300, 90, 1300, 800, false))
        );
        // Maximised: the normal place stays, flagged maximised (restoring brings it back).
        assert_eq!(
            observed_state(Some(normal), seen(-8, -8, 1936, 1048, true, false)),
            Some(place(100, 80, 1200, 760, true))
        );
        // Maximised with nothing saved before: the maximised bounds are all there is.
        assert_eq!(
            observed_state(None, seen(-8, -8, 1936, 1048, true, false)),
            Some(place(-8, -8, 1936, 1048, true))
        );
        // Minimised (Windows parks it at -32000): nothing changes.
        assert_eq!(
            observed_state(Some(normal), seen(-32000, -32000, 160, 28, false, true)),
            Some(normal)
        );
        assert_eq!(
            observed_state(None, seen(-32000, -32000, 160, 28, false, true)),
            None
        );
    }

    #[test]
    fn only_the_new_interface_has_no_frame() {
        let url = |s: &str| Url::parse(s).unwrap();
        assert!(frameless_page(&url("http://127.0.0.1:8181/app")));
        assert!(frameless_page(&url(
            "http://127.0.0.1:8181/app#/settings/tools"
        )));
        // The old panel and the wizard bring no title bar of their own yet.
        assert!(!frameless_page(&url("http://127.0.0.1:8181/admin")));
        assert!(!frameless_page(&url("http://127.0.0.1:8181/wizard")));
        assert!(!frameless_page(&url(
            "http://127.0.0.1:8181/desktop-login?key=x"
        )));
        assert!(!frameless_page(&url("http://127.0.0.1:8181/application")));
    }

    // ---------- Window commands: only for the engine's own address ----------

    mod commands {
        use tauri::ipc::{CallbackFn, InvokeBody};
        use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
        use tauri::webview::InvokeRequest;
        use tauri::{WebviewUrl, WebviewWindow, WebviewWindowBuilder};

        use super::super::{MAIN, grant_window_controls};

        const ENGINE: u16 = 8181;

        fn window(app: &tauri::App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
            let url = format!("http://127.0.0.1:{ENGINE}/app").parse().unwrap();
            WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
                .build()
                .unwrap()
        }

        /// Runs `cmd` as if the page at `origin` asked for it; true when it was allowed.
        fn allowed(window: &WebviewWindow<MockRuntime>, origin: &str, cmd: &str) -> bool {
            get_ipc_response(
                window,
                InvokeRequest {
                    cmd: cmd.into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: origin.parse().unwrap(),
                    body: InvokeBody::default(),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.to_string(),
                },
            )
            .is_ok()
        }

        const CONTROLS: [&str; 5] = [
            "plugin:window|minimize",
            "plugin:window|toggle_maximize",
            "plugin:window|internal_toggle_maximize",
            "plugin:window|start_dragging",
            "plugin:window|is_maximized",
        ];

        #[test]
        fn window_commands_are_allowed_only_for_the_engines_own_address() {
            let app = mock_builder()
                .build(tauri::generate_context!(test = true))
                .unwrap();
            let main = window(&app, MAIN);
            let own = format!("http://127.0.0.1:{ENGINE}");
            // Nothing is allowed to a web page by default.
            assert!(!allowed(&main, &own, "plugin:window|minimize"));

            grant_window_controls(app.handle(), ENGINE).unwrap();
            for cmd in CONTROLS {
                assert!(allowed(&main, &own, cmd), "{cmd} from the engine");
            }
            // Any other address: another local program, another name for this computer,
            // https, a web site, a look-alike host.
            for origin in [
                "http://127.0.0.1:8182",
                "http://localhost:8181",
                "https://127.0.0.1:8181",
                "http://127.0.0.2:8181",
                "https://evil.example",
                "http://127.0.0.1.evil.example:8181",
            ] {
                for cmd in CONTROLS {
                    assert!(!allowed(&main, origin, cmd), "{cmd} from {origin}");
                }
                assert!(
                    !allowed(&main, origin, "plugin:window|close"),
                    "close from {origin}"
                );
            }
            // Only the window controls, nothing else of Tauri.
            for cmd in [
                "plugin:window|set_title",
                "plugin:window|set_position",
                "plugin:webview|create_webview_window",
                "plugin:event|listen",
                "plugin:path|resolve_directory",
            ] {
                assert!(!allowed(&main, &own, cmd), "{cmd}");
            }
            // Another window of the same address gets nothing.
            let other = window(&app, "other");
            assert!(!allowed(&other, &own, "plugin:window|minimize"));
            // Close last (it acts on the window).
            assert!(allowed(&main, &own, "plugin:window|close"));
        }

        #[test]
        fn after_a_port_change_the_new_port_is_allowed_too() {
            let app = mock_builder()
                .build(tauri::generate_context!(test = true))
                .unwrap();
            let main = window(&app, MAIN);
            grant_window_controls(app.handle(), ENGINE).unwrap();
            assert!(!allowed(
                &main,
                "http://127.0.0.1:9000",
                "plugin:window|minimize"
            ));
            grant_window_controls(app.handle(), 9000).unwrap();
            assert!(allowed(
                &main,
                "http://127.0.0.1:9000",
                "plugin:window|minimize"
            ));
        }
    }

    #[test]
    fn window_follows_only_a_changed_port() {
        assert_eq!(port_to_follow(Some(8080), 8080), None);
        assert_eq!(port_to_follow(Some(8080), 8181), Some(8181));
        // No window opened yet: opening it uses the engine's port anyway.
        assert_eq!(port_to_follow(None, 8181), None);
    }
}
