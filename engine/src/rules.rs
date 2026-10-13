//! Request rules: source URL parsing, domain allow-list, metadata limits and
//! the in-memory sliding-window rate limiter (port of `src/queue/rules.ts`).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use url::{Host, Url};

use crate::db::{ChannelSettings, LinkMode};
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

/// The sites the "add popular sites" button of the settings puts in the list.
pub const POPULAR_DOMAINS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "twitch.tv",
    "kick.com",
    "vimeo.com",
    "dailymotion.com",
    "pixeldrain.com",
    "cda.pl",
    "soundcloud.com",
    "bilibili.com",
    "nicovideo.jp",
    "streamable.com",
    "reddit.com",
    "x.com",
    "twitter.com",
    "tiktok.com",
];

/// Why a link from a viewer is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// Not on the allow list, or on the block list.
    DomainNotAllowed,
    /// A name or address of the owner's own network (or of the machine itself).
    LocalAddress,
    /// Not `http` or `https` (or not a plain address that every program reads the same way).
    BadScheme,
    /// The name could not be looked up in time: refused rather than guessed.
    LookupFailed,
}

impl LinkError {
    /// The error code of the API answer (the key `errors.<code>`).
    pub fn code(self) -> &'static str {
        match self {
            Self::DomainNotAllowed => "domain_not_allowed",
            Self::LocalAddress => "local_address",
            Self::BadScheme => "bad_scheme",
            Self::LookupFailed => "lookup_failed",
        }
    }
}

/// Looks a host name up. The server uses the system resolver; tests use a table.
#[async_trait]
pub trait HostResolver: Send + Sync {
    async fn lookup(&self, host: &str) -> std::io::Result<Vec<IpAddr>>;
}

/// How long a name may take to resolve before the check gives up (and leaves the failure to
/// the fetch that follows).
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// The operating system's resolver.
pub struct SystemHostResolver;

#[async_trait]
impl HostResolver for SystemHostResolver {
    async fn lookup(&self, host: &str) -> std::io::Result<Vec<IpAddr>> {
        let found = tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((host, 0)))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "lookup timed out"))??;
        Ok(found.map(|a| a.ip()).collect())
    }
}

/// `true` for an address a viewer's link may point at: a public one. Loopback, private
/// (RFC 1918, `fc00::/7`), link-local, CGNAT (`100.64.0.0/10`), multicast, unspecified and every
/// other reserved range are not; an IPv4 address written inside an IPv6 one counts as that IPv4.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        // "This network" and the reserved class E.
        || a == 0
        || a >= 240
        // CGNAT.
        || (a == 100 && (b & 0xc0) == 64)
        // IETF protocol assignments, the 6to4 relays, benchmarking.
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b & 0xfe) == 18))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let s = ip.segments();
    let embedded = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    // NAT64 (`64:ff9b::/96`) and 6to4 (`2002::/16`) carry an IPv4 address.
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|&x| x == 0) {
        return is_public_v4(embedded(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return is_public_v4(embedded(s[1], s[2]));
    }
    // Only global unicast (`2000::/3`) is public; documentation and Teredo are not.
    (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && (s[1] == 0 || s[1] == 0x0db8))
}

/// Names that mean the owner's network or machine, without asking the DNS.
fn is_local_name(host: &str) -> bool {
    let host = host.to_lowercase();
    let host = host.trim_end_matches('.');
    // A name without a dot (`nas`, `printer`) is one of the owner's network.
    if !host.contains('.') {
        return true;
    }
    ["localhost", "local", "lan", "home.arpa", "internal"]
        .iter()
        .any(|s| host == *s || host.strip_suffix(s).is_some_and(|rest| rest.ends_with('.')))
}

/// Scheme and host of a viewer's address, without the DNS: `http`/`https` only; no local name,
/// no address that is not public.
fn check_scheme_and_host(url: &Url) -> Result<(), LinkError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(LinkError::BadScheme);
    }
    match url.host() {
        None => Err(LinkError::BadScheme),
        Some(Host::Ipv4(ip)) if !is_public_v4(ip) => Err(LinkError::LocalAddress),
        Some(Host::Ipv6(ip)) if !is_public_v6(ip) => Err(LinkError::LocalAddress),
        Some(Host::Domain(name)) if is_local_name(name) => Err(LinkError::LocalAddress),
        Some(_) => Ok(()),
    }
}

/// The checks of a viewer's link that need no DNS: scheme, local names and addresses, then the
/// link mode of the channel (the allow or block list).
pub fn check_viewer_link_offline(url: &Url, settings: &ChannelSettings) -> Result<(), LinkError> {
    check_scheme_and_host(url)?;
    let listed = match settings.link_mode {
        LinkMode::All => return Ok(()),
        LinkMode::Allowlist => is_allowed_domain(url, &settings.allowed_domains),
        LinkMode::Blocklist => !is_allowed_domain(url, &settings.blocked_domains),
    };
    listed.then_some(()).ok_or(LinkError::DomainNotAllowed)
}

