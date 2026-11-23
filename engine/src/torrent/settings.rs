//! Settings → Channel → Torrents (`torrentsEnabled`, `torrentUploadKbps`, `torrentDiskCapGb`,
//! `torrentDir`).

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::{DEFAULT_DISK_CAP_GB, DEFAULT_UPLOAD_KBPS};
use crate::i18n::Msg;

/// Most kilobytes a second the owner may allow for uploading (0: unlimited).
pub const MAX_UPLOAD_KBPS: u64 = 1_000_000;
/// Most gigabytes of disk the torrents may take.
pub const MAX_DISK_CAP_GB: u64 = 100_000;
/// The folder made inside the owner's chosen download folder: only it is ever cleaned.
pub const DIR_NAME: &str = "VRCinema-torrents";
/// Longest accepted folder path (characters).
pub const MAX_DIR_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentSettings {
    pub torrents_enabled: bool,
    pub torrent_upload_kbps: u64,
    pub torrent_disk_cap_gb: u64,
    /// The owner's download folder (empty: `<data dir>/torrents`); the files go into its
    /// [`DIR_NAME`] subfolder.
    pub torrent_dir: String,
}

impl Default for TorrentSettings {
    fn default() -> Self {
        Self {
            torrents_enabled: false,
            torrent_upload_kbps: DEFAULT_UPLOAD_KBPS,
            torrent_disk_cap_gb: DEFAULT_DISK_CAP_GB,
            torrent_dir: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TorrentPatch {
    pub enabled: Option<bool>,
    pub upload_kbps: Option<u64>,
    pub disk_cap_gb: Option<u64>,
    pub dir: Option<String>,
}

/// The torrents' folder of the setting: `default` when empty, else the [`DIR_NAME`] subfolder
/// of the chosen one.
pub fn root_of(torrent_dir: &str, default: &Path) -> PathBuf {
    if torrent_dir.is_empty() {
        default.to_path_buf()
    } else {
        Path::new(torrent_dir).join(DIR_NAME)
    }
}

/// A chosen download folder may be used: an absolute path of an existing folder where the
/// [`DIR_NAME`] subfolder can be made and written to (it is made now).
pub fn check_dir(dir: &str) -> Result<(), Msg> {
    let path = Path::new(dir);
    let bad = |code: &'static str| Msg::error(code).arg("path", dir);
    if !path.is_absolute() {
        return Err(bad("torrent_dir_not_absolute"));
    }
    if !path.is_dir() {
        return Err(bad("torrent_dir_missing"));
    }
    let ours = path.join(DIR_NAME);
    let probe = ours.join(".write-test");
    let writable = std::fs::create_dir_all(&ours).is_ok()
        && std::fs::write(&probe, b"ok").is_ok()
        && std::fs::remove_file(&probe).is_ok();
    if !writable {
        return Err(bad("torrent_dir_not_writable"));
    }
    Ok(())
}

/// A network share or a device path (`\\server\share`, `\\?\…`, `\\.\…`, also with `/`): only
/// a folder of a local drive is taken.
pub fn is_network_or_device(dir: &str) -> bool {
    let b = dir.as_bytes();
    b.len() >= 2 && matches!(b[0], b'\\' | b'/') && matches!(b[1], b'\\' | b'/')
}

/// The text form of `torrentDir`: trimmed, empty allowed (the default folder).
fn dir_text(key: &str, value: &Value) -> Result<String, Msg> {
    let s = value
        .as_str()
        .ok_or_else(|| Msg::error("torrent_dir_not_absolute").arg("path", key))?
        .trim();
    if s.chars().count() > MAX_DIR_LEN || s.contains('\0') {
        return Err(Msg::error("torrent_dir_not_absolute").arg("path", s));
    }
    if is_network_or_device(s) {
        return Err(Msg::error("torrent_dir_not_local").arg("path", s));
    }
    Ok(s.to_string())
}

/// Validates one settings key; `None`: not a torrent key.
pub fn sanitize_key(key: &str, value: &Value, out: &mut TorrentPatch) -> Option<Result<(), Msg>> {
    let range = |max: u64, min: u64| {
        Msg::error("setting_out_of_range")
            .arg("key", key)
            .arg("min", min)
            .arg("max", max)
    };
    let result = match key {
        "torrentsEnabled" => value
            .as_bool()
            .map(|b| out.enabled = Some(b))
            .ok_or_else(|| Msg::error("setting_not_boolean").arg("key", key)),
        "torrentUploadKbps" => value
            .as_u64()
            .filter(|v| *v <= MAX_UPLOAD_KBPS)
            .map(|v| out.upload_kbps = Some(v))
            .ok_or_else(|| range(MAX_UPLOAD_KBPS, 0)),
        "torrentDiskCapGb" => value
            .as_u64()
            .filter(|v| (1..=MAX_DISK_CAP_GB).contains(v))
            .map(|v| out.disk_cap_gb = Some(v))
            .ok_or_else(|| range(MAX_DISK_CAP_GB, 1)),
        "torrentDir" => dir_text(key, value).and_then(|d| {
            if !d.is_empty() {
                check_dir(&d)?;
            }
            out.dir = Some(d);
            Ok(())
        }),
        _ => return None,
    };
    Some(result)
}

impl TorrentPatch {
    /// The stored JSON texts of the keys present.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(b) = self.enabled {
            out.push(("torrentsEnabled", b.to_string()));
        }
        if let Some(v) = self.upload_kbps {
            out.push(("torrentUploadKbps", v.to_string()));
        }
        if let Some(v) = self.disk_cap_gb {
            out.push(("torrentDiskCapGb", v.to_string()));
        }
        if let Some(d) = &self.dir {
            out.push(("torrentDir", Value::String(d.clone()).to_string()));
        }
        out
    }
}

impl TorrentSettings {
    /// Applies a stored value; `false`: not a torrent key. A wrong value keeps the default.
    pub fn read_stored(&mut self, key: &str, value: &Value) -> bool {
        // The stored folder is kept as it is even when its drive is gone now: the torrents
        // then fail with that reason instead of filling the default drive.
        if key == "torrentDir" {
            if let Ok(d) = dir_text(key, value)
                && (d.is_empty() || Path::new(&d).is_absolute())
            {
                self.torrent_dir = d;
            }
            return true;
        }
        let mut patch = TorrentPatch::default();
        match sanitize_key(key, value, &mut patch) {
            None => false,
            Some(Err(_)) => true,
            Some(Ok(())) => {
                if let Some(b) = patch.enabled {
                    self.torrents_enabled = b;
                }
                if let Some(v) = patch.upload_kbps {
                    self.torrent_upload_kbps = v;
                }
                if let Some(v) = patch.disk_cap_gb {
                    self.torrent_disk_cap_gb = v;
                }
                true
            }
        }
    }
}
