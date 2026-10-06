//! Development helper for the web UI: runs the desktop app's engine without the window shell.
//!
//! `VRCINEMA_HOME=<temp dir> cargo run --example dev_app [-- <port>]` prints
//! `LOGIN http://127.0.0.1:<port>/desktop-login?key=...` (a one-time key, like the shell's);
//! every line typed on stdin prints a fresh one. Tool downloads go nowhere unless
//! `VRCINEMA_DEV_TOOLS_BASE` points at a local server; no update checks. Not part of the product.

use std::path::PathBuf;
use std::sync::Arc;

use vrcinema::app::{AppEngine, AppOptions, DesktopHooks, FakeHooks};
use vrcinema::tools::ToolSources;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let home = PathBuf::from(std::env::var("VRCINEMA_HOME").map_err(|_| {
        anyhow::anyhow!(
            "Ustaw VRCINEMA_HOME na pusty folder tymczasowy (nigdy data/ ani data-test/)"
        )
    })?);
    vrcinema::home::ensure_home(&home)?;
    // Never the program's default 8080 (the owner's channel may use it): port 0 unless given.
    let port = std::env::args().nth(1).unwrap_or_else(|| "0".into());
    std::fs::write(home.join("config.json"), format!("{{\"port\": {port}}}"))?;
    let base =
        std::env::var("VRCINEMA_DEV_TOOLS_BASE").unwrap_or_else(|_| "http://127.0.0.1:9".into());
    let mut options = AppOptions {
        check_updates: false,
        tool_sources: ToolSources {
            ytdlp_base: base.clone(),
            ffmpeg_base: base.clone(),
            deno_base: base,
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