/// A host name that resolves to anything but public addresses (one is enough) is refused, and so
/// is one whose lookup timed out. A name that does not exist passes: the fetch that follows
/// reports it.
pub async fn check_host_resolves_public(
    url: &Url,
    resolver: &dyn HostResolver,
) -> Result<(), LinkError> {
    let Some(Host::Domain(name)) = url.host() else {
        return Ok(());
    };
    match resolver.lookup(name).await {
        Ok(addresses) if addresses.iter().any(|ip| !is_public_ip(*ip)) => {
            Err(LinkError::LocalAddress)
        }
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Err(LinkError::LookupFailed),
        _ => Ok(()),
    }
}

/// Everything a link from a viewer must pass, in every link mode: the checks without the DNS,
/// then the DNS.
pub async fn check_viewer_link(
    url: &Url,
    settings: &ChannelSettings,
    resolver: &dyn HostResolver,
) -> Result<(), LinkError> {
    check_viewer_link_offline(url, settings)?;
    check_host_resolves_public(url, resolver).await
}

/// The address of the media a resolver found for a viewer's link, checked before ffprobe or ffmpeg
/// get it: public `http(s)` only. Returns the checked address as the parser wrote it; **that**
/// text, not `raw`, must be handed to the programs: they parse it by their own rules (a
/// backslash is a path separator for the URL standard, but ffmpeg connects to the host after the
/// `@`). An address with a backslash, a space or a control character is refused for the same
/// reason.
pub async fn check_media_url(raw: &str, resolver: &dyn HostResolver) -> Result<String, LinkError> {
    if raw
        .chars()
        .any(|c| c == '\\' || c.is_whitespace() || c.is_control())
    {
        return Err(LinkError::BadScheme);
    }
    let url = Url::parse(raw).map_err(|_| LinkError::BadScheme)?;
    check_scheme_and_host(&url)?;
    check_host_resolves_public(&url, resolver).await?;
    Ok(url.to_string())
}

/// [`check_media_url`] for every input at the same time (a slow lookup does not add up). The
/// first refusal in the order of the inputs wins.
pub async fn check_media_urls(
    raws: &[&str],
    resolver: &dyn HostResolver,
) -> Result<Vec<String>, LinkError> {
    futures_util::future::join_all(raws.iter().map(|raw| check_media_url(raw, resolver)))
        .await
        .into_iter()
        .collect()
}

/// A table instead of the DNS, for tests (a name that is not in it does not resolve).
#[cfg(test)]
pub(crate) struct TableHostResolver {
    table: HashMap<String, Vec<IpAddr>>,
    lookups: std::sync::atomic::AtomicUsize,
    timing_out: Vec<String>,
    delay: Duration,
}

