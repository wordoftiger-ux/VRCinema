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

    // Before anything in DATA_DIR is touched: fail fast on a taken port, then make sure no
    // other server uses this data folder. The listener is not served until the first real
    // playlist exists; until then the OS backlog holds early connections.
    let listener = tokio::net::TcpListener::bind((config.host.as_str(), config.port))
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Nie można nasłuchiwać na {}:{}: {e}",
                config.host,
                config.port
            )
        })?;
    // Held until the server stops (for `main`: the process lifetime).
    let _data_lock = lock_data_dir(&config.data_dir)?;

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

/// Name of the lock file in DATA_DIR.
pub const LOCK_FILE: &str = "vrcinema.lock";

/// Takes an exclusive lock on `<data_dir>/vrcinema.lock` (the OS releases it when the returned
/// file is closed or the process dies). Fails with a Polish message when another server holds
/// it: two servers on one data folder would wipe each other's clips, slates and live segments.
pub fn lock_data_dir(data_dir: &Path) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(data_dir).map_err(|e| {
        anyhow::anyhow!(
            "Nie można utworzyć folderu danych {}: {e}",
            data_dir.display()
        )
    })?;
    let path = data_dir.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| anyhow::anyhow!("Nie można otworzyć pliku {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(anyhow::anyhow!(
            "Inny serwer kanału już używa folderu danych: {}",
            data_dir.display()
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(anyhow::anyhow!(
            "Nie można zablokować pliku {}: {e}",
            path.display()
        )),
    }
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

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf()
    }

    /// A data folder with something in clips/, slates/ and live/ that startup would wipe.
    fn data_with_markers() -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let markers: Vec<PathBuf> = ["clips", "slates", "live/main"]
            .iter()
            .map(|d| data.join(d).join("marker.txt"))
            .collect();
        for m in &markers {
            std::fs::create_dir_all(m.parent().unwrap()).unwrap();
            std::fs::write(m, "x").unwrap();
        }
        (tmp, data, markers)
    }

    fn test_config(data: &Path, port: u16) -> AppConfig {
        let env: std::collections::HashMap<String, String> = [
            ("ADMIN_PASSWORD", "haslo-testowe-123"),
            ("DATA_DIR", data.to_str().unwrap()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let mut config = crate::config::load_config(&env, &repo_root(), cfg!(windows)).unwrap();
        config.port = port;
        config.host = "127.0.0.1".into();
        config.data_dir = data.to_path_buf();
        config
    }

    fn test_opts(tmp: &Path) -> RunOptions {
        let fake = tmp.join("fake.json");
        std::fs::write(&fake, "{}").unwrap();
        RunOptions {
            root: repo_root(),
            fake_resolver: Some(fake),
            update_ytdlp: false,
            ready: None,
        }
    }

    /// Runs the server and stops it right after it starts serving.
    async fn run_once(config: AppConfig, opts: RunOptions) -> anyhow::Result<()> {
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            run(config, opts, std::future::ready(())),
        )
        .await
        .expect("run() did not finish in 60 s")
    }

    #[tokio::test]
    async fn a_second_server_on_the_same_data_folder_exits_without_touching_it() {
        let (tmp, data, markers) = data_with_markers();
        // The other server: holds the lock on the data folder.
        let other = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data.join("vrcinema.lock"))
            .unwrap();
        other.try_lock().unwrap();

        let err = run_once(test_config(&data, 0), test_opts(tmp.path()))
            .await
            .expect_err("a second server must not start");
        assert_eq!(
            format!("{err:#}"),
            format!(
                "Inny serwer kanału już używa folderu danych: {}",
                data.display()
            )
        );
        for m in &markers {
            assert!(m.exists(), "{} was wiped", m.display());
        }
        drop(other);
    }

    #[tokio::test]
    async fn a_port_in_use_fails_before_the_data_folder_is_touched() {
        let (tmp, data, markers) = data_with_markers();
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();

        let err = run_once(test_config(&data, port), test_opts(tmp.path()))
            .await
            .expect_err("the port is taken");
        for m in &markers {
            assert!(m.exists(), "{} was wiped", m.display());
        }
        assert!(format!("{err:#}").contains(&port.to_string()), "{err:#}");
        assert!(!data.join("vrcinema.db").exists(), "database opened");
        assert!(!data.join("vrcinema.lock").exists(), "lock taken");
    }

    #[tokio::test]
    async fn a_running_server_holds_the_data_folder_lock_until_it_stops() {
        let exe = if cfg!(windows) { ".exe" } else { "" };
        if !repo_root()
            .join("bin")
            .join(format!("ffmpeg{exe}"))
            .exists()
        {
            eprintln!("pomijam: brak bin/ffmpeg");
            return;
        }
        let (tmp, data, _markers) = data_with_markers();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let mut opts = test_opts(tmp.path());
        opts.ready = Some(ready_tx);
        let server = tokio::spawn(run(test_config(&data, 0), opts, async {
            let _ = stop_rx.await;
        }));
        tokio::time::timeout(std::time::Duration::from_secs(60), ready_rx)
            .await
            .expect("server did not start in 60 s")
            .unwrap();

        let probe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(data.join("vrcinema.lock"))
            .unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        stop_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
        probe.try_lock().unwrap();
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
