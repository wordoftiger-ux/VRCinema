use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use vrcinema::config::{
    AppConfig, env_language, fake_resolver_file, find_root, load_config, merged_env,
};
use vrcinema::i18n::{Lang, Msg, error_msg};
use vrcinema::process::contain_this_process;
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
fn load_env(root: &std::path::Path) -> Result<HashMap<String, String>, Msg> {
    // vars() panics on non-Unicode entries; merged_env skips those instead.
    merged_env(std::env::vars_os(), root)
}

fn check_tools(config: &AppConfig) -> Result<(), Msg> {
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
        Err(Msg::new("log.headless_tools_missing").arg("tools", missing.join(", ")))
    }
}

/// `VIDEO_ENCODER=auto`: the encoder is tested at start (see [`vrcinema::tools::detect_encoder`]).
fn encoder_auto(env: &HashMap<String, String>) -> bool {
    env.get("VIDEO_ENCODER")
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("auto"))
}

/// The configuration and the merged variables (environment + `.env`). With
/// `VIDEO_ENCODER=auto` the configuration says libx264 until the start-up test.
fn prepare_server(root: &std::path::Path) -> Result<(AppConfig, HashMap<String, String>), String> {
    let env = load_env(root).map_err(|m| {
        Msg::new("log.headless_config_error")
            .arg_msg("error", m)
            .render(Lang::En)
    })?;
    let mut config_env = env.clone();
    if encoder_auto(&env) {
        config_env.insert("VIDEO_ENCODER".into(), "libx264".into());
    }
    let config = load_config(&config_env, root, cfg!(windows)).map_err(|e| {
        Msg::new("log.headless_config_error")
            .arg("error", e.message().render(env_language(&env)))
            .render(env_language(&env))
    })?;
    check_tools(&config).map_err(|m| m.render(config.language))?;
    if !env.contains_key("SESSION_SECRET") || env["SESSION_SECRET"].trim().is_empty() {
        log(config.language, &Msg::new("log.session_secret_missing"));
    }
    Ok((config, env))
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
        // The messages of `setup` are in the `LANGUAGE` of the environment or `.env`.
        let lang = load_env(&root).map_or(Lang::En, |env| env_language(&env));
        let result = match SetupOptions::parse(&args[1..]) {
            Ok(opts) => run_setup(&root, opts, lang).await,
            Err(e) => Err(e),
        };
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{}", error_msg(&e).render(lang));
                ExitCode::FAILURE
            }
        };
    }
    if let Some(other) = args.first() {
        eprintln!(
            "{}",
            Msg::new("log.headless_unknown_command")
                .arg("command", other)
                .render(Lang::En)
        );
        return ExitCode::FAILURE;
    }

    let (mut config, env) = match prepare_server(&root) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    if encoder_auto(&env) {
        // NVENC, QSV, AMF, VA-API (Linux) in turn (one second of test picture each), else
        // libx264.
        config.video_encoder =
            vrcinema::tools::detect_encoder(&config.ffmpeg_path, &config.vaapi_device).await;
        log(
            config.language,
            &Msg::new("log.encoder_test").arg("encoder", config.video_encoder.as_str()),
        );
    }
    if vrcinema::pipeline::ffmpeg_args::light_profile(config.video_encoder) {
        log(config.language, &Msg::new("log.light_profile"));
    }
    // Every process started from here on (ffmpeg, yt-dlp and whatever they start) is born
    // inside the job object and dies with this program (Windows; a no-op elsewhere).
    if let Err(e) = contain_this_process() {
        log(
            config.language,
            &Msg::new("log.job_containment_failed").arg("error", e),
        );
    }
    // Read after .env was applied; an empty value counts as unset (like TS).
    let fake_resolver = fake_resolver_file(&env);
    let opts = RunOptions {
        update_ytdlp: fake_resolver.is_none(),
        fake_resolver,
        // The info ticker starts switched on with a GPU encoder (first start only).
        ticker_default: Some(config.video_encoder.is_gpu()),
        ..RunOptions::new(root)
    };
    match run(config, opts, shutdown_signal()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