#[cfg(test)]
impl TableHostResolver {
    pub(crate) fn new(rows: &[(&str, &[&str])]) -> Self {
        Self {
            table: rows
                .iter()
                .map(|(name, ips)| {
                    (
                        (*name).to_string(),
                        ips.iter().map(|ip| ip.parse().unwrap()).collect(),
                    )
                })
                .collect(),
            lookups: std::sync::atomic::AtomicUsize::new(0),
            timing_out: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    /// A name whose lookup times out.
    pub(crate) fn timing_out(mut self, host: &str) -> Self {
        self.timing_out.push(host.to_string());
        self
    }

    /// Every lookup takes this long.
    pub(crate) fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub(crate) fn lookups(&self) -> usize {
        self.lookups.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
#[async_trait]
impl HostResolver for TableHostResolver {
    async fn lookup(&self, host: &str) -> std::io::Result<Vec<IpAddr>> {
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if self.timing_out.iter().any(|h| h == host) {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.table
            .get(host)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such name"))
    }
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

    // ---------- viewer links ----------

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn table() -> TableHostResolver {
        TableHostResolver::new(&[
            ("youtu.be", &["142.250.1.1"]),
            ("www.youtube.com", &["142.250.1.2", "2a00:1450::1"]),
            ("evil.example", &["192.168.0.10"]),
            ("mixed.example", &["93.184.216.34", "10.0.0.7"]),
            ("v6.example", &["fd00::5"]),
            ("mapped.example", &["::ffff:192.168.1.1"]),
            ("cdn.example", &["93.184.216.34"]),
        ])
    }

    fn settings(mode: LinkMode) -> ChannelSettings {
        ChannelSettings {
            link_mode: mode,
            allowed_domains: vec!["youtube.com".into(), "youtu.be".into()],
            blocked_domains: vec!["Bad.Example".into()],
            ..ChannelSettings::default()
        }
    }

    async fn check(raw: &str, mode: LinkMode) -> Result<(), LinkError> {
        check_viewer_link(&url(raw), &settings(mode), &table()).await
    }

    #[tokio::test]
    async fn all_mode_lets_any_public_site_through() {
        assert_eq!(
            check("https://cdn.example/v.mp4", LinkMode::All).await,
            Ok(())
        );
        assert_eq!(check("http://youtu.be/x", LinkMode::All).await, Ok(()));
    }

    #[tokio::test]
    async fn allowlist_mode_takes_listed_domains_and_their_subdomains_in_any_case() {
        let m = LinkMode::Allowlist;
        assert_eq!(check("https://www.youtube.com/watch?v=1", m).await, Ok(()));
        assert_eq!(check("https://YOUTU.BE./1", m).await, Ok(()));
        assert_eq!(
            check("https://cdn.example/v.mp4", m).await,
            Err(LinkError::DomainNotAllowed)
        );
        assert_eq!(
            check("https://evilyoutube.com/x", m).await,
            Err(LinkError::DomainNotAllowed)
        );
    }

    #[tokio::test]
    async fn blocklist_mode_refuses_listed_domains_and_subdomains_in_any_case() {
        let m = LinkMode::Blocklist;
        assert_eq!(check("https://cdn.example/v.mp4", m).await, Ok(()));
        assert_eq!(
            check("https://bad.example/x", m).await,
            Err(LinkError::DomainNotAllowed)
        );
        assert_eq!(
            check("https://CDN.Bad.Example./x", m).await,
            Err(LinkError::DomainNotAllowed)
        );
        assert_eq!(check("https://notbad.example/x", m).await, Ok(()));
    }

    #[tokio::test]
    async fn only_http_and_https_pass_in_every_mode() {
        for mode in LinkMode::ALL {
            for raw in [
                "file:///C:/Windows/win.ini",
                "ftp://youtu.be/x",
                "javascript:alert(1)",
                "local:0/a.mp4",
            ] {
                assert_eq!(
                    check(raw, mode).await,
                    Err(LinkError::BadScheme),
                    "{raw} {mode:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn addresses_of_the_home_network_are_refused_in_every_mode() {
        for mode in LinkMode::ALL {
            for raw in [
                "http://127.0.0.1/x",
                "http://10.1.2.3/x",
                "http://192.168.1.1/x",
                "http://172.16.0.1/x",
                "http://169.254.1.1/x",
                "http://100.64.0.1/x",
                "http://0.0.0.0/x",
                "http://[::1]/x",
                "http://[fd00::1]/x",
                "http://[fe80::1]/x",
                "http://[::ffff:192.168.1.1]/x",
                "http://[::ffff:7f00:1]/x",
                "http://2130706433/x",
                "http://0x7f.1/x",
                "http://user@127.0.0.1/x",
                "http://youtu.be@127.0.0.1/x",
                "http://localhost/x",
                "http://LOCALHOST./x",
                "http://app.localhost/x",
                "http://router.local/x",
                "http://nas.lan/x",
                "http://x.home.arpa/x",
                "http://printer.internal/x",
            ] {
                assert_eq!(
                    check(raw, mode).await,
                    Err(LinkError::LocalAddress),
                    "{raw} {mode:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_a_private_address_is_refused_in_every_mode() {
        for mode in [LinkMode::All, LinkMode::Blocklist] {
            for raw in [
                "http://evil.example/x",
                "http://mixed.example/x",
                "http://v6.example/x",
                "http://mapped.example/x",
            ] {
                assert_eq!(
                    check(raw, mode).await,
                    Err(LinkError::LocalAddress),
                    "{raw} {mode:?}"
                );
            }
        }
        // A listed domain that resolves to a private address is refused too.
        let s = ChannelSettings {
            link_mode: LinkMode::Allowlist,
            allowed_domains: vec!["evil.example".into()],
            ..ChannelSettings::default()
        };
        assert_eq!(
            check_viewer_link(&url("http://evil.example/x"), &s, &table()).await,
            Err(LinkError::LocalAddress)
        );
    }

    #[tokio::test]
    async fn a_name_that_does_not_resolve_is_left_to_the_fetch_to_report() {
        assert_eq!(
            check("https://nowhere.example/x", LinkMode::All).await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn the_list_is_checked_before_any_lookup() {
        let counting = TableHostResolver::new(&[]);
        let r = check_viewer_link(
            &url("https://cdn.example/x"),
            &settings(LinkMode::Allowlist),
            &counting,
        )
        .await;
        assert_eq!(r, Err(LinkError::DomainNotAllowed));
        assert_eq!(counting.lookups(), 0);
    }

    #[test]
    fn public_and_non_public_addresses() {
        for s in [
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "100.128.0.1",
            "2a00:1450::1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public_ip(ip(s)), "{s}");
        }
        for s in [
            "127.0.0.1",
            "127.255.0.9",
            "10.0.0.1",
            "172.16.5.5",
            "172.31.255.255",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "0.1.2.3",
            "224.0.0.1",
            "255.255.255.255",
            "240.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "::",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:c0a8:101::1",
        ] {
            assert!(!is_public_ip(ip(s)), "{s}");
        }
    }

    #[test]
    fn popular_domains_are_the_sixteen_of_the_spec() {
        assert_eq!(POPULAR_DOMAINS.len(), 16);
        assert_eq!(POPULAR_DOMAINS[0], "youtube.com");
        assert!(POPULAR_DOMAINS.contains(&"nicovideo.jp"));
    }

    #[tokio::test]
    async fn the_address_of_the_media_must_be_public_http() {
        let r = table();
        assert_eq!(
            check_media_url("https://cdn.example/v.mp4", &r).await,
            Ok("https://cdn.example/v.mp4".to_string())
        );
        assert_eq!(
            check_media_url("http://192.168.0.5/v.mp4", &r).await,
            Err(LinkError::LocalAddress)
        );
        assert_eq!(
            check_media_url("https://evil.example/v.m3u8", &r).await,
            Err(LinkError::LocalAddress)
        );
        assert_eq!(
            check_media_url("file:///C:/secret.mp4", &r).await,
            Err(LinkError::BadScheme)
        );
        assert_eq!(
            check_media_url("C:\\secret.mp4", &r).await,
            Err(LinkError::BadScheme)
        );
    }

    /// WHATWG reads `\` as a path separator; ffmpeg connects to the host after the `@`. The
    /// checked address and the one handed over must be the same text.
    #[tokio::test]
    async fn a_backslash_cannot_hide_the_real_host_of_the_media() {
        let r = table();
        for raw in [
            r"http://93.184.216.34\@192.168.1.1/v.mp4",
            r"http://user\@192.168.1.1/v.mp4",
            r"http://127.0.0.2:9\@127.0.0.1:9/v.mp4",
            r"http://a@cdn.example\@127.0.0.1/v.mp4",
            "http://cdn.example/a b.mp4\n",
        ] {
            assert_eq!(
                check_media_url(raw, &r).await,
                Err(LinkError::BadScheme),
                "{raw}"
            );
        }
    }

    #[tokio::test]
    async fn the_media_address_handed_to_ffmpeg_is_the_checked_serialisation() {
        let r = table();
        assert_eq!(
            check_media_url("HTTPS://CDN.Example", &r).await,
            Ok("https://cdn.example/".to_string())
        );
    }

    #[tokio::test]
    async fn single_label_hosts_and_repeated_dots_are_local() {
        for raw in [
            "http://nas/x",
            "http://printer./x",
            "http://localhost../x",
            "http://router.local../x",
        ] {
            assert_eq!(
                check(raw, LinkMode::All).await,
                Err(LinkError::LocalAddress),
                "{raw}"
            );
        }
    }

    #[tokio::test]
    async fn a_lookup_that_times_out_fails_closed_a_missing_name_passes() {
        let r = table().timing_out("slow.example");
        let url = url("https://slow.example/x");
        assert_eq!(
            check_host_resolves_public(&url, &r).await,
            Err(LinkError::LookupFailed)
        );
        assert_eq!(
            check_media_url("https://slow.example/x", &r).await,
            Err(LinkError::LookupFailed)
        );
        assert_eq!(
            check_media_url("https://nowhere.example/x", &r).await,
            Ok("https://nowhere.example/x".to_string())
        );
        assert_eq!(LinkError::LookupFailed.code(), "lookup_failed");
    }

    #[tokio::test(start_paused = true)]
    async fn the_inputs_are_checked_at_the_same_time() {
        let r = table().with_delay(Duration::from_secs(5));
        let started = tokio::time::Instant::now();
        let out = check_media_urls(
            &[
                "https://cdn.example/v.mp4",
                "https://cdn.example/a.m4a",
                "https://youtu.be/x",
            ],
            &r,
        )
        .await
        .unwrap();
        assert_eq!(out.len(), 3);
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "{:?}",
            started.elapsed()
        );
        let bad =
            check_media_urls(&["https://cdn.example/v.mp4", "http://192.168.0.5/x"], &r).await;
        assert_eq!(bad, Err(LinkError::LocalAddress));
    }

    #[test]
    fn link_errors_have_their_error_codes() {
        assert_eq!(LinkError::DomainNotAllowed.code(), "domain_not_allowed");
        assert_eq!(LinkError::LocalAddress.code(), "local_address");
        assert_eq!(LinkError::BadScheme.code(), "bad_scheme");
    }

    #[test]
    fn system_clock_is_plausible() {
        let now = system_clock()();
        assert!(now > 1_700_000_000_000);
    }
}
