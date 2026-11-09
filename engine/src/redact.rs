//! Keeps signed media addresses out of messages and the log.
//!
//! A YouTube address (`https://rr7---sn-….googlevideo.com/videoplayback?…&ip=…&sig=…`) carries
//! the owner's public IP address and access tokens; ffprobe, ffmpeg and yt-dlp quote such
//! addresses in their errors. What reaches a person (an item's failure reason, the log) keeps
//! only the scheme, the host and the first path segment of every `http(s)://` address.

/// Punctuation a program writes right after an address (`https://…: Server returned 403`).
const TRAILING: [char; 7] = [':', ',', ';', ')', '"', '\'', '.'];

/// `text` with every `http://` / `https://` address cut to `scheme://host/first-segment`
/// (no user info, query or fragment); everything else stays as it is.
pub fn redact_urls(text: &str) -> String {
    map_urls(text, redact_one)
}

/// `text` with every `http(s)://` address replaced by `f(address)` (trailing punctuation kept).
fn map_urls(text: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = find_scheme(rest) {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |i| start + i);
        let token = &rest[start..end];
        let trimmed = token.trim_end_matches(TRAILING);
        out.push_str(&f(trimmed));
        out.push_str(&token[trimmed.len()..]);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

// ---------- The system info for a bug report (Settings → About) ----------

/// What a bug report must never carry besides what [`redact_report`] finds by itself.
#[derive(Debug, Clone, Default)]
pub struct ReportSecrets {
    /// The user's profile folder (`C:\Users\name`, `/home/name`): written as [`PROFILE_MARK`].
    pub profile: Option<String>,
    /// Host names of the channel (the tunnel's, the permanent link's, the manual address's).
    pub hosts: Vec<String>,
}

/// How the user's profile folder is written in a report.
pub const PROFILE_MARK: &str = if cfg!(windows) { "%USERPROFILE%" } else { "~" };

/// Words this long with digits and letters mixed are taken for keys or tokens.
const TOKEN_MIN_CHARS: usize = 20;

/// `text` as a bug report may quote it: no web address at all (`<link>`), none of the
/// channel's host names (`<address>`), no IP address (`<ip>`), no long key, token or tunnel id
/// (`<token>`), and the profile folder as [`PROFILE_MARK`] (it holds the Windows user name).
pub fn redact_report(text: &str, secrets: &ReportSecrets) -> String {
    let mut out = map_urls(text, |_| "<link>".to_string());
    for host in secrets.hosts.iter().filter(|h| h.len() >= 4) {
        out = replace_ignoring_case(&out, host, "<address>");
    }
    if let Some(profile) = secrets.profile.as_deref().filter(|p| p.len() >= 4) {
        out = replace_ignoring_case(&out, profile.trim_end_matches(['\\', '/']), PROFILE_MARK);
    }
    let out = redact_ipv4(&out);
    let out = redact_words(&out, is_ipv6, "<ip>");
    redact_words(&out, is_token, "<token>")
}

/// `text` with every `needle` (ASCII case ignored) replaced by `with`.
fn replace_ignoring_case(text: &str, needle: &str, with: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(i) = lower[at..].find(&needle) {
        out.push_str(&text[at..at + i]);
        out.push_str(with);
        at += i + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// `text` with every dotted IPv4 address (`1.2.3.4`, also before `:port`) as `<ip>`.
fn redact_ipv4(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        let starts = b[i].is_ascii_digit()
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'.'));
        if starts && let Some(len) = ipv4_at(&b[i..]) {
            out.push_str(&text[copied..i]);
            out.push_str("<ip>");
            i += len;
            copied = i;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// The length of the IPv4 address at the start of `b`: four groups of 1–3 digits, not
/// followed by another digit, letter or dotted group.
fn ipv4_at(b: &[u8]) -> Option<usize> {
    let mut i = 0;
    for group in 0..4 {
        if group > 0 {
            if b.get(i) != Some(&b'.') {
                return None;
            }
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() && i - start < 3 {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let next = b.get(i).copied();
    let after = b.get(i + 1).copied();
    let continues = next.is_some_and(|c| c.is_ascii_alphanumeric())
        || (next == Some(b'.') && after.is_some_and(|c| c.is_ascii_digit()));
    (!continues).then_some(i)
}

/// The characters a key, a token or an IPv6 address is made of.
fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=' | ':' | '.')
}

/// `text` with every word (a run of [`word_char`]s) that `matches` replaced by `with`.
fn redact_words(text: &str, matches: fn(&str) -> bool, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word_start = None;
    for (i, c) in text.char_indices() {
        match (word_char(c), word_start) {
            (true, None) => word_start = Some(i),
            (false, Some(start)) => {
                push_word(&mut out, &text[start..i], matches, with);
                word_start = None;
                out.push(c);
            }
            (false, None) => out.push(c),
            (true, Some(_)) => {}
        }
    }
    if let Some(start) = word_start {
        push_word(&mut out, &text[start..], matches, with);
    }
    out
}

fn push_word(out: &mut String, word: &str, matches: fn(&str) -> bool, with: &str) {
    if matches(word) {
        out.push_str(with);
        return;
    }
    // A sentence's own punctuation around the word (`…: 2a01:4f8::1.`) stays.
    let core = word.trim_matches(|c| matches!(c, '.' | ':' | '-' | '='));
    if core.is_empty() || !matches(core) {
        out.push_str(word);
        return;
    }
    let lead = word.find(core).unwrap_or(0);
    out.push_str(&word[..lead]);
    out.push_str(with);
    out.push_str(&word[lead + core.len()..]);
}

/// An IPv6 address: hex groups and colons, with `::` or at least five colons (a time like
/// `10:34:44` is not one).
fn is_ipv6(word: &str) -> bool {
    let word = word.trim_start_matches('[').trim_end_matches(']');
    let colons = word.matches(':').count();
    word.chars()
        .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        && word.chars().any(|c| c.is_ascii_hexdigit())
        && colons >= 2
        && (word.contains("::") || colons >= 5)
}

/// A key, token or id: long, with at least three digits and three letters.
fn is_token(word: &str) -> bool {
    let digits = word.chars().filter(char::is_ascii_digit).count();
    let letters = word.chars().filter(char::is_ascii_alphabetic).count();
    word.len() >= TOKEN_MIN_CHARS && digits >= 3 && letters >= 3
}

/// Where the next address starts (ASCII lowercasing keeps the byte offsets).
fn find_scheme(s: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    [lower.find("https://"), lower.find("http://")]
        .into_iter()
        .flatten()
        .min()
}

fn redact_one(url: &str) -> String {
    let Some(sep) = url.find("://") else {
        return url.to_string();
    };
    let scheme_end = sep + 3;
    let after = &url[scheme_end..];
    let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..authority_end];
    let host = authority
        .rfind('@')
        .map_or(authority, |at| &authority[at + 1..]);
    let mut kept = format!("{}{host}", &url[..scheme_end]);
    if let Some(path) = after[authority_end..].strip_prefix('/') {
        let segment_end = path.find(['/', '?', '#']).unwrap_or(path.len());
        kept.push('/');
        kept.push_str(&path[..segment_end]);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What ffprobe wrote for the owner's video #128 (the IP and tokens made up).
    const FFPROBE_403: &str = "https://rr7---sn-u2oxu-2v1r.googlevideo.com/videoplayback?expire=1792700650&ei=abc&ip=188.47.115.50&id=o-AI6Ze&itag=137&source=youtube&requiressl=yes&mime=video%2Fmp4&c=TVHTML5&sefc=1&n=J45Mrnz&sparams=expire%2Cei%2Cip&lsig=APaTxxMwRAIgDBnV&sig=AE0s2JYwRgIhAKX5%3D: Server returned 403 Forbidden (access denied)";

    #[test]
    fn a_signed_youtube_address_keeps_only_its_host_and_first_segment() {
        let r = redact_urls(FFPROBE_403);
        assert_eq!(
            r,
            "https://rr7---sn-u2oxu-2v1r.googlevideo.com/videoplayback: Server returned 403 Forbidden (access denied)"
        );
        for secret in [
            "188.47.115.50",
            "ip=",
            "sig=",
            "lsig",
            "expire",
            "c=TVHTML5",
        ] {
            assert!(!r.contains(secret), "{secret} in {r}");
        }
        // The message with ffprobe's prefix, as stored with a failed item.
        assert_eq!(
            redact_urls(&format!("ffprobe: {FFPROBE_403}")),
            format!("ffprobe: {r}")
        );
    }

    #[test]
    fn path_details_user_info_and_fragments_are_dropped() {
        assert_eq!(
            redact_urls(
                "Opening 'https://manifest.googlevideo.com/api/manifest/hls_playlist/ip/1.2.3.4/x.m3u8' for reading"
            ),
            "Opening 'https://manifest.googlevideo.com/api' for reading"
        );
        assert_eq!(
            redact_urls("HTTP://user:pw@cdn.example:8443/v.mp4#t=1, again http://h?x=1"),
            "HTTP://cdn.example:8443/v.mp4, again http://h"
        );
        assert_eq!(
            redact_urls("https://host: refused"),
            "https://host: refused"
        );
    }

    #[test]
    fn text_without_addresses_stays_as_it_is() {
        for text in [
            "",
            "Invalid data found when processing input",
            "C:\\Users\\x\\tools\\ffprobe.exe: Zażółć gęślą jaźń",
            "[youtube] abc: Video unavailable",
        ] {
            assert_eq!(redact_urls(text), text);
        }
    }

    fn secrets() -> ReportSecrets {
        ReportSecrets {
            profile: Some(r"C:\Users\Kasia".into()),
            hosts: vec!["kino.example.pl".into()],
        }
    }

    #[test]
    fn a_report_line_keeps_no_address_ip_token_host_or_user_name() {
        let line = r"Tunnel: online — https://proba-kanalu.trycloudflare.com/x?t=1. OBS: connected from 192.168.1.20:51515, [2a01:4f8::1] and ::1; tunnel 6ff42ae2-765d-4adf-8112-31c55c1551ef for kino.example.pl; key AbCdEf0123456789xyzQ in C:\Users\kasia\AppData\Local\VRCinema\logs";
        let r = redact_report(line, &secrets());
        assert_eq!(
            r,
            r"Tunnel: online — <link>. OBS: connected from <ip>:51515, [<ip>] and <ip>; tunnel <token> for <address>; key <token> in %USERPROFILE%\AppData\Local\VRCinema\logs"
                .replace("%USERPROFILE%", PROFILE_MARK)
        );
        for secret in [
            "trycloudflare",
            "192.168",
            "2a01",
            "6ff42ae2",
            "kino.example",
            "Kasia",
            "kasia",
            "AbCdEf",
        ] {
            assert!(!r.contains(secret), "{secret} in {r}");
        }
    }

    #[test]
    fn a_report_line_keeps_times_versions_numbers_and_ordinary_words() {
        for text in [
            "[2026-10-12 10:34:44] Video #12 failed: ffmpeg exited (code 1) after 3.5 s",
            "yt-dlp 2026.10.26, deno 2.9.8, version 0.3.1",
            "ffmpeg-master-latest-win64-gpl-shared.zip: std::io::Error",
            "Zażółć gęślą jaźń 「動画」",
        ] {
            assert_eq!(redact_report(text, &secrets()), text);
        }
    }

    #[test]
    fn several_addresses_and_non_ascii_text_around_them() {
        assert_eq!(
            redact_urls("「動画」 https://a.example/p/q?s=1 → https://b.example/r?t=2"),
            "「動画」 https://a.example/p → https://b.example/r"
        );
    }
}
