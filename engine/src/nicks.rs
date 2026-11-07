//! Viewer nicks with protection against impersonation (spec section 8): a nick is reserved by
//! its skeleton (look-alike letters collapse into one), for the device that first used it.
//!
//! Limits, by design:
//! - Blocking is by device key. A blocked viewer can still send anonymous requests by dropping
//!   the key (or sending none); the mitigation is moderation mode, and once the request page
//!   requires a key for a nick (Task 16) a nick can no longer be used without one.
//! - The badge (360 hues x 12 shapes = 4320 combinations) is a visual aid to tell viewers
//!   apart, not proof of identity: two devices can collide.
//! - The store runs on the engine's single database connection (behind its mutex), so a check
//!   and the following claim are not interleaved with another request's; the claim still
//!   re-reads the holder inside its transaction, so it cannot take over a nick held by another
//!   device on its own.
//!
//! A nick can be protected with a PIN (spec `2026-11-07-nick-pin-design.md`): its devices are
//! then the keys bound in `nick_keys` (a device is bound to one protected nick at most), and
//! another device adopts it with the PIN or the recovery code (`crate::nick_pin` and the
//! routes check those; this store only records the result).

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

pub const NICK_MAX_CHARS: usize = 24;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
pub const NICK_HOLD_MS: i64 = 30 * DAY_MS;
/// How long a protected nick is held after its last use.
pub const NICK_PROTECTED_HOLD_MS: i64 = 180 * DAY_MS;
/// Most devices bound to one protected nick; binding another drops the oldest.
pub const NICK_MAX_DEVICES: i64 = 8;

/// The hold time of a nick.
pub fn hold_ms(protected: bool) -> i64 {
    if protected {
        NICK_PROTECTED_HOLD_MS
    } else {
        NICK_HOLD_MS
    }
}

/// The stored row of a nick, as the holder checks need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldNick {
    pub display: String,
    /// The device that used the nick last.
    pub key_hash: String,
    pub last_used_at: i64,
    /// Argon2 PHC of the PIN; `Some` for a protected nick.
    pub pin_hash: Option<String>,
    /// SHA-256 (hex) of the recovery code.
    pub recovery_hash: Option<String>,
}

impl HeldNick {
    pub fn protected(&self) -> bool {
        self.pin_hash.is_some()
    }

    /// Whether the nick is free for anyone at `now` (unused for its hold time).
    pub fn expired(&self, now: i64) -> bool {
        now.saturating_sub(self.last_used_at) >= hold_ms(self.protected())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    pub hue: u16,
    pub shape: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NickError {
    Taken,
    Reserved,
    Blocked,
    /// The database failed; the request must not pass or be told the nick is taken.
    Db(String),
}

impl From<rusqlite::Error> for NickError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NickEntry {
    pub skeleton: String,
    pub display: String,
    pub key_hash: String,
    pub last_used_at: i64,
    pub blocked: bool,
    /// Id of the block when the device is blocked.
    pub block_id: Option<i64>,
    /// Protected with a PIN.
    pub protected: bool,
    /// Devices bound to the nick (0 without a PIN).
    pub devices: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedDevice {
    pub id: i64,
    pub key_hash: String,
    /// The nick the device used last when it was blocked.
    pub nick: Option<String>,
    pub blocked_at: i64,
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

/// Invisible format characters (Unicode category Cf) and blank fillers: they show nothing, so
/// a nick with them looks like one without.
pub fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{8e2}'
        | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}'
        | '\u{fff9}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}' | '\u{13430}'..='\u{1343f}'
        | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}' | '\u{e0001}'
        | '\u{e0020}'..='\u{e007f}'
        // Blank fillers that are letters or symbols by category.
        | '\u{115f}' | '\u{1160}' | '\u{3164}' | '\u{ffa0}' | '\u{2800}')
}

