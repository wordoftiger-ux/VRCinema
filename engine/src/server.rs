//! Server startup (port of `src/server.ts`): order, banner, shutdown.

use std::ffi::OsString;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use tokio::sync::oneshot;

use crate::config::AppConfig;
use crate::db::{MAIN_CHANNEL_ID, SettingsRepo, open_database};
use crate::engine::{EngineHandle, EngineTask, idle_engine_handle, spawn_engine_paused};
use crate::http::{AppState, HttpConfig, build_router};
use crate::library::MediaLibrary;
use crate::live::LiveSettings;
use crate::pipeline::broadcaster::{Broadcaster, BroadcasterOptions, LogFn};
use crate::pipeline::channel::{Channel, ChannelOptions, PauseReason};
use crate::pipeline::clip_factory::{ClipFactoryDeps, SharedLogFn, create_clip_factory};
use crate::pipeline::probe::probe_duration;
use crate::pipeline::slates::{RETAIN_S, SlateOptions, generate_slates, request_url};
use crate::resolver::fake::FakeResolver;
use crate::resolver::file::FileResolver;
use crate::resolver::local::{FileDurationProbe, LocalResolver};
use crate::resolver::pixeldrain::{DurationProbe, PixeldrainResolver, ReqwestHttp};
use crate::resolver::ytdlp::YtDlpResolver;
use crate::resolver::{CompositeResolver, MediaResolver};
use crate::updater::spawn_updater;

pub struct RunOptions {
    /// Application root (`bin/` with the tools lives here).
    pub root: PathBuf,
    /// Use a `FakeResolver` loaded from this JSON file instead of the real resolvers.
    pub fake_resolver: Option<PathBuf>,
    /// Run `yt-dlp -U` at start and every 24 h.
    pub update_ytdlp: bool,
    /// Desktop app: a taken port moves on to the next free one (the bound address is reported
    /// through `ready`). The headless server keeps failing fast.
    pub port_fallback: bool,
    /// Receives the address the HTTP listener is bound to, once it accepts connections.
    pub ready: Option<oneshot::Sender<SocketAddr>>,
    /// Where log lines go (desktop app: its log file); `None` prints to stdout like stage 1.
    pub log: Option<SharedLogFn>,
    /// Desktop app without working tools: serve pages and API, but no channel loop (and no
    /// slates, which need ffmpeg). The headless server always runs the channel.
    pub start_channel: bool,
    /// Last touch on the HTTP state before it is served (desktop app: auth, window key, the
    /// `/api/desktop/*` routes; it also keeps a clone to steer the running server).
    pub configure_state: Option<Box<dyn FnOnce(AppState) -> AppState + Send>>,
    /// Counts running channel loops: +1 once the engine is spawned, -1 after it has stopped.
    pub channel_loops: Option<Arc<AtomicUsize>>,
    /// Settings that change while the server runs (desktop app); `None`: fixed from `config`.
    pub live: Option<LiveSettings>,
    /// Desktop app: the tunnel is enabled but has no address yet, so the start-up idle slate
    /// says "Łączenie z internetem…" (with `config.public_url` empty).
    pub slates_connecting: bool,
    /// Desktop app: pause reasons active right now (automatic pause); the channel loop starts
    /// with them, so a restarted server does not air a film before they reach it.
    pub pause_reasons: Vec<PauseReason>,
}

