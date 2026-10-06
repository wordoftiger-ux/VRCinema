//! Tools manager: downloads, installs, updates and repairs yt-dlp, the ffmpeg shared build and
//! Deno in the app's `tools` folder (spec section 5). See [`encoder`] for the video-encoder test.

pub mod download;
pub mod encoder;
pub mod install;
#[cfg(test)]
pub(crate) mod test_server;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};

pub use encoder::detect_encoder;

use crate::process::run_process;

const DOWNLOAD_DIR: &str = ".download";
const VERSIONS_FILE: &str = "versions.json";
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    Deno,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::YtDlp, Tool::Ffmpeg, Tool::Deno];

    /// Stable identifier (also the key in `versions.json`).
    pub fn name(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Deno => "deno",
        }
    }

    /// The executable that must exist for the tool to count as installed.
    pub fn exe(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp.exe",
            Tool::Ffmpeg => "ffmpeg.exe",
            Tool::Deno => "deno.exe",
        }
    }

    /// Arguments that make the tool print its version.
    fn version_args(self) -> &'static [&'static str] {
        match self {
            Tool::YtDlp => &["--version"],
            Tool::Ffmpeg => &["-hide_banner", "-version"],
            Tool::Deno => &["--version"],
        }
    }
}

/// Where each tool is downloaded from: a base URL (no trailing slash) plus fixed file names.
/// Overridable so tests can point at a local server.
#[derive(Debug, Clone)]
pub struct ToolSources {
    pub ytdlp_base: String,
    pub ffmpeg_base: String,
    pub deno_base: String,
}

impl Default for ToolSources {
    fn default() -> Self {
        ToolSources {
            ytdlp_base: "https://github.com/yt-dlp/yt-dlp/releases/latest/download".into(),
            ffmpeg_base: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest".into(),
            deno_base: "https://github.com/denoland/deno/releases/latest/download".into(),
        }
    }
}

/// What to download for one tool.
struct Download {
    /// Name of the published file (also the lookup name in the sums file).
    file: &'static str,
    url: String,
    sums_url: String,
}

impl ToolSources {
    fn download(&self, tool: Tool) -> Download {
        let (base, file, sums) = match tool {
            Tool::YtDlp => (&self.ytdlp_base, "yt-dlp.exe", "SHA2-256SUMS"),
            Tool::Ffmpeg => (
                &self.ffmpeg_base,
                "ffmpeg-master-latest-win64-gpl-shared.zip",
                "checksums.sha256",
            ),
            Tool::Deno => (
                &self.deno_base,
                "deno-x86_64-pc-windows-msvc.zip",
                "deno-x86_64-pc-windows-msvc.zip.sha256sum",
            ),
        };
        let base = base.trim_end_matches('/');
        Download {
            file,
            url: format!("{base}/{file}"),
            sums_url: format!("{base}/{sums}"),
        }
    }
}

