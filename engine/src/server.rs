//! Server startup (port of `src/server.ts`): order, banner, shutdown.

use std::ffi::OsString;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use tokio::sync::oneshot;

use crate::config::AppConfig;
use crate::db::{MAIN_CHANNEL_ID, SettingsRepo, open_database};
use crate::engine::spawn_engine_with;
use crate::http::{AppState, HttpConfig, build_router};
use crate::library::MediaLibrary;
use crate::pipeline::broadcaster::{Broadcaster, BroadcasterOptions, LogFn};
use crate::pipeline::channel::{Channel, ChannelOptions};
use crate::pipeline::clip_factory::{ClipFactoryDeps, SharedLogFn, create_clip_factory};
use crate::pipeline::probe::probe_duration;
use crate::pipeline::slates::{SlateOptions, generate_slates};
use crate::resolver::fake::FakeResolver;
use crate::resolver::local::{FileDurationProbe, LocalResolver};
use crate::resolver::pixeldrain::{DurationProbe, PixeldrainResolver, ReqwestHttp};
use crate::resolver::ytdlp::YtDlpResolver;
use crate::resolver::{CompositeResolver, MediaResolver};
use crate::updater::spawn_updater;

pub struct RunOptions {
    /// Application root (`public/` lives here).
    pub root: PathBuf,
    /// Use a `FakeResolver` loaded from this JSON file instead of the real resolvers.
    pub fake_resolver: Option<PathBuf>,
    /// Run `yt-dlp -U` at start and every 24 h.
    pub update_ytdlp: bool,
    /// Receives the address the HTTP listener is bound to, once it accepts connections.
    pub ready: Option<oneshot::Sender<SocketAddr>>,
}

/// yt-dlp JS runtime: the configured value verbatim, else `node` from PATH, else
/// `<root>/bin/deno`, else none.
pub fn detect_js_runtime(
    configured: Option<&str>,
    root: &Path,
    path_var: Option<OsString>,
) -> Option<String> {
    if let Some(value) = configured {
        return Some(value.to_string());
    }
    let node = if cfg!(windows) { "node.exe" } else { "node" };
    let on_path = path_var
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(node))
        .find(|p| p.is_file());
    if let Some(p) = on_path {
        return Some(format!("node:{}", p.display()));
    }
    let deno = root
        .join("bin")
        .join(if cfg!(windows) { "deno.exe" } else { "deno" });
    deno.is_file().then(|| format!("deno:{}", deno.display()))
}

pub fn log(msg: &str) {
    println!("[{}] {msg}", chrono::Local::now().format("%H:%M:%S"));
}

