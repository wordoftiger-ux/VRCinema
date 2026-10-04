use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use vrcinema::config::{AppConfig, apply_env, find_root, load_config, read_env_file};
use vrcinema::server::{RunOptions, log, run};
use vrcinema::setup::{SetupOptions, run_setup};

fn application_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| cwd.clone());
    find_root(&exe_dir, &cwd)
}

/// Environment plus `<root>/.env` (real environment variables win).
fn load_env(root: &std::path::Path) -> Result<HashMap<String, String>, String> {
    // vars() panics on non-Unicode entries; skip those instead.
    let mut env: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v.to_str()?.to_string())))
        .collect();
    let env_file = root.join(".env");
    if env_file.exists() {
        let values = read_env_file(&env_file)
            .map_err(|e| format!("Nie można odczytać pliku {}: {e}", env_file.display()))?;
        apply_env(&mut env, values);
    }
    Ok(env)
}

fn check_tools(config: &AppConfig) -> Result<(), String> {
    let missing: Vec<String> = [
        &config.ffmpeg_path,
        &config.ffprobe_path,
        &config.ytdlp_path,
    ]
    .into_iter()
    .filter(|p| !p.exists())
    .map(|p| p.display().to_string())
    .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "\nBrak narzędzi: {}\nUruchom najpierw: vrcinema setup\n",
            missing.join(", ")
        ))
    }
}

fn prepare_server(root: &std::path::Path) -> Result<AppConfig, String> {
    let env = load_env(root)?;
    let config = load_config(&env, root, cfg!(windows))
        .map_err(|e| format!("\nBłąd konfiguracji: {e}\n"))?;
    check_tools(&config)?;
    if !env.contains_key("SESSION_SECRET") || env["SESSION_SECRET"].trim().is_empty() {
        log(
            "Uwaga: brak SESSION_SECRET w .env — po restarcie trzeba będzie zalogować się ponownie.",
        );
    }
    Ok(config)
}

async fn ctrl_c() {
    // If the signal handler cannot be installed, never resolve instead of shutting down at once.
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(unix)]
async fn terminate() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut sigterm) => {
            sigterm.recv().await;
        }
        Err(_) => std::future::pending::<()>().await,
    }
}

#[cfg(not(unix))]
async fn terminate() {
    std::future::pending::<()>().await;
}

/// Resolves on Ctrl+C (SIGINT) or, on unix, SIGTERM (`docker stop`, systemd).
async fn shutdown_signal() {
    tokio::select! {
        () = ctrl_c() => {}
        () = terminate() => {}
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let root = application_root();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().is_some_and(|a| a == "setup") {
        let result = match SetupOptions::parse(&args[1..]) {
            Ok(opts) => run_setup(&root, opts).await,
            Err(e) => Err(e),
        };
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e:#}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(other) = args.first() {
        eprintln!("Nieznane polecenie: {other}\nUżycie: vrcinema [setup [--force] [--no-env]]");
        return ExitCode::FAILURE;
    }

    let config = match prepare_server(&root) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    let fake_resolver = std::env::var_os("VRCINEMA_FAKE_RESOLVER").map(PathBuf::from);
    let opts = RunOptions {
        root,
        update_ytdlp: fake_resolver.is_none(),
        fake_resolver,
        ready: None,
    };
    match run(config, opts, shutdown_signal()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
