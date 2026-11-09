//! The system info for a bug report (Settings → About → "Copy system info"): plain English
//! text with the program's version, the system, the encoder, the tools' versions, the tunnel's
//! mode, the language, the data folder and the last problems of the log. Never the channel's
//! address, a key, a password, a viewer's nick or a video's title: the log's fields are
//! replaced by what they are (`<nick>`, `<title>`, `<address>`) and the rest goes through
//! [`redact_report`].

use std::fmt::Write as _;

use tokio::sync::OnceCell;

use crate::i18n::{Lang, Msg};
use crate::logging::LogEntry;
use crate::redact::{ReportSecrets, redact_report};

/// How many lines of the log's problems a report quotes.
pub const PROBLEM_LINES: usize = 20;

/// Words in the key of a log entry that make it a problem (an error or a warning).
const PROBLEM_WORDS: [&str; 17] = [
    "fail",
    "error",
    "missing",
    "cannot",
    "corrupt",
    "unreadable",
    "timeout",
    "crash",
    "lost",
    "unavailable",
    "wrong",
    "locked_out",
    "aborted",
    "bad_",
    "warning",
    "retry",
    "alarm",
];

/// What a report says, gathered by the app engine.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub version: String,
    pub os: String,
    /// The encoder in use, the setting (`auto` or one) and what the test found.
    pub encoder: String,
    pub encoder_setting: String,
    pub detected_encoder: Option<String>,
    /// Name, installed, recorded version.
    pub tools: Vec<(String, bool, Option<String>)>,
    /// `off`, `quick` or `permanent`, with the tunnel's state (never its address).
    pub tunnel: String,
    pub language: String,
    pub data_folder: String,
    /// `running`, or why not (redacted).
    pub channel: String,
    /// The report's local time.
    pub made_at: String,
    pub problems: Vec<String>,
}

/// The report's text.
pub fn report(f: &Facts) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "VRCinema {}", f.version);
    let _ = writeln!(out, "System: {}", f.os);
    let detected = f.detected_encoder.as_deref().unwrap_or("not tested");
    let _ = writeln!(
        out,
        "Video encoder: {} (setting: {}, graphics card test: {detected})",
        f.encoder, f.encoder_setting
    );
    let _ = writeln!(out, "Tools:");
    for (name, installed, version) in &f.tools {
        let version = match (installed, version.as_deref()) {
            (false, _) => "missing",
            (true, Some(v)) if !v.is_empty() => v,
            (true, _) => "unknown version",
        };
        let _ = writeln!(out, "  {name}: {version}");
    }
    let _ = writeln!(out, "Tunnel: {}", f.tunnel);
    let _ = writeln!(out, "Language: {}", f.language);
    let _ = writeln!(out, "Data folder: {}", f.data_folder);
    let _ = writeln!(out, "Channel: {}", f.channel);
    let _ = writeln!(out, "Report made: {}", f.made_at);
    let _ = writeln!(
        out,
        "\nLast problems in the log ({} lines; addresses, names, titles and keys removed):",
        f.problems.len()
    );
    if f.problems.is_empty() {
        let _ = writeln!(out, "  none");
    }
    for line in &f.problems {
        let _ = writeln!(out, "  {line}");
    }
    out
}

/// Whether a log entry is a problem (an error or a warning): by the words of its key, or a
/// text from outside that says so.
pub fn is_problem(entry: &LogEntry) -> bool {
    let key = entry.code();
    if key.is_empty() {
        let text = entry.msg.render(Lang::En).to_ascii_lowercase();
        return text.contains("error") || text.contains("warning") || text.contains("failed");
    }
    let key = key.to_ascii_lowercase();
    PROBLEM_WORDS.iter().any(|w| key.contains(w))
}

/// What a field of a log entry is replaced with in a report (`None`: the text is redacted).
fn placeholder(field: &str) -> Option<&'static str> {
    match field {
        "viewer" | "nick" | "name" => Some("<nick>"),
        "title" => Some("<title>"),
        "url" | "address" | "hostname" | "host" | "record" | "local" | "target" | "loginUrl" => {
            Some("<address>")
        }
        "tunnel" | "credentials" | "key" | "token" | "password" => Some("<hidden>"),
        _ => None,
    }
}

/// A log entry as a report quotes it: its fields replaced or redacted, in English.
pub fn redact_entry(msg: &Msg, secrets: &ReportSecrets) -> Msg {
    msg.map_texts(&|field, text| match placeholder(field) {
        Some(mark) => mark.to_string(),
        None => redact_report(text, secrets),
    })
}

/// The last `max` lines of the log's problems, redacted, oldest first.
pub fn problem_lines(entries: &[LogEntry], secrets: &ReportSecrets, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = entries
        .iter()
        .filter(|e| is_problem(e))
        .flat_map(|e| {
            LogEntry {
                at: e.at.clone(),
                msg: redact_entry(&e.msg, secrets),
            }
            .lines(Lang::En)
        })
        // A text from outside is redacted as a whole once more (its fields are not known).
        .map(|line| redact_report(&line, secrets))
        .collect();
    let drop = lines.len().saturating_sub(max);
    lines.drain(..drop);
    lines
}

/// The operating system and its version (asked once).
pub async fn os_description() -> String {
    static OS: OnceCell<String> = OnceCell::const_new();
    OS.get_or_init(read_os).await.clone()
}