fn banner(config: &AppConfig, port: u16) -> String {
    let local = format!("http://localhost:{port}");
    let mut lines = vec![
        String::new(),
        "  Kanał działa.".to_string(),
        String::new(),
        format!("  Panel administratora:  {local}/admin"),
        format!("  Strona zgłoszeń:       {local}/"),
        format!("  Link kanału (lokalnie): {local}/live/main.m3u8"),
    ];
    lines.extend(
        lan_addresses()
            .into_iter()
            .map(|ip| format!("  W sieci domowej:        http://{ip}:{port}/live/main.m3u8")),
    );
    if config.public_url.is_empty() {
        lines.extend([
            String::new(),
            "  Dla VRChat potrzebny jest adres publiczny, np.:".to_string(),
            format!("    cloudflared tunnel --url {local}"),
        ]);
    } else {
        lines.push(format!(
            "  Link dla VRChat:        {}/live/main.m3u8",
            config.public_url
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn lan_addresses() -> Vec<Ipv4Addr> {
    local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, ip)| match ip {
            IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
            _ => None,
        })
        .collect()
}

fn build_resolver(
    config: &AppConfig,
    root: &Path,
    library: &Arc<MediaLibrary>,
    fake: Option<&Path>,
) -> anyhow::Result<Arc<dyn MediaResolver>> {
    if let Some(path) = fake {
        log(&format!(
            "Uwaga: aktywny fałszywy resolver ({}) — to tryb testowy.",
            path.display()
        ));
        return Ok(Arc::new(FakeResolver::from_file(path)?));
    }
    let ffprobe = config.ffprobe_path.clone();
    let pixeldrain_probe: DurationProbe = {
        let ffprobe = ffprobe.clone();
        Box::new(move |url, headers| {
            let ffprobe = ffprobe.clone();
            Box::pin(async move { probe_duration(&ffprobe, &url, &headers).await })
        })
    };
    let pixeldrain = PixeldrainResolver::new(
        Box::new(ReqwestHttp::new()?),
        pixeldrain_probe,
        config.pixeldrain_api_key.clone(),
    );
    let js_runtime = detect_js_runtime(
        config.ytdlp_js_runtime.as_deref(),
        root,
        std::env::var_os("PATH"),
    );
    let ytdlp = YtDlpResolver::new(
        config.ytdlp_path.clone(),
        js_runtime,
        config.ytdlp_cookies.clone(),
    );
    let local: Option<Box<dyn MediaResolver>> = library.enabled().then(|| {
        let probe: FileDurationProbe = Box::new(move |file| {
            let ffprobe = ffprobe.clone();
            Box::pin(async move { probe_duration(&ffprobe, &file.to_string_lossy(), &[]).await })
        });
        Box::new(LocalResolver::new(Arc::clone(library), probe)) as Box<dyn MediaResolver>
    });
    Ok(Arc::new(CompositeResolver::new(
        Box::new(pixeldrain),
        Box::new(ytdlp),
        local,
    )))
}

/// Starts everything and serves until `shutdown` completes, then stops cleanly.
pub async fn run(
    config: AppConfig,
    opts: RunOptions,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let RunOptions {
        root,
        fake_resolver,
        update_ytdlp,
        ready,
    } = opts;

    let db = Arc::new(Mutex::new(open_database(
        &config.data_dir.join("vrcinema.db"),
    )?));

    let library = Arc::new(MediaLibrary::new(config.media_dirs.clone()));
    for r in library.roots() {
        if !r.path.exists() {
            log(&format!(
                "Uwaga: folder z MEDIA_DIRS nie istnieje: {}",
                r.path.display()
            ));
        }
    }
    let resolver = build_resolver(&config, &root, &library, fake_resolver.as_deref())?;

    let clips_dir = config.data_dir.join("clips");
    let _ = std::fs::remove_dir_all(&clips_dir);
    std::fs::create_dir_all(&clips_dir)?;

    log("Generowanie plansz...");
    let slates = generate_slates(&SlateOptions {
        ffmpeg_path: config.ffmpeg_path.clone(),
        font_file: config.font_file.clone(),
        dir: config.data_dir.join("slates"),
        request_url: if config.public_url.is_empty() {
            String::new()
        } else {
            format!("{}/", config.public_url)
        },
        segment_seconds: config.segment_seconds,
    })
    .await?;

    let shared_log: SharedLogFn = Arc::new(log);
    let build = {
        let db = Arc::clone(&db);
        let resolver = Arc::clone(&resolver);
        let config = config.clone();
        let shared_log = Arc::clone(&shared_log);
        move || {
            let boxed_log = || -> LogFn {
                let l = Arc::clone(&shared_log);
                Box::new(move |m| l(m))
            };
            let factory = create_clip_factory(ClipFactoryDeps {
                resolver: Arc::clone(&resolver),
                ffmpeg_path: config.ffmpeg_path.clone(),
                ffprobe_path: config.ffprobe_path.clone(),
                encoder: config.video_encoder,
                segment_seconds: config.segment_seconds,
                clips_dir: config.data_dir.join("clips"),
                db: Arc::clone(&db),
                channel_id: MAIN_CHANNEL_ID,
                log: Some(Arc::clone(&shared_log)),
            });
            let mut options = ChannelOptions::new(
                MAIN_CHANNEL_ID,
                Arc::clone(&db),
                Arc::new(factory),
                slates.clone(),
            );
            options.log = Some(boxed_log());
            let tags_db = Arc::clone(&db);
            let broadcaster = Broadcaster::new(
                BroadcasterOptions {
                    dir: config.data_dir.join("live").join("main"),
                    window_size: config.window_size,
                    segment_seconds: f64::from(config.segment_seconds),
                    path_prefix: "main/".to_string(),
                    discontinuity_tags: Box::new(move || {
                        let conn = tags_db.lock().unwrap_or_else(PoisonError::into_inner);
                        SettingsRepo::new(&conn)
                            .get(MAIN_CHANNEL_ID)
                            .map_or(true, |s| s.discontinuity_tags)
                    }),
                    retain_s: Some(60),
                    log: Some(boxed_log()),
                },
                monotonic_clock(),
            );
            (broadcaster, Channel::new(options))
        }
    };
    let (handle, task) = spawn_engine_with(build, log);

    // The listener only opens once the channel has published its first real playlist.
    let mut snapshot = handle.snapshot.clone();
    snapshot.wait_for(|s| !s.playlist.is_empty()).await?;

    let state = AppState::new(
        HttpConfig::from(&config),
        MAIN_CHANNEL_ID,
        Arc::clone(&db),
        resolver,
        handle,
        root.join("public"),
    )?
    .with_library(Some(library))
    .with_log(Arc::clone(&shared_log));
    let router = build_router(state);

    let listener = tokio::net::TcpListener::bind((config.host.as_str(), config.port)).await?;
    let addr = listener.local_addr()?;
    println!("{}", banner(&config, addr.port()));
    if let Some(ready) = ready {
        let _ = ready.send(addr);
    }

    let updater =
        update_ytdlp.then(|| spawn_updater(config.ytdlp_path.clone(), Arc::clone(&shared_log)));

    let serve = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    );
    let result = tokio::select! {
        r = serve => r.map_err(anyhow::Error::from),
        () = shutdown => Ok(()),
    };

    log("Zamykanie...");
    if let Some(updater) = updater {
        updater.abort();
    }
    task.shutdown().await;
    result
}

fn monotonic_clock() -> Box<dyn Fn() -> u64 + Send> {
    let t0 = Instant::now();
    Box::new(move || u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(name: &str) -> String {
        format!("{name}{}", if cfg!(windows) { ".exe" } else { "" })
    }

    #[test]
    fn configured_runtime_wins_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(exe("node")), "").unwrap();
        let path = std::env::join_paths([tmp.path()]).unwrap();
        assert_eq!(
            detect_js_runtime(Some("bun:/x/bun"), tmp.path(), Some(path)),
            Some("bun:/x/bun".to_string())
        );
    }

    #[test]
    fn finds_node_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let node = tmp.path().join(exe("node"));
        std::fs::write(&node, "").unwrap();
        let path = std::env::join_paths([tmp.path()]).unwrap();
        assert_eq!(
            detect_js_runtime(None, Path::new("/nonexistent"), Some(path)),
            Some(format!("node:{}", node.display()))
        );
    }

    #[test]
    fn falls_back_to_deno_in_bin_then_none() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(detect_js_runtime(None, root.path(), None), None);
        std::fs::create_dir_all(root.path().join("bin")).unwrap();
        let deno = root.path().join("bin").join(exe("deno"));
        std::fs::write(&deno, "").unwrap();
        let empty = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([empty.path()]).unwrap();
        assert_eq!(
            detect_js_runtime(None, root.path(), Some(path)),
            Some(format!("deno:{}", deno.display()))
        );
    }
}
