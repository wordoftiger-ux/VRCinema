//! The `public/` pages embedded in the binary. `VRCINEMA_PUBLIC_DIR` serves them from disk
//! instead (development).

use std::borrow::Cow;
use std::path::{Component, Path};

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../public/"]
struct Public;

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Only plain relative paths (no `..`, root or drive prefix) are served.
fn is_safe(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// File content and MIME type of `path` (relative to `public/`, e.g. `common.js`).
pub fn asset(path: &str) -> Option<(Cow<'static, [u8]>, &'static str)> {
    let dir = std::env::var_os("VRCINEMA_PUBLIC_DIR").filter(|d| !d.is_empty());
    asset_from(path, dir.as_deref().map(Path::new))
}

fn asset_from(path: &str, dir: Option<&Path>) -> Option<(Cow<'static, [u8]>, &'static str)> {
    if !is_safe(path) {
        return None;
    }
    let bytes = match dir {
        Some(dir) => Cow::Owned(std::fs::read(dir.join(path)).ok()?),
        None => Public::get(path)?.data,
    };
    Some((bytes, mime_for(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_public(file: &str) -> Vec<u8> {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../public")
                .join(file),
        )
        .unwrap()
    }

    #[test]
    fn embedded_pages_match_the_files_and_have_mime_types() {
        let (html, mime) = asset_from("index.html", None).unwrap();
        assert_eq!(&*html, repo_public("index.html").as_slice());
        assert_eq!(mime, "text/html; charset=utf-8");
        assert!(asset_from("admin.html", None).is_some());
        assert_eq!(
            asset_from("common.js", None).unwrap().1,
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            asset_from("style.css", None).unwrap().1,
            "text/css; charset=utf-8"
        );
    }

    #[test]
    fn missing_and_unsafe_paths_are_none() {
        for p in [
            "",
            "nope.css",
            "../Cargo.toml",
            "a/../index.html",
            "/index.html",
            "..\\x",
        ] {
            assert!(asset_from(p, None).is_none(), "{p}");
        }
    }

    #[test]
    fn dev_override_reads_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), "dysk").unwrap();
        let (bytes, _) = asset_from("index.html", Some(tmp.path())).unwrap();
        assert_eq!(&*bytes, b"dysk");
        assert!(asset_from("admin.html", Some(tmp.path())).is_none());
        assert!(asset_from("../x", Some(tmp.path())).is_none());
    }
}
