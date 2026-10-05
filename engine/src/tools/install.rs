//! Unpacking a downloaded tool and swapping it into the tools folder so that a failed install
//! leaves the previous working tools untouched.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use super::Tool;
use crate::setup::{extract_zip, find_file};

const STAGE_PREFIX: &str = ".install-";
const BACKUP_DIR: &str = ".backup";

/// Installs the downloaded `file` (the bare exe for yt-dlp, the zip for ffmpeg and Deno) into
/// `tools_dir`. Everything is first extracted into a temporary folder inside `tools_dir`;
/// only a complete, valid result is moved into place (with rollback if a move fails).
pub fn install_downloaded(tool: Tool, file: &Path, tools_dir: &Path) -> Result<()> {
    let stage = tools_dir.join(format!("{STAGE_PREFIX}{}", tool.name()));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage)?;
    let result = (|| {
        let items = stage_files(tool, file, &stage, tools_dir)?;
        swap_in(&items, tools_dir)
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

/// Prepares `(staged file, destination)` pairs; fails if the download is not what we expect.
fn stage_files(
    tool: Tool,
    file: &Path,
    stage: &Path,
    tools_dir: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    match tool {
        Tool::YtDlp => {
            let staged = stage.join("yt-dlp.exe");
            fs::rename(file, &staged).or_else(|_| fs::copy(file, &staged).map(|_| ()))?;
            Ok(vec![(staged, tools_dir.join("yt-dlp.exe"))])
        }
        Tool::Deno => {
            extract_zip(file, stage).context("Rozpakowanie Deno nie powiodło się")?;
            let exe =
                find_file(stage, "deno.exe").ok_or_else(|| anyhow!("W archiwum brak deno.exe"))?;
            Ok(vec![(exe, tools_dir.join("deno.exe"))])
        }
        Tool::Ffmpeg => {
            extract_zip(file, stage).context("Rozpakowanie ffmpeg nie powiodło się")?;
            let ffmpeg = find_file(stage, "ffmpeg.exe")
                .ok_or_else(|| anyhow!("W archiwum brak ffmpeg.exe"))?;
            let bin = ffmpeg.parent().expect("file has a parent");
            if !bin.join("ffprobe.exe").is_file() {
                bail!("W archiwum brak ffprobe.exe");
            }
            // The shared build needs its DLLs next to the exes: take the whole `bin` folder.
            let mut items = Vec::new();
            for entry in fs::read_dir(bin)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    items.push((entry.path(), tools_dir.join(entry.file_name())));
                }
            }
            Ok(items)
        }
    }
}

