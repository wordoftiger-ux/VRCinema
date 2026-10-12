//! Resumable downloads and SHA-256 helpers for the tools manager.

use std::fs;
use std::path::Path;

use anyhow::Result;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::i18n::{Msg, MsgContext, msg_error};

/// Finds the checksum of `name` in a sums file: lines `<hex>  <name>` (or `<hex> *<name>`);
/// a file that holds just one bare hex digest applies to `name` as well (Deno).
pub fn parse_sums(text: &str, name: &str) -> Option<String> {
    let is_hex = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
    let mut bare = Vec::new();
    for line in text.lines() {
        // `Hash : <hex>` (a PowerShell Get-FileHash listing, as Deno publishes it).
        let line = match line.split_once(':') {
            Some((label, rest)) if label.trim().eq_ignore_ascii_case("hash") => rest,
            _ => line,
        };
        let mut parts = line.split_whitespace();
        let (Some(hash), second) = (parts.next(), parts.next()) else {
            continue;
        };
        if !is_hex(hash) {
            continue;
        }
        match second {
            Some(file) => {
                let file = file.trim_start_matches('*');
                let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
                if base == name {
                    return Some(hash.to_ascii_lowercase());
                }
            }
            None => bare.push(hash.to_ascii_lowercase()),
        }
    }
    if bare.len() == 1 { bare.pop() } else { None }
}

/// Finds the SHA-256 of `name` in a GitHub release description: the first line that contains
/// the file name and exactly one 64-digit hex token (`name: <hex>`, markdown table rows, ...).
pub fn parse_release_notes(body: &str, name: &str) -> Option<String> {
    // Markup (`*`, backticks, `|`, `:`, brackets, spaces) separates tokens; a file name is a
    // run of letters, digits, `-`, `_` and `.`, so `name.sha256` or `xname` are other files.
    let is_name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    body.lines()
        .filter(|line| {
            line.split(|c: char| !is_name_char(c))
                .any(|t| t.trim_end_matches('.') == name)
        })
        .find_map(|line| {
            let mut hashes = line
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()));
            match (hashes.next(), hashes.next()) {
                (Some(hash), None) => Some(hash.to_ascii_lowercase()),
                _ => None,
            }
        })
}

/// Reads the checksum of `name` from the description of a GitHub release (`api_url` is the
/// "latest release" API). `None` when the API is unreachable, answers with an error or the
/// description holds no checksum for `name` — the publisher then simply has none to offer.
pub async fn fetch_release_notes_sum(
    http: &reqwest::Client,
    api_url: &str,
    name: &str,
) -> Option<String> {
    let res = http
        .get(api_url)
        .header(reqwest::header::USER_AGENT, "VRCinema")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    let json: serde_json::Value = res.json().await.ok()?;
    parse_release_notes(json.get("body")?.as_str()?, name)
}

/// Downloads a small text file. `Ok(None)` when the publisher has none (HTTP 404/410).
pub async fn fetch_text(http: &reqwest::Client, url: &str) -> Result<Option<String>> {
    let res = http
        .get(url)
        .send()
        .await
        .msg_context(Msg::error("download_failed").arg("url", url))?;
    let status = res.status();
    if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(msg_error(
            Msg::error("download_failed_status")
                .arg("status", status.as_u16())
                .arg("url", url),
        ));
    }
    Ok(Some(res.text().await?))
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `bytes */<total>` or `bytes <start>-<end>/<total>` → total.
fn content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.trim().parse().ok()
}

