//! Viewer nicks with protection against impersonation (spec section 8): a nick is reserved by
//! its skeleton (look-alike letters collapse into one), for the device that first used it.

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

pub const NICK_MAX_CHARS: usize = 24;
pub const NICK_HOLD_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    pub hue: u16,
    pub shape: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NickError {
    Taken,
    Reserved,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NickEntry {
    pub skeleton: String,
    pub display: String,
    pub key_hash: String,
    pub last_used_at: i64,
    pub blocked: bool,
}

/// Words only the owner may use as a nick, in every interface language (compared by skeleton).
const RESERVED: [&str; 6] = [
    "Gospodarz",
    "Host",
    "Anfitri\u{f3}n",
    "Gastgeber",
    "H\u{f4}te",
    "\u{30db}\u{30b9}\u{30c8}",
];

/// What a character becomes when it looks like another one (Cyrillic and Greek look-alikes
/// become Latin letters, then digits and bars that read as letters).
fn confusable(c: char) -> Option<&'static str> {
    Some(match c {
        // Letters without a decomposition.
        '\u{142}' => "i",
        '\u{111}' => "d",
        '\u{f8}' => "o",
        '\u{df}' => "ss",
        '\u{e6}' => "ae",
        '\u{153}' => "oe",
        // Cyrillic.
        '\u{430}' => "a",
        '\u{432}' => "b",
        '\u{435}' | '\u{451}' => "e",
        '\u{43a}' => "k",
        '\u{43c}' => "m",
        '\u{43d}' => "h",
        '\u{43e}' => "o",
        '\u{440}' => "p",
        '\u{441}' => "c",
        '\u{442}' => "t",
        '\u{443}' => "y",
        '\u{445}' => "x",
        '\u{456}' | '\u{457}' => "i",
        '\u{458}' => "j",
        '\u{455}' => "s",
        // Greek.
        '\u{3b1}' => "a",
        '\u{3b2}' => "b",
        '\u{3b5}' => "e",
        '\u{3b7}' => "n",
        '\u{3b9}' => "i",
        '\u{3ba}' => "k",
        '\u{3bd}' | '\u{3c5}' => "v",
        '\u{3bf}' => "o",
        '\u{3c1}' => "p",
        '\u{3c4}' => "t",
        '\u{3c7}' => "x",
        '\u{3c9}' => "w",
        // Digits and bars that read as letters.
        '1' | 'l' | '|' => "i",
        '0' => "o",
        '5' => "s",
        _ => return None,
    })
}

/// The reservation key of a nick: NFKC, lower case, no diacritics, look-alike characters
/// folded, no spaces, dots, hyphens or underscores, and `rn` read as `m`.
pub fn skeleton(nick: &str) -> String {
    let lowered: String = nick.nfkc().flat_map(char::to_lowercase).collect();
    let mut out = String::new();
    for c in lowered.nfd() {
        if ('\u{300}'..='\u{36f}').contains(&c) {
            continue;
        }
        if c.is_whitespace() || matches!(c, '.' | '-' | '_') {
            continue;
        }
        match confusable(c) {
            Some(s) => out.push_str(s),
            None => out.push(c),
        }
    }
    out.replace("rn", "m")
}

fn is_reserved(skel: &str) -> bool {
    RESERVED.iter().any(|w| skeleton(w) == skel)
}

/// The icon of a device: a hue (0-359) and one of 12 shapes (1-12) from its key hash.
pub fn badge(key_hash: &str) -> Badge {
    let n = u32::from_str_radix(key_hash.get(..8).unwrap_or(key_hash), 16).unwrap_or(0);
    Badge {
        hue: (n % 360) as u16,
        shape: ((n / 360) % 12) as u8 + 1,
    }
}

/// SHA-256 (hex) of a device key; `None` unless the key is 32 hex digits. The key itself is
/// never stored.
pub fn hash_device_key(key: &str) -> Option<String> {
    if key.len() != 32 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digest = Sha256::digest(key.to_ascii_lowercase().as_bytes());
    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}

pub struct NickStore<'a> {
    conn: &'a Connection,
}

