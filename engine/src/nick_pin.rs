//! The PIN of a protected nick (spec `2026-11-07-nick-pin-design.md`): what a PIN may be, the
//! one-time recovery code, and the guard that limits guessing (per address and per nick). The
//! PIN and the code are never kept in clear and never logged; the store keeps an Argon2 PHC of
//! the PIN and a SHA-256 of the code.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::rules::{Clock, RateLimiter};

/// Why a PIN is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinError {
    /// Not 4–8 digits nor a passphrase of 8–64 characters.
    Invalid,
    /// Digits that are too easy to guess (all the same, a run like 1234 or 9876).
    Weak,
}

impl PinError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "nick_pin_invalid",
            Self::Weak => "nick_pin_weak",
        }
    }
}

pub const PASSPHRASE_MIN: usize = 8;
pub const PASSPHRASE_MAX: usize = 64;

/// The PIN as it is hashed: digits as they are, a passphrase NFC-normalised (the same text
/// typed on another device gives the same bytes).
pub fn normalize_pin(pin: &str) -> String {
    pin.nfc().collect()
}

/// Checks a new PIN: 4–8 ASCII digits that are not all the same nor a run, or a passphrase of
/// 8–64 characters that is not digits only.
pub fn validate_pin(pin: &str) -> Result<(), PinError> {
    let pin = normalize_pin(pin);
    if pin.bytes().all(|b| b.is_ascii_digit()) && !pin.is_empty() {
        if !(4..=8).contains(&pin.len()) {
            return Err(PinError::Invalid);
        }
        let d: Vec<i16> = pin.bytes().map(|b| i16::from(b - b'0')).collect();
        let steps: Vec<i16> = d.windows(2).map(|w| w[1] - w[0]).collect();
        if steps.iter().all(|&s| s == 0)
            || steps.iter().all(|&s| s == 1)
            || steps.iter().all(|&s| s == -1)
        {
            return Err(PinError::Weak);
        }
        return Ok(());
    }
    let chars = pin.chars().count();
    if !(PASSPHRASE_MIN..=PASSPHRASE_MAX).contains(&chars) || pin.chars().any(char::is_control) {
        return Err(PinError::Invalid);
    }
    Ok(())
}

/// Argon2id PHC of the PIN (the remote panel's hashing).
pub fn hash_pin(pin: &str) -> String {
    crate::http::auth::hash_password(&normalize_pin(pin))
}

/// Whether the PIN matches the PHC; false for anything malformed.
pub fn verify_pin(pin: &str, phc: &str) -> bool {
    crate::http::auth::verify_password(&normalize_pin(pin), phc)
}

/// A PHC to verify against when there is nothing to verify (no such nick, no PIN), so a miss
/// costs the same Argon2 work as a wrong PIN.
pub fn dummy_phc() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| crate::http::auth::hash_password("vrcinema-no-such-pin"))
}

/// Crockford base 32 (no I, L, O, U).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_SYMBOLS: usize = 16;

