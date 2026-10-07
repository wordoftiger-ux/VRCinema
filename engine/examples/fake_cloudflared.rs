//! Fake `cloudflared` for the tunnel manager tests (`tests/tunnel.rs`). Not part of the product.
//!
//! Tests copy this program into a temporary folder as `cloudflared.exe`, next to a script
//! `fake_cloudflared.json`: a JSON array with one entry per start (the last entry repeats),
//! so tests running in parallel never share settings. Fields of an entry (all optional):
//!
//! - `delay_ms`: wait before anything else (a start still in progress);
//! - `serve_metrics` (default true): serve `/quicktunnel` and `/ready` on the `--metrics` address;
//! - `hostname`: what `/quicktunnel` reports (`""` when absent);
//! - `ready_after_ms` (default 0): `/ready` answers 200 this long after the start, 503 before;
//! - `ready_down`: `[[from_ms, to_ms], …]` after the start during which `/ready` answers 503
//!   again (connection to Cloudflare lost while the process keeps running);
//! - `stderr`: lines printed to stderr;
//! - `exit_after_ms` + `exit_code`: exit on its own (runs until killed when absent).
//!
//! Every start appends `{"event":"start","pid","args","ms","overlap"}` to `fake_cloudflared.log`
//! in the same folder (and `{"event":"exit",…}` before exiting on its own). `overlap` is true
//! when another instance from that folder was still running: each instance holds an exclusive
//! lock on `instance.lock` for its whole life (Windows). The path of every request to the
//! metrics server is appended to `requests.log`.
//!
//! The permanent link's management commands (`tunnel login|create|token|route dns|delete`, see
//! [`management`]) are scripted separately by `fake_cloudflared_commands.json`; they log
//! `{"event":"command",…}` (never `start`) and never take `instance.lock`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct Run {
    delay_ms: u64,
    serve_metrics: bool,
    hostname: Option<String>,
    ready_after_ms: u64,
    ready_down: Vec<[u64; 2]>,
    stderr: Vec<String>,
    exit_after_ms: Option<u64>,
    exit_code: i32,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            delay_ms: 0,
            serve_metrics: true,
            hostname: None,
            ready_after_ms: 0,
            ready_down: Vec::new(),
            stderr: Vec::new(),
            exit_after_ms: None,
            exit_code: 0,
        }
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn append(log: &Path, value: serde_json::Value) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .expect("open fake_cloudflared.log");
    // One write per entry: the manager may kill this process at any moment, and a partial
    // line would corrupt the next one.
    file.write_all(format!("{value}\n").as_bytes())
        .expect("write fake_cloudflared.log");
}

/// The exclusive instance lock, or `None` when another instance holds it.
fn lock(dir: &Path) -> Option<File> {
    exclusive(&dir.join("instance.lock"))
}

/// `path` opened exclusively (Windows), or `None` when another process holds it.
#[cfg(windows)]
fn exclusive(path: &Path) -> Option<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .share_mode(0)
        .open(path)
        .ok()
}

#[cfg(not(windows))]
fn exclusive(path: &Path) -> Option<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()
}

fn serve(listener: TcpListener, run: Run, started: Instant, requests: PathBuf) {
    for stream in listener.incoming().flatten() {
        let run = run.clone();
        let requests = requests.clone();
        std::thread::spawn(move || answer(stream, &run, started, &requests));
    }
}

fn is_ready(run: &Run, started: Instant) -> bool {
    let ms = started.elapsed().as_millis() as u64;
    ms >= run.ready_after_ms
        && !run
            .ready_down
            .iter()
            .any(|[from, to]| (*from..*to).contains(&ms))
}

