//! Tools manager: downloads, installs, updates and repairs yt-dlp, the ffmpeg shared build,
//! Deno and cloudflared in the app's `tools` folder (spec section 5). See [`encoder`] for the video-encoder test.

pub mod download;
pub mod encoder;
pub mod install;
pub mod platform;
#[cfg(test)]
pub(crate) mod test_server;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};

pub use encoder::detect_encoder;
pub use platform::Platform;

use crate::i18n::{Msg, MsgContext, error_msg, msg_error};
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
    Cloudflared,
}

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::YtDlp, Tool::Ffmpeg, Tool::Deno, Tool::Cloudflared];

    /// Stable identifier (also the key in `versions.json`).
    pub fn name(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Deno => "deno",
            Tool::Cloudflared => "cloudflared",
        }
    }

    /// The executable that must exist for the tool to count as installed (on this system).
    pub fn exe(self) -> &'static str {
        self.exe_for(Platform::current())
    }

    /// [Tool::exe] on platform: yt-dlp.exe on Windows, yt-dlp elsewhere.
    pub fn exe_for(self, platform: Platform) -> &'static str {
        let windows = platform.exe_suffix() == ".exe";
        match (self, windows) {
            (Tool::YtDlp, true) => "yt-dlp.exe",
            (Tool::YtDlp, false) => "yt-dlp",
            (Tool::Ffmpeg, true) => "ffmpeg.exe",
            (Tool::Ffmpeg, false) => "ffmpeg",
            (Tool::Deno, true) => "deno.exe",
            (Tool::Deno, false) => "deno",
            (Tool::Cloudflared, true) => "cloudflared.exe",
            (Tool::Cloudflared, false) => "cloudflared",
        }
    }

    /// Arguments that make the tool print its version.
    fn version_args(self) -> &'static [&'static str] {
        match self {
            Tool::YtDlp => &["--version"],
            Tool::Ffmpeg => &["-hide_banner", "-version"],
            Tool::Deno => &["--version"],
            Tool::Cloudflared => &["--version"],
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
    pub cloudflared_base: String,
    /// "Latest release" API of cloudflared: its description is the only place with checksums.
    pub cloudflared_release_api: String,
}

impl Default for ToolSources {
    fn default() -> Self {
        ToolSources {
            ytdlp_base: "https://github.com/yt-dlp/yt-dlp/releases/latest/download".into(),
            ffmpeg_base: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest".into(),
            deno_base: "https://github.com/denoland/deno/releases/latest/download".into(),
            cloudflared_base: "https://github.com/cloudflare/cloudflared/releases/latest/download"
                .into(),
            cloudflared_release_api:
                "https://api.github.com/repos/cloudflare/cloudflared/releases/latest".into(),
        }
    }
}

/// Where the checksum of a download is published.
enum Sums {
    /// A sums file next to the download (missing file: install unverified; a file without an
    /// entry for the download: failure).
    File(String),
    /// The description of a GitHub release (no entry: install unverified).
    ReleaseNotes(String),
}

/// What to download for one tool.
struct Download {
    /// Name of the published file (also the lookup name for its checksum).
    file: &'static str,
    url: String,
    sums: Sums,
}

impl ToolSources {
    /// What to download for `tool` on `platform`; `None`: nothing is published for it.
    fn download(&self, tool: Tool, platform: Platform) -> Option<Download> {
        let asset = platform.asset(tool)?;
        let base = match tool {
            Tool::YtDlp => &self.ytdlp_base,
            Tool::Ffmpeg => &self.ffmpeg_base,
            Tool::Deno => &self.deno_base,
            Tool::Cloudflared => &self.cloudflared_base,
        }
        .trim_end_matches('/');
        let sums = match asset.sums {
            platform::SumsSource::File(name) => Sums::File(format!("{base}/{name}")),
            platform::SumsSource::ReleaseNotes => {
                Sums::ReleaseNotes(self.cloudflared_release_api.clone())
            }
        };
        Some(Download {
            file: asset.file,
            url: format!("{base}/{}", asset.file),
            sums,
        })
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
    #[serde(skip)]
    pub error: Option<Msg>,
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
    /// Whose programs are installed (this system's; tests pick one).
    platform: Platform,
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
            log: std::sync::Arc::new(|m: Msg| eprintln!("{m}")),
            platform: Platform::current(),
        }
    }

    /// Sends the manager's notes to `log` instead of stderr.
    pub fn with_log(mut self, log: crate::pipeline::clip_factory::SharedLogFn) -> Self {
        self.log = log;
        self
    }

    /// Installs the programs of `platform` instead of this system's (tests).
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// Where `tool`'s program is (or will be) installed.
    pub fn exe_path(&self, tool: Tool) -> PathBuf {
        self.tools_dir.join(tool.exe_for(self.platform))
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
                let ffprobe = format!("ffprobe{}", self.platform.exe_suffix());
                has(tool.exe_for(self.platform))
                    && has(&ffprobe)
                    && (!self.platform.shared_ffmpeg() || self.has_dll_with_prefix("avcodec"))
            }
            _ => has(tool.exe_for(self.platform)),
        }
    }

    /// The shared ffmpeg build (Windows) is useless without its DLLs.
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
                let message = error_msg(&e);
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
        let source = self.sources.download(tool, self.platform).ok_or_else(|| {
            msg_error(
                Msg::error("tools_unsupported_platform")
                    .arg("tool", tool.name())
                    .arg("platform", self.platform),
            )
        })?;
        let download_dir = self.tools_dir.join(DOWNLOAD_DIR);
        fs::create_dir_all(&download_dir)
            .msg_context(Msg::error("cannot_create_dir").arg("path", download_dir.display()))?;

        let unverified = || {
            (self.log)(Msg::new("log.tools_no_checksums").arg("tool", tool.name()));
            None
        };
        let expected = match &source.sums {
            Sums::File(url) => match download::fetch_text(&self.http, url).await {
                Ok(Some(text)) => {
                    let sum = download::parse_sums(&text, source.file);
                    if sum.is_none() {
                        return Err(msg_error(
                            Msg::error("checksum_entry_missing").arg("file", source.file),
                        ));
                    }
                    sum
                }
                Ok(None) => unverified(),
                Err(e) => return Err(e),
            },
            Sums::ReleaseNotes(api_url) => {
                match download::fetch_release_notes_sum(&self.http, api_url, source.file).await {
                    Some(sum) => Some(sum),
                    None => unverified(),
                }
            }
        };

        let part = download_dir.join(format!("{}.part", source.file));
        let mut on_progress = |downloaded: u64, total: Option<u64>| {
            self.set_progress(|p| {
                p.downloaded = downloaded;
                p.total = total;
            });
        };
        // At most twice: a resumed download may continue an older file published under the
        // same name ("latest" assets change), which only the checksum reveals.
        let mut fresh_retry = true;
        loop {
            let resumed = fs::metadata(&part).is_ok_and(|m| m.len() > 0);
            self.set_progress(|p| p.phase = "download".into());
            download::download_file(&self.http, &source.url, &part, &mut on_progress).await?;
            self.set_progress(|p| p.phase = "verify".into());
            let Some(expected) = &expected else {
                break;
            };
            let path = part.clone();
            let actual =
                tokio::task::spawn_blocking(move || download::sha256_file(&path)).await??;
            if &actual == expected {
                break;
            }
            // Corrupt data must not be resumed on the next attempt.
            let _ = fs::remove_file(&part);
            if resumed && fresh_retry {
                fresh_retry = false;
                (self.log)(Msg::new("log.tools_resumed_file_mismatch").arg("file", source.file));
                continue;
            }
            return Err(msg_error(
                Msg::error("checksum_mismatch")
                    .arg("file", source.file)
                    .arg("expected", expected)
                    .arg("actual", actual),
            ));
        }

        self.set_progress(|p| p.phase = "install".into());
        let complete = download_dir.join(source.file);
        fs::rename(&part, &complete)?;
        let tools_dir = self.tools_dir.clone();
        let file = complete.clone();
        let platform = self.platform;
        let installed = tokio::task::spawn_blocking(move || {
            install::install_downloaded(tool, &file, &tools_dir, platform)
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
        let exe = self.exe_path(tool);
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
        cloudflared: Vec<u8>,
    }

    /// The JSON the "latest release" API answers with.
    fn release_json(body: &str) -> String {
        serde_json::json!({ "tag_name": "2099.1.0", "body": body }).to_string()
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
            let cloudflared: Vec<u8> = (0..120_000).map(|i| (i % 239) as u8).collect();
            server.put("cf/cloudflared-windows-amd64.exe", cloudflared.clone());
            server.put(
                "cf-api",
                release_json(&format!(
                    "## Release\n| File | SHA256 |\n|---|---|\n| cloudflared-windows-386.exe | {} |\n| cloudflared-windows-amd64.exe | {} |\n",
                    hex(b"other"),
                    hex(&cloudflared)
                )),
            );
            Fixture {
                server,
                tools,
                _tmp: tmp,
                ytdlp,
                deno_zip,
                cloudflared,
            }
        }

        fn sources(&self) -> ToolSources {
            ToolSources {
                ytdlp_base: format!("{}/yt", self.server.base),
                ffmpeg_base: format!("{}/ff/", self.server.base),
                deno_base: format!("{}/dn", self.server.base),
                cloudflared_base: format!("{}/cf", self.server.base),
                cloudflared_release_api: format!("{}/cf-api", self.server.base),
            }
        }

        fn manager(&self) -> ToolsManager {
            ToolsManager::with_sources(self.tools.clone(), reqwest::Client::new(), self.sources())
                .with_platform(Platform::WINDOWS_X64)
        }

        /// A manager whose notes are collected in the returned list.
        fn manager_with_log(
            &self,
        ) -> (ToolsManager, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
            let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let sink = lines.clone();
            let m = self.manager().with_log(std::sync::Arc::new(move |m: Msg| {
                sink.lock().unwrap().push(m.to_string());
            }));
            (m, lines)
        }
    }

    #[test]
    fn default_sources_point_at_the_real_publishers() {
        let s = ToolSources::default();
        let yt = s.download(Tool::YtDlp, Platform::WINDOWS_X64).unwrap();
        assert_eq!(
            yt.url,
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        );
        assert!(matches!(
            &yt.sums,
            Sums::File(u) if u == "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS"
        ));
        let ff = s.download(Tool::Ffmpeg, Platform::WINDOWS_X64).unwrap();
        assert_eq!(
            ff.url,
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl-shared.zip"
        );
        assert!(matches!(&ff.sums, Sums::File(u) if u.ends_with("/latest/checksums.sha256")));
        let dn = s.download(Tool::Deno, Platform::WINDOWS_X64).unwrap();
        assert_eq!(
            dn.url,
            "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip"
        );
        assert!(matches!(
            &dn.sums,
            Sums::File(u) if u.ends_with("deno-x86_64-pc-windows-msvc.zip.sha256sum")
        ));
        let cf = s
            .download(Tool::Cloudflared, Platform::WINDOWS_X64)
            .unwrap();
        assert_eq!(
            cf.url,
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe"
        );
        assert!(matches!(
            &cf.sums,
            Sums::ReleaseNotes(u)
                if u == "https://api.github.com/repos/cloudflare/cloudflared/releases/latest"
        ));
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
    async fn install_missing_installs_all_four_with_versions_and_progress() {
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
        assert_eq!(
            fs::read(f.tools.join("cloudflared.exe")).unwrap(),
            f.cloudflared
        );
        assert!(!f.tools.join("LICENSE.txt").exists());

        let status = m.status();
        assert!(status.missing.is_empty(), "{status:?}");
        let versions: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(f.tools.join("versions.json")).unwrap())
                .unwrap();
        for name in ["yt-dlp", "ffmpeg", "deno", "cloudflared"] {
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
        fs::write(f.tools.join("cloudflared.exe"), b"KEEP").unwrap();
        f.manager().install_missing().await.unwrap();
        assert_eq!(fs::read(f.tools.join("yt-dlp.exe")).unwrap(), b"KEEP");
        assert_eq!(fs::read(f.tools.join("deno.exe")).unwrap(), b"KEEP");
        assert_eq!(fs::read(f.tools.join("cloudflared.exe")).unwrap(), b"KEEP");
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
        assert!(err.to_string().contains("checksum"), "{err}");
        assert_eq!(
            fs::read(f.tools.join("yt-dlp.exe")).unwrap(),
            b"OLD-WORKING"
        );
        let p = m.progress().borrow().clone();
        assert!(p.done);
        assert!(p.error.unwrap().to_string().contains("checksum"));
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
            "The checksum file has no entry for yt-dlp.exe."
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
                deno_base: format!("{}/dn-nosums", server.base),
                ..f.sources()
            },
        )
        .with_platform(Platform::WINDOWS_X64);
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

    /// A "latest" asset replaced on the server since yesterday's interrupted download: the
    /// resumed file fails the checksum, so the same job starts it over and installs the new one.
    #[tokio::test]
    async fn a_stale_partial_download_of_a_replaced_asset_is_started_over() {
        let f = Fixture::new().await;
        f.server.drop_next_response_after(90_000);
        let m = f.manager();
        assert!(m.update(Tool::YtDlp).await.is_err());
        let part = f.tools.join(".download/yt-dlp.exe.part");
        let kept = fs::metadata(&part).unwrap().len();
        assert!(kept > 0, "kept {kept}");

        let newer: Vec<u8> = (0..260_000).map(|i| (i % 241) as u8 ^ 0x5a).collect();
        f.server.put("yt/yt-dlp.exe", newer.clone());
        f.server
            .put("yt/SHA2-256SUMS", format!("{}  yt-dlp.exe\n", hex(&newer)));

        m.update(Tool::YtDlp).await.unwrap();
        assert_eq!(fs::read(f.tools.join("yt-dlp.exe")).unwrap(), newer);
        assert!(m.progress().borrow().error.is_none());
        let downloads: Vec<_> = f
            .server
            .requests()
            .into_iter()
            .filter(|(p, _)| p == "yt/yt-dlp.exe")
            .map(|(_, r)| r)
            .collect();
        assert_eq!(
            downloads,
            [None, Some(format!("bytes={kept}-")), None],
            "first try, the resume, then one fresh download"
        );
        assert_eq!(fs::read_dir(f.tools.join(".download")).unwrap().count(), 0);
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
        assert_eq!(
            f.manager().status().missing,
            [Tool::YtDlp, Tool::Deno, Tool::Cloudflared]
        );
    }

    // ---------- cloudflared ----------

    #[tokio::test]
    async fn cloudflared_with_a_checksum_in_the_release_notes_is_verified() {
        let f = Fixture::new().await;
        let (m, log) = f.manager_with_log();
        m.update(Tool::Cloudflared).await.unwrap();
        assert_eq!(
            fs::read(f.tools.join("cloudflared.exe")).unwrap(),
            f.cloudflared
        );
        assert!(log.lock().unwrap().is_empty(), "{:?}", log.lock().unwrap());
        assert_eq!(fs::read_dir(f.tools.join(".download")).unwrap().count(), 0);
        let paths = f.server.requested_paths();
        assert!(paths.contains(&"cf-api".to_string()), "{paths:?}");
        let versions: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(f.tools.join("versions.json")).unwrap())
                .unwrap();
        assert_eq!(versions["cloudflared"]["version"], "unknown");
        assert!(m.status().tools.iter().any(|t| t.tool == Tool::Cloudflared
            && t.installed
            && t.version.as_deref() == Some("unknown")));
    }

    #[tokio::test]
    async fn cloudflared_checksum_mismatch_is_rejected_and_the_old_file_stays() {
        let f = Fixture::new().await;
        fs::write(f.tools.join("cloudflared.exe"), b"OLD-WORKING").unwrap();
        f.server.put(
            "cf-api",
            release_json(&format!(
                "cloudflared-windows-amd64.exe: {}",
                hex(b"something else")
            )),
        );
        let m = f.manager();
        let err = m.update(Tool::Cloudflared).await.unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");
        assert_eq!(
            fs::read(f.tools.join("cloudflared.exe")).unwrap(),
            b"OLD-WORKING"
        );
        assert!(m.progress().borrow().error.is_some());
        assert_eq!(fs::read_dir(f.tools.join(".download")).unwrap().count(), 0);
        assert!(!f.tools.join("versions.json").exists());
    }

    #[tokio::test]
    async fn cloudflared_installs_unverified_when_no_checksum_is_published() {
        // (api answer, description of the scenario)
        let f = Fixture::new().await;
        let cases = [
            (
                "notes without the file",
                Some(release_json(&format!(
                    "{}  cloudflared-linux-amd64\n",
                    hex(b"x")
                ))),
                None,
            ),
            (
                "line with two hashes",
                Some(release_json(&format!(
                    "cloudflared-windows-amd64.exe {} {}",
                    hex(b"a"),
                    hex(b"b")
                ))),
                None,
            ),
            ("API answers 404", None, Some(404)),
            ("API answers 500", None, Some(500)),
            ("API answers garbage", Some("not json".to_string()), None),
        ];
        for (what, body, fail) in cases {
            let _ = fs::remove_file(f.tools.join("cloudflared.exe"));
            match (&body, fail) {
                (Some(b), _) => f.server.put("cf-api", b.clone()),
                (None, Some(code)) => f.server.fail_with("cf-api", code),
                _ => unreachable!(),
            }
            let (m, log) = f.manager_with_log();
            m.update(Tool::Cloudflared)
                .await
                .unwrap_or_else(|e| panic!("{what}: {e:#}"));
            assert_eq!(
                fs::read(f.tools.join("cloudflared.exe")).unwrap(),
                f.cloudflared,
                "{what}"
            );
            let lines = log.lock().unwrap().clone();
            assert_eq!(lines.len(), 1, "{what}: {lines:?}");
            assert!(
                lines[0].contains("cloudflared") && lines[0].contains("without verification"),
                "{what}: {lines:?}"
            );
        }
    }

    #[tokio::test]
    async fn cloudflared_unreachable_api_still_installs_unverified() {
        let f = Fixture::new().await;
        let m = ToolsManager::with_sources(
            f.tools.clone(),
            reqwest::Client::new(),
            ToolSources {
                cloudflared_release_api: "http://127.0.0.1:9/api".into(),
                ..f.sources()
            },
        )
        .with_platform(Platform::WINDOWS_X64)
        .with_log(std::sync::Arc::new(|_: Msg| {}));
        m.update(Tool::Cloudflared).await.unwrap();
        assert!(f.tools.join("cloudflared.exe").is_file());
    }
}
