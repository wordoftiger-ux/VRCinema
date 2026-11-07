//! The RTMP listener: binds `127.0.0.1` (or every address with the home-network switch, moved
//! over live), limits who may try, and serves each client with [`super::rtmp::run_session`].

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

use super::rtmp::{Ingest, SessionEnd, Timeouts, run_session};
use super::{Listening, ObsHub, key_matches};
use crate::i18n::Msg;
use crate::live::LiveValue;

pub type IngestLog = Arc<dyn Fn(Msg) + Send + Sync>;

/// Wrong keys from one address within [`FAILURE_WINDOW`] before it is locked out.
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const LOCKOUT: Duration = Duration::from_secs(60);
/// Clients being set up (not yet streaming) at once.
const MAX_HANDSHAKES: usize = 4;
const TIMEOUTS: Timeouts = Timeouts {
    auth: Duration::from_secs(10),
    idle: Duration::from_secs(10),
};
/// How often the listener looks whether it must move (the home-network switch).
const REBIND_CHECK: Duration = Duration::from_secs(1);
/// A port that could not be opened is tried again after this long.
const RETRY_BIND: Duration = Duration::from_secs(5);

/// Wrong keys per address, and who is locked out.
#[derive(Debug, Default)]
pub struct Lockout {
    failures: HashMap<IpAddr, Vec<Instant>>,
    locked: HashMap<IpAddr, Instant>,
}

impl Lockout {
    /// Whether `ip` may try now.
    pub fn allowed(&mut self, ip: IpAddr, now: Instant) -> bool {
        self.locked.retain(|_, until| *until > now);
        !self.locked.contains_key(&ip)
    }

    /// A wrong key from `ip`; true when that locks it out.
    pub fn failed(&mut self, ip: IpAddr, now: Instant) -> bool {
        let list = self.failures.entry(ip).or_default();
        list.retain(|at| now.duration_since(*at) < FAILURE_WINDOW);
        list.push(now);
        if list.len() < MAX_FAILURES {
            return false;
        }
        self.failures.remove(&ip);
        self.locked.insert(ip, now + LOCKOUT);
        // Old entries of addresses that stopped trying go.
        self.failures.retain(|_, l| {
            l.last()
                .is_some_and(|at| now.duration_since(*at) < FAILURE_WINDOW)
        });
        true
    }
}

pub struct IngestOptions {
    pub hub: ObsHub,
    pub port: u16,
    pub key: LiveValue<String>,
    pub lan: LiveValue<bool>,
    pub log: Option<IngestLog>,
}

/// One client's view of the listener.
struct Conn {
    o: Arc<IngestOptions>,
    ip: IpAddr,
    /// The publisher's id once it streams.
    id: Option<u64>,
    /// Held while the client is being set up.
    permit: Option<OwnedSemaphorePermit>,
}

impl Ingest for Conn {
    fn key_ok(&mut self, name: &str) -> bool {
        key_matches(&self.o.key.get(), name)
    }

    fn open(&mut self) -> Option<mpsc::Sender<Vec<u8>>> {
        let (id, tx) = self.o.hub.open()?;
        self.id = Some(id);
        self.permit = None;
        log(
            &self.o,
            Msg::new("log.obs_connected").arg("address", self.ip),
        );
        Some(tx)
    }
}

fn log(o: &IngestOptions, msg: Msg) {
    if let Some(log) = &o.log {
        log(msg);
    }
}

/// Runs the listener until the task is aborted (the server stops).
pub async fn run(o: IngestOptions) {
    let o = Arc::new(o);
    let lockout = Arc::new(Mutex::new(Lockout::default()));
    let handshakes = Arc::new(Semaphore::new(MAX_HANDSHAKES));
    let mut last_error: Option<String> = None;
    loop {
        let lan = o.lan.get();
        let host = if lan { "0.0.0.0" } else { "127.0.0.1" };
        let address = format!("{host}:{}", o.port);
        let listener = match TcpListener::bind(&address).await {
            Ok(l) => l,
            Err(e) => {
                let error = Msg::text(&e);
                // Logged once, not every retry.
                if last_error.as_deref() != Some(&e.to_string()) {
                    last_error = Some(e.to_string());
                    log(
                        &o,
                        Msg::new("log.obs_port_failed")
                            .arg("address", &address)
                            .arg_msg("error", error.clone()),
                    );
                }
                o.hub.set_listening(Listening::Failed { address, error });
                tokio::time::sleep(RETRY_BIND).await;
                continue;
            }
        };
        last_error = None;
        let bound = listener.local_addr().ok();
        if let Some(addr) = bound {
            o.hub.set_listening(Listening::On(addr));
            log(&o, Msg::new("log.obs_listening").arg("address", addr));
        }
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, peer)) = accepted else { continue };
                    accept(&o, &lockout, &handshakes, stream, peer);
                }
                () = tokio::time::sleep(REBIND_CHECK) => {
                    if o.lan.get() != lan {
                        break;
                    }
                }
            }
        }
        // Dropping the listener closes the port; streaming clients keep their connection.
    }
}