fn answer(mut stream: TcpStream, run: &Run, started: Instant, requests: &Path) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request.split_whitespace().nth(1).unwrap_or("");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(requests) {
        let _ = file.write_all(format!("{path}\n").as_bytes());
    }
    let (status, body) = match path {
        "/quicktunnel" => (
            "200 OK",
            serde_json::json!({ "hostname": run.hostname.clone().unwrap_or_default() }).to_string(),
        ),
        "/ready" => {
            if is_ready(run, started) {
                (
                    "200 OK",
                    r#"{"status":200,"readyConnections":1}"#.to_string(),
                )
            } else {
                (
                    "503 Service Unavailable",
                    r#"{"status":503,"readyConnections":0}"#.to_string(),
                )
            }
        }
        _ => ("404 Not Found", String::new()),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn main() {
    let started = Instant::now();
    let exe = std::env::current_exe().expect("current_exe");
    let dir = exe.parent().expect("folder of the fake").to_path_buf();
    let log = dir.join("fake_cloudflared.log");
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(command) = management::command_of(&args) {
        append(
            &log,
            serde_json::json!({
                "event": "command",
                "command": command,
                "pid": std::process::id(),
                "args": args,
                "ms": now_ms(),
            }),
        );
        let code = management::run(&dir, command, &args);
        std::process::exit(code);
    }

    let instance = lock(&dir);
    let previous_starts = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(r#""event":"start""#))
        .count();
    append(
        &log,
        serde_json::json!({
            "event": "start",
            "pid": std::process::id(),
            "args": args,
            "ms": now_ms(),
            "overlap": instance.is_none(),
        }),
    );

    let script: Vec<Run> = std::fs::read_to_string(dir.join("fake_cloudflared.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let run = script
        .get(previous_starts)
        .or(script.last())
        .cloned()
        .unwrap_or_default();

    std::thread::sleep(Duration::from_millis(run.delay_ms));

    if run.serve_metrics {
        let metrics = args
            .iter()
            .position(|a| a == "--metrics")
            .and_then(|i| args.get(i + 1))
            .expect("--metrics <address>");
        let listener = TcpListener::bind(metrics).expect("bind the metrics address");
        let served = run.clone();
        let requests = dir.join("requests.log");
        std::thread::spawn(move || serve(listener, served, started, requests));
    }

    for line in &run.stderr {
        eprintln!("{line}");
    }

    match run.exit_after_ms {
        Some(ms) => {
            std::thread::sleep(Duration::from_millis(ms));
            append(
                &log,
                serde_json::json!({ "event": "exit", "pid": std::process::id(), "ms": now_ms() }),
            );
            // The lock stays held until the OS ends the process (no destructors run).
            std::process::exit(run.exit_code);
        }
        None => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
    }
}

/// The management commands, scripted by `fake_cloudflared_commands.json` next to the program:
/// an object with an optional entry per command. They work on temporary files only: `login`
/// writes its certificate into the script's `cert_dir` (it fails without one, so a test can
/// never reach the real `%USERPROFILE%\.cloudflared`), the others into the paths they are given.
/// Like the real cloudflared, `create`, `token`, `route` and `delete` fail without an existing
/// `--origincert` file, and output goes to stderr (log lines) and stdout (results).
///
/// - `login`: `url` (printed like cloudflared does), `delay_ms` (the owner in the browser),
///   `wait_forever`, `cert_dir`, `cert` (contents), `exit_code` + `stderr` (a failure). An
///   existing non-empty `cert_dir\cert.pem` ends the login at once (the real one refuses to
///   overwrite it). The process holds `login.lock` exclusively for its whole life.
/// - `create`: `exists` (a tunnel `vrcinema` is already there), `id`, `secret` (written into the
///   credentials file), `error`.
/// - `token`: `id`, `secret` (default: `create`'s), `error`. Without `--cred-file` it prints the
///   token (the secret) on stdout, like the real one.
/// - `route`: `taken` (another DNS record has the name; `--overwrite-dns` replaces it), `zone`
///   (the certificate's domain: a host outside it gets `<host>.<zone>`), `error`.
/// - `delete`: `missing` (no such tunnel), `error`.
mod management {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use serde::Deserialize;

    const ID: &str = "6ff42ae2-765d-4adf-8112-31c55c1551ef";
    const NAMES: [&str; 5] = ["login", "create", "token", "route", "delete"];
    /// Flags followed by a value.
    const VALUE_FLAGS: [&str; 7] = [
        "--origincert",
        "--metrics",
        "--url",
        "--credentials-file",
        "--cred-file",
        "--loglevel",
        "--output",
    ];

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Script {
        login: Login,
        create: Create,
        token: Token,
        route: Route,
        delete: Delete,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Login {
        url: Option<String>,
        delay_ms: u64,
        wait_forever: bool,
        cert_dir: Option<PathBuf>,
        cert: Option<String>,
        exit_code: i32,
        stderr: Vec<String>,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Create {
        exists: bool,
        id: Option<String>,
        secret: Option<String>,
        error: Option<String>,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Token {
        id: Option<String>,
        secret: Option<String>,
        error: Option<String>,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Route {
        taken: bool,
        zone: Option<String>,
        error: Option<String>,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Delete {
        missing: bool,
        error: Option<String>,
    }

    /// The words that are not flags or flag values.
    fn positional(args: &[String]) -> Vec<&str> {
        let mut words = Vec::new();
        let mut skip = false;
        for arg in args {
            if skip {
                skip = false;
            } else if arg.starts_with('-') {
                skip = VALUE_FLAGS.contains(&arg.as_str());
            } else {
                words.push(arg.as_str());
            }
        }
        words
    }

    /// `tunnel <command> …` for a management command, `None` for a tunnel run.
    pub fn command_of(args: &[String]) -> Option<&'static str> {
        let words = positional(args);
        if words.first() != Some(&"tunnel") {
            return None;
        }
        let word = *words.get(1)?;
        NAMES.into_iter().find(|n| *n == word)
    }

    fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }

    fn credentials(id: &str, secret: &str) -> String {
        serde_json::json!({
            "AccountTag": "fake-account",
            "TunnelSecret": secret,
            "TunnelID": id,
        })
        .to_string()
    }

    fn fail(message: &str) -> i32 {
        eprintln!("{message}");
        1
    }

    pub fn run(dir: &Path, command: &str, args: &[String]) -> i32 {
        let script: Script = std::fs::read_to_string(dir.join("fake_cloudflared_commands.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if command == "login" {
            return login(dir, &script.login);
        }
        let cert = flag(args, "--origincert");
        if !cert.is_some_and(|c| Path::new(c).is_file()) {
            return fail(
                "Cannot determine default origin certificate path. No file cert.pem in [~/.cloudflared ~/.cloudflare-warp ~/cloudflare-warp /etc/cloudflared /usr/local/etc/cloudflared]. You need to specify the origin certificate path by specifying the origincert option in the configuration file, or set TUNNEL_ORIGIN_CERT environment variable originCertPath=",
            );
        }
        let words = positional(args);
        let create_id = script.create.id.clone().unwrap_or_else(|| ID.into());
        let create_secret = script
            .create
            .secret
            .clone()
            .unwrap_or_else(|| "fake-secret".into());
        match command {
            "create" => {
                if let Some(error) = &script.create.error {
                    return fail(error);
                }
                if script.create.exists {
                    return fail(
                        "failed to create tunnel: Create Tunnel API call failed: tunnel with name already exists",
                    );
                }
                let Some(path) = flag(args, "--credentials-file") else {
                    return fail("fake: --credentials-file is required");
                };
                std::fs::write(path, credentials(&create_id, &create_secret))
                    .expect("write the credentials");
                eprintln!(
                    "2026-10-07T10:00:00Z INF Tunnel credentials written to {path}. cloudflared chose this file based on where your origin certificate was found. Keep this file secret. To revoke these credentials, delete the tunnel."
                );
                println!("Created tunnel vrcinema with id {create_id}");
                0
            }
            "token" => {
                if let Some(error) = &script.token.error {
                    return fail(error);
                }
                let id = script.token.id.clone().unwrap_or(create_id);
                let secret = script.token.secret.clone().unwrap_or(create_secret);
                match flag(args, "--cred-file") {
                    Some(path) => {
                        std::fs::write(path, credentials(&id, &secret))
                            .expect("write the credentials");
                    }
                    None => print!("token-{secret}"),
                }
                0
            }
            "route" => {
                if let Some(error) = &script.route.error {
                    return fail(error);
                }
                let (Some(tunnel), Some(host)) = (words.get(3), words.get(4)) else {
                    return fail("fake: route dns <tunnel> <host>");
                };
                let overwrite = args.iter().any(|a| a == "--overwrite-dns" || a == "-f");
                if script.route.taken && !overwrite {
                    return fail(&format!(
                        "Failed to add route: code: 1003, reason: Failed to create record {host} with err An A, AAAA, or CNAME record with that host already exists. For more details, refer to <https://developers.cloudflare.com/dns/manage-dns-records/troubleshooting/records-with-same-name/>."
                    ));
                }
                let record = match &script.route.zone {
                    Some(zone) if *host != zone && !host.ends_with(&format!(".{zone}")) => {
                        format!("{host}.{zone}")
                    }
                    _ => host.to_string(),
                };
                eprintln!(
                    "2026-10-07T10:00:01Z INF Added CNAME {record} which will route to this tunnel tunnelID={tunnel}"
                );
                0
            }
            "delete" => {
                if let Some(error) = &script.delete.error {
                    return fail(error);
                }
                let name = words.last().copied().unwrap_or("");
                if script.delete.missing {
                    return fail(&format!(
                        "{name} is neither the ID nor the name of any of your tunnels"
                    ));
                }
                eprintln!("2026-10-07T10:00:02Z INF Deleted tunnel {name}");
                0
            }
            _ => fail("fake: unknown command"),
        }
    }

    fn login(dir: &Path, login: &Login) -> i32 {
        let _lock = super::exclusive(&dir.join("login.lock"));
        let Some(cert_dir) = &login.cert_dir else {
            return fail("fake: the script has no login.cert_dir");
        };
        let cert = cert_dir.join("cert.pem");
        if std::fs::metadata(&cert).is_ok_and(|m| m.len() > 0) {
            println!(
                "You have an existing certificate at {} which login would overwrite.\nIf this is intentional, please move or delete that file then run this command again.",
                cert.display()
            );
            return 0;
        }
        if let Some(url) = &login.url {
            eprintln!(
                "Please open the following URL and log in with your Cloudflare account:\n\n{url}\n\nLeave cloudflared running to download the cert automatically."
            );
        }
        if login.wait_forever {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        std::thread::sleep(Duration::from_millis(login.delay_ms));
        if login.exit_code != 0 {
            for line in &login.stderr {
                eprintln!("{line}");
            }
            return login.exit_code;
        }
        std::fs::create_dir_all(cert_dir).expect("create the certificate folder");
        let contents = login.cert.clone().unwrap_or_else(|| "fake-cert".into());
        std::fs::write(&cert, contents).expect("write the certificate");
        println!(
            "You have successfully logged in.\nIf you wish to copy your credentials to a server, they have been saved to:\n{}",
            cert.display()
        );
        0
    }
}
