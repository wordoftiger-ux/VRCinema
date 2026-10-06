//! Self-update of `VRCinema.exe`: look for the latest GitHub release, download it next to the
//! running file, verify SHA-256 and swap the files (rolling back on any failure).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::tools::download::{download_file, fetch_text, parse_sums, sha256_file};

pub const DEFAULT_API_URL: &str =
    "https://api.github.com/repos/wordoftiger-ux/vrcinema/releases/latest";
const EXE_ASSET: &str = "VRCinema.exe";
const SUM_ASSET: &str = "VRCinema.exe.sha256";
const NEW_NAME: &str = "VRCinema.new.exe";
const OLD_NAME: &str = "VRCinema.old.exe";
const USER_AGENT: &str = concat!("VRCinema/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub exe_url: String,
    pub sha256_url: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: Version,
    pub available: Option<Release>,
    pub error: Option<String>,
}

// Serialized for the HTTP layer (camelCase, version as a string).
impl Serialize for Release {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Release", 4)?;
        st.serialize_field("version", &self.version.to_string())?;
        st.serialize_field("exeUrl", &self.exe_url)?;
        st.serialize_field("sha256Url", &self.sha256_url)?;
        st.serialize_field("notes", &self.notes)?;
        st.end()
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

fn parse_version(tag: &str) -> Option<Version> {
    Version::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Turns the API answer into a usable release (`None`: draft/prerelease, bad tag or missing
/// assets).
fn parse_release(body: &str) -> Result<Option<Release>> {
    let api: ApiRelease = serde_json::from_str(body).context("Niepoprawna odpowiedź GitHuba")?;
    if api.draft || api.prerelease {
        return Ok(None);
    }
    let Some(version) = parse_version(&api.tag_name) else {
        return Ok(None);
    };
    let find = |name: &str| {
        api.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
    };
    let (Some(exe_url), Some(sha256_url)) = (find(EXE_ASSET), find(SUM_ASSET)) else {
        return Ok(None);
    };
    Ok(Some(Release {
        version,
        exe_url,
        sha256_url,
        notes: api.body.unwrap_or_default(),
    }))
}

async fn fetch_latest(http: &reqwest::Client, api_base: &str) -> Result<Option<Release>> {
    let res = http
        .get(api_base)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .context("Brak połączenia z GitHubem")?;
    let status = res.status();
    if !status.is_success() {
        bail!("GitHub odpowiedział kodem {}", status.as_u16());
    }
    parse_release(&res.text().await?)
}

/// Asks GitHub for the latest release. Never fails: problems end up in `error`, and a release
/// that is not newer than `current` (or equals `skipped_version`) is not offered.
pub async fn check_latest(
    http: &reqwest::Client,
    api_base: &str,
    current: &Version,
    skipped_version: Option<&str>,
) -> UpdateInfo {
    let (available, error) = match fetch_latest(http, api_base).await {
        Ok(release) => {
            let skipped = skipped_version.and_then(parse_version);
            let release = release
                .filter(|r| r.version > *current)
                .filter(|r| skipped.as_ref() != Some(&r.version));
            (release, None)
        }
        Err(e) => (None, Some(format!("{e:#}"))),
    };
    UpdateInfo {
        current: current.clone(),
        available,
        error,
    }
}

/// Downloads, verifies and installs `release` over `exe_path`; the caller restarts the process.
pub async fn apply(release: &Release, exe_path: &Path, http: &reqwest::Client) -> Result<()> {
    apply_with(release, exe_path, http, &|from, to| fs::rename(from, to)).await
}

async fn apply_with(
    release: &Release,
    exe_path: &Path,
    http: &reqwest::Client,
    rename: &(dyn Fn(&Path, &Path) -> io::Result<()> + Sync),
) -> Result<()> {
    let new_path = exe_path.with_file_name(NEW_NAME);
    let old_path = exe_path.with_file_name(OLD_NAME);
    // A leftover from an earlier attempt must never be resumed or reused.
    let _ = fs::remove_file(&new_path);

    if let Err(e) = prepare(release, &new_path, http).await {
        let _ = fs::remove_file(&new_path);
        return Err(e);
    }
    if let Err(e) = swap(exe_path, &new_path, &old_path, rename) {
        let _ = fs::remove_file(&new_path);
        return Err(e);
    }
    Ok(())
}

/// Download + checksum check; leaves the verified file at `new_path`.
async fn prepare(release: &Release, new_path: &Path, http: &reqwest::Client) -> Result<()> {
    let sums = fetch_text(http, &release.sha256_url)
        .await?
        .ok_or_else(|| anyhow!("Brak pliku sumy kontrolnej ({SUM_ASSET})"))?;
    let expected = parse_sums(&sums, EXE_ASSET)
        .ok_or_else(|| anyhow!("Nieczytelny plik sumy kontrolnej ({SUM_ASSET})"))?;
    download_file(http, &release.exe_url, new_path, &mut |_, _| {}).await?;
    let actual = sha256_file(new_path)?;
    if actual != expected {
        bail!("Suma kontrolna pobranego pliku się nie zgadza");
    }
    Ok(())
}

/// exe → old, new → exe; when the second step fails the original name is restored.
fn swap(
    exe_path: &Path,
    new_path: &Path,
    old_path: &Path,
    rename: &(dyn Fn(&Path, &Path) -> io::Result<()> + Sync),
) -> Result<()> {
    if old_path.exists() {
        fs::remove_file(old_path).context("Nie można usunąć poprzedniej kopii programu")?;
    }
    rename(exe_path, old_path).context("Nie można zmienić nazwy bieżącego programu")?;
    if let Err(e) = rename(new_path, exe_path) {
        let restored = rename(old_path, exe_path);
        return Err(match restored {
            Ok(()) => anyhow!(e).context("Nie można zainstalować nowej wersji"),
            Err(r) => anyhow!(e).context(format!(
                "Nie można zainstalować nowej wersji ani przywrócić starej (kopia: {}): {r}",
                old_path.display()
            )),
        });
    }
    Ok(())
}

/// Removes `VRCinema.old.exe` next to `exe_path` (the old process may still be exiting, so retry).
pub async fn cleanup_after_update(exe_path: &Path) {
    let old: PathBuf = exe_path.with_file_name(OLD_NAME);
    for _ in 0..20 {
        if !old.exists() || fs::remove_file(&old).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_server::TestServer;
    use sha2::{Digest, Sha256};

    fn http() -> reqwest::Client {
        reqwest::Client::new()
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn hex(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn blob(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 253) as u8).collect()
    }

    /// Serves release JSON at /api/latest plus assets; returns the API URL.
    fn publish(server: &TestServer, tag: &str, exe: &[u8], sums: Option<&str>) -> String {
        server.put("exe/VRCinema.exe", exe);
        let sums_text = sums.map(str::to_string).unwrap_or_else(|| hex(exe));
        server.put("exe/VRCinema.exe.sha256", sums_text);
        let json = serde_json::json!({
            "tag_name": tag,
            "body": "notes",
            "draft": false,
            "prerelease": false,
            "assets": [
                {"name": "VRCinema.exe", "browser_download_url": format!("{}/exe/VRCinema.exe", server.base)},
                {"name": "VRCinema.exe.sha256", "browser_download_url": format!("{}/exe/VRCinema.exe.sha256", server.base)},
            ],
        });
        server.put("api/latest", json.to_string());
        format!("{}/api/latest", server.base)
    }

    fn release(server: &TestServer) -> Release {
        Release {
            version: v("9.9.9"),
            exe_url: format!("{}/exe/VRCinema.exe", server.base),
            sha256_url: format!("{}/exe/VRCinema.exe.sha256", server.base),
            notes: String::new(),
        }
    }

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("VRCinema.exe");
        fs::write(&exe, b"old-bytes").unwrap();
        (dir, exe)
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn newer_version_is_offered() {
        let server = TestServer::start().await;
        let api = publish(&server, "v0.3.0", b"x", None);
        let info = check_latest(&http(), &api, &v("0.2.0"), None).await;
        let rel = info.available.clone().expect("update expected");
        assert_eq!(rel.version, v("0.3.0"));
        assert_eq!(rel.notes, "notes");
        assert!(rel.exe_url.ends_with("/exe/VRCinema.exe"));
        assert!(info.error.is_none());
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["current"], "0.2.0");
        assert_eq!(json["available"]["version"], "0.3.0");
        assert!(json["available"]["exeUrl"].is_string());
    }

    #[tokio::test]
    async fn equal_and_older_versions_are_not_offered() {
        let server = TestServer::start().await;
        let api = publish(&server, "v0.2.0", b"x", None);
        assert!(
            check_latest(&http(), &api, &v("0.2.0"), None)
                .await
                .available
                .is_none()
        );
        assert!(
            check_latest(&http(), &api, &v("0.3.0"), None)
                .await
                .available
                .is_none()
        );
    }

    #[tokio::test]
    async fn skipped_version_is_not_offered_but_a_newer_one_is() {
        let server = TestServer::start().await;
        let api = publish(&server, "v0.3.0", b"x", None);
        let skipped = check_latest(&http(), &api, &v("0.2.0"), Some("0.3.0")).await;
        assert!(skipped.available.is_none());
        assert!(skipped.error.is_none());
        let other = check_latest(&http(), &api, &v("0.2.0"), Some("0.2.5")).await;
        assert!(other.available.is_some());
    }

    #[tokio::test]
    async fn release_without_both_assets_is_unusable() {
        let server = TestServer::start().await;
        let json = serde_json::json!({
            "tag_name": "v1.0.0",
            "assets": [{"name": "VRCinema.exe", "browser_download_url": "http://x/e"}],
        });
        server.put("api/latest", json.to_string());
        let api = format!("{}/api/latest", server.base);
        let info = check_latest(&http(), &api, &v("0.2.0"), None).await;
        assert!(info.available.is_none());
    }

    #[tokio::test]
    async fn not_found_gives_no_update_and_an_error_message() {
        let server = TestServer::start().await;
        let api = format!("{}/api/missing", server.base);
        let info = check_latest(&http(), &api, &v("0.2.0"), None).await;
        assert!(info.available.is_none());
        assert!(info.error.unwrap().contains("404"));
    }

    #[tokio::test]
    async fn unreachable_server_gives_no_update() {
        let info = check_latest(&http(), "http://127.0.0.1:1/x", &v("0.2.0"), None).await;
        assert!(info.available.is_none());
        assert!(info.error.is_some());
    }

    #[tokio::test]
    async fn success_replaces_exe_and_keeps_old() {
        let server = TestServer::start().await;
        let data = blob(200_000);
        publish(
            &server,
            "v9.9.9",
            &data,
            Some(&format!("{}  VRCinema.exe", hex(&data))),
        );
        let (dir, exe) = setup();
        apply(&release(&server), &exe, &http()).await.unwrap();
        assert_eq!(fs::read(&exe).unwrap(), data);
        assert_eq!(
            fs::read(dir.path().join("VRCinema.old.exe")).unwrap(),
            b"old-bytes"
        );
        assert_eq!(leftovers(dir.path()), ["VRCinema.exe", "VRCinema.old.exe"]);
    }

    #[tokio::test]
    async fn stale_old_and_new_files_are_replaced() {
        let server = TestServer::start().await;
        let data = blob(5_000);
        publish(&server, "v9.9.9", &data, None);
        let (dir, exe) = setup();
        fs::write(dir.path().join("VRCinema.old.exe"), b"stale-old").unwrap();
        fs::write(dir.path().join("VRCinema.new.exe"), b"stale-new-partial").unwrap();
        apply(&release(&server), &exe, &http()).await.unwrap();
        assert_eq!(fs::read(&exe).unwrap(), data);
        assert_eq!(
            fs::read(dir.path().join("VRCinema.old.exe")).unwrap(),
            b"old-bytes"
        );
        assert_eq!(leftovers(dir.path()), ["VRCinema.exe", "VRCinema.old.exe"]);
    }

    #[tokio::test]
    async fn interrupted_download_leaves_original_and_no_new_file() {
        let server = TestServer::start().await;
        let data = blob(300_000);
        publish(&server, "v9.9.9", &data, None);
        server.drop_next_response_after(50_000);
        let (dir, exe) = setup();
        let err = apply(&release(&server), &exe, &http()).await.unwrap_err();
        assert!(!format!("{err:#}").is_empty());
        assert_eq!(fs::read(&exe).unwrap(), b"old-bytes");
        assert_eq!(leftovers(dir.path()), ["VRCinema.exe"]);
    }

    #[tokio::test]
    async fn wrong_checksum_leaves_original_untouched() {
        let server = TestServer::start().await;
        let data = blob(10_000);
        publish(&server, "v9.9.9", &data, Some(&hex(b"something else")));
        let (dir, exe) = setup();
        let err = apply(&release(&server), &exe, &http()).await.unwrap_err();
        assert!(format!("{err:#}").contains("Suma kontrolna"));
        assert_eq!(fs::read(&exe).unwrap(), b"old-bytes");
        assert_eq!(leftovers(dir.path()), ["VRCinema.exe"]);
    }

    #[tokio::test]
    async fn missing_sums_asset_leaves_original_untouched() {
        let server = TestServer::start().await;
        server.put("exe/VRCinema.exe", blob(100));
        let (dir, exe) = setup();
        assert!(apply(&release(&server), &exe, &http()).await.is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"old-bytes");
        assert_eq!(leftovers(dir.path()), ["VRCinema.exe"]);
    }

    #[tokio::test]
    async fn failing_second_rename_restores_the_original() {
        let server = TestServer::start().await;
        let data = blob(20_000);
        publish(&server, "v9.9.9", &data, None);
        let (dir, exe) = setup();
        // Fails only for new -> exe; every other rename is real.
        let rename = |from: &Path, to: &Path| {
            if from.file_name().is_some_and(|n| n == NEW_NAME) {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            } else {
                fs::rename(from, to)
            }
        };
        let err = apply_with(&release(&server), &exe, &http(), &rename)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("denied"));
        assert_eq!(fs::read(&exe).unwrap(), b"old-bytes");
        assert_eq!(leftovers(dir.path()), ["VRCinema.exe"]);
    }

    #[tokio::test]
    async fn failing_first_rename_leaves_everything_untouched() {
        let server = TestServer::start().await;
        publish(&server, "v9.9.9", &blob(2_000), None);
        let (dir, exe) = setup();
        let rename = |_: &Path, _: &Path| Err(io::Error::other("locked"));
        assert!(
            apply_with(&release(&server), &exe, &http(), &rename)
                .await
                .is_err()
        );
        assert_eq!(fs::read(&exe).unwrap(), b"old-bytes");
        assert_eq!(leftovers(dir.path()), ["VRCinema.exe"]);
    }

    #[tokio::test]
    async fn cleanup_removes_old_exe_only() {
        let (dir, exe) = setup();
        let old = dir.path().join("VRCinema.old.exe");
        fs::write(&old, b"o").unwrap();
        cleanup_after_update(&exe).await;
        assert!(!old.exists());
        assert!(exe.exists());
        cleanup_after_update(&exe).await; // nothing to remove: no panic
    }
}
