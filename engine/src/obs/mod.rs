//! OBS as a source (spec `2026-11-06-obs-design.md`): an RTMP listener of its own (never the
//! HTTP port, so never reachable through the tunnel), the stream key, and the hub that hands an
//! authenticated stream to the channel.

pub mod ingest;
pub mod rtmp;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::i18n::Msg;

/// The RTMP port OBS knows.
pub const DEFAULT_PORT: u16 = 1935;
/// The application part of the address (`rtmp://host:port/live`).
pub const APP: &str = "live";
/// Length of a generated stream key.
const KEY_LEN: usize = 24;
/// FLV pieces waiting for ffmpeg (about a few seconds of a stream).
const SESSION_BUFFER: usize = 512;

/// A new random stream key: lower-case letters and digits (about 124 bits).
pub fn new_key() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; KEY_LEN];
    getrandom::fill(&mut bytes).expect("system random number generator");
    bytes
        .iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect()
}

/// Whether `given` is the stream key, in constant time (both are hashed first, so neither the
/// position of a difference nor the key's length shows in the time taken). An empty key
/// matches nothing.
pub fn key_matches(expected: &str, given: &str) -> bool {
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(given.as_bytes());
    let diff = a
        .iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));
    !expected.is_empty() && diff == 0
}

/// A publisher's stream for the channel: FLV bytes, from the header on.
#[derive(Debug)]
pub struct ObsSession {
    pub id: u64,
    pub input: mpsc::Receiver<Vec<u8>>,
}

/// The listener's state for the window.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Listening {
    #[default]
    Off,
    On(SocketAddr),
    /// The port could not be opened (taken, not allowed); it is tried again.
    Failed {
        address: String,
        error: Msg,
    },
}

#[derive(Debug, Default)]
struct HubState {
    /// A publisher's stream the channel has not taken yet.
    pending: Option<ObsSession>,
    /// The publisher streaming now (one at a time).
    active: Option<u64>,
    next_id: u64,
    listening: Listening,
}

/// Shared by the listener (which opens sessions) and the channel (which takes them); cheap to
/// clone.
#[derive(Debug, Clone, Default)]
pub struct ObsHub(Arc<Mutex<HubState>>);

impl ObsHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HubState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new publisher's stream: its id and where its FLV goes; `None` while another one
    /// streams.
    pub fn open(&self) -> Option<(u64, mpsc::Sender<Vec<u8>>)> {
        let mut s = self.lock();
        if s.active.is_some() {
            return None;
        }
        s.next_id += 1;
        let id = s.next_id;
        let (tx, rx) = mpsc::channel(SESSION_BUFFER);
        s.active = Some(id);
        s.pending = Some(ObsSession { id, input: rx });
        Some((id, tx))
    }

    /// The publisher `id` is gone (a stream the channel never took goes with it).
    pub fn close(&self, id: u64) {
        let mut s = self.lock();
        if s.active == Some(id) {
            s.active = None;
        }
        if s.pending.as_ref().is_some_and(|p| p.id == id) {
            s.pending = None;
        }
    }

    /// The stream of a publisher that has just started, for the channel.
    pub fn take_session(&self) -> Option<ObsSession> {
        self.lock().pending.take()
    }

    /// A publisher is streaming now.
    pub fn connected(&self) -> bool {
        self.lock().active.is_some()
    }

    pub fn listening(&self) -> Listening {
        self.lock().listening.clone()
    }

    pub(crate) fn set_listening(&self, listening: Listening) {
        self.lock().listening = listening;
    }
}

/// The OBS input's settings (desktop: `config.json`; headless: `OBS_*`).
#[derive(Debug, Clone, PartialEq)]
pub struct ObsConfig {
    /// Whether the listener runs at all.
    pub enabled: bool,
    pub port: u16,
    /// The stream key (never logged, never in public answers).
    pub key: String,
    /// Other devices of the home network may connect (listens on every address).
    pub lan: bool,
    /// The stream takes the air at once; the film on air pauses where it is.
    pub priority: bool,
}

impl Default for ObsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            key: String::new(),
            lan: false,
            priority: true,
        }
    }
}

/// What a publisher's `onMetaData` says about its stream.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StreamMeta {
    pub height: Option<u32>,
    pub fps: Option<f64>,
    /// `audiocodecid` is there (OBS always sends audio).
    pub has_audio: bool,
}

/// The metadata of an FLV script tag (a whole tag as [`rtmp::flv_tag`] makes it); `None`: not
/// a script tag with `onMetaData`.
pub fn stream_meta(tag: &[u8]) -> Option<StreamMeta> {
    if tag.first() != Some(&18) || tag.len() < 15 {
        return None;
    }
    let values = rtmp::amf_decode(&tag[11..tag.len() - 4]).ok()?;
    if values.first()?.as_str()? != "onMetaData" {
        return None;
    }
    let props = match values.get(1)? {
        rtmp::Amf::EcmaArray(p) | rtmp::Amf::Object(p) => p,
        _ => return None,
    };
    let number = |key: &str| {
        props
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_number())
            .filter(|n| n.is_finite() && *n > 0.0)
    };
    Some(StreamMeta {
        height: number("height").map(|h| h as u32),
        fps: number("framerate").or_else(|| number("videoframerate")),
        has_audio: props.iter().any(|(k, _)| k == "audiocodecid"),
    })
}

/// The address OBS's "Server" field takes for `host`.
pub fn server_url(host: &str, port: u16) -> String {
    format!("rtmp://{host}:{port}/{APP}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_random_letters_and_digits() {
        let (a, b) = (new_key(), new_key());
        assert_eq!(a.len(), KEY_LEN);
        assert_ne!(a, b);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn only_the_same_key_matches_and_an_empty_one_never() {
        assert!(key_matches("abc123", "abc123"));
        assert!(!key_matches("abc123", "abc124"));
        assert!(!key_matches("abc123", "abc12"));
        assert!(!key_matches("abc123", ""));
        assert!(!key_matches("", ""));
    }

    #[test]
    fn one_publisher_at_a_time_and_its_stream_goes_with_it() {
        let hub = ObsHub::default();
        let (first, _tx) = hub.open().unwrap();
        assert!(hub.connected());
        assert!(hub.open().is_none(), "a second publisher is refused");
        hub.close(first);
        assert!(hub.take_session().is_none(), "the untaken stream is gone");
        let (second, _tx) = hub.open().unwrap();
        assert_eq!(hub.take_session().unwrap().id, second);
        assert!(hub.take_session().is_none());
    }
}
