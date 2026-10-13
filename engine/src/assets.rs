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
        "js" | "mjs" => "application/javascript; charset=utf-8",
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

    fn public_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../public")
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Spec section 10: the interface's libraries are bundled in `public/vendor` (no CDN), each
    /// with its SHA-256 in `manifest.json`; a changed or unlisted file fails here.
    #[test]
    fn vendor_libraries_match_the_sums_of_their_manifest() {
        let dir = public_dir().join("vendor");
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap())
                .unwrap();
        let files = manifest["files"]
            .as_object()
            .expect("files in manifest.json");
        for name in [
            "preact.mjs",
            "hooks.mjs",
            "htm.mjs",
            "signals-core.mjs",
            "signals.mjs",
            "hls.mjs",
        ] {
            assert!(files.contains_key(name), "{name} is not in the manifest");
        }
        let mut on_disk = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == "manifest.json" {
                continue;
            }
            let entry = files
                .get(&name)
                .unwrap_or_else(|| panic!("{name} is not in the manifest"));
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(
                entry["sha256"].as_str(),
                Some(sha256_hex(&bytes).as_str()),
                "SHA-256 of {name}"
            );
            // The binary serves the same bytes; the libraries as JavaScript modules.
            let (embedded, mime) = asset_from(&format!("vendor/{name}"), None).unwrap();
            assert_eq!(&*embedded, bytes.as_slice(), "embedded {name}");
            if name.ends_with(".mjs") {
                assert!(
                    entry["version"].as_str().is_some_and(|v| !v.is_empty()),
                    "{name}: version"
                );
                assert_eq!(mime, "application/javascript; charset=utf-8");
            }
            on_disk.push(name);
        }
        for name in files.keys() {
            assert!(on_disk.contains(name), "{name} is listed but missing");
        }
    }

    /// Text files of `dir` (recursively) with one of `extensions`.
    fn text_files(dir: &Path, extensions: &[&str], recursive: bool) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if recursive {
                    out.extend(text_files(&path, extensions, true));
                }
            } else if path
                .extension()
                .is_some_and(|e| extensions.iter().any(|x| e == *x))
            {
                let text = std::fs::read_to_string(&path).unwrap();
                out.push((path.display().to_string(), text));
            }
        }
        out
    }

    /// Where `text` names an address on another site (`http://`, `https://` or `//host` in a
    /// place that loads it). Addresses shown as an example in a `placeholder="…"` are allowed:
    /// they load nothing.
    fn external_references(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        let lower = text.to_ascii_lowercase();
        for scheme in ["http://", "https://"] {
            for (at, _) in lower.match_indices(scheme) {
                let line_start = lower[..at].rfind('\n').map_or(0, |i| i + 1);
                let before = &lower[line_start..at];
                let in_placeholder = before
                    .rfind("placeholder=\"")
                    .is_some_and(|p| !before[p + "placeholder=\"".len()..].contains('"'));
                if !in_placeholder {
                    let end = (at + 60).min(text.len());
                    found.push(text[at..text.floor_char_boundary(end)].to_string());
                }
            }
        }
        for loader in [
            "src=\"//",
            "src='//",
            "href=\"//",
            "href='//",
            "url(//",
            "url(\"//",
            "url('//",
            "from \"//",
            "from '//",
            "import(\"//",
            "import('//",
            "@import \"//",
        ] {
            if lower.contains(loader) {
                found.push(loader.to_string());
            }
        }
        found
    }

    #[test]
    fn the_reference_check_finds_addresses_outside_placeholders() {
        assert!(external_references("<input placeholder=\"https://x.example/\">").is_empty());
        assert!(external_references("const a = '/static/app/main.js';").is_empty());
        assert_eq!(
            external_references("<script src=\"https://cdn.example/x.js\"></script>").len(),
            1
        );
        assert_eq!(
            external_references("<link href=\"//fonts.example/css\">").len(),
            1
        );
        assert_eq!(
            external_references("import x from 'HTTPS://cdn.example/x.mjs';").len(),
            1
        );
        // A placeholder earlier on the line does not cover a later attribute.
        assert_eq!(
            external_references("<input placeholder=\"a\" src=\"http://x.example\">").len(),
            1
        );
    }

    /// Spec section 10 and the global constraints: the interface loads nothing from the
    /// internet — no CDN, no web fonts — in the new interface and in every page.
    #[test]
    fn the_interface_and_the_pages_load_nothing_from_the_internet() {
        let public = public_dir();
        let mut files = text_files(&public.join("app"), &["js", "css", "html"], true);
        files.extend(text_files(&public, &["html", "css"], false));
        assert!(
            files.iter().any(|(p, _)| p.ends_with("main.js")),
            "public/app was not scanned"
        );
        assert!(
            files.iter().any(|(p, _)| p.ends_with("app.html")),
            "public/app.html was not scanned"
        );
        for (path, text) in &files {
            let refs = external_references(text);
            assert!(refs.is_empty(), "{path} refers to {refs:?}");
        }
    }

    #[test]
    fn module_files_have_the_javascript_type() {
        assert_eq!(
            mime_for("vendor/preact.mjs"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            mime_for("app/main.js"),
            "application/javascript; charset=utf-8"
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