/// What a character becomes when it looks like another one (Cyrillic and Greek look-alikes
/// become Latin letters, then digits and bars that read as letters).
fn confusable(c: char) -> Option<&'static str> {
    Some(match c {
        // Letters without a decomposition.
        '\u{142}' => "i",
        '\u{131}' => "i",
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
        '\u{456}' | '\u{457}' | '\u{4cf}' => "i",
        '\u{458}' => "j",
        '\u{455}' => "s",
        // Greek.
        '\u{3b1}' => "a",
        '\u{3b2}' => "b",
        '\u{3b5}' => "e",
        '\u{3b7}' => "n",
        '\u{3b9}' => "i",
        '\u{3ba}' => "k",
        '\u{3bd}' => "v",
        '\u{3c5}' => "u",
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

/// The reservation key of a nick: NFKC, lower case, no invisible characters, whitespace or
/// combining marks, look-alike characters folded (hiragana read as katakana), no dots, hyphens
/// or underscores, and `rn` read as `m`.
pub fn skeleton(nick: &str) -> String {
    let lowered: String = nick.nfkc().flat_map(char::to_lowercase).collect();
    let mut out = String::new();
    for c in lowered.nfd() {
        if is_invisible(c) || is_combining_mark(c) {
            continue;
        }
        if c.is_whitespace() || matches!(c, '.' | '-' | '_') {
            continue;
        }
        let c = if ('\u{3041}'..='\u{3096}').contains(&c) {
            char::from_u32(u32::from(c) + 0x60).unwrap_or(c)
        } else {
            c
        };
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

    /// The stored row of a nick (by skeleton), whether or not its hold time is over.
    pub fn held(&self, skel: &str) -> rusqlite::Result<Option<HeldNick>> {
        self.conn
            .query_row(
                "SELECT display, key_hash, last_used_at, pin_hash, recovery_hash
                 FROM viewer_nicks WHERE skeleton = ?",
                [skel],
                |r| {
                    Ok(HeldNick {
                        display: r.get(0)?,
                        key_hash: r.get(1)?,
                        last_used_at: r.get(2)?,
                        pin_hash: r.get(3)?,
                        recovery_hash: r.get(4)?,
                    })
                },
            )
            .optional()
    }

    /// Whether the device holds the nick: the device that used it last for a nick without a
    /// PIN, any bound device for a protected one.
    fn holds(&self, skel: &str, held: &HeldNick, key_hash: &str) -> rusqlite::Result<bool> {
        if held.protected() {
            self.is_bound(skel, key_hash)
        } else {
            Ok(held.key_hash == key_hash)
        }
    }

    /// Whether the device is bound to this protected nick.
    pub fn is_bound(&self, skel: &str, key_hash: &str) -> rusqlite::Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM nick_keys WHERE key_hash = ? AND skeleton = ?",
                [key_hash, skel],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Whether a device of the nick (the one that used it last or a bound one) is blocked.
    pub fn nick_blocked(&self, skel: &str) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM blocked_devices b WHERE b.key_hash IN (
               SELECT key_hash FROM viewer_nicks WHERE skeleton = ?1
               UNION SELECT key_hash FROM nick_keys WHERE skeleton = ?1))",
            [skel],
            |r| r.get(0),
        )
    }

    /// Reserves `nick` for the device (or renews its reservation) at `now` (Unix ms). A nick
    /// held by another device within its hold time is taken; a device changing its nick frees
    /// the old one, unless that one is protected (it stays with its PIN).
    pub fn claim(&self, nick: &str, key_hash: &str, now: i64) -> Result<(), NickError> {
        self.check(nick, Some(key_hash), now)?;
        let skel = skeleton(nick);
        let tx = self.conn.unchecked_transaction()?;
        // Read again inside the transaction: whatever happened between `check` and here, a
        // nick another device holds is not taken over.
        let held = self.held(&skel)?;
        let holder = match &held {
            Some(h) => self.holds(&skel, h, key_hash)?,
            None => false,
        };
        if let Some(h) = &held
            && !holder
            && !h.expired(now)
        {
            return Err(NickError::Taken);
        }
        tx.execute(
            "DELETE FROM viewer_nicks WHERE key_hash = ? AND skeleton != ? AND pin_hash IS NULL",
            params![key_hash, skel],
        )?;
        match &held {
            None => {
                tx.execute(
                    "INSERT INTO viewer_nicks (skeleton, display, key_hash, created_at, last_used_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![skel, nick, key_hash, now],
                )?;
            }
            Some(_) if holder => {
                tx.execute(
                    "UPDATE viewer_nicks SET display = ?2, key_hash = ?3, last_used_at = ?4
                     WHERE skeleton = ?1",
                    params![skel, nick, key_hash, now],
                )?;
            }
            // Expired: a new holder, without the old PIN, code or devices.
            Some(_) => {
                tx.execute("DELETE FROM nick_keys WHERE skeleton = ?", [&skel])?;
                tx.execute(
                    "UPDATE viewer_nicks SET display = ?2, key_hash = ?3, created_at = ?4,
                       last_used_at = ?4, pin_hash = NULL, recovery_hash = NULL
                     WHERE skeleton = ?1",
                    params![skel, nick, key_hash, now],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether `nick` may be used by the device (`None`: a viewer without a device key, who
    /// can use a free nick but never reserves it).
    pub fn check(&self, nick: &str, key_hash: Option<&str>, now: i64) -> Result<(), NickError> {
        if let Some(key) = key_hash
            && self.is_blocked(key)?
        {
            return Err(NickError::Blocked);
        }
        let skel = skeleton(nick);
        if is_reserved(&skel) {
            return Err(NickError::Reserved);
        }
        let Some(held) = self.held(&skel)? else {
            return Ok(());
        };
        if held.expired(now) {
            return Ok(());
        }
        let holder = match key_hash {
            Some(key) => self.holds(&skel, &held, key)?,
            None => false,
        };
        if !holder {
            return Err(NickError::Taken);
        }
        // A protected nick whose device was blocked stays blocked on its other devices.
        if held.protected() && self.nick_blocked(&skel)? {
            return Err(NickError::Blocked);
        }
        Ok(())
    }

    /// Binds the device to the protected nick (moving it from another one), keeping at most
    /// [`NICK_MAX_DEVICES`] devices (the oldest binding goes).
    fn bind(&self, skel: &str, key_hash: &str, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO nick_keys (key_hash, skeleton, added_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key_hash) DO UPDATE SET skeleton = excluded.skeleton,
               added_at = excluded.added_at",
            params![key_hash, skel, now],
        )?;
        self.conn.execute(
            "DELETE FROM nick_keys WHERE skeleton = ?1 AND key_hash NOT IN (
               SELECT key_hash FROM nick_keys WHERE skeleton = ?1
               ORDER BY added_at DESC, rowid DESC LIMIT ?2)",
            params![skel, NICK_MAX_DEVICES],
        )?;
        Ok(())
    }

    /// Sets the PIN (and the recovery code) of `nick` for the device, claiming the nick first
    /// like a request does (a free one becomes the device's). The caller has checked the
    /// current PIN of a nick that already has one.
    pub fn protect(
        &self,
        nick: &str,
        key_hash: &str,
        pin_hash: &str,
        recovery_hash: &str,
        now: i64,
    ) -> Result<(), NickError> {
        self.claim(nick, key_hash, now)?;
        let skel = skeleton(nick);
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE viewer_nicks SET pin_hash = ?2, recovery_hash = ?3 WHERE skeleton = ?1",
            params![skel, pin_hash, recovery_hash],
        )?;
        self.bind(&skel, key_hash, now)?;
        tx.commit()?;
        Ok(())
    }

    /// Binds the device to the protected nick after the caller verified its PIN or recovery
    /// code; `sign_out_others` drops every other device, `new_pin` (PHC, code hash) replaces the
    /// PIN and the code. Returns the nick as it is written. A blocked device — this one or one
    /// of the nick — is refused.
    pub fn adopt(
        &self,
        skel: &str,
        key_hash: &str,
        sign_out_others: bool,
        new_pin: Option<(&str, &str)>,
        now: i64,
    ) -> Result<String, NickError> {
        if self.is_blocked(key_hash)? || self.nick_blocked(skel)? {
            return Err(NickError::Blocked);
        }
        let held = self
            .held(skel)?
            .filter(|h| h.protected())
            .ok_or(NickError::Taken)?;
        let tx = self.conn.unchecked_transaction()?;
        if sign_out_others {
            tx.execute("DELETE FROM nick_keys WHERE skeleton = ?", [skel])?;
        }
        if let Some((pin_hash, recovery_hash)) = new_pin {
            tx.execute(
                "UPDATE viewer_nicks SET pin_hash = ?2, recovery_hash = ?3 WHERE skeleton = ?1",
                params![skel, pin_hash, recovery_hash],
            )?;
        }
        // Like a claim: the device's other nick without a PIN is freed.
        tx.execute(
            "DELETE FROM viewer_nicks WHERE key_hash = ? AND skeleton != ? AND pin_hash IS NULL",
            params![key_hash, skel],
        )?;
        self.bind(skel, key_hash, now)?;
        tx.execute(
            "UPDATE viewer_nicks SET key_hash = ?2, last_used_at = ?3 WHERE skeleton = ?1",
            params![skel, key_hash, now],
        )?;
        tx.commit()?;
        Ok(held.display)
    }

    /// The owner's reset: the nick loses its PIN, code and bound devices and stays with the
    /// device that used it last, as a nick without a PIN. Returns whether it had a PIN.
    pub fn reset_pin(&self, skel: &str) -> rusqlite::Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE viewer_nicks SET pin_hash = NULL, recovery_hash = NULL
             WHERE skeleton = ? AND pin_hash IS NOT NULL",
            [skel],
        )?;
        tx.execute("DELETE FROM nick_keys WHERE skeleton = ?", [skel])?;
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn is_blocked(&self, key_hash: &str) -> rusqlite::Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM blocked_devices WHERE key_hash = ?",
                [key_hash],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Frees a nick; returns whether it was held.
    pub fn release(&self, skeleton: &str) -> rusqlite::Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM nick_keys WHERE skeleton = ?", [skeleton])?;
        let held = tx.execute("DELETE FROM viewer_nicks WHERE skeleton = ?", [skeleton])? > 0;
        tx.commit()?;
        Ok(held)
    }

    /// Blocks every device of the nick: the one that used it last and, for a protected nick,
    /// each bound one. Returns whether the nick is held.
    pub fn block_nick(&self, skeleton: &str) -> rusqlite::Result<bool> {
        let Some(held) = self.held(skeleton)? else {
            return Ok(false);
        };
        let tx = self.conn.unchecked_transaction()?;
        self.block_device(&held.key_hash)?;
        tx.execute(
            "INSERT INTO blocked_devices (key_hash, blocked_at, nick)
             SELECT key_hash, ?2, ?3 FROM nick_keys WHERE skeleton = ?1
             ON CONFLICT(key_hash) DO NOTHING",
            params![skeleton, crate::db::now_ms(), held.display],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Blocks the device, remembering the nick it used last (kept when the nick is released).
    pub fn block_device(&self, key_hash: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO blocked_devices (key_hash, blocked_at, nick)
             VALUES (?1, ?2, (SELECT display FROM viewer_nicks WHERE key_hash = ?1
                              ORDER BY last_used_at DESC LIMIT 1))
             ON CONFLICT(key_hash) DO UPDATE SET nick = COALESCE(excluded.nick, nick)",
            params![key_hash, crate::db::now_ms()],
        )?;
        Ok(())
    }

    /// Lifts the block with this id; returns whether there was one.
    pub fn unblock(&self, id: i64) -> rusqlite::Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM blocked_devices WHERE rowid = ?", [id])?
            > 0)
    }

    /// Lifts the block of this device; returns whether there was one.
    pub fn unblock_key(&self, key_hash: &str) -> rusqlite::Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM blocked_devices WHERE key_hash = ?", [key_hash])?
            > 0)
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

    /// Nicks used within their hold time, most recently used first (older ones are free
    /// anyway).
    pub fn list(&self, now: i64) -> rusqlite::Result<Vec<NickEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.skeleton, n.display, n.key_hash, n.last_used_at, b.rowid,
               n.pin_hash IS NOT NULL,
               (SELECT COUNT(*) FROM nick_keys k WHERE k.skeleton = n.skeleton)
             FROM viewer_nicks n LEFT JOIN blocked_devices b ON b.key_hash = n.key_hash
             WHERE n.last_used_at > CASE WHEN n.pin_hash IS NULL THEN ?1 ELSE ?2 END
             ORDER BY n.last_used_at DESC, n.skeleton",
        )?;
        stmt.query_map(
            [
                now.saturating_sub(NICK_HOLD_MS),
                now.saturating_sub(NICK_PROTECTED_HOLD_MS),
            ],
            |r| {
                let block_id: Option<i64> = r.get(4)?;
                Ok(NickEntry {
                    skeleton: r.get(0)?,
                    display: r.get(1)?,
                    key_hash: r.get(2)?,
                    last_used_at: r.get(3)?,
                    blocked: block_id.is_some(),
                    block_id,
                    protected: r.get(5)?,
                    devices: r.get(6)?,
                })
            },
        )?
        .collect()
    }

    /// Every blocked device, newest block first, whether or not it still holds a nick.
    pub fn blocked(&self) -> rusqlite::Result<Vec<BlockedDevice>> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, key_hash, nick, blocked_at FROM blocked_devices
             ORDER BY blocked_at DESC, rowid DESC",
        )?;
        stmt.query_map([], |r| {
            Ok(BlockedDevice {
                id: r.get(0)?,
                key_hash: r.get(1)?,
                nick: r.get(2)?,
                blocked_at: r.get(3)?,
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

    const NOW: i64 = 10_000;
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
        assert_eq!(s.list(NOW).unwrap().len(), 2);
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
        assert!(s.list(NOW).unwrap().is_empty());
    }

    #[test]
    fn blocked_device_cannot_claim_and_release_frees_the_nick() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.claim("Kasia", &h(K1), 1000).unwrap();
        let sk = skeleton("Kasia");
        assert_eq!(s.key_hash_of(&sk).unwrap(), Some(h(K1)));
        s.block_device(&h(K1)).unwrap();
        assert!(s.is_blocked(&h(K1)).unwrap());
        assert!(!s.is_blocked(&h(K2)).unwrap());
        assert_eq!(s.claim("Inna", &h(K1), 2000), Err(NickError::Blocked));
        assert!(
            s.list(NOW)
                .unwrap()
                .iter()
                .all(|e| e.blocked == (e.key_hash == h(K1)))
        );
        assert!(s.release(&sk).unwrap());
        assert!(!s.release(&sk).unwrap());
        assert_eq!(s.claim("Kasia", &h(K2), 3000), Ok(()));
    }
}

#[cfg(test)]
mod invisible_tests {
    use super::*;

    #[test]
    fn invisible_characters_and_marks_do_not_change_the_skeleton() {
        let base = skeleton("Kasia");
        for s in [
            "Kas\u{200B}ia",
            "Ka\u{200D}sia",
            "K\u{2060}asia",
            "Ka\u{ad}sia",
            "\u{200E}Kasia\u{200F}",
            "\u{202E}Kasia",
            "Ka\u{3099}sia",
            "Ka\u{FE0F}sia",
            "Ka\u{FEFF}sia",
            "Ka\u{a0}s\u{2003}ia",
            "Ka\u{3164}sia",
            "Ka\u{301}sia",
        ] {
            assert_eq!(skeleton(s), base, "{s:?}");
        }
        assert_eq!(skeleton("Ho\u{200B}st"), skeleton("Host"));
        assert_eq!(skeleton("r\u{200B}n"), skeleton("m"));
    }

    #[test]
    fn more_look_alikes_fold() {
        assert_eq!(skeleton("\u{3ba}\u{3c5}"), skeleton("ku"));
        assert_eq!(skeleton("ほすと"), skeleton("ホスト"));
        assert_eq!(skeleton("\u{4cf}"), skeleton("i"));
        assert_eq!(skeleton("\u{131}"), skeleton("i"));
        assert_eq!(skeleton("Kas\u{131}a"), skeleton("Kasia"));
    }
}

#[cfg(test)]
mod block_tests {
    use super::*;
    use crate::db::open_database;

    const K1: &str = "1111111111111111111111111111111a";
    const K2: &str = "2222222222222222222222222222222b";

    fn mem() -> Connection {
        open_database(std::path::Path::new(":memory:")).unwrap()
    }

    fn h(k: &str) -> String {
        hash_device_key(k).unwrap()
    }

    #[test]
    fn a_block_outlives_the_release_and_can_be_lifted() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.claim("Kasia", &h(K1), 1000).unwrap();
        s.block_device(&h(K1)).unwrap();
        assert!(s.release(&skeleton("Kasia")).unwrap());
        // The nick is gone, the block is not: it is listed with the last nick and its date.
        assert!(s.list(2000).unwrap().is_empty());
        let blocked = s.blocked().unwrap();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].nick.as_deref(), Some("Kasia"));
        assert!(blocked[0].blocked_at > 0);
        assert_eq!(s.claim("Inna", &h(K1), 3000), Err(NickError::Blocked));
        // Blocking again keeps the nick and the single entry.
        s.block_device(&h(K1)).unwrap();
        assert_eq!(s.blocked().unwrap().len(), 1);
        assert_eq!(s.blocked().unwrap()[0].nick.as_deref(), Some("Kasia"));
        // Lifting it.
        assert!(s.unblock(blocked[0].id).unwrap());
        assert!(!s.unblock(blocked[0].id).unwrap());
        assert!(s.blocked().unwrap().is_empty());
        assert_eq!(s.claim("Inna", &h(K1), 3000), Ok(()));
    }

    #[test]
    fn the_list_hides_nicks_unused_for_30_days() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.claim("Kasia", &h(K1), 1000).unwrap();
        assert_eq!(s.list(1000 + NICK_HOLD_MS - 1).unwrap().len(), 1);
        assert!(s.list(1000 + NICK_HOLD_MS).unwrap().is_empty());
    }

    #[test]
    fn the_upsert_cannot_take_a_nick_held_by_another_device() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.claim("Kasia", &h(K1), 1000).unwrap();
        // Even without `check`, the write itself refuses.
        let tx_changed = conn
            .execute(
                "INSERT INTO viewer_nicks (skeleton, display, key_hash, created_at, last_used_at)
                 VALUES (?1, 'x', ?2, 2000, 2000)
                 ON CONFLICT(skeleton) DO UPDATE SET key_hash = excluded.key_hash
                 WHERE viewer_nicks.key_hash = excluded.key_hash OR viewer_nicks.last_used_at <= ?3",
                params![skeleton("Kasia"), h(K2), 2000 - NICK_HOLD_MS],
            )
            .unwrap();
        assert_eq!(tx_changed, 0);
        assert_eq!(s.key_hash_of(&skeleton("Kasia")).unwrap(), Some(h(K1)));
    }

    #[test]
    fn a_database_failure_is_an_error_not_a_taken_nick() {
        let conn = mem();
        conn.execute_batch("DROP TABLE blocked_devices").unwrap();
        let s = NickStore::new(&conn);
        assert!(matches!(
            s.claim("Kasia", &h(K1), 1000),
            Err(NickError::Db(_))
        ));
        assert!(s.is_blocked(&h(K1)).is_err());
        conn.execute_batch("DROP TABLE viewer_nicks").unwrap();
        assert!(matches!(
            s.check("Kasia", None, 1000),
            Err(NickError::Db(_))
        ));
    }
}

