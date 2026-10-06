//! Resumable downloads and SHA-256 helpers for the tools manager.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

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

/// Downloads a small text file. `Ok(None)` when the publisher has none (HTTP 404/410).
pub async fn fetch_text(http: &reqwest::Client, url: &str) -> Result<Option<String>> {
    let res = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("Pobieranie nie powiodło się: {url}"))?;
    let status = res.status();
    if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
        return Ok(None);
    }
    if !status.is_success() {
        bail!("Pobieranie nie powiodło się: {} {url}", status.as_u16());
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
        .with_context(|| format!("Pobieranie nie powiodło się: {url}"))?;
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
        bail!("Pobieranie nie powiodło się: {} {url}", status.as_u16());
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
        bail!("Serwer zwrócił nieoczekiwany zakres danych: {url}");
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
    while let Some(chunk) = res
        .chunk()
        .await
        .with_context(|| format!("Pobieranie przerwane: {url}"))?
    {
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush().await?;
    if let Some(total) = total
        && downloaded != total
    {
        bail!("Pobieranie przerwane ({downloaded} z {total} bajtów): {url}");
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
        assert!(format!("{err:#}").contains("przerwane"), "{err:#}");
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
