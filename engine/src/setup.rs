//! `vrcinema setup [--force] [--no-env]`: downloads yt-dlp and ffmpeg/ffprobe into `<root>/bin`
//! and creates `.env` (port of `scripts/setup.ts`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::process::run_process;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetupOptions {
    pub force: bool,
    pub no_env: bool,
}

impl SetupOptions {
    /// Parses the arguments after `setup`.
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut o = Self::default();
        for a in args {
            match a.as_str() {
                "--force" => o.force = true,
                "--no-env" => o.no_env = true,
                other => bail!("Nieznana opcja: {other} (dostępne: --force, --no-env)"),
            }
        }
        Ok(o)
    }
}

fn exe_suffix() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

fn ytdlp_url() -> &'static str {
    if cfg!(windows) {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
    }
}

fn ffmpeg_url() -> &'static str {
    if cfg!(windows) {
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
    } else {
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"
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

fn extract_tar_xz(archive: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive)?;
    tar::Archive::new(xz2::read::XzDecoder::new(file)).unpack(dest)?;
    Ok(())
}

/// Unpacks the archive (`.zip` or `.tar.xz`) next to `bin_dir`'s temp folder, copies
/// `ffmpeg<exe>` and `ffprobe<exe>` from wherever they are nested into `bin_dir` and removes
/// the temp folder.
pub fn extract_ffmpeg_tools(archive: &Path, bin_dir: &Path, exe: &str) -> Result<()> {
    let tmp = bin_dir.join("ffmpeg-extract");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    let result = (|| -> Result<()> {
        if archive.extension().is_some_and(|e| e == "zip") {
            extract_zip(archive, &tmp)
        } else {
            extract_tar_xz(archive, &tmp)
        }
        .context("Rozpakowanie ffmpeg nie powiodło się")?;
        for tool in ["ffmpeg", "ffprobe"] {
            let name = format!("{tool}{exe}");
            let found = find_file(&tmp, &name).ok_or_else(|| anyhow!("W archiwum brak {name}"))?;
            let dest = bin_dir.join(&name);
            fs::copy(&found, &dest)?;
            make_executable(&dest)?;
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

async fn download(url: &str, dest: &Path) -> Result<()> {
    println!("Pobieranie {url}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()?;
    let mut res = client.get(url).send().await?;
    if !res.status().is_success() {
        bail!(
            "Pobieranie nie powiodło się: {} {url}",
            res.status().as_u16()
        );
    }
    let mut file = fs::File::create(dest)?;
    while let Some(chunk) = res.chunk().await? {
        file.write_all(&chunk)?;
    }
    file.flush()?;
    Ok(())
}

async fn setup_ytdlp(bin: &Path, force: bool) -> Result<()> {
    let dest = bin.join(format!("yt-dlp{}", exe_suffix()));
    if dest.exists() && !force {
        println!("yt-dlp już jest.");
        return Ok(());
    }
    download(ytdlp_url(), &dest).await?;
    make_executable(&dest)
}

async fn setup_ffmpeg(bin: &Path, force: bool) -> Result<()> {
    let exe = exe_suffix();
    let ffmpeg = bin.join(format!("ffmpeg{exe}"));
    let ffprobe = bin.join(format!("ffprobe{exe}"));
    if ffmpeg.exists() && ffprobe.exists() && !force {
        println!("ffmpeg już jest.");
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        bail!("Na macOS zainstaluj ffmpeg przez Homebrew i ustaw FFMPEG_PATH/FFPROBE_PATH w .env.");
    }
    let archive = bin.join(if cfg!(windows) {
        "ffmpeg.zip"
    } else {
        "ffmpeg.tar.xz"
    });
    let result = async {
        download(ffmpeg_url(), &archive).await?;
        println!("Rozpakowywanie ffmpeg...");
        let (a, b) = (archive.clone(), bin.to_path_buf());
        tokio::task::spawn_blocking(move || extract_ffmpeg_tools(&a, &b, exe_suffix())).await?
    }
    .await;
    let _ = fs::remove_file(&archive);
    result
}

async fn print_version(bin: &Path, tool: &str, args: &[&str]) {
    let path = bin.join(format!("{tool}{}", exe_suffix()));
    let args: Vec<_> = args.iter().map(Into::into).collect();
    let text = match run_process(&path, &args, Duration::from_secs(30)).await {
        Ok(out) if !out.stdout.trim().is_empty() => out.stdout,
        Ok(out) if !out.stderr.trim().is_empty() => out.stderr,
        _ => "błąd".to_string(),
    };
    println!("{tool}: {}", text.lines().next().unwrap_or("błąd"));
}

pub async fn run_setup(root: &Path, opts: SetupOptions) -> Result<()> {
    let bin = root.join("bin");
    fs::create_dir_all(&bin)?;
    setup_ytdlp(&bin, opts.force).await?;
    setup_ffmpeg(&bin, opts.force).await?;
    print_version(&bin, "ffmpeg", &["-hide_banner", "-version"]).await;
    print_version(&bin, "yt-dlp", &["--version"]).await;
    let env = root.join(".env");
    if !opts.no_env && !env.exists() {
        fs::copy(root.join(".env.example"), &env).context("Brak pliku .env.example")?;
        println!("Utworzono .env z .env.example — ustaw w nim ADMIN_PASSWORD.");
    }
    println!("Gotowe.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_ffmpeg_and_ffprobe_from_nested_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("ffmpeg.zip");
        fs::write(
            &archive,
            build_zip(&[
                ("x/readme.txt", b"hi"),
                ("x/bin/ffmpeg.exe", b"FFMPEG"),
                ("x/bin/ffprobe.exe", b"FFPROBE"),
                ("x/bin/ffplay.exe", b"FFPLAY"),
            ]),
        )
        .unwrap();
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();

        extract_ffmpeg_tools(&archive, &bin, ".exe").unwrap();

        assert_eq!(fs::read(bin.join("ffmpeg.exe")).unwrap(), b"FFMPEG");
        assert_eq!(fs::read(bin.join("ffprobe.exe")).unwrap(), b"FFPROBE");
        assert!(!bin.join("ffplay.exe").exists());
        assert!(!bin.join("ffmpeg-extract").exists(), "temp dir cleaned up");
    }

    #[test]
    fn reports_missing_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("ffmpeg.zip");
        fs::write(&archive, build_zip(&[("x/bin/ffmpeg.exe", b"F")])).unwrap();
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let err = extract_ffmpeg_tools(&archive, &bin, ".exe").unwrap_err();
        assert_eq!(err.to_string(), "W archiwum brak ffprobe.exe");
        assert!(!bin.join("ffmpeg-extract").exists());
    }

    #[test]
    fn parses_options() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            SetupOptions::parse(&a(&[])).unwrap(),
            SetupOptions::default()
        );
        let o = SetupOptions::parse(&a(&["--force", "--no-env"])).unwrap();
        assert!(o.force && o.no_env);
        assert!(SetupOptions::parse(&a(&["--x"])).is_err());
    }
}
