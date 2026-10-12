//! The app's home directory: `VRCINEMA_HOME` or `%LOCALAPPDATA%\VRCinema`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::i18n::{Msg, msg_error};

/// Subfolders created inside the home directory.
pub const SUBDIRS: [&str; 3] = ["data", "tools", "logs"];

/// Chooses the home directory: a non-empty override wins, then `%LOCALAPPDATA%\VRCinema`,
/// then (non-Windows) `$HOME/.local/share/VRCinema`.
pub fn resolve_home(
    override_dir: Option<OsString>,
    local_app_data: Option<OsString>,
    home_dir: Option<OsString>,
) -> anyhow::Result<PathBuf> {
    let non_empty = |v: Option<OsString>| v.filter(|s| !s.is_empty());
    if let Some(dir) = non_empty(override_dir) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = non_empty(local_app_data) {
        return Ok(PathBuf::from(dir).join("VRCinema"));
    }
    if let Some(dir) = non_empty(home_dir) {
        return Ok(PathBuf::from(dir)
            .join(".local")
            .join("share")
            .join("VRCinema"));
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
            resolve_home(os("D:/x"), os("C:/L"), None).unwrap(),
            PathBuf::from("D:/x")
        );
        assert_eq!(
            resolve_home(None, os("C:/L"), None).unwrap(),
            PathBuf::from("C:/L").join("VRCinema")
        );
        assert_eq!(
            resolve_home(os(""), os("C:/L"), None).unwrap(),
            PathBuf::from("C:/L").join("VRCinema")
        );
    }

    #[test]
    fn nothing_known_is_an_error() {
        assert!(resolve_home(None, None, None).is_err());
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
