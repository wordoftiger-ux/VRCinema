# Third-party software in VRCinema

VRCinema itself is under the [MIT licence](../LICENSE) (Copyright (c) 2026 wordoftiger-ux).
It uses the software below. The same list is in the program: **Settings → About → Licences and
credits**.

## Programs VRCinema downloads

VRCinema does not ship these programs. On the first start (and from **Settings → Tools**) it
downloads them from their official release pages into its data folder, checks their SHA-256 sums
and runs them as separate programs.

| Program | Used for | Licence | Downloaded from |
|---|---|---|---|
| FFmpeg (ffmpeg, ffprobe) | Converting and streaming the video (ffmpeg), reading its details (ffprobe) | **GPL-3.0-or-later**: the "gpl" variant of BtbN's builds (`ffmpeg-master-latest-win64-gpl-shared.zip`; on Linux `…-linux64-gpl` / `…-linuxarm64-gpl`), the Windows build is configured with `--enable-gpl --enable-version3` and without `--enable-nonfree` (`ffmpeg -L` says "GNU General Public License … version 3 … or (at your option) any later version") | [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (build scripts; FFmpeg's source: [ffmpeg.org](https://ffmpeg.org/download.html)) |
| yt-dlp | Getting videos from YouTube and other sites | yt-dlp is under the **Unlicense**; its standalone release programs (`yt-dlp.exe`, `yt-dlp_linux…`) are built with PyInstaller and also contain GPL-3.0-or-later code, so those files as a whole are GPL-3.0-or-later (see yt-dlp's `THIRD_PARTY_LICENSES.txt`) | [yt-dlp/yt-dlp](https://github.com/yt-dlp/yt-dlp) |
| Deno | The JavaScript runtime yt-dlp needs for YouTube | **MIT** | [denoland/deno](https://github.com/denoland/deno) |
| cloudflared | The built-in tunnel (Cloudflare) | **Apache-2.0** | [cloudflare/cloudflared](https://github.com/cloudflare/cloudflared) |

## The interface

The interface's libraries are bundled in [`public/vendor`](../public/vendor) (never loaded from
the internet); their versions and checksums are in `public/vendor/manifest.json` and their full
licence texts in [`public/vendor/LICENSES.txt`](../public/vendor/LICENSES.txt), which the program
also serves at `/static/vendor/LICENSES.txt`.

| Library | Version | Used for | Licence |
|---|---|---|---|
| [Preact](https://github.com/preactjs/preact) (with its hooks) | 10.29.8 | The interface | MIT |
| [htm](https://github.com/developit/htm) | 3.1.1 | The interface's templates | Apache-2.0 |
| [@preact/signals](https://github.com/preactjs/signals), @preact/signals-core | 1.3.4, 1.14.4 | The interface's state | MIT |
| [hls.js](https://github.com/video-dev/hls.js) | 1.7.3 | Playing the channel in the program's window | Apache-2.0 |

## The window

| Component | Used for | Licence |
|---|---|---|
| [Tauri](https://github.com/tauri-apps/tauri) and its plugins (single-instance, autostart, clipboard-manager) | The program's window, tray icon and single instance | Apache-2.0 OR MIT |
| [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) | Shows the window's pages | A **Microsoft** component of Windows under Microsoft's own licence terms. It is not part of VRCinema and is not shipped with it: Windows 11 has it, and VRCinema offers the download page when it is missing. |

## Rust libraries

`VRCinema.exe` is built from the crates listed in [`engine/Cargo.lock`](../engine/Cargo.lock)
and [`desktop/Cargo.lock`](../desktop/Cargo.lock). Their licences, as each crate publishes them
(from `cargo metadata`, Windows build), are mostly permissive (MIT and/or Apache-2.0; also BSD,
ISC, Zlib, Unicode-3.0, BSL-1.0, CC0 and Unlicense, often as one choice among
several). A few crates are under **MPL-2.0**, a file-level copyleft licence (their source is
unchanged on crates.io): see the table at the end. The SQLite library is compiled in through
`rusqlite` (feature `bundled`); SQLite is in the public domain.

<!-- rust-dependencies:begin (node tools/licenses/rust.mjs) -->

Direct dependencies of `engine/`:

| Crate | Version | Licence | Used as |
| --- | --- | --- | --- |
| [anyhow](https://github.com/dtolnay/anyhow) | 1.0.104 | MIT OR Apache-2.0 | compiled in |
| [argon2](https://github.com/RustCrypto/password-hashes) | 0.6.0 | MIT OR Apache-2.0 | compiled in |
| [async-trait](https://github.com/dtolnay/async-trait) | 0.1.92 | MIT OR Apache-2.0 | compiled in |
| [axum](https://github.com/tokio-rs/axum) | 0.8.9 | MIT | compiled in |
| [base64](https://github.com/marshallpierce/rust-base64) | 0.23.1 | MIT OR Apache-2.0 | compiled in |
| [chrono](https://github.com/chronotope/chrono) | 0.4.45 | MIT OR Apache-2.0 | compiled in |
| [futures-util](https://github.com/rust-lang/futures-rs) | 0.3.34 | MIT OR Apache-2.0 | compiled in |
| [getrandom](https://github.com/rust-random/getrandom) | 0.4.3 | MIT OR Apache-2.0 | compiled in |
| [hmac](https://github.com/RustCrypto/MACs) | 0.13.0 | MIT OR Apache-2.0 | compiled in |
| [image](https://github.com/image-rs/image) | 0.25.10 | MIT OR Apache-2.0 | compiled in |
| [local-ip-address](https://github.com/LeoBorai/local-ip-address) | 0.6.13 | MIT OR Apache-2.0 | compiled in |
| [qrcode](https://github.com/kennytm/qrcode-rust) | 0.14.1 | MIT OR Apache-2.0 | compiled in |
| [reqwest](https://github.com/seanmonstar/reqwest) | 0.13.5 | MIT OR Apache-2.0 | compiled in |
| [rusqlite](https://github.com/rusqlite/rusqlite) | 0.40.2 | MIT | compiled in |
| [rust-embed](https://pyrossh.dev/repos/rust-embed) | 8.12.0 | MIT | compiled in |
| [semver](https://github.com/dtolnay/semver) | 1.0.28 | MIT OR Apache-2.0 | compiled in |
| [serde](https://github.com/serde-rs/serde) | 1.0.229 | MIT OR Apache-2.0 | compiled in |
| [serde_json](https://github.com/serde-rs/json) | 1.0.151 | MIT OR Apache-2.0 | compiled in |
| [sha2](https://github.com/RustCrypto/hashes) | 0.11.0 | MIT OR Apache-2.0 | compiled in |
| [tar](https://github.com/composefs/tar-rs) | 0.4.46 | MIT OR Apache-2.0 | compiled in |
| [thiserror](https://github.com/dtolnay/thiserror) | 2.0.21 | MIT OR Apache-2.0 | compiled in |
| [tokio](https://github.com/tokio-rs/tokio) | 1.53.2 | MIT | compiled in |
| [tower-http](https://github.com/tower-rs/tower-http) | 0.7.1 | MIT | compiled in |
| [unicode-normalization](https://github.com/unicode-rs/unicode-normalization) | 0.1.25 | MIT OR Apache-2.0 | compiled in |
| [url](https://github.com/servo/rust-url) | 2.5.8 | MIT OR Apache-2.0 | compiled in |
| [xz2](https://github.com/alexcrichton/xz2-rs) | 0.1.7 | MIT/Apache-2.0 | compiled in |
| [zip](https://github.com/zip-rs/zip2) | 8.6.0 | MIT | compiled in |
| [windows-sys](https://github.com/microsoft/windows-rs) | 0.61.2 | MIT OR Apache-2.0 | compiled in, `cfg(windows)` |

Direct dependencies of `desktop/`:

| Crate | Version | Licence | Used as |
| --- | --- | --- | --- |
| [anyhow](https://github.com/dtolnay/anyhow) | 1.0.104 | MIT OR Apache-2.0 | compiled in |
| [rfd](https://github.com/PolyMeilex/rfd) | 0.16.0 | MIT | compiled in |
| [semver](https://github.com/dtolnay/semver) | 1.0.28 | MIT OR Apache-2.0 | compiled in |
| [serde_json](https://github.com/serde-rs/json) | 1.0.151 | MIT OR Apache-2.0 | compiled in |
| [tauri](https://github.com/tauri-apps/tauri) | 2.12.1 | Apache-2.0 OR MIT | compiled in |
| [tauri-plugin-autostart](https://github.com/tauri-apps/plugins-workspace) | 2.7.0 | Apache-2.0 OR MIT | compiled in |
| [tauri-plugin-clipboard-manager](https://github.com/tauri-apps/plugins-workspace) | 2.4.1 | Apache-2.0 OR MIT | compiled in |
| [tauri-plugin-single-instance](https://github.com/tauri-apps/plugins-workspace) | 2.5.2 | Apache-2.0 OR MIT | compiled in |
| [tokio](https://github.com/tokio-rs/tokio) | 1.53.2 | MIT | compiled in |
| [tauri-build](https://github.com/tauri-apps/tauri) | 2.7.1 | Apache-2.0 OR MIT | build only |
| [windows-sys](https://github.com/microsoft/windows-rs) | 0.61.2 | MIT OR Apache-2.0 | compiled in, `cfg(windows)` |

All 347 crates the Windows program is built from (normal dependencies, procedural macros included), by licence (SPDX expression as published):

| Licence | Crates |
| --- | --- |
| MIT OR Apache-2.0 | 167 |
| MIT | 66 |
| Apache-2.0 OR MIT | 31 |
| Unicode-3.0 | 18 |
| MIT/Apache-2.0 | 11 |
| Unlicense OR MIT | 9 |
| MPL-2.0 | 5: cssparser, cssparser-macros, dtoa-short, option-ext, selectors |
| MIT OR Apache-2.0 OR Zlib | 4: lru-slab, raw-window-handle, zune-core, zune-jpeg |
| Apache-2.0 | 3: sync_wrapper, tao, zopfli |
| BSD-3-Clause | 3: alloc-no-stdlib, alloc-stdlib, subtle |
| Apache-2.0 OR ISC OR MIT | 2: hyper-rustls, rustls |
| BSD-2-Clause OR Apache-2.0 OR MIT | 2: zerocopy, zerocopy-derive |
| BSD-3-Clause OR Apache-2.0 | 2: moxcms, pxfm |
| BSL-1.0 | 2: clipboard-win, error-code |
| ISC | 2: rustls-webpki, untrusted |
| MIT OR Zlib OR Apache-2.0 | 2: miniz_oxide |
| Unlicense/MIT | 2: same-file, walkdir |
| Zlib | 2: foldhash, zlib-rs |
| Zlib OR Apache-2.0 OR MIT | 2: bytemuck, tinyvec |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1: unicode-ident |
| 0BSD OR MIT OR Apache-2.0 | 1: adler2 |
| Apache-2.0 / MIT | 1: fnv |
| Apache-2.0 AND ISC | 1: ring |
| Apache-2.0 AND MIT | 1: dpi |
| Apache-2.0 OR BSL-1.0 | 1: ryu |
| BSD-3-Clause AND MIT | 1: brotli |
| BSD-3-Clause/MIT | 1: brotli-decompressor |
| CC0-1.0 OR MIT-0 OR Apache-2.0 | 1: dunce |
| ISC AND (Apache-2.0 OR ISC) | 1: aws-lc-rs |
| ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) | 1: aws-lc-sys |
| MIT AND BSD-3-Clause | 1: matchit |

<!-- rust-dependencies:end -->
