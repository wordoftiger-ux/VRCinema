//! Request rules: source URL parsing, domain allow-list, metadata limits and
//! the in-memory sliding-window rate limiter (port of `src/queue/rules.ts`).

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use url::Url;

use crate::db::ChannelSettings;
use crate::i18n::Msg;
use crate::resolver::types::MediaMeta;

/// Returns the URL only for valid http(s) addresses.
pub fn parse_source_url(raw: &str) -> Option<Url> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    let url = Url::parse(text).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// Allowed when the host equals a listed domain or is a subdomain of it.
pub fn is_allowed_domain(url: &Url, domains: &[String]) -> bool {
    let host = url.host_str().unwrap_or("").to_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    domains.iter().any(|d| {
        let domain = d.to_lowercase();
        host == domain
            || host
                .strip_suffix(domain.as_str())
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

fn format_duration(s: f64) -> Msg {
    // `as` saturates and maps NaN to 0, which is what we want for display.
    let h = (s / 3600.0).floor() as u64;
    let m = ((s % 3600.0) / 60.0).floor() as u64;
    if h > 0 {
        Msg::new("units.hours_minutes").arg("h", h).arg("m", m)
    } else {
        Msg::new("units.minutes").arg("m", m)
    }
}

/// Returns the rejection reason, or `None` when the video may be queued.
pub fn validate_meta(meta: &MediaMeta, settings: &ChannelSettings, is_admin: bool) -> Option<Msg> {
    if is_admin {
        return None;
    }
    if meta.is_playlist {
        return Some(Msg::error("playlist_owner_only"));
    }
    if meta.is_live {
        return Some(Msg::error("live_owner_only"));
    }
    match meta.duration_s {
        Some(d) if d > settings.max_duration_s => {
            Some(Msg::error("too_long").arg_msg("limit", format_duration(settings.max_duration_s)))
        }
        _ => None,
    }
}

/// Milliseconds since some fixed point; injectable so tests are deterministic.
pub type Clock = Box<dyn Fn() -> u64 + Send + Sync>;

/// Wall-clock milliseconds since the Unix epoch.
pub fn system_clock() -> Clock {
    Box::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    })
}

/// Window length in whole milliseconds, saturating instead of overflowing.
fn window_to_ms(window: Duration) -> u64 {
    u64::try_from(window.as_millis()).unwrap_or(u64::MAX)
}

/// Number of tracked keys above which expired ones are dropped.
const PRUNE_THRESHOLD: usize = 10_000;

/// Simple sliding-window limiter kept in memory.
pub struct RateLimiter {
    hits: HashMap<String, Vec<u64>>,
    limit: u32,
    window_ms: u64,
    clock: Clock,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration, clock: Clock) -> Self {
        Self {
            hits: HashMap::new(),
            limit,
            window_ms: window_to_ms(window),
            clock,
        }
    }

    pub fn set_limits(&mut self, limit: u32, window: Duration) {
        self.limit = limit;
        self.window_ms = window_to_ms(window);
    }

    /// Registers an attempt. Returns `false` when the limit in the window is exceeded.
    pub fn hit(&mut self, key: &str) -> bool {
        let now = (self.clock)();
        let window_ms = self.window_ms;
        let recent = self.hits.entry(key.to_string()).or_default();
        recent.retain(|&t| now.saturating_sub(t) < window_ms);
        if recent.len() >= self.limit as usize {
            if recent.is_empty() {
                // Limit 0: do not keep an empty entry per rejected key.
                self.hits.remove(key);
            }
            return false;
        }
        recent.push(now);
        if self.hits.len() > PRUNE_THRESHOLD {
            self.prune(now);
        }
        true
    }

    /// Number of attempts in the current window, without registering a new one.
    pub fn peek(&self, key: &str) -> usize {
        let now = (self.clock)();
        self.hits.get(key).map_or(0, |times| {
            times
                .iter()
                .filter(|&&t| now.saturating_sub(t) < self.window_ms)
                .count()
        })
    }

    fn prune(&mut self, now: u64) {
        let window_ms = self.window_ms;
        self.hits
            .retain(|_, times| times.iter().any(|&t| now.saturating_sub(t) < window_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn domains() -> Vec<String> {
        ChannelSettings::default().allowed_domains
    }

    #[test]
    fn parse_accepts_http_and_trims() {
        assert_eq!(
            parse_source_url("  https://youtu.be/abc ")
                .unwrap()
                .as_str(),
            "https://youtu.be/abc"
        );
    }

    #[test]
    fn parse_rejects_other_schemes_and_garbage() {
        assert!(parse_source_url("file:///etc/passwd").is_none());
        assert!(parse_source_url("nie link").is_none());
        assert!(parse_source_url("").is_none());
        assert!(parse_source_url("   ").is_none());
    }

    #[test]
    fn domain_and_subdomains_pass() {
        let d = domains();
        assert!(is_allowed_domain(
            &url("https://www.youtube.com/watch?v=1"),
            &d
        ));
        assert!(is_allowed_domain(
            &url("https://m.youtube.com/watch?v=1"),
            &d
        ));
        assert!(is_allowed_domain(&url("https://youtu.be/1"), &d));
        assert!(is_allowed_domain(&url("https://www.cda.pl/video/1"), &d));
        assert!(is_allowed_domain(&url("https://YOUTU.BE./1"), &d));
    }

    #[test]
    fn lookalike_and_foreign_domains_are_blocked() {
        let d = domains();
        assert!(!is_allowed_domain(&url("https://evilyoutube.com/x"), &d));
        assert!(!is_allowed_domain(
            &url("https://youtube.com.evil.net/x"),
            &d
        ));
        assert!(!is_allowed_domain(&url("https://vimeo.com/1"), &d));
    }

    fn meta() -> MediaMeta {
        MediaMeta {
            title: "t".into(),
            duration_s: Some(100.0),
            is_live: false,
            is_playlist: false,
            entries: None,
        }
    }

    #[test]
    fn valid_video_passes() {
        assert_eq!(
            validate_meta(&meta(), &ChannelSettings::default(), false),
            None
        );
    }

    #[test]
    fn too_long_video_rejected_for_public_but_not_admin() {
        let long = MediaMeta {
            duration_s: Some(7201.0),
            ..meta()
        };
        let s = ChannelSettings::default();
        assert_eq!(
            polish(validate_meta(&long, &s, false)).as_deref(),
            Some("Film jest za długi (limit: 2 h 0 min).")
        );
        assert_eq!(validate_meta(&long, &s, true), None);
    }

    #[test]
    fn duration_limit_formatting_without_hours() {
        let s = ChannelSettings {
            max_duration_s: 3000.0,
            ..ChannelSettings::default()
        };
        let long = MediaMeta {
            duration_s: Some(3001.0),
            ..meta()
        };
        assert_eq!(
            polish(validate_meta(&long, &s, false)).as_deref(),
            Some("Film jest za długi (limit: 50 min).")
        );
        let s = ChannelSettings {
            max_duration_s: 5400.0,
            ..ChannelSettings::default()
        };
        assert_eq!(
            polish(validate_meta(&long, &s, false)).as_deref(),
            None,
            "3001 s is under 5400 s"
        );
        let very_long = MediaMeta {
            duration_s: Some(5401.0),
            ..meta()
        };
        assert_eq!(
            polish(validate_meta(&very_long, &s, false)).as_deref(),
            Some("Film jest za długi (limit: 1 h 30 min).")
        );
    }

    #[test]
    fn unknown_duration_passes() {
        let m = MediaMeta {
            duration_s: None,
            ..meta()
        };
        assert_eq!(validate_meta(&m, &ChannelSettings::default(), false), None);
    }

    #[test]
    fn live_and_playlists_rejected_for_public_only() {
        let s = ChannelSettings::default();
        let live = MediaMeta {
            is_live: true,
            duration_s: None,
            ..meta()
        };
        let playlist = MediaMeta {
            is_playlist: true,
            ..meta()
        };
        assert_eq!(
            polish(validate_meta(&live, &s, false)).as_deref(),
            Some("Transmisje na żywo może dodawać tylko właściciel kanału.")
        );
        assert_eq!(
            polish(validate_meta(&playlist, &s, false)).as_deref(),
            Some("Playlisty może dodawać tylko właściciel kanału.")
        );
        assert_eq!(validate_meta(&live, &s, true), None);
        assert_eq!(validate_meta(&playlist, &s, true), None);
    }

    #[test]
    fn playlist_is_reported_before_live() {
        let both = MediaMeta {
            is_live: true,
            is_playlist: true,
            ..meta()
        };
        assert_eq!(
            polish(validate_meta(&both, &ChannelSettings::default(), false)).as_deref(),
            Some("Playlisty może dodawać tylko właściciel kanału.")
        );
    }

    fn polish(reason: Option<Msg>) -> Option<String> {
        reason.map(|m| m.render(crate::i18n::Lang::Pl))
    }

    fn fake_clock() -> (Arc<AtomicU64>, Clock) {
        let t = Arc::new(AtomicU64::new(0));
        let t2 = Arc::clone(&t);
        (t, Box::new(move || t2.load(Ordering::SeqCst)))
    }

    #[test]
    fn limiter_allows_limit_then_blocks_then_allows_after_window() {
        let (t, clock) = fake_clock();
        let mut rl = RateLimiter::new(3, Duration::from_millis(600_000), clock);
        assert!(rl.hit("ip"));
        assert!(rl.hit("ip"));
        assert!(rl.hit("ip"));
        assert!(!rl.hit("ip"));
        assert!(rl.hit("inny"));
        assert_eq!(rl.peek("ip"), 3);
        t.store(600_001, Ordering::SeqCst);
        assert_eq!(rl.peek("ip"), 0);
        assert!(rl.hit("ip"));
    }

    #[test]
    fn window_boundary_is_exclusive() {
        let (t, clock) = fake_clock();
        let mut rl = RateLimiter::new(1, Duration::from_millis(1000), clock);
        assert!(rl.hit("a"));
        t.store(999, Ordering::SeqCst);
        assert!(!rl.hit("a"));
        t.store(1000, Ordering::SeqCst);
        assert!(rl.hit("a"));
    }

    #[test]
    fn set_limits_changes_limit_on_the_fly() {
        let (_t, clock) = fake_clock();
        let mut rl = RateLimiter::new(1, Duration::from_millis(1000), clock);
        assert!(rl.hit("a"));
        assert!(!rl.hit("a"));
        rl.set_limits(5, Duration::from_millis(1000));
        assert!(rl.hit("a"));
    }

    #[test]
    fn huge_window_does_not_overflow() {
        let (t, clock) = fake_clock();
        t.store(5_000, Ordering::SeqCst);
        let mut rl = RateLimiter::new(1, Duration::from_secs(u64::MAX), clock);
        assert!(rl.hit("a"));
        assert!(!rl.hit("a"));
        assert_eq!(rl.peek("a"), 1);
        t.store(u64::MAX, Ordering::SeqCst);
        assert!(!rl.hit("a"));
    }

    #[test]
    fn clock_going_backwards_does_not_panic() {
        let (t, clock) = fake_clock();
        t.store(10_000, Ordering::SeqCst);
        let mut rl = RateLimiter::new(2, Duration::from_millis(1000), clock);
        assert!(rl.hit("a"));
        t.store(5, Ordering::SeqCst);
        assert!(rl.hit("a"));
        assert!(!rl.hit("a"));
    }

    #[test]
    fn zero_limit_blocks_everything() {
        let (_t, clock) = fake_clock();
        let mut rl = RateLimiter::new(0, Duration::from_millis(1000), clock);
        assert!(!rl.hit("a"));
    }

    #[test]
    fn many_keys_are_pruned() {
        let (t, clock) = fake_clock();
        let mut rl = RateLimiter::new(1, Duration::from_millis(10), clock);
        for i in 0..10_001 {
            assert!(rl.hit(&format!("k{i}")));
        }
        t.store(1_000, Ordering::SeqCst);
        assert!(rl.hit("fresh"));
        assert!(
            rl.hits.len() <= 2,
            "expired keys must be dropped, got {}",
            rl.hits.len()
        );
    }

    #[test]
    fn system_clock_is_plausible() {
        let now = system_clock()();
        assert!(now > 1_700_000_000_000);
    }
}
