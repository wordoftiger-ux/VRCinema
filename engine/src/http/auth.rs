//! Admin sessions: stateless signed cookie (port of `src/http/auth.ts`).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

pub const SESSION_COOKIE: &str = "vrcinema_session";
pub const SESSION_TTL_MS: u64 = 30 * 24 * 3600 * 1000;

fn sign(payload: &str, secret: &str) -> String {
    // HMAC accepts keys of any length, so this cannot fail; an empty MAC just never verifies.
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return String::new();
    };
    mac.update(payload.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Equality that does not leak the position of the first difference: both sides are hashed to
/// 32 bytes first, then compared without early exit.
pub(crate) fn safe_equal(a: &str, b: &str) -> bool {
    let ha = Sha256::digest(a.as_bytes());
    let hb = Sha256::digest(b.as_bytes());
    ha.iter()
        .zip(hb.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Token sesji: "<wygaśnięcie ms>.<HMAC>". Bez stanu po stronie serwera.
pub fn create_session_token(secret: &str, ttl_ms: u64, now_ms: u64) -> String {
    let expires = now_ms.saturating_add(ttl_ms).to_string();
    format!("{expires}.{}", sign(&expires, secret))
}

/// Either kind of session counts (`/api/admin/*`).
pub fn verify_session_token(token: Option<&str>, secret: &str, now_ms: u64) -> bool {
    session_kind(token, secret, now_ms).is_some()
}

/// Rodzaj sesji zapisany w podpisanym tokenie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Remote,
    Desktop,
}

/// Jak admin może się zalogować.
#[derive(Debug, Clone)]
pub enum AdminAuth {
    /// Tryb `.env` (etap 1): jedno hasło w postaci jawnej.
    PlainPassword(String),
    /// Aplikacja z oknem: panel zdalny z hasłem Argon2id (PHC) albo wyłączony.
    App {
        remote_enabled: bool,
        password_hash: Option<String>,
        /// Raised whenever the remote panel is switched on again; part of the key remote
        /// sessions are signed with (see [`remote_session_secret`]).
        session_epoch: u64,
    },
}

/// Key for remote-session tokens in the app: derived from the session secret, the current
/// password hash and the epoch, so a new password or switching the panel on again ends every
/// remote session issued before. Window sessions keep using the plain session secret.
pub fn remote_session_secret(
    session_secret: &str,
    password_hash: Option<&str>,
    epoch: u64,
) -> String {
    let hash = password_hash.map_or_else(|| "-".to_string(), |h| format!("+{h}"));
    sign(&format!("remote-session.{epoch}.{hash}"), session_secret)
}

/// Jednorazowy klucz otwierający sesję okna.
#[derive(Debug)]
pub struct DesktopKey {
    current: Mutex<Option<String>>,
}

fn random_key() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("system random number generator");
    URL_SAFE_NO_PAD.encode(bytes)
}

impl DesktopKey {
    pub fn generate() -> (Self, String) {
        let key = random_key();
        (
            Self {
                current: Mutex::new(Some(key.clone())),
            },
            key,
        )
    }

    /// Nowy klucz; poprzedni przestaje działać.
    pub fn rotate(&self) -> String {
        let key = random_key();
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(key.clone());
        key
    }

    /// Prawda najwyżej raz na klucz: udane użycie unieważnia go, błędne nie.
    pub fn redeem(&self, presented: &str) -> bool {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        let ok = current
            .as_deref()
            .is_some_and(|expected| safe_equal(presented, expected));
        if ok {
            *current = None;
        }
        ok
    }
}

/// Argon2id with the default parameters, random salt; PHC string.
pub fn hash_password(p: &str) -> String {
    Argon2::default()
        .hash_password(p.as_bytes())
        .expect("argon2 hashing with default parameters")
        .to_string()
}

/// False for a wrong password and for anything that is not a valid PHC string.
pub fn verify_password(p: &str, phc: &str) -> bool {
    PasswordHash::new(phc).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(p.as_bytes(), &parsed)
            .is_ok()
    })
}

/// Token sesji okna: "<wygaśnięcie ms>.d.<HMAC>"; HMAC obejmuje wygaśnięcie i rodzaj.
pub fn create_desktop_session_token(secret: &str, ttl_ms: u64, now_ms: u64) -> String {
    let expires = now_ms.saturating_add(ttl_ms).to_string();
    let mac = sign(&format!("{expires}.d"), secret);
    format!("{expires}.d.{mac}")
}

pub fn session_kind(token: Option<&str>, secret: &str, now_ms: u64) -> Option<SessionKind> {
    let token = token.filter(|t| !t.is_empty())?;
    let parts: Vec<&str> = token.split('.').collect();
    let (kind, expires, mac) = match parts.as_slice() {
        [expires, "d", mac] => (SessionKind::Desktop, *expires, *mac),
        // Stage-1 format; like the TS split, anything after the MAC is ignored.
        [expires, mac, ..] => (SessionKind::Remote, *expires, *mac),
        _ => return None,
    };
    if expires.is_empty() || mac.is_empty() || !expires.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let signed = match kind {
        SessionKind::Desktop => format!("{expires}.d"),
        SessionKind::Remote => expires.to_string(),
    };
    if !safe_equal(mac, &sign(&signed, secret)) {
        return None;
    }
    expires
        .parse::<f64>()
        .is_ok_and(|e| e > now_ms as f64)
        .then_some(kind)
}

pub fn check_password(input: Option<&str>, expected: &str) -> bool {
    !expected.is_empty() && input.is_some_and(|i| safe_equal(i, expected))
}

/// Like JS `decodeURIComponent`: `None` on malformed escapes or invalid UTF-8.
fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn parse_cookies(header: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(header) = header else {
        return out;
    };
    for part in header.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let value = value.trim();
        out.insert(
            key.trim().to_string(),
            decode_uri_component(value).unwrap_or_else(|| value.to_string()),
        );
    }
    out
}

pub fn session_cookie(token: &str, secure: bool, max_age_s: u64) -> String {
    let mut parts = vec![
        format!("{SESSION_COOKIE}={token}"),
        "Path=/".to_string(),
        "HttpOnly".to_string(),
        "SameSite=Lax".to_string(),
        format!("Max-Age={max_age_s}"),
    ];
    if secure {
        parts.push("Secure".to_string());
    }
    parts.join("; ")
}
