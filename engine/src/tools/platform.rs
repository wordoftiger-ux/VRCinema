//! Which published file of each tool fits a system (spec `2026-11-07-linux-design.md`):
//! Windows x86_64 (also used on Windows on ARM, through emulation), Linux x86_64 and Linux
//! aarch64 (e.g. a Raspberry Pi 5 with a 64-bit system). Other systems have no downloads; the
//! owner points `FFMPEG_PATH` etc. at programs installed some other way.

use std::fmt;

use super::Tool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

/// Where the checksum of a published file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SumsSource {
    /// A sums file with this name next to the download.
    File(&'static str),
    /// The description of the latest GitHub release (cloudflared).
    ReleaseNotes,
}

/// One published file of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    pub file: &'static str,
    pub sums: SumsSource,
}

impl Platform {
    pub const WINDOWS_X64: Platform = Platform {
        os: Os::Windows,
        arch: Arch::X86_64,
    };
    pub const LINUX_X64: Platform = Platform {
        os: Os::Linux,
        arch: Arch::X86_64,
    };
    pub const LINUX_ARM64: Platform = Platform {
        os: Os::Linux,
        arch: Arch::Aarch64,
    };

    /// The system this program was built for.
    pub const fn current() -> Platform {
        let os = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        };
        let arch = if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::Other
        };
        Platform { os, arch }
    }

    /// `.exe` on Windows, nothing elsewhere.
    pub const fn exe_suffix(self) -> &'static str {
        match self.os {
            Os::Windows => ".exe",
            _ => "",
        }
    }

    /// The ffmpeg build keeps its libraries next to the programs (Windows shared build);
    /// the Linux builds are static.
    pub fn shared_ffmpeg(self) -> bool {
        self.os == Os::Windows
    }

    /// The file to download for `tool`; `None`: nothing is published for this system.
    pub fn asset(self, tool: Tool) -> Option<Asset> {
        use Arch::{Aarch64, X86_64};
        use SumsSource::{File, ReleaseNotes};
        let asset = |file, sums| Some(Asset { file, sums });
        match (self.os, self.arch, tool) {
            // Windows: the x64 programs (Windows on ARM runs them emulated).
            (Os::Windows, _, Tool::YtDlp) => asset("yt-dlp.exe", File("SHA2-256SUMS")),
            (Os::Windows, _, Tool::Ffmpeg) => asset(
                "ffmpeg-master-latest-win64-gpl-shared.zip",
                File("checksums.sha256"),
            ),
            (Os::Windows, _, Tool::Deno) => asset(
                "deno-x86_64-pc-windows-msvc.zip",
                File("deno-x86_64-pc-windows-msvc.zip.sha256sum"),
            ),
            (Os::Windows, _, Tool::Cloudflared) => {
                asset("cloudflared-windows-amd64.exe", ReleaseNotes)
            }
            (Os::Linux, X86_64, Tool::YtDlp) => asset("yt-dlp_linux", File("SHA2-256SUMS")),
            (Os::Linux, Aarch64, Tool::YtDlp) => {
                asset("yt-dlp_linux_aarch64", File("SHA2-256SUMS"))
            }
            (Os::Linux, X86_64, Tool::Ffmpeg) => asset(
                "ffmpeg-master-latest-linux64-gpl.tar.xz",
                File("checksums.sha256"),
            ),
            (Os::Linux, Aarch64, Tool::Ffmpeg) => asset(
                "ffmpeg-master-latest-linuxarm64-gpl.tar.xz",
                File("checksums.sha256"),
            ),
            (Os::Linux, X86_64, Tool::Deno) => asset(
                "deno-x86_64-unknown-linux-gnu.zip",
                File("deno-x86_64-unknown-linux-gnu.zip.sha256sum"),
            ),
            (Os::Linux, Aarch64, Tool::Deno) => asset(
                "deno-aarch64-unknown-linux-gnu.zip",
                File("deno-aarch64-unknown-linux-gnu.zip.sha256sum"),
            ),
            (Os::Linux, X86_64, Tool::Cloudflared) => {
                asset("cloudflared-linux-amd64", ReleaseNotes)
            }
            (Os::Linux, Aarch64, Tool::Cloudflared) => {
                asset("cloudflared-linux-arm64", ReleaseNotes)
            }
            _ => None,
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let os = match self.os {
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::Other => std::env::consts::OS,
        };
        let arch = match self.arch {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
            Arch::Other => std::env::consts::ARCH,
        };
        write!(f, "{os}-{arch}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(p: Platform) -> Vec<Option<&'static str>> {
        Tool::ALL
            .into_iter()
            .map(|t| p.asset(t).map(|a| a.file))
            .collect()
    }

    #[test]
    fn every_supported_platform_has_all_four_tools() {
        assert_eq!(
            files(Platform::WINDOWS_X64),
            [
                Some("yt-dlp.exe"),
                Some("ffmpeg-master-latest-win64-gpl-shared.zip"),
                Some("deno-x86_64-pc-windows-msvc.zip"),
                Some("cloudflared-windows-amd64.exe"),
            ]
        );
        assert_eq!(
            files(Platform::LINUX_X64),
            [
                Some("yt-dlp_linux"),
                Some("ffmpeg-master-latest-linux64-gpl.tar.xz"),
                Some("deno-x86_64-unknown-linux-gnu.zip"),
                Some("cloudflared-linux-amd64"),
            ]
        );
        assert_eq!(
            files(Platform::LINUX_ARM64),
            [
                Some("yt-dlp_linux_aarch64"),
                Some("ffmpeg-master-latest-linuxarm64-gpl.tar.xz"),
                Some("deno-aarch64-unknown-linux-gnu.zip"),
                Some("cloudflared-linux-arm64"),
            ]
        );
    }

    #[test]
    fn checksums_come_from_where_each_publisher_puts_them() {
        let p = Platform::LINUX_ARM64;
        assert_eq!(
            p.asset(Tool::YtDlp).unwrap().sums,
            SumsSource::File("SHA2-256SUMS")
        );
        assert_eq!(
            p.asset(Tool::Ffmpeg).unwrap().sums,
            SumsSource::File("checksums.sha256")
        );
        assert_eq!(
            p.asset(Tool::Deno).unwrap().sums,
            SumsSource::File("deno-aarch64-unknown-linux-gnu.zip.sha256sum")
        );
        assert_eq!(
            p.asset(Tool::Cloudflared).unwrap().sums,
            SumsSource::ReleaseNotes
        );
    }

    #[test]
    fn other_systems_have_no_downloads() {
        let p = Platform {
            os: Os::Linux,
            arch: Arch::Other,
        };
        assert_eq!(files(p), [None, None, None, None]);
        let p = Platform {
            os: Os::Other,
            arch: Arch::X86_64,
        };
        assert_eq!(files(p), [None, None, None, None]);
        // Windows on ARM runs the x64 programs.
        let p = Platform {
            os: Os::Windows,
            arch: Arch::Aarch64,
        };
        assert_eq!(files(p), files(Platform::WINDOWS_X64));
    }

    #[test]
    fn names_and_suffixes() {
        assert_eq!(Platform::WINDOWS_X64.exe_suffix(), ".exe");
        assert_eq!(Platform::LINUX_X64.exe_suffix(), "");
        assert!(Platform::WINDOWS_X64.shared_ffmpeg());
        assert!(!Platform::LINUX_ARM64.shared_ffmpeg());
        assert_eq!(Platform::LINUX_ARM64.to_string(), "linux-aarch64");
        assert_eq!(Platform::WINDOWS_X64.to_string(), "windows-x86_64");
        assert_eq!(
            Platform::current().exe_suffix(),
            std::env::consts::EXE_SUFFIX
        );
    }
}