impl RunOptions {
    /// Stage-1 behaviour for `root`: channel on, no port fallback, stdout log.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            fake_resolver: None,
            update_ytdlp: false,
            port_fallback: false,
            ready: None,
            log: None,
            start_channel: true,
            configure_state: None,
            channel_loops: None,
            live: None,
            slates_connecting: false,
            pause_reasons: Vec::new(),
        }
    }
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
    live: &LiveSettings,
    log: &SharedLogFn,
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
    )
    .with_api_key(live.pixeldrain_api_key.clone());
    let js_runtime = detect_js_runtime(
        config.ytdlp_js_runtime.as_deref(),
        root,
        std::env::var_os("PATH"),
    );
    let ytdlp = YtDlpResolver::new(
        config.ytdlp_path.clone(),
        js_runtime,
        config.ytdlp_cookies.clone(),
    )
    .with_cookies(live.ytdlp_cookies.clone());
    let file_probe = |ffprobe: std::path::PathBuf| -> FileDurationProbe {
        Box::new(move |file| {
            let ffprobe = ffprobe.clone();
            Box::pin(async move { probe_duration(&ffprobe, &file.to_string_lossy(), &[]).await })
        })
    };
    let local: Option<Box<dyn MediaResolver>> = library.enabled().then(|| {
        Box::new(LocalResolver::new(
            Arc::clone(library),
            file_probe(ffprobe.clone()),
        )) as Box<dyn MediaResolver>
    });
    Ok(Arc::new(
        CompositeResolver::new(Box::new(pixeldrain), Box::new(ytdlp), local)
            .with_file(Box::new(FileResolver::new(file_probe(ffprobe)))),
    ))
}

/// How many ports after the configured one are tried when the fallback is on.
pub const PORT_FALLBACK_SPAN: u16 = 20;

/// With the fallback on, a bind error of this kind moves on to the next port: taken, or
/// refused (Windows reserves ranges for Hyper-V/WSL, and binding there is "access denied").
fn try_next_port(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
    )
}

/// Binds `host:port`. With `fallback`, a taken (or refused) port moves on to the next ones (up
/// to [`PORT_FALLBACK_SPAN`] further); otherwise it fails with the Polish "port in use" error.
pub async fn bind_listener(
    host: &str,
    port: u16,
    fallback: bool,
) -> anyhow::Result<tokio::net::TcpListener> {
    let last = if fallback && port != 0 {
        port.saturating_add(PORT_FALLBACK_SPAN)
    } else {
        port
    };
    let mut candidate = port;
    loop {
        match tokio::net::TcpListener::bind((host, candidate)).await {
            Ok(listener) => return Ok(listener),
            Err(e) if try_next_port(e.kind()) && candidate < last => {
                candidate += 1;
            }
            Err(e) => {
                let tried = if candidate == port {
                    format!("{host}:{port}")
                } else {
                    format!("{host}:{port}-{candidate}")
                };
                return Err(anyhow::anyhow!("Nie można nasłuchiwać na {tried}: {e}"));
            }
        }
    }
}

