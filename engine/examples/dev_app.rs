//! Development helper for the web UI: runs the desktop app's engine without the window shell.
//!
//! `VRCINEMA_HOME=<temp dir> cargo run --example dev_app [-- <port>]` prints
//! `LOGIN http://127.0.0.1:<port>/desktop-login?key=...` (a one-time key, like the shell's);
//! every line typed on stdin prints a fresh one. Settings already in `<home>/config.json` are
//! kept (only the port is set). Tool downloads go nowhere unless
//! `VRCINEMA_DEV_TOOLS_BASE` points at a local server; no update checks. Not part of the product.

use std::path::PathBuf;
use std::sync::Arc;

use vrcinema::app::{AppEngine, AppOptions, DesktopHooks, FakeHooks};
use vrcinema::tools::ToolSources;
use vrcinema::tunnel::named::NamedOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let home = PathBuf::from(std::env::var("VRCINEMA_HOME").map_err(|_| {
        anyhow::anyhow!(
            "Ustaw VRCINEMA_HOME na pusty folder tymczasowy (nigdy data/ ani data-test/)"
        )
    })?);
    vrcinema::home::ensure_home(&home)?;
    // Never the program's default 8080 (the owner's channel may use it): port 0 unless given.
    let port: u16 = std::env::args()
        .nth(1)
        .map_or(Ok(0), |p| p.parse())
        .map_err(|_| anyhow::anyhow!("Port musi być liczbą"))?;
    // Other settings already in the folder's config.json (a prepared scenario) are kept.
    let file = home.join("config.json");
    let mut config: serde_json::Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    config["port"] = port.into();
    std::fs::write(&file, config.to_string())?;
    let base =
        std::env::var("VRCINEMA_DEV_TOOLS_BASE").unwrap_or_else(|_| "http://127.0.0.1:9".into());
    let mut options = AppOptions {
        check_updates: false,
        tool_sources: ToolSources {
            ytdlp_base: base.clone(),
            ffmpeg_base: base.clone(),
            deno_base: base.clone(),
            cloudflared_base: base.clone(),
            cloudflared_release_api: format!("{base}/release-api"),
        },
        // Never the owner's real `%USERPROFILE%\.cloudflared`: one inside the temporary home.
        named_tunnel: NamedOptions {
            user_dir: Some(home.join("user").join(".cloudflared")),
            ..NamedOptions::default()
        },
        ..AppOptions::default()
    };
    let fake = home.join("fake-resolver.json");
    std::fs::write(&fake, "{}")?;
    options.fake_resolver = Some(fake);
    let hooks = Arc::new(FakeHooks::default());
    let engine = AppEngine::start_with(home, hooks as Arc<dyn DesktopHooks>, options).await?;
    let port = engine.port();
    let login = |engine: &AppEngine| {
        println!(
            "LOGIN http://127.0.0.1:{port}/desktop-login?key={}",
            engine.new_desktop_key()
        );
    };
    login(&engine);
    // A line on stdin means "another key"; end of input stops the app.
    while tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0)
    })
    .await?
    {
        login(&engine);
    }
    engine.shutdown().await;
    Ok(())
}
