//! Admin sessions: stateless signed cookie (port of `src/http/auth.ts`).

use std::collections::HashMap;

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
fn safe_equal(a: &str, b: &str) -> bool {
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

pub fn verify_session_token(token: Option<&str>, secret: &str, now_ms: u64) -> bool {
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return false;
    };
    let mut parts = token.split('.');
    let (Some(expires), Some(mac)) = (parts.next(), parts.next()) else {
        return false;
    };
    if expires.is_empty() || mac.is_empty() || !expires.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if !safe_equal(mac, &sign(expires, secret)) {
        return false;
    }
    expires.parse::<f64>().is_ok_and(|e| e > now_ms as f64)
}

pub fn check_password(input: Option<&str>, expected: &str) -> bool {
    input.is_some_and(|i| safe_equal(i, expected))
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
