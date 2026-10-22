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
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = find_scheme(rest) {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |i| start + i);
        let token = &rest[start..end];
        let trimmed = token.trim_end_matches(TRAILING);
        out.push_str(&redact_one(trimmed));
        out.push_str(&token[trimmed.len()..]);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
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

    #[test]
    fn several_addresses_and_non_ascii_text_around_them() {
        assert_eq!(
            redact_urls("「動画」 https://a.example/p/q?s=1 → https://b.example/r?t=2"),
            "「動画」 https://a.example/p → https://b.example/r"
        );
    }
}