fn content_range_start(value: &str) -> Option<u64> {
    value
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Streams `url` into `part`. When `part` already holds data the download resumes with an HTTP
/// `Range` request; a server that ignores `Range` (answers 200) makes it start over. A failed
/// transfer keeps the partial file so the next call can resume. `on_progress(downloaded, total)`
/// is called after every chunk. Returns the number of bytes in the finished file.
pub async fn download_file(
    http: &reqwest::Client,
    url: &str,
    part: &Path,
    on_progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> Result<u64> {
    let existing = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let mut request = http.get(url);
    if existing > 0 {
        request = request.header(RANGE, format!("bytes={existing}-"));
    }
    let mut res = request
        .send()
        .await
        .msg_context(Msg::error("download_failed").arg("url", url))?;
    let status = res.status();

    if status == StatusCode::RANGE_NOT_SATISFIABLE && existing > 0 {
        let total = res
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(content_range_total);
        if total == Some(existing) {
            on_progress(existing, total);
            return Ok(existing); // the previous run had actually finished
        }
        // The partial file does not fit the file on the server: start over.
        fs::remove_file(part)?;
        return Box::pin(download_file(http, url, part, on_progress)).await;
    }
    if !status.is_success() {
        return Err(msg_error(
            Msg::error("download_failed_status")
                .arg("status", status.as_u16())
                .arg("url", url),
        ));
    }

    let range_start = res
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(content_range_start);
    let resumed = status == StatusCode::PARTIAL_CONTENT && range_start == Some(existing);
    let (mut downloaded, total) = if resumed {
        (existing, res.content_length().map(|n| n + existing))
    } else if status == StatusCode::PARTIAL_CONTENT {
        // The partial file cannot be continued from what the server sends: never again.
        let _ = fs::remove_file(part);
        return Err(msg_error(
            Msg::error("download_unexpected_range").arg("url", url),
        ));
    } else {
        (0, res.content_length())
    };

    let mut file = if resumed {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .await?
    } else {
        tokio::fs::File::create(part).await?
    };
    on_progress(downloaded, total);
    let copied: Result<()> = async {
        while let Some(chunk) = res
            .chunk()
            .await
            .msg_context(Msg::error("download_interrupted").arg("url", url))?
        {
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            on_progress(downloaded, total);
        }
        Ok(())
    }
    .await;
    // tokio finishes a write on its blocking pool after `write_all` returned: wait for it
    // also when the transfer broke, so the partial file holds every byte reported (the next
    // attempt resumes from its length).
    let flushed = file.flush().await;
    drop(file);
    copied?;
    flushed?;
    if let Some(total) = total
        && downloaded != total
    {
        return Err(msg_error(
            Msg::error("download_incomplete")
                .arg("downloaded", downloaded)
                .arg("total", total)
                .arg("url", url),
        ));
    }
    Ok(downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_server::TestServer;

    fn blob(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn client() -> reqwest::Client {
        reqwest::Client::new()
    }

    const H1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const H2: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    #[test]
    fn sums_with_names_are_matched_exactly() {
        let text = format!("{H1}  yt-dlp_x86.exe\n{H2} *yt-dlp.exe\n");
        assert_eq!(parse_sums(&text, "yt-dlp.exe").unwrap(), H2.to_lowercase());
        assert_eq!(parse_sums(&text, "yt-dlp_x86.exe").unwrap(), H1);
        assert_eq!(parse_sums(&text, "other.exe"), None);
    }

    #[test]
    fn sums_tolerate_paths_and_crlf() {
        let text = format!("{H1}  ./dir/ffmpeg.zip\r\n\r\n");
        assert_eq!(parse_sums(&text, "ffmpeg.zip").unwrap(), H1);
    }

    #[test]
    fn bare_hash_file_applies_to_any_name() {
        assert_eq!(parse_sums(&format!("{H1}\n"), "deno.zip").unwrap(), H1);
        assert_eq!(
            parse_sums(&format!("{H1}  deno.zip"), "deno.zip").unwrap(),
            H1
        );
    }

    #[test]
    fn powershell_get_filehash_listing_is_a_bare_hash() {
        // What Deno really publishes as `*.zip.sha256sum` (Get-FileHash | Format-List).
        let text = format!(
            "\r\nAlgorithm : SHA256\r\nHash      : {H2}\r\nPath      : C:\\a\\deno\\deno.zip\r\n\r\n"
        );
        assert_eq!(parse_sums(&text, "deno.zip").unwrap(), H2.to_lowercase());
    }

    #[test]
    fn garbage_sums_give_none() {
        assert_eq!(parse_sums("", "x"), None);
        assert_eq!(parse_sums("<html>404</html>", "x"), None);
        assert_eq!(parse_sums(&format!("{H1}\n{H2}\n"), "x"), None);
    }

    #[test]
    fn release_notes_checksum_formats() {
        let name = "cloudflared-windows-amd64.exe";
        for body in [
            format!("{name}: {H2}"),
            format!("- {name}  {H2}"),
            format!("| {name} | {H2} |"),
            format!("| Windows 64 | `{name}` | `{H2}` |"),
            format!("{H2} *{name}"),
            format!("intro\r\n{H1} cloudflared-windows-386.exe\r\n**{name}** (sha256: {H2})\r\n"),
        ] {
            assert_eq!(
                parse_release_notes(&body, name).as_deref(),
                Some(H2.to_lowercase().as_str()),
                "{body}"
            );
        }
        // No file name, no hash, two hashes, or a hash of the wrong length: not published.
        assert_eq!(parse_release_notes(&format!("{H1} other.exe"), name), None);
        assert_eq!(parse_release_notes(name, name), None);
        assert_eq!(
            parse_release_notes(&format!("{name} {H1} {H2}"), name),
            None
        );
        assert_eq!(parse_release_notes(&format!("{name} {H1}{H1}"), name), None);
        assert_eq!(parse_release_notes("", name), None);
    }

    #[test]
    fn release_notes_match_the_file_name_exactly() {
        let name = "cloudflared-windows-amd64.exe";
        // Lines about other files that merely contain the name never count.
        for other in [
            format!("{name}.sha256: {H1}"),
            format!("{name}.sig {H1}"),
            format!("x{name} {H1}"),
            format!("| cloudflared-windows-amd64.msi | {H1} |"),
            format!("{name}-old: {H1}"),
        ] {
            assert_eq!(parse_release_notes(&other, name), None, "{other}");
            // ...and do not shadow the real line that follows.
            let both = format!("{other}\n{name}: {H2}\n");
            assert_eq!(
                parse_release_notes(&both, name).as_deref(),
                Some(H2.to_lowercase().as_str()),
                "{both}"
            );
        }
        assert_eq!(
            parse_release_notes(&format!("|{name}|{H2}|"), name).as_deref(),
            Some(H2.to_lowercase().as_str())
        );
    }

    #[tokio::test]
    async fn fetch_text_returns_none_on_404() {
        let server = TestServer::start().await;
        server.put("a/sums", "hello");
        let url = |p: &str| format!("{}/{p}", server.base);
        assert_eq!(
            fetch_text(&client(), &url("a/sums"))
                .await
                .unwrap()
                .unwrap(),
            "hello"
        );
        assert_eq!(fetch_text(&client(), &url("a/none")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn progress_grows_to_the_total() {
        let server = TestServer::start().await;
        let data = blob(200_000);
        server.put("f.bin", data.clone());
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("f.part");
        let mut seen: Vec<(u64, Option<u64>)> = Vec::new();
        let n = download_file(
            &client(),
            &format!("{}/f.bin", server.base),
            &part,
            &mut |d, t| seen.push((d, t)),
        )
        .await
        .unwrap();
        assert_eq!(n, 200_000);
        assert_eq!(fs::read(&part).unwrap(), data);
        assert!(seen.len() > 2, "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "monotonic");
        assert!(seen.iter().all(|(_, t)| *t == Some(200_000)));
        assert_eq!(seen.last().unwrap().0, 200_000);
    }

    #[tokio::test]
    async fn interrupted_download_resumes_with_range_and_matches_checksum() {
        let server = TestServer::start().await;
        let data = blob(300_000);
        server.put("f.bin", data.clone());
        server.drop_next_response_after(100_000);
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("f.part");
        let url = format!("{}/f.bin", server.base);

        let err = download_file(&client(), &url, &part, &mut |_, _| {})
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("interrupted"), "{err:#}");
        let kept = fs::metadata(&part).unwrap().len();
        assert!(kept > 0 && kept < 300_000, "kept {kept}");

        let mut first = None;
        download_file(&client(), &url, &part, &mut |d, _| {
            first.get_or_insert(d);
        })
        .await
        .unwrap();
        assert_eq!(
            first,
            Some(kept),
            "progress continues from the partial size"
        );
        assert_eq!(fs::read(&part).unwrap(), data);
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let expect: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sha256_file(&part).unwrap(), expect);
        let ranges = server.requests();
        assert_eq!(ranges[0].1, None);
        assert_eq!(
            ranges[1].1.as_deref(),
            Some(format!("bytes={kept}-").as_str())
        );
    }

    /// Every byte reported before a failed transfer is in the partial file when the call
    /// returns (the next attempt resumes from its length). tokio hands file writes to its
    /// blocking pool; here that pool has one thread, kept busy for a moment after every chunk,
    /// so the last write is still queued when the connection breaks.
    #[test]
    fn a_failed_transfer_leaves_every_reported_byte_on_disk() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let server = TestServer::start().await;
            server.put("f.bin", blob(200_000));
            server.drop_next_response_after(100_000);
            let dir = tempfile::tempdir().unwrap();
            let part = dir.path().join("f.part");
            let url = format!("{}/f.bin", server.base);
            let mut last = 0;
            let err = download_file(&client(), &url, &part, &mut |d, _| {
                last = d;
                tokio::task::spawn_blocking(|| {
                    std::thread::sleep(std::time::Duration::from_millis(100))
                });
            })
            .await
            .unwrap_err();
            assert!(format!("{err:#}").contains("interrupted"), "{err:#}");
            let on_disk = fs::metadata(&part).unwrap().len();
            assert_eq!(on_disk, last, "reported {last}, on disk {on_disk}");
        });
    }

    #[tokio::test]
    async fn a_206_from_the_wrong_offset_discards_the_partial_file() {
        let server = TestServer::start().await;
        let data = blob(120_000);
        server.put("f.bin", data.clone());
        server.set_wrong_range_start(true);
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("f.part");
        fs::write(&part, &data[..50_000]).unwrap();
        let url = format!("{}/f.bin", server.base);
        let err = download_file(&client(), &url, &part, &mut |_, _| {})
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected range"), "{err:#}");
        assert!(!part.exists(), "the partial file must not be resumed again");
        server.set_wrong_range_start(false);
        download_file(&client(), &url, &part, &mut |_, _| {})
            .await
            .unwrap();
        assert_eq!(fs::read(&part).unwrap(), data);
    }

    #[tokio::test]
    async fn server_ignoring_range_restarts_from_scratch() {
        let server = TestServer::start().await;
        let data = blob(120_000);
        server.put("f.bin", data.clone());
        server.set_ignore_range(true);
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("f.part");
        fs::write(&part, &data[..50_000]).unwrap();
        download_file(
            &client(),
            &format!("{}/f.bin", server.base),
            &part,
            &mut |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&part).unwrap(), data);
    }

    #[tokio::test]
    async fn complete_part_file_is_accepted_on_416() {
        let server = TestServer::start().await;
        let data = blob(50_000);
        server.put("f.bin", data.clone());
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("f.part");
        fs::write(&part, &data).unwrap();
        let n = download_file(
            &client(),
            &format!("{}/f.bin", server.base),
            &part,
            &mut |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(n, 50_000);
        assert_eq!(fs::read(&part).unwrap(), data);
    }

    #[tokio::test]
    async fn oversized_part_file_is_discarded_and_redownloaded() {
        let server = TestServer::start().await;
        let data = blob(10_000);
        server.put("f.bin", data.clone());
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("f.part");
        fs::write(&part, blob(30_000)).unwrap();
        download_file(
            &client(),
            &format!("{}/f.bin", server.base),
            &part,
            &mut |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&part).unwrap(), data);
    }

    #[tokio::test]
    async fn http_error_is_reported() {
        let server = TestServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let err = download_file(
            &client(),
            &format!("{}/missing", server.base),
            &dir.path().join("x.part"),
            &mut |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }
}
