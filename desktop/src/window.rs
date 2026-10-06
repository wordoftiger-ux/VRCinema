//! The panel window: opened on the engine's one-time login link, hidden instead of closed,
//! files dropped on it go to the queue, and it follows the server to a new port.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::webview::NewWindowResponse;
use tauri::{
    AppHandle, DragDropEvent, Manager, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

use crate::Shell;

pub const MAIN: &str = "main";
/// Subfolder of the app's home with the WebView2 profile.
const WEBVIEW_DIR: &str = "webview";
const PORT_CHECK: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const HIDDEN_TITLE: &str = "VRCinema";
const HIDDEN_TEXT: &str = "VRCinema działa w tle — kanał nadal nadaje.";

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
    match window {
        Some(w) => {
            if let Err(e) = w.navigate(url) {
                shell.log(&format!("Nie udało się otworzyć panelu: {e}"));
            }
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        None => {
            if let Err(e) = create(app, url) {
                shell.log(&format!("Nie udało się utworzyć okna: {e}"));
            }
        }
    }
}

fn create(app: &AppHandle, url: Url) -> tauri::Result<()> {
    let nav_app = app.clone();
    let new_window_app = app.clone();
    let window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(url))
        .title("VRCinema")
        .inner_size(1100.0, 800.0)
        .min_inner_size(480.0, 600.0)
        .center()
        .on_navigation(move |url| allow_navigation(&nav_app, url))
        .on_new_window(move |url, _features| new_window(&new_window_app, url))
        // The WebView2 profile stays with the rest of the app's data.
        .data_directory(app.state::<Shell>().home.join(WEBVIEW_DIR))
        .build()?;
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
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            if let Some(w) = app.get_webview_window(MAIN) {
                let _ = w.hide();
            }
            let shell = app.state::<Shell>();
            if !shell.quitting() && !shell.hidden_notice_shown.swap(true, Ordering::SeqCst) {
                crate::tray::show_balloon(HIDDEN_TITLE, HIDDEN_TEXT);
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
                    let msg = format!("{e:#}");
                    engine.log(&format!("Nie udało się dodać plików: {msg}"));
                    Err(msg)
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
            shell.log(&format!(
                "Serwer działa na porcie {port} — przełączam okno."
            ));
            *shell.window_port() = Some(port);
            if let Err(e) = window.navigate(login_url(port, &key)) {
                shell.log(&format!("Nie udało się przełączyć okna: {e}"));
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

    #[test]
    fn window_follows_only_a_changed_port() {
        assert_eq!(port_to_follow(Some(8080), 8080), None);
        assert_eq!(port_to_follow(Some(8080), 8181), Some(8181));
        // No window opened yet: opening it uses the engine's port anyway.
        assert_eq!(port_to_follow(None, 8181), None);
    }
}