fn accept(
    o: &Arc<IngestOptions>,
    lockout: &Arc<Mutex<Lockout>>,
    handshakes: &Arc<Semaphore>,
    stream: TcpStream,
    peer: SocketAddr,
) {
    let ip = peer.ip();
    let allowed = lockout
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .allowed(ip, Instant::now());
    if !allowed {
        return;
    }
    let Ok(permit) = Arc::clone(handshakes).try_acquire_owned() else {
        return; // too many clients being set up: dropped
    };
    let _ = stream.set_nodelay(true);
    let o = Arc::clone(o);
    let lockout = Arc::clone(lockout);
    tokio::spawn(async move {
        let mut conn = Conn {
            o: Arc::clone(&o),
            ip,
            id: None,
            permit: Some(permit),
        };
        let end = run_session(stream, &mut conn, TIMEOUTS).await;
        if let Some(id) = conn.id {
            o.hub.close(id);
            log(&o, Msg::new("log.obs_disconnected").arg("address", ip));
        }
        match end {
            Ok(SessionEnd::WrongKey) => {
                let locked = lockout
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .failed(ip, Instant::now());
                log(&o, Msg::new("log.obs_wrong_key").arg("address", ip));
                if locked {
                    log(
                        &o,
                        Msg::new("log.obs_locked_out")
                            .arg("address", ip)
                            .arg("seconds", LOCKOUT.as_secs()),
                    );
                }
            }
            Ok(SessionEnd::Busy) => log(&o, Msg::new("log.obs_busy").arg("address", ip)),
            Ok(SessionEnd::Closed | SessionEnd::Dropped) | Err(_) => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_wrong_keys_in_a_minute_lock_the_address_out_for_a_minute() {
        let mut l = Lockout::default();
        let ip: IpAddr = "192.168.1.20".parse().unwrap();
        let other: IpAddr = "127.0.0.1".parse().unwrap();
        let t0 = Instant::now();
        for i in 0..4 {
            assert!(!l.failed(ip, t0 + Duration::from_secs(i)));
        }
        assert!(l.allowed(ip, t0 + Duration::from_secs(5)));
        assert!(l.failed(ip, t0 + Duration::from_secs(5)));
        assert!(!l.allowed(ip, t0 + Duration::from_secs(6)));
        assert!(
            l.allowed(other, t0 + Duration::from_secs(6)),
            "others are not"
        );
        assert!(
            l.allowed(ip, t0 + Duration::from_secs(66)),
            "the lockout ends"
        );
        // Failures spread over more than a minute never lock.
        let mut l = Lockout::default();
        for i in 0..10 {
            assert!(!l.failed(ip, t0 + Duration::from_secs(20 * i)));
        }
    }

    /// `bin/<name>` of the repository, or of `VRCINEMA_BIN_DIR` (a worktree without the tools).
    fn tool(name: &str) -> Option<std::path::PathBuf> {
        let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
        std::env::var_os("VRCINEMA_BIN_DIR")
            .map(std::path::PathBuf::from)
            .into_iter()
            .chain([root.join("bin")])
            .map(|dir| dir.join(&exe))
            .find(|p| p.is_file())
    }

    /// The real thing: ffmpeg publishes a test pattern with sound, the FLV that comes out is a
    /// file ffprobe reads (H.264 and AAC); a wrong key gets nothing through.
    #[tokio::test]
    async fn ffmpeg_publishes_and_its_stream_arrives_as_flv() {
        let (Some(ffmpeg), Some(ffprobe)) = (tool("ffmpeg"), tool("ffprobe")) else {
            eprintln!("skip: bin/ffmpeg missing");
            return;
        };
        let hub = ObsHub::default();
        let task = tokio::spawn(run(IngestOptions {
            hub: hub.clone(),
            port: 0,
            key: LiveValue::new("dobryklucz".to_string()),
            lan: LiveValue::new(false),
            log: None,
        }));
        let port = loop {
            if let Listening::On(addr) = hub.listening() {
                break addr.port();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let publish = |key: &str| {
            let url = format!("rtmp://127.0.0.1:{port}/live/{key}");
            let mut cmd = tokio::process::Command::new(&ffmpeg);
            cmd.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-re",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=25:duration=3",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-c:v",
                "libx264",
                "-g",
                "25",
                "-c:a",
                "aac",
                "-f",
                "flv",
                &url,
            ])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
            cmd
        };
        let wrong = publish("zlyklucz").output().await.unwrap();
        assert!(!wrong.status.success(), "a wrong key is refused");
        assert!(hub.take_session().is_none());

        let mut child = publish("dobryklucz").spawn().unwrap();
        let mut session = loop {
            if let Some(s) = hub.take_session() {
                break s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let mut flv = Vec::new();
        while let Some(piece) = session.input.recv().await {
            flv.extend(piece);
        }
        assert!(child.wait().await.unwrap().success());
        assert!(!hub.connected(), "the publisher is gone");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("obs.flv");
        std::fs::write(&file, &flv).unwrap();
        let probe = tokio::process::Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "csv=p=0",
            ])
            .arg(&file)
            .output()
            .await
            .unwrap();
        let codecs = String::from_utf8_lossy(&probe.stdout);
        assert!(
            codecs.contains("h264") && codecs.contains("aac"),
            "{codecs} {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        task.abort();
    }
}