/// Moves every staged file over its destination. Existing destinations are first moved to a
/// backup folder; if anything fails all moves are undone.
fn swap_in(items: &[(PathBuf, PathBuf)], tools_dir: &Path) -> Result<()> {
    let backup = tools_dir.join(BACKUP_DIR);
    let _ = fs::remove_dir_all(&backup);
    fs::create_dir_all(&backup)?;

    // (destination, backup of the old file if there was one)
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| -> Result<()> {
        for (src, dest) in items {
            let old = if dest.exists() {
                let kept = backup.join(dest.file_name().expect("file name"));
                fs::rename(dest, &kept).with_context(|| {
                    format!("Nie można zastąpić pliku {} (używany?)", dest.display())
                })?;
                Some(kept)
            } else {
                None
            };
            if let Err(e) = fs::rename(src, dest) {
                if let Some(kept) = &old {
                    let _ = fs::rename(kept, dest);
                }
                return Err(e).with_context(|| format!("Nie można zapisać {}", dest.display()));
            }
            done.push((dest.clone(), old));
        }
        Ok(())
    })();

    if result.is_err() {
        for (dest, old) in done.into_iter().rev() {
            let _ = fs::remove_file(&dest);
            if let Some(kept) = old {
                let _ = fs::rename(kept, dest);
            }
        }
    }
    let _ = fs::remove_dir_all(&backup);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_server::build_zip;

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut v: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn ffmpeg_shared_bin_folder_is_copied_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tmp.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        let zip = tmp.path().join("ffmpeg.zip");
        fs::write(
            &zip,
            build_zip(&[
                ("ffmpeg-master-latest-win64-gpl-shared/LICENSE.txt", b"lic"),
                (
                    "ffmpeg-master-latest-win64-gpl-shared/bin/ffmpeg.exe",
                    b"FFMPEG",
                ),
                (
                    "ffmpeg-master-latest-win64-gpl-shared/bin/ffprobe.exe",
                    b"FFPROBE",
                ),
                (
                    "ffmpeg-master-latest-win64-gpl-shared/bin/ffplay.exe",
                    b"FFPLAY",
                ),
                (
                    "ffmpeg-master-latest-win64-gpl-shared/bin/avcodec-62.dll",
                    b"DLL1",
                ),
                (
                    "ffmpeg-master-latest-win64-gpl-shared/bin/avutil-60.dll",
                    b"DLL2",
                ),
                (
                    "ffmpeg-master-latest-win64-gpl-shared/doc/manual.html",
                    b"doc",
                ),
            ]),
        )
        .unwrap();

        install_downloaded(Tool::Ffmpeg, &zip, &tools).unwrap();

        assert_eq!(
            dir_names(&tools),
            [
                "avcodec-62.dll",
                "avutil-60.dll",
                "ffmpeg.exe",
                "ffplay.exe",
                "ffprobe.exe"
            ]
        );
        assert_eq!(fs::read(tools.join("ffmpeg.exe")).unwrap(), b"FFMPEG");
        assert_eq!(fs::read(tools.join("avcodec-62.dll")).unwrap(), b"DLL1");
    }

    #[test]
    fn deno_and_ytdlp_install_and_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tmp.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(tools.join("yt-dlp.exe"), b"OLD").unwrap();

        let deno = tmp.path().join("deno.zip");
        fs::write(&deno, build_zip(&[("deno.exe", b"DENO")])).unwrap();
        install_downloaded(Tool::Deno, &deno, &tools).unwrap();
        assert_eq!(fs::read(tools.join("deno.exe")).unwrap(), b"DENO");

        let yt = tmp.path().join("yt-dlp.exe.download");
        fs::write(&yt, b"NEW").unwrap();
        install_downloaded(Tool::YtDlp, &yt, &tools).unwrap();
        assert_eq!(fs::read(tools.join("yt-dlp.exe")).unwrap(), b"NEW");
        assert_eq!(
            dir_names(&tools),
            ["deno.exe", "yt-dlp.exe"],
            "no temp leftovers"
        );
    }

    #[test]
    fn invalid_archives_leave_previous_tools_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tmp.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(tools.join("ffmpeg.exe"), b"OLD-FFMPEG").unwrap();
        fs::write(tools.join("ffprobe.exe"), b"OLD-FFPROBE").unwrap();

        let corrupt = tmp.path().join("corrupt.zip");
        fs::write(&corrupt, b"this is not a zip").unwrap();
        assert!(install_downloaded(Tool::Ffmpeg, &corrupt, &tools).is_err());

        let no_probe = tmp.path().join("noprobe.zip");
        fs::write(&no_probe, build_zip(&[("x/bin/ffmpeg.exe", b"NEW")])).unwrap();
        let err = install_downloaded(Tool::Ffmpeg, &no_probe, &tools).unwrap_err();
        assert_eq!(err.to_string(), "W archiwum brak ffprobe.exe");

        assert_eq!(fs::read(tools.join("ffmpeg.exe")).unwrap(), b"OLD-FFMPEG");
        assert_eq!(fs::read(tools.join("ffprobe.exe")).unwrap(), b"OLD-FFPROBE");
        assert_eq!(dir_names(&tools), ["ffmpeg.exe", "ffprobe.exe"]);
    }

    #[test]
    fn failed_swap_rolls_back_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tmp.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(tools.join("a.exe"), b"OLD-A").unwrap();
        let stage = tmp.path().join("stage");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("a.exe"), b"NEW-A").unwrap();
        // The second staged file does not exist: its move fails after a.exe was already swapped.
        let items = vec![
            (stage.join("a.exe"), tools.join("a.exe")),
            (stage.join("missing.dll"), tools.join("b.dll")),
        ];
        assert!(swap_in(&items, &tools).is_err());
        assert_eq!(fs::read(tools.join("a.exe")).unwrap(), b"OLD-A");
        assert_eq!(dir_names(&tools), ["a.exe"]);
    }
}