/// Folders of slate sets made while an earlier server ran (`g1`, `g2`, ...): nothing airs them now.
fn remove_swapped_slate_sets(slates_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(slates_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let swapped = name
            .strip_prefix('g')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if swapped {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Generates the slates and spawns the supervised channel loop.
async fn start_channel_loop(
    config: &AppConfig,
    db: &Arc<Mutex<rusqlite::Connection>>,
    resolver: &Arc<dyn MediaResolver>,
    live: &LiveSettings,
    shared_log: &SharedLogFn,
    connecting: bool,
    pause_reasons: Vec<PauseReason>,
) -> anyhow::Result<(EngineHandle, EngineTask)> {
    shared_log("Generowanie plansz...");
    remove_swapped_slate_sets(&config.data_dir.join("slates"));
    let slates = generate_slates(&SlateOptions {
        ffmpeg_path: config.ffmpeg_path.clone(),
        font_file: config.font_file.clone(),
        dir: config.data_dir.join("slates"),
        request_url: request_url(&config.public_url),
        connecting,
        segment_seconds: config.segment_seconds,
        generation: 0,
    })
    .await?;

    let build = {
        let db = Arc::clone(db);
        let resolver = Arc::clone(resolver);
        let config = config.clone();
        let shared_log = Arc::clone(shared_log);
        let encoder = live.encoder.clone();
        move || {
            let boxed_log = || -> LogFn {
                let l = Arc::clone(&shared_log);
                Box::new(move |m| l(m))
            };
            let factory = create_clip_factory(ClipFactoryDeps {
                resolver: Arc::clone(&resolver),
                ffmpeg_path: config.ffmpeg_path.clone(),
                ffprobe_path: config.ffprobe_path.clone(),
                encoder: encoder.clone(),
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
                    retain_s: Some(RETAIN_S),
                    log: Some(boxed_log()),
                },
                monotonic_clock(),
            );
            (broadcaster, Channel::new(options))
        }
    };
    let engine_log = Arc::clone(shared_log);
    Ok(spawn_engine_paused(
        build,
        move |m: &str| engine_log(m),
        pause_reasons,
    ))
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
        port_fallback,
        ready,
        log: custom_log,
        start_channel,
        configure_state,
        channel_loops,
        live,
        slates_connecting,
        pause_reasons,
    } = opts;
    let live = live.unwrap_or_else(|| LiveSettings::from_config(&config));
    let print_banner = custom_log.is_none();
    let shared_log: SharedLogFn = custom_log.unwrap_or_else(|| Arc::new(log));

    // Before anything in DATA_DIR is touched: fail fast on a taken port, then make sure no
    // other server uses this data folder. The listener is not served until the first real
    // playlist exists; until then the OS backlog holds early connections.
    let listener = bind_listener(&config.host, config.port, port_fallback).await?;
    // Held until the server stops (for `main`: the process lifetime).
    let _data_lock = lock_data_dir(&config.data_dir)?;

    let db = Arc::new(Mutex::new(open_database(
        &config.data_dir.join("vrcinema.db"),
    )?));

    let library = Arc::new(MediaLibrary::new(config.media_dirs.clone()));
    for r in library.roots() {
        if !r.path.exists() {
            shared_log(&format!(
                "Uwaga: folder z MEDIA_DIRS nie istnieje: {}",
                r.path.display()
            ));
        }
    }
    let resolver = build_resolver(
        &config,
        &root,
        &library,
        fake_resolver.as_deref(),
        &live,
        &shared_log,
    )?;

    let clips_dir = config.data_dir.join("clips");
    let _ = std::fs::remove_dir_all(&clips_dir);
    std::fs::create_dir_all(&clips_dir)?;

    let (handle, engine_task) = if start_channel {
        let (handle, task) = start_channel_loop(
            &config,
            &db,
            &resolver,
            &live,
            &shared_log,
            slates_connecting,
            pause_reasons,
        )
        .await?;
        (handle, LoopGuard::new(Some(task), channel_loops))
    } else {
        shared_log("Kanał nie wystartuje: brak narzędzi (ffmpeg). Strony i panel działają.");
        (idle_engine_handle(), LoopGuard::new(None, None))
    };

    let served = async {
        // The listener only opens once the channel has published its first real playlist.
        tokio::pin!(shutdown);
        if engine_task.has_loop() {
            let mut snapshot = handle.snapshot.clone();
            // A stop requested while starting (e.g. the app gives up waiting) ends it here.
            tokio::select! {
                r = snapshot.wait_for(|s| !s.playlist.is_empty()) => { r?; }
                () = &mut shutdown => {
                    shared_log("Zamykanie...");
                    return Ok(());
                }
            }
        }

        let mut state = AppState::new(
            HttpConfig::from(&config),
            MAIN_CHANNEL_ID,
            Arc::clone(&db),
            resolver,
            handle,
        )?
        .with_library(Some(library))
        .with_log(Arc::clone(&shared_log))
        .with_live(live.clone());
        if let Some(configure) = configure_state {
            state = configure(state);
        }
        let router = build_router(state);

        let addr = listener.local_addr()?;
        let text = banner(&config, addr.port());
        if print_banner {
            println!("{text}");
        } else if start_channel {
            shared_log(&text);
        } else {
            shared_log(&format!(
                "Serwer działa bez kanału: http://localhost:{}/admin",
                addr.port()
            ));
        }
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
            () = &mut shutdown => Ok(()),
        };

        shared_log("Zamykanie...");
        if let Some(updater) = updater {
            updater.abort();
        }
        result
    }
    .await;

    // Also when serving never started: the loop must not outlive this call.
    engine_task.shutdown().await;
    served
}

/// The channel loop of one [`run`] call, counted in `channel_loops` while it runs. Stopped by
/// [`LoopGuard::shutdown`] — or, when `run` is dropped mid-way (its task aborted), on drop: the
/// loop then stops in the background and the counter follows once it has ended.
/// ([`EngineTask`] itself stops nothing when dropped: `spawn_engine` relies on that.)
struct LoopGuard {
    task: Option<EngineTask>,
    loops: Option<Arc<AtomicUsize>>,
}

impl LoopGuard {
    fn new(task: Option<EngineTask>, loops: Option<Arc<AtomicUsize>>) -> Self {
        let loops = task.as_ref().and(loops);
        if let Some(loops) = &loops {
            loops.fetch_add(1, Ordering::SeqCst);
        }
        Self { task, loops }
    }

    fn has_loop(&self) -> bool {
        self.task.is_some()
    }

    /// Stops the loop and the channel and waits until both are done.
    async fn shutdown(mut self) {
        if let Some(task) = &mut self.task {
            task.request_stop();
            task.wait().await;
            self.task = None;
            if let Some(loops) = self.loops.take() {
                loops.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

impl Drop for LoopGuard {
    fn drop(&mut self) {
        let Some(mut task) = self.task.take() else {
            return;
        };
        task.request_stop();
        let loops = self.loops.take();
        let ended = move || {
            if let Some(loops) = loops {
                loops.fetch_sub(1, Ordering::SeqCst);
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move {
                    task.wait().await;
                    ended();
                });
            }
            Err(_) => ended(),
        }
    }
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
            fake_resolver: Some(fake),
            ..RunOptions::new(repo_root())
        }
    }

    #[test]
    fn the_fallback_skips_taken_and_refused_ports_only() {
        use std::io::ErrorKind;
        assert!(try_next_port(ErrorKind::AddrInUse));
        assert!(try_next_port(ErrorKind::PermissionDenied));
        assert!(!try_next_port(ErrorKind::AddrNotAvailable));
        assert!(!try_next_port(ErrorKind::InvalidInput));
    }

    fn capture_log() -> (SharedLogFn, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&lines);
        (
            Arc::new(move |m: &str| sink.lock().unwrap().push(m.to_string())),
            lines,
        )
    }

    async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
        let res = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        let status = res.status().as_u16();
        (status, res.text().await.unwrap())
    }

    #[tokio::test]
    async fn without_the_channel_pages_and_api_are_served_at_once() {
        let (tmp, data, _markers) = data_with_markers();
        let mut config = test_config(&data, 0);
        // No tools at all: slates would fail if they were attempted.
        config.ffmpeg_path = tmp.path().join("brak").join("ffmpeg.exe");
        config.ffprobe_path = tmp.path().join("brak").join("ffprobe.exe");
        let (log, lines) = capture_log();
        let loops = Arc::new(AtomicUsize::new(0));
        let configured = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&configured);
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let opts = RunOptions {
            ready: Some(ready_tx),
            log: Some(log),
            start_channel: false,
            channel_loops: Some(Arc::clone(&loops)),
            configure_state: Some(Box::new(move |s: AppState| {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                s
            })),
            ..test_opts(tmp.path())
        };
        let server = tokio::spawn(run(config, opts, async {
            let _ = stop_rx.await;
        }));
        let addr = tokio::time::timeout(std::time::Duration::from_secs(10), ready_rx)
            .await
            .expect("server did not start in 10 s")
            .unwrap();
        assert!(configured.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(http_get(addr, "/").await.0, 200);
        let (status, body) = http_get(addr, "/api/channels/main/state").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(loops.load(std::sync::atomic::Ordering::SeqCst), 0);
        stop_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
        let lines = lines.lock().unwrap().clone();
        assert!(
            lines.iter().any(|l| l.contains("Zamykanie")),
            "the injected log is used: {lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("Generowanie plansz")));
    }

    #[tokio::test]
    async fn the_channel_loop_counter_follows_the_engine() {
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
        let (log, lines) = capture_log();
        let loops = Arc::new(AtomicUsize::new(0));
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let opts = RunOptions {
            ready: Some(ready_tx),
            log: Some(log),
            channel_loops: Some(Arc::clone(&loops)),
            ..test_opts(tmp.path())
        };
        let server = tokio::spawn(run(test_config(&data, 0), opts, async {
            let _ = stop_rx.await;
        }));
        let addr = tokio::time::timeout(std::time::Duration::from_secs(60), ready_rx)
            .await
            .expect("server did not start in 60 s")
            .unwrap();
        assert_eq!(loops.load(std::sync::atomic::Ordering::SeqCst), 1);
        let (status, body) = http_get(addr, "/live/main.m3u8").await;
        assert_eq!(status, 200);
        assert!(body.contains(".ts"), "{body}");
        stop_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
        assert_eq!(loops.load(std::sync::atomic::Ordering::SeqCst), 0);
        let lines = lines.lock().unwrap().clone();
        assert!(
            lines.iter().any(|l| l.contains("Generowanie plansz")),
            "{lines:?}"
        );
    }

    /// The app aborts a server that takes too long to start: its channel loop must stop too
    /// (and the loop counter must come back to zero), not run on with nobody to stop it.
    #[tokio::test]
    async fn aborting_run_stops_the_channel_loop() {
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
        let (log, _lines) = capture_log();
        let loops = Arc::new(AtomicUsize::new(0));
        let opts = RunOptions {
            log: Some(log),
            channel_loops: Some(Arc::clone(&loops)),
            ..test_opts(tmp.path())
        };
        let server = tokio::spawn(run(test_config(&data, 0), opts, std::future::pending()));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        while loops.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the loop did not start in 60 s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        while loops.load(std::sync::atomic::Ordering::SeqCst) != 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the channel loop outlived the aborted server"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn bind_listener_without_fallback_fails_on_a_taken_port() {
        let taken = bind_listener("127.0.0.1", 0, false).await.unwrap();
        let port = taken.local_addr().unwrap().port();
        let err = bind_listener("127.0.0.1", port, false).await.unwrap_err();
        assert!(
            err.to_string().contains("Nie można nasłuchiwać na"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn bind_listener_with_fallback_takes_the_next_free_port() {
        // Two adjacent ports: hold the first two, expect the third (or a later free one).
        let (a, b) = loop {
            let a = bind_listener("127.0.0.1", 0, false).await.unwrap();
            let p = a.local_addr().unwrap().port();
            if let Ok(b) = bind_listener("127.0.0.1", p + 1, false).await {
                break (a, b);
            }
        };
        let base = a.local_addr().unwrap().port();
        let got = bind_listener("127.0.0.1", base, true).await.unwrap();
        let port = got.local_addr().unwrap().port();
        assert!(
            port > base + 1 && port <= base + PORT_FALLBACK_SPAN,
            "{port}"
        );
        drop(b);
    }

    #[tokio::test]
    async fn bind_listener_gives_up_after_the_span() {
        let mut held = Vec::new();
        let first = bind_listener("127.0.0.1", 0, false).await.unwrap();
        let base = first.local_addr().unwrap().port();
        if base > u16::MAX - PORT_FALLBACK_SPAN - 1 {
            return;
        }
        for p in base + 1..=base + PORT_FALLBACK_SPAN {
            match bind_listener("127.0.0.1", p, false).await {
                Ok(l) => held.push(l),
                Err(_) => return, // someone else holds it: cannot build the scenario
            }
        }
        let err = bind_listener("127.0.0.1", base, true).await.unwrap_err();
        assert!(
            err.to_string().contains("Nie można nasłuchiwać na"),
            "{err}"
        );
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