#[cfg(windows)]
async fn read_os() -> String {
    use std::ffi::OsString;
    use std::time::Duration;
    let arch = std::env::consts::ARCH;
    let out = crate::process::run_process(
        std::path::Path::new("cmd"),
        &[OsString::from("/c"), OsString::from("ver")],
        Duration::from_secs(5),
    )
    .await;
    // "Microsoft Windows [Version 10.0.26200.6899]"
    let version = out.ok().and_then(|o| {
        let text = o.stdout;
        let start = text.find("Version ")? + "Version ".len();
        let end = text[start..].find(']')? + start;
        Some(text[start..end].trim().to_string())
    });
    match version {
        Some(v) => {
            let build: u32 = v
                .split('.')
                .nth(2)
                .and_then(|b| b.parse().ok())
                .unwrap_or(0);
            let name = if build >= 22000 {
                "Windows 11"
            } else {
                "Windows 10"
            };
            format!("{name} (version {v}, {arch})")
        }
        None => format!("Windows ({arch})"),
    }
}

#[cfg(not(windows))]
async fn read_os() -> String {
    let arch = std::env::consts::ARCH;
    let name = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|k| format!(", kernel {}", k.trim()))
        .unwrap_or_default();
    format!("{name} ({arch}{kernel})")
}

/// The user's profile folder (it is written as `%USERPROFILE%` / `~` in a report).
pub fn profile_dir() -> Option<String> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(var).ok().filter(|p| !p.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(msg: Msg) -> LogEntry {
        LogEntry {
            at: "2026-11-09 21:00:00".into(),
            msg,
        }
    }

    fn secrets() -> ReportSecrets {
        ReportSecrets {
            profile: Some(r"C:\Users\Kasia".into()),
            hosts: vec!["kino.example.pl".into()],
        }
    }

    #[test]
    fn only_problems_are_quoted_without_nicks_titles_addresses_or_keys() {
        let entries = vec![
            entry(Msg::new("log.start").arg("version", "0.3.1")),
            entry(
                Msg::new("log.request_received")
                    .arg("viewer", "Zosia")
                    .arg("title", "Tajny film")
                    .arg("status", "ok"),
            ),
            entry(Msg::new("log.tunnel_online").arg("url", "https://abc-def.trycloudflare.com")),
            entry(
                Msg::new("log.clip_failed")
                    .arg("id", 7)
                    .arg("title", "Tajny film")
                    .arg_msg(
                        "error",
                        Msg::text(
                            "https://rr1.googlevideo.com/videoplayback?ip=188.47.1.2&sig=x: 403",
                        ),
                    ),
            ),
            entry(Msg::new("log.obs_wrong_key").arg("address", "192.168.1.5:1935")),
            entry(
                Msg::new("log.named_dns_failed")
                    .arg("hostname", "kino.example.pl")
                    .arg_msg(
                        "reason",
                        Msg::text(r"C:\Users\Kasia\.cloudflared\cert.pem: denied"),
                    ),
            ),
            entry(Msg::text("Wpis z powłoki")),
        ];
        let lines = problem_lines(&entries, &secrets(), PROBLEM_LINES);
        assert_eq!(
            lines,
            vec![
                "[2026-11-09 21:00:00] Video #7 \"<title>\": <link>: 403".to_string(),
                "[2026-11-09 21:00:00] OBS: wrong stream key from <address>".to_string(),
                format!(
                    r"[2026-11-09 21:00:00] Permanent link: setting <address> in DNS failed: {}\.cloudflared\cert.pem: denied",
                    crate::redact::PROFILE_MARK
                ),
            ]
        );
        let all = lines.join("\n");
        for secret in [
            "Zosia",
            "Tajny",
            "trycloudflare",
            "googlevideo",
            "188.47",
            "192.168",
            "kino",
            "Kasia",
        ] {
            assert!(!all.contains(secret), "{secret} in {all}");
        }
    }

    #[test]
    fn only_the_last_lines_are_kept() {
        let entries: Vec<LogEntry> = (0..30)
            .map(|i| {
                entry(Msg::new("log.server_error").arg_msg("error", Msg::text(format!("e{i}"))))
            })
            .collect();
        let lines = problem_lines(&entries, &ReportSecrets::default(), PROBLEM_LINES);
        assert_eq!(lines.len(), PROBLEM_LINES);
        assert!(lines[0].ends_with("Server error: e10"), "{}", lines[0]);
        assert!(lines[19].ends_with("Server error: e29"), "{}", lines[19]);
    }

    #[test]
    fn the_report_lists_everything_in_plain_text() {
        let text = report(&Facts {
            version: "0.3.1".into(),
            os: "Windows 11 (version 10.0.26200.1, x86_64)".into(),
            encoder: "h264_nvenc".into(),
            encoder_setting: "auto".into(),
            detected_encoder: Some("h264_nvenc".into()),
            tools: vec![
                ("ffmpeg".into(), true, Some("ffmpeg version N-1".into())),
                ("cloudflared".into(), false, None),
                ("deno".into(), true, None),
            ],
            tunnel: "quick (online)".into(),
            language: "pl".into(),
            data_folder: r"%USERPROFILE%\AppData\Local\VRCinema".into(),
            channel: "running".into(),
            made_at: "2026-11-09 21:00".into(),
            problems: vec![],
        });
        for part in [
            "VRCinema 0.3.1\n",
            "System: Windows 11",
            "Video encoder: h264_nvenc (setting: auto, graphics card test: h264_nvenc)",
            "  ffmpeg: ffmpeg version N-1\n",
            "  cloudflared: missing\n",
            "  deno: unknown version\n",
            "Tunnel: quick (online)\n",
            "Language: pl\n",
            "(0 lines;",
            "  none\n",
        ] {
            assert!(text.contains(part), "{part:?} not in {text}");
        }
    }
}
