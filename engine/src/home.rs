//! The app's home directory: `VRCINEMA_HOME`, `%LOCALAPPDATA%\VRCinema` (Windows) or
//! `$XDG_DATA_HOME/vrcinema` / `~/.local/share/vrcinema` (Linux).
//!
//! The program was called VRCinema before: when the new folder does not exist yet but the old one
//! (`VRCinema` / `vrcinema` in the same place) does, the old folder simply stays in use (nothing is
//! moved or copied, so the owner's data can never be lost half-way).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::i18n::{Msg, msg_error};

/// Subfolders created inside the home directory.
pub const SUBDIRS: [&str; 3] = ["data", "tools", "logs"];

/// Folder name under `%LOCALAPPDATA%` (Windows) and under the XDG data folder (elsewhere).
const DIR_WINDOWS: &str = "VRCinema";
const DIR_XDG: &str = "vrcinema";
/// The same folders while the program was called VRCinema.
const LEGACY_DIR_WINDOWS: &str = "VRCinema";
const LEGACY_DIR_XDG: &str = "vrcinema";

/// `parent/name`, unless only `parent/legacy` exists: then that one (the data of VRCinema).
fn current_or_legacy(parent: PathBuf, name: &str, legacy: &str) -> PathBuf {
    let new = parent.join(name);
    let old = parent.join(legacy);
    if !new.exists() && old.is_dir() {
        old
    } else {
        new
    }
}

/// Chooses the home directory: a non-empty override wins, then `%LOCALAPPDATA%\VRCinema`,
/// then (non-Windows) `$XDG_DATA_HOME/vrcinema` (an absolute path only, as the XDG rules say),
/// then `$HOME/.local/share/vrcinema`. Each falls back to the old VRCinema folder in the same
/// place when only that one exists (see the module notes).
pub fn resolve_home(
    override_dir: Option<OsString>,
    local_app_data: Option<OsString>,
    xdg_data_home: Option<OsString>,
    home_dir: Option<OsString>,
) -> anyhow::Result<PathBuf> {
    let non_empty = |v: Option<OsString>| v.filter(|s| !s.is_empty());
    if let Some(dir) = non_empty(override_dir) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = non_empty(local_app_data) {
        return Ok(current_or_legacy(
            PathBuf::from(dir),
            DIR_WINDOWS,
            LEGACY_DIR_WINDOWS,
        ));
    }
    if let Some(dir) = non_empty(xdg_data_home).map(PathBuf::from)
        && dir.has_root()
    {
        return Ok(current_or_legacy(dir, DIR_XDG, LEGACY_DIR_XDG));
    }
    if let Some(dir) = non_empty(home_dir) {
        let share = PathBuf::from(dir).join(".local").join("share");
        return Ok(current_or_legacy(share, DIR_XDG, LEGACY_DIR_XDG));
    }
    Err(msg_error(Msg::error("home_unknown")))
}

/// Creates the home directory with its `data`, `tools` and `logs` subfolders.
pub fn ensure_home(home: &Path) -> anyhow::Result<()> {
    for sub in SUBDIRS {
        let dir = home.join(sub);
        std::fs::create_dir_all(&dir).map_err(|e| {
            msg_error(
                Msg::error("cannot_create_data_dir")
                    .arg("path", dir.display())
                    .arg("detail", e),
            )
        })?;
    }
    Ok(())
}

/// The home directory from the environment, created if missing.
pub fn app_home() -> anyhow::Result<PathBuf> {
    let home = resolve_home(
        std::env::var_os("VRCINEMA_HOME"),
        std::env::var_os("LOCALAPPDATA"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )?;
    ensure_home(&home)?;
    Ok(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    #[test]
    fn override_wins_then_local_app_data() {
        assert_eq!(
            resolve_home(os("D:/x"), os("C:/L"), None, None).unwrap(),
            PathBuf::from("D:/x")
        );
        assert_eq!(
            resolve_home(None, os("C:/L"), os("/x"), None).unwrap(),
            PathBuf::from("C:/L").join("VRCinema")
        );
        assert_eq!(
            resolve_home(os(""), os("C:/L"), None, os("/home/a")).unwrap(),
            PathBuf::from("C:/L").join("VRCinema")
        );
    }

    #[test]
    fn linux_uses_the_xdg_data_folder_then_the_home_folder() {
        assert_eq!(
            resolve_home(None, None, os("/data/xdg"), os("/home/a")).unwrap(),
            PathBuf::from("/data/xdg").join("vrcinema")
        );
        // Unset, empty or relative XDG_DATA_HOME: ~/.local/share.
        for xdg in [None, os(""), os("rel/dir")] {
            assert_eq!(
                resolve_home(None, None, xdg, os("/home/a")).unwrap(),
                PathBuf::from("/home/a/.local/share").join("vrcinema")
            );
        }
        assert_eq!(
            resolve_home(os("/srv/mp4"), None, os("/data/xdg"), os("/home/a")).unwrap(),
            PathBuf::from("/srv/mp4")
        );
    }

    #[test]
    fn the_old_vrcinema_folder_stays_in_use_until_a_new_one_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let base = || Some(tmp.path().as_os_str().to_owned());
        // Neither exists: the new name.
        assert_eq!(
            resolve_home(None, base(), None, None).unwrap(),
            tmp.path().join("VRCinema")
        );
        assert_eq!(
            resolve_home(None, None, base(), None).unwrap(),
            tmp.path().join("vrcinema")
        );
        // Only the old one (the names differ only in case on Windows): it stays in use.
        std::fs::create_dir(tmp.path().join("VRCinema")).unwrap();
        let _ = std::fs::create_dir(tmp.path().join("vrcinema"));
        assert_eq!(
            resolve_home(None, base(), None, None).unwrap(),
            tmp.path().join("VRCinema")
        );
        assert_eq!(
            resolve_home(None, None, base(), None).unwrap(),
            tmp.path().join("vrcinema")
        );
        // Both: the new one wins.
        std::fs::create_dir(tmp.path().join("VRCinema")).unwrap();
        let _ = std::fs::create_dir(tmp.path().join("vrcinema"));
        assert_eq!(
            resolve_home(None, base(), None, None).unwrap(),
            tmp.path().join("VRCinema")
        );
        assert_eq!(
            resolve_home(None, None, base(), None).unwrap(),
            tmp.path().join("vrcinema")
        );
        // An override is never redirected.
        assert_eq!(
            resolve_home(Some(tmp.path().join("x").into()), base(), None, None).unwrap(),
            tmp.path().join("x")
        );
    }

    #[test]
    fn nothing_known_is_an_error() {
        assert!(resolve_home(None, None, None, None).is_err());
    }

    #[test]
    fn ensure_home_creates_subfolders() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("nowy").join("VRCinema");
        ensure_home(&home).unwrap();
        for sub in SUBDIRS {
            assert!(home.join(sub).is_dir(), "{sub}");
        }
        ensure_home(&home).unwrap();
    }
}
