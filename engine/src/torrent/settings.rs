//! Settings → Channel → Torrents (`torrentsEnabled`, `torrentUploadKbps`, `torrentDiskCapGb`).

use serde::Serialize;
use serde_json::Value;

use super::{DEFAULT_DISK_CAP_GB, DEFAULT_UPLOAD_KBPS};
use crate::i18n::Msg;

/// Most kilobytes a second the owner may allow for uploading (0: unlimited).
pub const MAX_UPLOAD_KBPS: u64 = 1_000_000;
/// Most gigabytes of disk the torrents may take.
pub const MAX_DISK_CAP_GB: u64 = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentSettings {
    pub torrents_enabled: bool,
    pub torrent_upload_kbps: u64,
    pub torrent_disk_cap_gb: u64,
}

impl Default for TorrentSettings {
    fn default() -> Self {
        Self {
            torrents_enabled: false,
            torrent_upload_kbps: DEFAULT_UPLOAD_KBPS,
            torrent_disk_cap_gb: DEFAULT_DISK_CAP_GB,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TorrentPatch {
    pub enabled: Option<bool>,
    pub upload_kbps: Option<u64>,
    pub disk_cap_gb: Option<u64>,
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
        out
    }
}

impl TorrentSettings {
    /// Applies a stored value; `false`: not a torrent key. A wrong value keeps the default.
    pub fn read_stored(&mut self, key: &str, value: &Value) -> bool {
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
