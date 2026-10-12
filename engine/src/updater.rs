//! yt-dlp self-update (`yt-dlp -U`) at start and every 24 hours.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::i18n::Msg;
use crate::process::run_process;

pub type LogFn = Arc<dyn Fn(Msg) + Send + Sync>;

const UPDATE_TIMEOUT: Duration = Duration::from_secs(120);
const INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// Last non-empty line of stdout + stderr (empty string when there is none).
pub fn last_line(stdout: &str, stderr: &str) -> String {
    format!("{stdout}\n{stderr}")
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string()
}

/// One update attempt; logs the outcome.
pub async fn update_ytdlp(path: &Path, log: &LogFn) {
    match run_process(path, &["-U".into()], UPDATE_TIMEOUT).await {
        Ok(out) => {
            log(Msg::new("log.ytdlp_update").arg("line", last_line(&out.stdout, &out.stderr)))
        }
        Err(e) => log(Msg::new("log.ytdlp_update_failed").arg("error", e)),
    }
}

/// Runs an update now and then every 24 hours until the handle is aborted.
pub fn spawn_updater(path: PathBuf, log: LogFn) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(INTERVAL);
        loop {
            interval.tick().await;
            update_ytdlp(&path, &log).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_last_non_empty_line() {
        assert_eq!(
            last_line("Current version\nUpdated to 2026.1\n\n", ""),
            "Updated to 2026.1"
        );
        assert_eq!(last_line("a\r\n", "b\r\n  \r\n"), "b");
        assert_eq!(last_line("", ""), "");
    }
}