/// Progress of the running (or last) tools operation, for the wizard and the settings tab.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ToolsProgress {
    /// `idle`, `download`, `verify`, `install`, `version`, `done`.
    pub phase: String,
    /// Name of the tool being processed (see [`Tool::name`]); empty when idle.
    pub tool: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Set when the operation failed (the tool that failed is in `tool`).
    pub error: Option<String>,
    /// The whole operation has finished (successfully, or with `error`).
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolInfo {
    pub tool: Tool,
    pub installed: bool,
    pub version: Option<String>,
    pub installed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolsStatus {
    pub tools: Vec<ToolInfo>,
    pub missing: Vec<Tool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionEntry {
    version: String,
    installed_at: String,
}

pub struct ToolsManager {
    tools_dir: PathBuf,
    http: reqwest::Client,
    sources: ToolSources,
    progress: watch::Sender<ToolsProgress>,
    /// Only one install/update/repair at a time.
    busy: Mutex<()>,
    /// Where notes go (desktop app: its log); stderr by default.
    log: crate::pipeline::clip_factory::SharedLogFn,
}

impl ToolsManager {
    pub fn new(tools_dir: PathBuf, http: reqwest::Client) -> Self {
        Self::with_sources(tools_dir, http, ToolSources::default())
    }

    pub fn with_sources(tools_dir: PathBuf, http: reqwest::Client, sources: ToolSources) -> Self {
        let (progress, _) = watch::channel(ToolsProgress {
            phase: "idle".into(),
            ..Default::default()
        });
        ToolsManager {
            tools_dir,
            http,
            sources,
            progress,
            busy: Mutex::new(()),
            log: std::sync::Arc::new(|m: &str| eprintln!("{m}")),
        }
    }

    /// Sends the manager's notes to `log` instead of stderr.
    pub fn with_log(mut self, log: crate::pipeline::clip_factory::SharedLogFn) -> Self {
        self.log = log;
        self
    }

    pub fn tools_dir(&self) -> &Path {
        &self.tools_dir
    }

    pub fn progress(&self) -> watch::Receiver<ToolsProgress> {
        self.progress.subscribe()
    }

    /// Which tools are present, their recorded versions and which are missing.
    pub fn status(&self) -> ToolsStatus {
        let versions = self.read_versions();
        let tools: Vec<ToolInfo> = Tool::ALL
            .into_iter()
            .map(|tool| {
                let installed = self.is_installed(tool);
                let entry = versions.get(tool.name()).filter(|_| installed);
                ToolInfo {
                    tool,
                    installed,
                    version: entry.map(|e| e.version.clone()),
                    installed_at: entry.map(|e| e.installed_at.clone()),
                }
            })
            .collect();
        let missing = tools
            .iter()
            .filter(|t| !t.installed)
            .map(|t| t.tool)
            .collect();
        ToolsStatus { tools, missing }
    }

    /// Downloads and installs every tool that is not installed.
    pub async fn install_missing(&self) -> Result<()> {
        let missing = self.status().missing;
        self.run(&missing).await
    }

    /// Re-downloads one tool (also the way to update it).
    pub async fn update(&self, tool: Tool) -> Result<()> {
        self.run(&[tool]).await
    }

    /// Downloads everything again.
    pub async fn repair(&self) -> Result<()> {
        self.run(&Tool::ALL).await
    }

    fn is_installed(&self, tool: Tool) -> bool {
        let has = |name: &str| self.tools_dir.join(name).is_file();
        match tool {
            Tool::Ffmpeg => {
                has("ffmpeg.exe") && has("ffprobe.exe") && self.has_dll_with_prefix("avcodec")
            }
            _ => has(tool.exe()),
        }
    }

    /// The shared ffmpeg build is useless without its DLLs.
    fn has_dll_with_prefix(&self, prefix: &str) -> bool {
        fs::read_dir(&self.tools_dir).is_ok_and(|rd| {
            rd.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                name.starts_with(prefix) && name.ends_with(".dll")
            })
        })
    }

    fn set_progress(&self, update: impl FnOnce(&mut ToolsProgress)) {
        self.progress.send_modify(update);
    }

    async fn run(&self, tools: &[Tool]) -> Result<()> {
        let _guard = self.busy.lock().await;
        let mut result = Ok(());
        for &tool in tools {
            self.set_progress(|p| {
                *p = ToolsProgress {
                    phase: "download".into(),
                    tool: tool.name().into(),
                    ..Default::default()
                }
            });
            if let Err(e) = self.install_one(tool).await {
                let message = format!("{e:#}");
                self.set_progress(|p| p.error = Some(message));
                result = Err(e);
                break;
            }
        }
        self.set_progress(|p| {
            p.phase = "done".into();
            p.done = true;
        });
        result
    }

    async fn install_one(&self, tool: Tool) -> Result<()> {
        let source = self.sources.download(tool);
        let download_dir = self.tools_dir.join(DOWNLOAD_DIR);
        fs::create_dir_all(&download_dir)
            .with_context(|| format!("Nie można utworzyć folderu {}", download_dir.display()))?;

        let expected = match download::fetch_text(&self.http, &source.sums_url).await {
            Ok(Some(text)) => {
                let sum = download::parse_sums(&text, source.file);
                if sum.is_none() {
                    anyhow::bail!(
                        "Plik sum kontrolnych nie zawiera wpisu dla {}.",
                        source.file
                    );
                }
                sum
            }
            Ok(None) => {
                (self.log)(&format!(
                    "Narzędzia: wydawca nie udostępnia sum kontrolnych dla {}; instalacja bez weryfikacji.",
                    tool.name()
                ));
                None
            }
            Err(e) => return Err(e),
        };

        let part = download_dir.join(format!("{}.part", source.file));
        let mut on_progress = |downloaded: u64, total: Option<u64>| {
            self.set_progress(|p| {
                p.downloaded = downloaded;
                p.total = total;
            });
        };
        download::download_file(&self.http, &source.url, &part, &mut on_progress).await?;

        if let Some(expected) = expected {
            self.set_progress(|p| p.phase = "verify".into());
            let path = part.clone();
            let actual =
                tokio::task::spawn_blocking(move || download::sha256_file(&path)).await??;
            if actual != expected {
                // Corrupt data must not be resumed on the next attempt.
                let _ = fs::remove_file(&part);
                anyhow::bail!(
                    "Suma kontrolna pliku {} nie zgadza się (oczekiwano {expected}, otrzymano {actual}).",
                    source.file
                );
            }
        } else {
            self.set_progress(|p| p.phase = "verify".into());
        }

        self.set_progress(|p| p.phase = "install".into());
        let complete = download_dir.join(source.file);
        fs::rename(&part, &complete)?;
        let tools_dir = self.tools_dir.clone();
        let file = complete.clone();
        let installed = tokio::task::spawn_blocking(move || {
            install::install_downloaded(tool, &file, &tools_dir)
        })
        .await?;
        let _ = fs::remove_file(&complete);
        installed?;

        self.set_progress(|p| p.phase = "version".into());
        let version = self.query_version(tool).await;
        self.record_version(tool, version)?;
        Ok(())
    }

    /// First line of the tool's version output, or `unknown` if it cannot be run.
    async fn query_version(&self, tool: Tool) -> String {
        let args: Vec<OsString> = tool.version_args().iter().map(OsString::from).collect();
        let exe = self.tools_dir.join(tool.exe());
        match run_process(&exe, &args, VERSION_TIMEOUT).await {
            Ok(out) if out.code == Some(0) => out
                .stdout
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("unknown")
                .to_string(),
            _ => "unknown".to_string(),
        }
    }

    fn read_versions(&self) -> BTreeMap<String, VersionEntry> {
        fs::read_to_string(self.tools_dir.join(VERSIONS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn record_version(&self, tool: Tool, version: String) -> Result<()> {
        let mut versions = self.read_versions();
        versions.insert(
            tool.name().to_string(),
            VersionEntry {
                version,
                installed_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        let tmp = self.tools_dir.join(format!("{VERSIONS_FILE}.tmp"));
        fs::write(&tmp, serde_json::to_string_pretty(&versions)?)?;
        fs::rename(&tmp, self.tools_dir.join(VERSIONS_FILE))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::download::sha256_file;
    use crate::tools::test_server::{TestServer, build_zip};
    use sha2::{Digest, Sha256};

    fn hex(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    struct Fixture {
        server: TestServer,
        tools: PathBuf,
        _tmp: tempfile::TempDir,
        ytdlp: Vec<u8>,
        deno_zip: Vec<u8>,
    }

    impl Fixture {
        async fn new() -> Self {
            let server = TestServer::start().await;
            let tmp = tempfile::tempdir().unwrap();
            let tools = tmp.path().join("tools");
            fs::create_dir_all(&tools).unwrap();
            let ytdlp: Vec<u8> = (0..250_000).map(|i| (i % 251) as u8).collect();
            let ffmpeg_zip = build_zip(&[
                (
                    "ffmpeg-master-latest-win64-gpl-shared/bin/ffmpeg.exe",
                    b"FFMPEG",
                ),
                (
                    "ffmpeg-master-latest-win64-gpl-shared/bin/ffprobe.exe",
                    b"FFPROBE",
                ),
                (
                    "ffmpeg-master-latest-win64-gpl-shared/bin/avcodec-62.dll",
                    b"DLL",
                ),
                ("ffmpeg-master-latest-win64-gpl-shared/LICENSE.txt", b"lic"),
            ]);
            let deno_zip = build_zip(&[("deno.exe", b"DENO")]);
            server.put("yt/yt-dlp.exe", ytdlp.clone());
            server.put(
                "yt/SHA2-256SUMS",
                format!(
                    "{}  yt-dlp_x86.exe\n{}  yt-dlp.exe\n",
                    hex(b"other"),
                    hex(&ytdlp)
                ),
            );
            server.put(
                "ff/ffmpeg-master-latest-win64-gpl-shared.zip",
                ffmpeg_zip.clone(),
            );
            server.put(
                "ff/checksums.sha256",
                format!(
                    "{}  ffmpeg-master-latest-win64-gpl-shared.zip\n",
                    hex(&ffmpeg_zip)
                ),
            );
            server.put("dn/deno-x86_64-pc-windows-msvc.zip", deno_zip.clone());
            server.put(
                "dn/deno-x86_64-pc-windows-msvc.zip.sha256sum",
                format!("{}\n", hex(&deno_zip)),
            );
            Fixture {
                server,
                tools,
                _tmp: tmp,
                ytdlp,
                deno_zip,
            }
        }

        fn manager(&self) -> ToolsManager {
            ToolsManager::with_sources(
                self.tools.clone(),
                reqwest::Client::new(),
                ToolSources {
                    ytdlp_base: format!("{}/yt", self.server.base),
                    ffmpeg_base: format!("{}/ff/", self.server.base),
                    deno_base: format!("{}/dn", self.server.base),
                },
            )
        }
    }

    #[test]
    fn default_sources_point_at_the_real_publishers() {
        let s = ToolSources::default();
        let yt = s.download(Tool::YtDlp);
        assert_eq!(
            yt.url,
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        );
        assert_eq!(
            yt.sums_url,
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS"
        );
        let ff = s.download(Tool::Ffmpeg);
        assert_eq!(
            ff.url,
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl-shared.zip"
        );
        assert!(ff.sums_url.ends_with("/latest/checksums.sha256"));
        let dn = s.download(Tool::Deno);
        assert_eq!(
            dn.url,
            "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip"
        );
        assert!(
            dn.sums_url
                .ends_with("deno-x86_64-pc-windows-msvc.zip.sha256sum")
        );
    }

    #[tokio::test]
    async fn empty_folder_reports_everything_missing() {
        let f = Fixture::new().await;
        let status = f.manager().status();
        assert_eq!(status.missing, Tool::ALL);
        assert!(
            status
                .tools
                .iter()
                .all(|t| !t.installed && t.version.is_none())
        );
    }

    #[tokio::test]
    async fn install_missing_installs_all_three_with_versions_and_progress() {
        let f = Fixture::new().await;
        let m = f.manager();
        let mut rx = m.progress();
        assert_eq!(rx.borrow().phase, "idle");
        let seen = tokio::spawn(async move {
            let mut downloaded = Vec::new();
            while rx.changed().await.is_ok() {
                let p = rx.borrow_and_update().clone();
                if p.tool == "yt-dlp" {
                    downloaded.push(p.downloaded);
                }
                if p.done {
                    return (downloaded, p);
                }
            }
            (downloaded, ToolsProgress::default())
        });

        m.install_missing().await.unwrap();

        let (downloaded, last) = seen.await.unwrap();
        assert!(last.done && last.error.is_none(), "{last:?}");
        assert!(downloaded.windows(2).all(|w| w[0] <= w[1]));
        assert!(downloaded.iter().any(|d| *d > 0));
        assert_eq!(fs::read(f.tools.join("yt-dlp.exe")).unwrap(), f.ytdlp);
        assert_eq!(fs::read(f.tools.join("ffmpeg.exe")).unwrap(), b"FFMPEG");
        assert_eq!(fs::read(f.tools.join("ffprobe.exe")).unwrap(), b"FFPROBE");
        assert_eq!(fs::read(f.tools.join("avcodec-62.dll")).unwrap(), b"DLL");
        assert_eq!(fs::read(f.tools.join("deno.exe")).unwrap(), b"DENO");
        assert!(!f.tools.join("LICENSE.txt").exists());

        let status = m.status();
        assert!(status.missing.is_empty(), "{status:?}");
        let versions: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(f.tools.join("versions.json")).unwrap())
                .unwrap();
        for name in ["yt-dlp", "ffmpeg", "deno"] {
            // The fixtures are not runnable programs, so the label is "unknown".
            assert_eq!(versions[name]["version"], "unknown", "{name}");
            let at = versions[name]["installedAt"].as_str().unwrap();
            assert!(chrono::DateTime::parse_from_rfc3339(at).is_ok(), "{at}");
        }
        assert!(
            status
                .tools
                .iter()
                .all(|t| t.version.as_deref() == Some("unknown"))
        );
        // Nothing but tools and versions.json is left in the folder.
        let mut left: Vec<_> = fs::read_dir(&f.tools)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.') || n.ends_with(".part") || n.ends_with(".tmp"))
            .collect();
        left.sort();
        assert_eq!(left, [".download"]);
        assert_eq!(fs::read_dir(f.tools.join(".download")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn install_missing_skips_tools_already_present() {
        let f = Fixture::new().await;
        fs::write(f.tools.join("yt-dlp.exe"), b"KEEP").unwrap();
        fs::write(f.tools.join("deno.exe"), b"KEEP").unwrap();
        f.manager().install_missing().await.unwrap();
        assert_eq!(fs::read(f.tools.join("yt-dlp.exe")).unwrap(), b"KEEP");
        assert_eq!(fs::read(f.tools.join("deno.exe")).unwrap(), b"KEEP");
        assert!(f.tools.join("avcodec-62.dll").exists());
        assert!(
            f.server
                .requested_paths()
                .iter()
                .all(|p| p.starts_with("ff/")),
            "{:?}",
            f.server.requested_paths()
        );
    }

    #[tokio::test]
    async fn bad_checksum_fails_and_keeps_the_previous_tool() {
        let f = Fixture::new().await;
        fs::write(f.tools.join("yt-dlp.exe"), b"OLD-WORKING").unwrap();
        f.server.put(
            "yt/SHA2-256SUMS",
            format!("{}  yt-dlp.exe\n", hex(b"something else")),
        );
        let m = f.manager();
        let err = m.update(Tool::YtDlp).await.unwrap_err();
        assert!(err.to_string().contains("Suma kontrolna"), "{err}");
        assert_eq!(
            fs::read(f.tools.join("yt-dlp.exe")).unwrap(),
            b"OLD-WORKING"
        );
        let p = m.progress().borrow().clone();
        assert!(p.done);
        assert!(p.error.unwrap().contains("Suma kontrolna"));
        assert_eq!(fs::read_dir(f.tools.join(".download")).unwrap().count(), 0);
        assert!(!f.tools.join("versions.json").exists());
    }

    #[tokio::test]
    async fn corrupt_ffmpeg_archive_keeps_previous_ffmpeg() {
        let f = Fixture::new().await;
        for (n, c) in [
            ("ffmpeg.exe", "OLD1"),
            ("ffprobe.exe", "OLD2"),
            ("avcodec-61.dll", "OLD3"),
        ] {
            fs::write(f.tools.join(n), c).unwrap();
        }
        let bad = b"not a zip at all".to_vec();
        f.server
            .put("ff/ffmpeg-master-latest-win64-gpl-shared.zip", bad.clone());
        f.server.put(
            "ff/checksums.sha256",
            format!("{}  ffmpeg-master-latest-win64-gpl-shared.zip\n", hex(&bad)),
        );
        assert!(f.manager().update(Tool::Ffmpeg).await.is_err());
        assert_eq!(fs::read(f.tools.join("ffmpeg.exe")).unwrap(), b"OLD1");
        assert_eq!(fs::read(f.tools.join("avcodec-61.dll")).unwrap(), b"OLD3");
        assert!(!f.tools.join("avcodec-62.dll").exists());
    }

    #[tokio::test]
    async fn sums_file_without_an_entry_for_the_download_fails_and_installs_nothing() {
        let f = Fixture::new().await;
        fs::write(f.tools.join("yt-dlp.exe"), b"OLD-WORKING").unwrap();
        f.server.put(
            "yt/SHA2-256SUMS",
            format!(
                "{}  yt-dlp_x86.exe\n{}  yt-dlp_macos\n",
                hex(b"a"),
                hex(b"b")
            ),
        );
        let m = f.manager();
        let err = m.update(Tool::YtDlp).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Plik sum kontrolnych nie zawiera wpisu dla yt-dlp.exe."
        );
        assert_eq!(
            fs::read(f.tools.join("yt-dlp.exe")).unwrap(),
            b"OLD-WORKING"
        );
        assert!(
            !f.server
                .requested_paths()
                .contains(&"yt/yt-dlp.exe".to_string()),
            "must not even download"
        );
        assert!(m.progress().borrow().error.is_some());
    }

    #[tokio::test]
    async fn missing_sums_file_installs_without_verification() {
        let f = Fixture::new().await;
        // Deno's sums are published nowhere in this scenario.
        let server = &f.server;
        let m = ToolsManager::with_sources(
            f.tools.clone(),
            reqwest::Client::new(),
            ToolSources {
                ytdlp_base: format!("{}/yt", server.base),
                ffmpeg_base: format!("{}/ff", server.base),
                deno_base: format!("{}/dn-nosums", server.base),
            },
        );
        server.put(
            "dn-nosums/deno-x86_64-pc-windows-msvc.zip",
            f.deno_zip.clone(),
        );
        m.update(Tool::Deno).await.unwrap();
        assert_eq!(fs::read(f.tools.join("deno.exe")).unwrap(), b"DENO");
    }

    #[tokio::test]
    async fn interrupted_install_resumes_with_range_and_verifies() {
        let f = Fixture::new().await;
        f.server.drop_next_response_after(90_000);
        let m = f.manager();
        // Order of operations for yt-dlp: sums first, then the dropped download.
        assert!(m.update(Tool::YtDlp).await.is_err());
        let part = f.tools.join(".download/yt-dlp.exe.part");
        let kept = fs::metadata(&part).unwrap().len();
        assert!(kept > 0 && kept < f.ytdlp.len() as u64, "kept {kept}");
        assert!(
            !f.tools.join("yt-dlp.exe").exists(),
            "nothing installed yet"
        );
        assert!(m.progress().borrow().error.is_some());

        m.update(Tool::YtDlp).await.unwrap();
        assert_eq!(
            sha256_file(&f.tools.join("yt-dlp.exe")).unwrap(),
            hex(&f.ytdlp)
        );
        assert!(
            f.server.requests().iter().any(|(p, r)| p == "yt/yt-dlp.exe"
                && r.as_deref() == Some(format!("bytes={kept}-").as_str())),
            "{:?}",
            f.server.requests()
        );
        assert!(m.progress().borrow().error.is_none());
    }

    #[tokio::test]
    async fn repair_reinstalls_everything() {
        let f = Fixture::new().await;
        let m = f.manager();
        m.install_missing().await.unwrap();
        fs::write(f.tools.join("ffmpeg.exe"), b"BROKEN").unwrap();
        fs::remove_file(f.tools.join("deno.exe")).unwrap();
        m.repair().await.unwrap();
        assert_eq!(fs::read(f.tools.join("ffmpeg.exe")).unwrap(), b"FFMPEG");
        assert_eq!(fs::read(f.tools.join("deno.exe")).unwrap(), b"DENO");
        assert!(m.status().missing.is_empty());
    }

    #[tokio::test]
    async fn ffmpeg_without_its_dlls_counts_as_missing() {
        let f = Fixture::new().await;
        fs::write(f.tools.join("ffmpeg.exe"), b"x").unwrap();
        fs::write(f.tools.join("ffprobe.exe"), b"x").unwrap();
        assert_eq!(f.manager().status().missing, Tool::ALL);
        fs::write(f.tools.join("avcodec-62.dll"), b"x").unwrap();
        assert_eq!(f.manager().status().missing, [Tool::YtDlp, Tool::Deno]);
    }
}