#[cfg(test)]
mod pin_tests {
    use super::*;
    use crate::db::open_database;

    const K1: &str = "1111111111111111111111111111111a";
    const K2: &str = "2222222222222222222222222222222b";
    const K3: &str = "3333333333333333333333333333333c";
    const T: i64 = 1_000_000_000_000;

    fn mem() -> Connection {
        open_database(std::path::Path::new(":memory:")).unwrap()
    }

    fn h(k: &str) -> String {
        hash_device_key(k).unwrap()
    }

    fn sk() -> String {
        skeleton("Kasia")
    }

    #[test]
    fn a_protected_nick_is_held_by_its_bound_devices_only() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.protect("Kasia", &h(K1), "phc", "rec", T).unwrap();
        assert!(s.is_bound(&sk(), &h(K1)).unwrap());
        assert_eq!(s.claim("Kasia", &h(K2), T + 1), Err(NickError::Taken));
        // Adopted by a second device, keeping the first.
        assert_eq!(
            s.adopt(&sk(), &h(K2), false, None, T + 2),
            Ok("Kasia".into())
        );
        assert_eq!(s.claim("kasia", &h(K1), T + 3), Ok(()));
        assert_eq!(s.claim("KASIA", &h(K2), T + 4), Ok(()));
        let e = &s.list(T + 5).unwrap()[0];
        assert!(e.protected);
        assert_eq!(e.devices, 2);
        // Signing the others out leaves only the adopting device.
        s.adopt(&sk(), &h(K3), true, None, T + 6).unwrap();
        assert_eq!(s.claim("Kasia", &h(K1), T + 7), Err(NickError::Taken));
        assert_eq!(s.claim("Kasia", &h(K3), T + 7), Ok(()));
    }

    #[test]
    fn a_protected_nick_survives_a_change_of_nick_and_holds_for_180_days() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.protect("Kasia", &h(K1), "phc", "rec", T).unwrap();
        s.claim("Inna", &h(K1), T + 1).unwrap();
        assert_eq!(s.claim("Kasia", &h(K2), T + 2), Err(NickError::Taken));
        assert_eq!(s.claim("Kasia", &h(K1), T + 3), Ok(()));
        // Not freed after 30 days...
        let later = T + 3 + NICK_HOLD_MS;
        assert_eq!(s.claim("Kasia", &h(K2), later), Err(NickError::Taken));
        assert_eq!(s.list(later).unwrap().len(), 1, "only the protected nick");
        // ...but after 180 days, and then without its PIN and devices.
        let free = T + 3 + NICK_PROTECTED_HOLD_MS;
        assert_eq!(s.claim("Kasia", &h(K2), free), Ok(()));
        let held = s.held(&sk()).unwrap().unwrap();
        assert!(held.pin_hash.is_none() && held.recovery_hash.is_none());
        assert!(!s.is_bound(&sk(), &h(K1)).unwrap());
    }

    #[test]
    fn a_device_is_bound_to_one_protected_nick_and_at_most_eight_devices_stay() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.protect("Kasia", &h(K1), "phc", "rec", T).unwrap();
        s.protect("Basia", &h(K1), "phc", "rec", T + 1).unwrap();
        assert!(!s.is_bound(&sk(), &h(K1)).unwrap());
        let basia = skeleton("Basia");
        assert!(s.is_bound(&basia, &h(K1)).unwrap());
        for i in 0..10u32 {
            let key = format!("{i:032x}");
            s.adopt(&basia, &h(&key), false, None, T + 10 + i64::from(i))
                .unwrap();
        }
        let devices: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nick_keys WHERE skeleton = ?",
                [&basia],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(devices, NICK_MAX_DEVICES);
        assert!(!s.is_bound(&basia, &h(K1)).unwrap(), "the oldest went");
    }

    #[test]
    fn reset_release_and_block_cover_every_device() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.protect("Kasia", &h(K1), "phc", "rec", T).unwrap();
        s.adopt(&sk(), &h(K2), false, None, T + 1).unwrap();
        // Reset: no PIN, no bindings, the last device keeps the nick.
        assert!(s.reset_pin(&sk()).unwrap());
        assert!(!s.reset_pin(&sk()).unwrap());
        assert_eq!(s.claim("Kasia", &h(K1), T + 2), Err(NickError::Taken));
        assert_eq!(s.claim("Kasia", &h(K2), T + 2), Ok(()));
        // Block: every bound device, and adopting is refused.
        s.protect("Kasia", &h(K2), "phc", "rec", T + 3).unwrap();
        s.adopt(&sk(), &h(K1), false, None, T + 4).unwrap();
        assert!(s.block_nick(&sk()).unwrap());
        assert!(s.is_blocked(&h(K1)).unwrap() && s.is_blocked(&h(K2)).unwrap());
        let blocked = s.blocked().unwrap();
        assert_eq!(blocked.len(), 2);
        assert!(blocked.iter().all(|b| b.nick.as_deref() == Some("Kasia")));
        assert_eq!(
            s.adopt(&sk(), &h(K3), false, None, T + 5),
            Err(NickError::Blocked)
        );
        // Release removes the bindings too.
        assert!(s.release(&sk()).unwrap());
        assert!(!s.is_bound(&sk(), &h(K1)).unwrap());
    }

    #[test]
    fn a_protected_nick_with_a_blocked_device_is_blocked_on_the_others() {
        let conn = mem();
        let s = NickStore::new(&conn);
        s.protect("Kasia", &h(K1), "phc", "rec", T).unwrap();
        s.adopt(&sk(), &h(K2), false, None, T + 1).unwrap();
        s.block_device(&h(K1)).unwrap();
        assert_eq!(
            s.check("Kasia", Some(&h(K2)), T + 2),
            Err(NickError::Blocked)
        );
    }

    #[test]
    fn adopting_needs_a_protected_nick_and_may_replace_the_pin() {
        let conn = mem();
        let s = NickStore::new(&conn);
        assert_eq!(
            s.adopt(&sk(), &h(K1), false, None, T),
            Err(NickError::Taken)
        );
        s.claim("Kasia", &h(K1), T).unwrap();
        assert_eq!(
            s.adopt(&sk(), &h(K2), false, None, T),
            Err(NickError::Taken)
        );
        s.protect("Kasia", &h(K1), "phc", "rec", T).unwrap();
        s.adopt(&sk(), &h(K2), true, Some(("phc2", "rec2")), T + 1)
            .unwrap();
        let held = s.held(&sk()).unwrap().unwrap();
        assert_eq!(held.pin_hash.as_deref(), Some("phc2"));
        assert_eq!(held.recovery_hash.as_deref(), Some("rec2"));
        assert_eq!(held.key_hash, h(K2));
    }
}