/// A new recovery code: 80 random bits as `XXXX-XXXX-XXXX-XXXX`.
pub fn new_recovery_code() -> String {
    let mut bytes = [0u8; 10];
    getrandom::fill(&mut bytes).expect("system random number generator");
    let mut bits: u128 = 0;
    for b in bytes {
        bits = (bits << 8) | u128::from(b);
    }
    let symbols: Vec<char> = (0..CODE_SYMBOLS)
        .map(|i| char::from(ALPHABET[((bits >> (5 * (CODE_SYMBOLS - 1 - i))) & 31) as usize]))
        .collect();
    symbols
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

/// The code as typed, normalised: upper case, separators dropped, O read as 0 and I/L as 1;
/// `None` unless 16 symbols of the alphabet remain.
pub fn normalize_recovery_code(input: &str) -> Option<String> {
    let mut out = String::new();
    for c in input.chars() {
        if c.is_whitespace() || c == '-' {
            continue;
        }
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_SYMBOLS).then_some(out)
}

/// SHA-256 (hex) of a normalised code.
pub fn hash_recovery_code(normalized: &str) -> String {
    Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Equality without an early exit (both sides hashed to 32 bytes first).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let ha = Sha256::digest(a.as_bytes());
    let hb = Sha256::digest(b.as_bytes());
    ha.iter()
        .zip(hb.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Failed attempts (PIN or code) one address may make per [`IP_WINDOW`].
pub const IP_FAILURES: u32 = 10;
/// Attempts of any outcome one address may make per [`IP_WINDOW`].
pub const IP_ATTEMPTS: u32 = 30;
pub const IP_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Wrong PINs that lock a nick.
pub const NICK_FAILURES: u32 = 5;
/// Argon2 runs (PIN checks and new PINs) at the same time; more wait their turn.
pub const ARGON2_PARALLEL: usize = 2;
const FIRST_LOCK_MS: u64 = 15 * 60 * 1000;
const MAX_LOCK_MS: u64 = 24 * 60 * 60 * 1000;
/// PINs one address may set per hour.
pub const PROTECTS_PER_HOUR: u32 = 5;
const PRUNE_AT: usize = 10_000;

#[derive(Debug, Default, Clone, Copy)]
struct NickLock {
    /// Wrong PINs since the last lock, attempts in progress included.
    failures: u32,
    lockouts: u32,
    locked_until: u64,
    last_failure: u64,
}

/// After this long an attempt that was never settled counts as wrong.
const UNSETTLED_MS: u64 = 60 * 1000;

/// Locks the nick: 15 minutes, doubled with each further lock, at most a day.
fn lock_nick(lock: &mut NickLock, now: u64) {
    lock.failures = 0;
    lock.lockouts = lock.lockouts.saturating_add(1);
    let shift = (lock.lockouts - 1).min(16);
    let ms = FIRST_LOCK_MS.saturating_mul(1 << shift).min(MAX_LOCK_MS);
    lock.locked_until = now.saturating_add(ms);
}

pub type SharedClock = Arc<dyn Fn() -> u64 + Send + Sync>;

fn boxed(clock: &SharedClock) -> Clock {
    let c = Arc::clone(clock);
    Box::new(move || c())
}

/// An attempt [`PinGuard::begin`] let through and counted as a failure in advance; settle it
/// with [`PinGuard::fail`] or [`PinGuard::succeed`] (which gives the failure back).
#[derive(Debug)]
pub struct Attempt {
    ip: String,
    skel: Option<String>,
    at: u64,
}

/// Limits guessing, in memory. Per address: [`IP_FAILURES`] failures and [`IP_ATTEMPTS`]
/// attempts of any outcome per window. Per nick (skeleton, whether or not it exists, so the
/// lock tells nothing): [`NICK_FAILURES`] wrong PINs lock it for 15 minutes, doubling with each
/// further lock up to 24 hours. Recovery codes count against the address only.
///
/// An attempt is counted as a failure when it starts and given back when it succeeds, so
/// attempts running at the same time (the Argon2 check runs without this guard's lock) cannot
/// pass the limits together.
pub struct PinGuard {
    /// Failure times per address, attempts in progress included.
    ip_failures: HashMap<String, Vec<u64>>,
    ip_attempts: RateLimiter,
    protects: RateLimiter,
    nicks: HashMap<String, NickLock>,
    clock: SharedClock,
}

impl PinGuard {
    pub fn new(clock: SharedClock) -> Self {
        Self {
            ip_failures: HashMap::new(),
            ip_attempts: RateLimiter::new(IP_ATTEMPTS, IP_WINDOW, boxed(&clock)),
            protects: RateLimiter::new(PROTECTS_PER_HOUR, Duration::from_secs(3600), boxed(&clock)),
            nicks: HashMap::new(),
            clock,
        }
    }

    pub fn system() -> Self {
        Self::new(Arc::from(crate::rules::system_clock()))
    }

    /// Starts an attempt (`skel`: a PIN for that nick; `None`: a recovery code): `None` when
    /// the address is over its limits or the nick is locked (or has its failures in progress).
    pub fn begin(&mut self, ip: &str, skel: Option<&str>) -> Option<Attempt> {
        let now = (self.clock)();
        let window = IP_WINDOW.as_millis() as u64;
        if self.ip_failures.len() > PRUNE_AT {
            self.ip_failures
                .retain(|_, times| times.iter().any(|&t| now.saturating_sub(t) < window));
        }
        if self.nicks.len() > PRUNE_AT {
            self.nicks.retain(|_, l| {
                now < l.locked_until
                    || l.failures > 0
                    || now.saturating_sub(l.last_failure) < MAX_LOCK_MS
            });
        }
        let failures = self.ip_failures.entry(ip.to_string()).or_default();
        failures.retain(|&t| now.saturating_sub(t) < window);
        if failures.len() >= IP_FAILURES as usize {
            return None;
        }
        if let Some(lock) = skel.and_then(|s| self.nicks.get_mut(s)) {
            // Attempts never settled (a dropped connection) count as wrong once they are old.
            if lock.failures >= NICK_FAILURES
                && now.saturating_sub(lock.last_failure) >= UNSETTLED_MS
            {
                lock_nick(lock, now);
            }
            if now < lock.locked_until || lock.failures >= NICK_FAILURES {
                return None;
            }
        }
        if !self.ip_attempts.hit(ip) {
            return None;
        }
        self.ip_failures
            .entry(ip.to_string())
            .or_default()
            .push(now);
        if let Some(s) = skel {
            let lock = self.nicks.entry(s.to_string()).or_default();
            lock.failures += 1;
            lock.last_failure = now;
        }
        Some(Attempt {
            ip: ip.to_string(),
            skel: skel.map(str::to_string),
            at: now,
        })
    }

    /// A wrong PIN or code: the failure counted at the start stays; the nick may lock.
    pub fn fail(&mut self, attempt: Attempt) {
        let Some(skel) = attempt.skel else {
            return;
        };
        let now = (self.clock)();
        let lock = self.nicks.entry(skel).or_default();
        if lock.failures >= NICK_FAILURES {
            lock_nick(lock, now);
        }
    }

    /// A correct PIN or code: its failure is given back (it still counts as an attempt).
    pub fn succeed(&mut self, attempt: Attempt) {
        if let Some(times) = self.ip_failures.get_mut(&attempt.ip)
            && let Some(at) = times.iter().rposition(|&t| t == attempt.at)
        {
            times.remove(at);
        }
        if let Some(lock) = attempt.skel.and_then(|s| self.nicks.get_mut(&s)) {
            lock.failures = lock.failures.saturating_sub(1);
        }
    }

    /// Registers setting a PIN from the address; false over [`PROTECTS_PER_HOUR`].
    pub fn may_protect(&mut self, ip: &str) -> bool {
        self.protects.hit(ip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn pins_are_four_to_eight_digits_or_a_passphrase() {
        for ok in [
            "2580",
            "13579",
            "20481024",
            "correct horse",
            "zażółć gęślą",
            "pass word",
        ] {
            assert_eq!(validate_pin(ok), Ok(()), "{ok}");
        }
        for invalid in ["", "123", "123456789", "abc", "short1", "\u{7}abcdefgh"] {
            assert_eq!(validate_pin(invalid), Err(PinError::Invalid), "{invalid}");
        }
        assert_eq!(validate_pin(&"x".repeat(65)), Err(PinError::Invalid));
        for weak in ["0000", "1111", "1234", "4321", "98765", "012345"] {
            assert_eq!(validate_pin(weak), Err(PinError::Weak), "{weak}");
        }
    }

    #[test]
    fn a_pin_verifies_after_normalisation_and_never_against_garbage() {
        let phc = hash_pin("ka\u{301}wa kawa");
        assert!(!phc.contains("kawa"));
        assert!(verify_pin("k\u{e1}wa kawa", &phc));
        assert!(!verify_pin("kawa kawa", &phc));
        assert!(!verify_pin("2580", "not a phc"));
        assert!(!verify_pin("2580", dummy_phc()));
    }

    #[test]
    fn recovery_codes_are_random_and_read_leniently() {
        let a = new_recovery_code();
        let b = new_recovery_code();
        assert_ne!(a, b);
        assert_eq!(a.len(), 19);
        assert_eq!(a.matches('-').count(), 3);
        let n = normalize_recovery_code(&a).unwrap();
        assert_eq!(n.len(), 16);
        assert_eq!(
            normalize_recovery_code(&a.to_lowercase().replace('-', " ")),
            Some(n.clone())
        );
        // O reads as 0, I and L as 1.
        assert_eq!(
            normalize_recovery_code("o1il-0000-AAAA-BBBB").as_deref(),
            Some("01110000AAAABBBB")
        );
        assert_eq!(normalize_recovery_code("UUUU-0000-AAAA-BBBB"), None);
        assert_eq!(normalize_recovery_code("0000-AAAA"), None);
        let h = hash_recovery_code(&n);
        assert_eq!(h.len(), 64);
        assert!(constant_time_eq(&h, &hash_recovery_code(&n)));
        assert!(!constant_time_eq(
            &h,
            &hash_recovery_code("0000000000000000")
        ));
    }

    fn guard() -> (Arc<AtomicU64>, PinGuard) {
        let t = Arc::new(AtomicU64::new(1_000));
        let t2 = Arc::clone(&t);
        (
            t,
            PinGuard::new(Arc::new(move || t2.load(Ordering::SeqCst))),
        )
    }

    /// Whether an attempt may start now (settled as a success, which gives its failure back).
    fn can(g: &mut PinGuard, ip: &str, skel: Option<&str>) -> bool {
        g.begin(ip, skel).map(|a| g.succeed(a)).is_some()
    }

    fn wrong(g: &mut PinGuard, ip: &str, skel: Option<&str>) {
        let a = g.begin(ip, skel).expect("attempt allowed");
        g.fail(a);
    }

    #[test]
    fn five_wrong_pins_lock_the_nick_with_a_growing_lock() {
        let (t, mut g) = guard();
        for i in 0..NICK_FAILURES {
            wrong(&mut g, &format!("ip{i}"), Some("kasia"));
        }
        // Locked for every address, other nicks are not.
        assert!(!can(&mut g, "fresh", Some("kasia")));
        assert!(can(&mut g, "fresh", Some("basia")));
        // A recovery code is not stopped by the nick's lock.
        assert!(can(&mut g, "fresh", None));
        t.fetch_add(FIRST_LOCK_MS, Ordering::SeqCst);
        assert!(can(&mut g, "fresh", Some("kasia")));
        for i in 0..NICK_FAILURES {
            wrong(&mut g, &format!("other{i}"), Some("kasia"));
        }
        t.fetch_add(FIRST_LOCK_MS, Ordering::SeqCst);
        assert!(
            !can(&mut g, "fresh", Some("kasia")),
            "the second lock is longer"
        );
        t.fetch_add(FIRST_LOCK_MS, Ordering::SeqCst);
        assert!(can(&mut g, "fresh", Some("kasia")));
    }

    #[test]
    fn attempts_in_progress_count_until_they_succeed() {
        let (t, mut g) = guard();
        // Attempts never settled (dropped connections) lock the nick once they are old.
        let lost: Vec<Attempt> = (0..NICK_FAILURES)
            .map(|i| g.begin(&format!("x{i}"), Some("basia")).unwrap())
            .collect();
        drop(lost);
        t.fetch_add(UNSETTLED_MS, Ordering::SeqCst);
        assert!(!can(&mut g, "late", Some("basia")));
        t.fetch_add(FIRST_LOCK_MS, Ordering::SeqCst);
        assert!(can(&mut g, "late", Some("basia")));
        // Five attempts at once on one nick: a sixth cannot start, even from another address.
        let running: Vec<Attempt> = (0..NICK_FAILURES)
            .map(|i| g.begin(&format!("ip{i}"), Some("kasia")).unwrap())
            .collect();
        assert!(g.begin("late", Some("kasia")).is_none());
        // Successes give their failures back.
        for a in running {
            g.succeed(a);
        }
        assert!(can(&mut g, "late", Some("kasia")));
        // The same for an address: ten attempts in progress, then none.
        let running: Vec<Attempt> = (0..IP_FAILURES)
            .map(|i| g.begin("1.1.1.1", Some(&format!("n{i}"))).unwrap())
            .collect();
        assert!(g.begin("1.1.1.1", None).is_none());
        for a in running {
            g.succeed(a);
        }
        assert!(can(&mut g, "1.1.1.1", None));
    }

    #[test]
    fn an_address_has_ten_failures_thirty_attempts_and_five_pins() {
        let (t, mut g) = guard();
        for i in 0..IP_FAILURES {
            wrong(&mut g, "1.1.1.1", Some(&format!("nick{i}")));
        }
        assert!(!can(&mut g, "1.1.1.1", None));
        assert!(!can(&mut g, "1.1.1.1", Some("free")));
        assert!(can(&mut g, "2.2.2.2", Some("free")));
        t.fetch_add(IP_WINDOW.as_millis() as u64, Ordering::SeqCst);
        // Correct attempts are capped too.
        for _ in 0..IP_ATTEMPTS {
            assert!(can(&mut g, "1.1.1.1", None));
        }
        assert!(!can(&mut g, "1.1.1.1", None));
        for _ in 0..PROTECTS_PER_HOUR {
            assert!(g.may_protect("1.1.1.1"));
        }
        assert!(!g.may_protect("1.1.1.1"));
        assert!(g.may_protect("2.2.2.2"));
    }

    #[test]
    fn the_lock_is_capped_at_a_day() {
        let (t, mut g) = guard();
        for _ in 0..20 {
            for _ in 0..NICK_FAILURES {
                wrong(&mut g, "x", Some("kasia"));
            }
            t.fetch_add(MAX_LOCK_MS, Ordering::SeqCst);
            assert!(can(&mut g, "y", Some("kasia")));
        }
    }
}
