//! Unpacking a downloaded tool and swapping it into the tools folder so that a failed install
//! leaves the previous working tools untouched.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::Tool;
use crate::i18n::{Msg, MsgContext, msg_error};
use crate::setup::{extract_zip, find_file};

const STAGE_PREFIX: &str = ".install-";
const BACKUP_DIR: &str = ".backup";

/// Installs the downloaded `file` (the bare exe for yt-dlp and cloudflared, the zip for ffmpeg and Deno) into
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

/// The downloaded archive does not hold `file`.
fn archive_lacks(file: &str) -> anyhow::Error {
    msg_error(Msg::error("archive_lacks_file").arg("file", file))
}

/// Prepares `(staged file, destination)` pairs; fails if the download is not what we expect.
fn stage_files(
    tool: Tool,
    file: &Path,
    stage: &Path,
    tools_dir: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    match tool {
        // Single executables (no archive): the download is the program.
        Tool::YtDlp | Tool::Cloudflared => {
            let exe = tool.exe();
            let staged = stage.join(exe);
            fs::rename(file, &staged).or_else(|_| fs::copy(file, &staged).map(|_| ()))?;
            Ok(vec![(staged, tools_dir.join(exe))])
        }
        Tool::Deno => {
            extract_zip(file, stage).msg_context(Msg::error("deno_unpack_failed"))?;
            let exe = find_file(stage, "deno.exe").ok_or_else(|| archive_lacks("deno.exe"))?;
            Ok(vec![(exe, tools_dir.join("deno.exe"))])
        }
        Tool::Ffmpeg => {
            extract_zip(file, stage).msg_context(Msg::error("ffmpeg_unpack_failed"))?;
            let ffmpeg =
                find_file(stage, "ffmpeg.exe").ok_or_else(|| archive_lacks("ffmpeg.exe"))?;
            let bin = ffmpeg.parent().expect("file has a parent");
            if !bin.join("ffprobe.exe").is_file() {
                return Err(archive_lacks("ffprobe.exe"));
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

/// Removes the backup folders of earlier swaps (best effort: one still holding a running
/// program stays until a later call).
pub fn clean_old_backups(tools_dir: &Path) {
    let Ok(entries) = fs::read_dir(tools_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(BACKUP_DIR)
            && entry.file_type().is_ok_and(|t| t.is_dir())
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// A backup folder no earlier swap used: a program replaced earlier may still run from the
/// previous one (the broadcaster's ffmpeg), so its files can be neither deleted nor replaced.
fn new_backup_dir(tools_dir: &Path) -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    for n in 0..100u32 {
        let dir = tools_dir.join(format!("{BACKUP_DIR}-{stamp}-{n}"));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e)
                    .msg_context(Msg::error("cannot_create_dir").arg("path", dir.display()));
            }
        }
    }
    Err(msg_error(
        Msg::error("backup_dir_failed").arg("path", tools_dir.display()),
    ))
}

/// Moves every staged file over its destination. Existing destinations are first moved to a
/// backup folder of this swap; if anything fails all moves are undone.
fn swap_in(items: &[(PathBuf, PathBuf)], tools_dir: &Path) -> Result<()> {
    clean_old_backups(tools_dir);
    let backup = new_backup_dir(tools_dir)?;

    // (destination, backup of the old file if there was one)
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| -> Result<()> {
        for (src, dest) in items {
            let old = if dest.exists() {
                let kept = backup.join(dest.file_name().expect("file name"));
                fs::rename(dest, &kept)
                    .msg_context(Msg::error("cannot_replace_file").arg("path", dest.display()))?;
                Some(kept)
            } else {
                None
            };
            if let Err(e) = fs::rename(src, dest) {
                if let Some(kept) = &old {
                    let _ = fs::rename(kept, dest);
                }
                return Err(e)
                    .msg_context(Msg::error("cannot_write_file").arg("path", dest.display()));
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
    // Best effort: a backup still in use is removed by a later swap or at the next start.
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
        assert_eq!(
            err.to_string(),
            "The downloaded archive has no ffprobe.exe."
        );

        assert_eq!(fs::read(tools.join("ffmpeg.exe")).unwrap(), b"OLD-FFMPEG");
        assert_eq!(fs::read(tools.join("ffprobe.exe")).unwrap(), b"OLD-FFPROBE");
        assert_eq!(dir_names(&tools), ["ffmpeg.exe", "ffprobe.exe"]);
    }

    /// Kills the test's child process even when an assertion fails.
    #[cfg(windows)]
    struct KillOnDrop(std::process::Child);

    #[cfg(windows)]
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// The broadcaster's ffmpeg keeps running from the file the first update moved away, so
    /// that backup cannot be deleted; a second update in the same session must still work.
    #[cfg(windows)]
    #[test]
    fn a_second_swap_works_while_a_replaced_program_still_runs() {
        let system = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        let ping = system.join("System32").join("ping.exe");
        if !ping.is_file() {
            eprintln!("pomijam: brak ping.exe");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let tools = tmp.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        let running = tools.join("tool.exe");
        fs::copy(&ping, &running).unwrap();
        let child = std::process::Command::new(&running)
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut child = KillOnDrop(child);

        let stage = tmp.path().join("stage");
        fs::create_dir_all(&stage).unwrap();
        for round in ["NEW-1", "NEW-2", "NEW-3"] {
            fs::write(stage.join("tool.exe"), round).unwrap();
            swap_in(&[(stage.join("tool.exe"), running.clone())], &tools)
                .unwrap_or_else(|e| panic!("{round}: {e:#}"));
            assert_eq!(fs::read_to_string(&running).unwrap(), round);
        }

        let _ = child.0.kill();
        let _ = child.0.wait();
        // Next start: the backups nobody uses any more are cleaned up.
        clean_old_backups(&tools);
        assert_eq!(dir_names(&tools), ["tool.exe"]);
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