impl<'a> NickStore<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Reserves `nick` for the device (or renews its reservation) at `now` (Unix ms). A nick
    /// held by another device that used it within 30 days is taken; a device changing its nick
    /// frees the old one.
    pub fn claim(&self, nick: &str, key_hash: &str, now: i64) -> Result<(), NickError> {
        self.check(nick, Some(key_hash), now)?;
        let skel = skeleton(nick);
        let write = || -> rusqlite::Result<()> {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM viewer_nicks WHERE key_hash = ? AND skeleton != ?",
                params![key_hash, skel],
            )?;
            tx.execute(
                "INSERT INTO viewer_nicks (skeleton, display, key_hash, created_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(skeleton) DO UPDATE SET display = excluded.display,
                   last_used_at = excluded.last_used_at,
                   created_at = CASE WHEN viewer_nicks.key_hash = excluded.key_hash
                     THEN viewer_nicks.created_at ELSE excluded.created_at END,
                   key_hash = excluded.key_hash",
                params![skel, nick, key_hash, now],
            )?;
            tx.commit()
        };
        // A database failure must not let a request through under a contested nick.
        write().map_err(|_| NickError::Taken)
    }

    /// Whether `nick` may be used by the device (`None`: a viewer without a device key, who
    /// can use a free nick but never reserves it).
    pub fn check(&self, nick: &str, key_hash: Option<&str>, now: i64) -> Result<(), NickError> {
        if key_hash.is_some_and(|k| self.is_blocked(k)) {
            return Err(NickError::Blocked);
        }
        let skel = skeleton(nick);
        if is_reserved(&skel) {
            return Err(NickError::Reserved);
        }
        let held: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT key_hash, last_used_at FROM viewer_nicks WHERE skeleton = ?",
                [&skel],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| NickError::Taken)?;
        if let Some((holder, last)) = &held
            && Some(holder.as_str()) != key_hash
            && now.saturating_sub(*last) < NICK_HOLD_MS
        {
            return Err(NickError::Taken);
        }
        Ok(())
    }

    pub fn is_blocked(&self, key_hash: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM blocked_devices WHERE key_hash = ?",
                [key_hash],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Frees a nick; returns whether it was held.
    pub fn release(&self, skeleton: &str) -> rusqlite::Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM viewer_nicks WHERE skeleton = ?", [skeleton])?
            > 0)
    }

    pub fn block_device(&self, key_hash: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO blocked_devices (key_hash, blocked_at) VALUES (?, ?)",
            params![key_hash, crate::db::now_ms()],
        )?;
        Ok(())
    }

    pub fn key_hash_of(&self, skeleton: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT key_hash FROM viewer_nicks WHERE skeleton = ?",
                [skeleton],
                |r| r.get(0),
            )
            .optional()
    }

    /// Held nicks, most recently used first.
    pub fn list(&self) -> rusqlite::Result<Vec<NickEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.skeleton, n.display, n.key_hash, n.last_used_at,
               EXISTS(SELECT 1 FROM blocked_devices b WHERE b.key_hash = n.key_hash)
             FROM viewer_nicks n ORDER BY n.last_used_at DESC, n.skeleton",
        )?;
        stmt.query_map([], |r| {
            Ok(NickEntry {
                skeleton: r.get(0)?,
                display: r.get(1)?,
                key_hash: r.get(2)?,
                last_used_at: r.get(3)?,
                blocked: r.get::<_, i64>(4)? != 0,
            })
        })?
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_database;

    fn mem() -> Connection {
        open_database(std::path::Path::new(":memory:")).unwrap()
    }

    const K1: &str = "1111111111111111111111111111111a";
    const K2: &str = "2222222222222222222222222222222b";

    fn h(k: &str) -> String {
        hash_device_key(k).unwrap()
    }

    #[test]
    fn look_alike_spellings_share_one_skeleton() {
        let base = skeleton("Kasia");
        for s in [
            "kásia",
            "KASIA",
            "K\u{0430}sia",
            "K a s i a",
            "Ka5ia",
            "K.a-s_i a",
            "Kas1a",
            "Kasla",
            "Kas|a",
        ] {
            assert_eq!(skeleton(s), base, "{s}");
        }
        assert_ne!(skeleton("Kasia"), skeleton("Kasiu"));
        assert_eq!(skeleton("rn"), skeleton("m"));
        assert_eq!(skeleton("Marn"), skeleton("Mam"));
        assert_eq!(skeleton("D0m"), skeleton("dom"));
        assert_eq!(skeleton("Łukasz"), skeleton("lukasz"));
        assert_eq!(skeleton("ホスト"), "ホスト");
    }

    #[test]
    fn device_key_must_be_32_hex_and_is_stored_as_sha256() {
        assert!(hash_device_key("short").is_none());
        assert!(hash_device_key(&"g".repeat(32)).is_none());
        let a = hash_device_key(K1).unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, K1);
        assert_eq!(hash_device_key(&K1.to_uppercase()), Some(a));
    }

    #[test]
    fn badge_is_deterministic_and_differs_between_keys() {
        assert_eq!(badge(&h(K1)), badge(&h(K1)));
        let b = badge(&h(K1));
        assert!(b.hue < 360 && (1..=12).contains(&b.shape));
        let mut keys = Vec::new();
        let mut seed: u64 = 0x1234_5678_9abc_def0;
        for _ in 0..1000 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let hi = seed;
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            keys.push(badge(&h(&format!("{hi:016x}{seed:016x}"))));
        }
        let mut differ = 0u32;
        let mut pairs = 0u32;
        for i in 0..keys.len() {
            for j in i + 1..keys.len() {
                pairs += 1;
                differ += u32::from(keys[i] != keys[j]);
            }
        }
        assert!(f64::from(differ) / f64::from(pairs) >= 0.95);
    }

    #[test]
    fn nick_is_reserved_for_the_first_device() {
        let conn = mem();
        let s = NickStore::new(&conn);
        let now = 1_000_000_000_000;
        assert_eq!(s.claim("Kasia", &h(K1), now), Ok(()));
        assert_eq!(s.claim("kasia", &h(K1), now + 1), Ok(()));
        assert_eq!(s.claim("Ka5ia", &h(K2), now + 2), Err(NickError::Taken));
        assert_eq!(s.claim("Kasiu", &h(K2), now + 2), Ok(()));
    }

    #[test]
    fn nick_is_free_after_30_days_without_use() {
        let conn = mem();
        let s = NickStore::new(&conn);
        let t = 1_000_000_000_000;
        s.claim("Kasia", &h(K1), t).unwrap();
        assert_eq!(
            s.claim("Kasia", &h(K2), t + NICK_HOLD_MS - 1),
            Err(NickError::Taken)
        );
        assert_eq!(s.claim("Kasia", &h(K2), t + NICK_HOLD_MS), Ok(()));
        assert_eq!(
            s.claim("Kasia", &h(K1), t + NICK_HOLD_MS + 1),
            Err(NickError::Taken)
        );
    }

    #[test]
    fn changing_the_nick_releases_the_old_one() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.claim("Kasia", &h(K1), 1000).unwrap();
        s.claim("Basia", &h(K1), 2000).unwrap();
        assert_eq!(s.claim("Kasia", &h(K2), 3000), Ok(()));
        assert_eq!(s.list().unwrap().len(), 2);
    }

    #[test]
    fn host_words_are_reserved_in_every_language() {
        let conn = mem();
        let s = NickStore::new(&conn);
        for w in [
            "Gospodarz",
            "Host",
            "host",
            "H0st",
            "Anfitrión",
            "anfitrion",
            "Gastgeber",
            "Hôte",
            "Hote",
            "ホスト",
            "Hоst",
        ] {
            assert_eq!(s.claim(w, &h(K1), 1), Err(NickError::Reserved), "{w}");
        }
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn blocked_device_cannot_claim_and_release_frees_the_nick() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.claim("Kasia", &h(K1), 1000).unwrap();
        let sk = skeleton("Kasia");
        assert_eq!(s.key_hash_of(&sk).unwrap(), Some(h(K1)));
        s.block_device(&h(K1)).unwrap();
        assert!(s.is_blocked(&h(K1)));
        assert!(!s.is_blocked(&h(K2)));
        assert_eq!(s.claim("Inna", &h(K1), 2000), Err(NickError::Blocked));
        assert!(
            s.list()
                .unwrap()
                .iter()
                .all(|e| e.blocked == (e.key_hash == h(K1)))
        );
        assert!(s.release(&sk).unwrap());
        assert!(!s.release(&sk).unwrap());
        assert_eq!(s.claim("Kasia", &h(K2), 3000), Ok(()));
    }
}
