//! `vrcinema setup [--force] [--no-env] [--cloudflared]`: installs yt-dlp, ffmpeg/ffprobe and
//! Deno (the JavaScript runtime yt-dlp needs for YouTube) into `<root>/bin` with the tools
//! manager (checksums verified, interrupted downloads resumed; Windows, Linux x86_64 and
//! aarch64), optionally cloudflared, and creates `.env`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

use crate::i18n::{Lang, Msg, MsgContext, msg_error};
use crate::tools::{Platform, Tool, ToolsManager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetupOptions {
    pub force: bool,
    pub no_env: bool,
    /// Also install cloudflared (for `cloudflared tunnel --url ...`).
    pub cloudflared: bool,
}

impl SetupOptions {
    /// Parses the arguments after `setup`.
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut o = Self::default();
        for a in args {
            match a.as_str() {
                "--force" => o.force = true,
                "--no-env" => o.no_env = true,
                "--cloudflared" => o.cloudflared = true,
                other => {
                    return Err(msg_error(
                        Msg::error("setup_unknown_option").arg("option", other),
                    ));
                }
            }
        }
        Ok(o)
    }

    /// The tools to install, in order.
    pub fn tools(&self) -> Vec<Tool> {
        let mut tools = vec![Tool::YtDlp, Tool::Ffmpeg, Tool::Deno];
        if self.cloudflared {
            tools.push(Tool::Cloudflared);
        }
        tools
    }
}

/// Finds a file named `name` anywhere under `dir`.
pub fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in &entries {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if entry.file_name() == name {
            return Some(path);
        }
    }
    None
}

pub(crate) fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut f = fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }
    Ok(())
}

/// Unpacks a `.tar.xz` archive (the Linux ffmpeg builds); entries leaving `dest` are skipped.
pub(crate) fn extract_tar_xz(archive: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive)?;
    tar::Archive::new(xz2::read::XzDecoder::new(file)).unpack(dest)?;
    Ok(())
}

/// Lets everyone run the program (`0755`); nothing to do on Windows.
#[cfg(unix)]
pub(crate) fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Prints `msg` in `lang`.
fn say(lang: Lang, msg: Msg) {
    println!("{}", msg.render(lang));
}

/// `vrcinema setup`; its messages are in `lang` (the `LANGUAGE` of the `.env`, English by default).
pub async fn run_setup(root: &Path, opts: SetupOptions, lang: Lang) -> Result<()> {
    let platform = Platform::current();
    if platform.asset(Tool::Ffmpeg).is_none() {
        return Err(msg_error(
            Msg::error("setup_unsupported_platform").arg("platform", platform),
        ));
    }
    let bin = root.join("bin");
    fs::create_dir_all(&bin)?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .user_agent(concat!("VRCinema/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let manager =
        ToolsManager::new(bin, http).with_log(std::sync::Arc::new(move |m: Msg| say(lang, m)));
    let missing = manager.status().missing;
    for tool in opts.tools() {
        if !opts.force && !missing.contains(&tool) {
            say(lang, Msg::new("log.setup_already").arg("tool", tool.name()));
            continue;
        }
        say(
            lang,
            Msg::new("log.setup_installing").arg("tool", tool.name()),
        );
        manager.update(tool).await?;
        let version = manager
            .status()
            .tools
            .into_iter()
            .find(|t| t.tool == tool)
            .and_then(|t| t.version)
            .unwrap_or_else(|| "unknown".into());
        println!("{}: {version}", tool.name());
    }
    let env = root.join(".env");
    if !opts.no_env && !env.exists() {
        fs::copy(root.join(".env.example"), &env)
            .msg_context(Msg::error("setup_env_example_missing"))?;
        say(lang, Msg::new("log.setup_env_created"));
    }
    say(lang, Msg::new("log.setup_done"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let none = SetupOptions::parse(&a(&[])).unwrap();
        assert_eq!(none, SetupOptions::default());
        assert_eq!(none.tools(), [Tool::YtDlp, Tool::Ffmpeg, Tool::Deno]);
        let o = SetupOptions::parse(&a(&["--force", "--no-env", "--cloudflared"])).unwrap();
        assert!(o.force && o.no_env && o.cloudflared);
        assert_eq!(
            o.tools(),
            [Tool::YtDlp, Tool::Ffmpeg, Tool::Deno, Tool::Cloudflared]
        );
        let err = SetupOptions::parse(&a(&["--x"])).unwrap_err();
        assert_eq!(
            crate::i18n::error_msg(&err).render(Lang::En),
            "Unknown option: --x (available: --force, --no-env, --cloudflared)"
        );
        assert_eq!(
            crate::i18n::error_msg(&err).render(Lang::Pl),
            "Nieznana opcja: --x (dostępne: --force, --no-env, --cloudflared)"
        );
    }

    #[test]
    fn finds_a_nested_file() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("bin");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("ffprobe"), b"x").unwrap();
        assert_eq!(
            find_file(tmp.path(), "ffprobe"),
            Some(nested.join("ffprobe"))
        );
        assert_eq!(find_file(tmp.path(), "ffmpeg"), None);
    }
}
