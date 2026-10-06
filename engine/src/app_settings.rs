//! App settings stored in `config.json` (desktop app only; the headless server uses `.env`).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, VideoEncoder};

pub const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub port: u16,
    pub lan_access: bool,
    pub public_url: String,
    pub remote_admin: bool,
    pub admin_password_hash: Option<String>,
    pub session_secret: String,
    /// `auto`, `libx264`, `h264_nvenc`, `h264_qsv` or `h264_amf`.
    pub video_encoder: String,
    pub detected_encoder: Option<String>,
    pub ytdlp_cookies: Option<PathBuf>,
    pub pixeldrain_api_key: Option<String>,
    pub autostart: bool,
    pub trust_cloudflare: bool,
    pub first_run_completed: bool,
    pub tools_installed: bool,
    pub skipped_update_version: Option<String>,
    /// Raised each time the remote panel is switched on or gets a new password: remote
    /// sessions signed under an older epoch stop working (see `http::auth`).
    pub remote_session_epoch: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            port: 8080,
            lan_access: false,
            public_url: String::new(),
            remote_admin: false,
            admin_password_hash: None,
            session_secret: String::new(),
            video_encoder: "auto".into(),
            detected_encoder: None,
            ytdlp_cookies: None,
            pixeldrain_api_key: None,
            autostart: false,
            trust_cloudflare: false,
            first_run_completed: false,
            tools_installed: false,
            skipped_update_version: None,
            remote_session_epoch: 0,
        }
    }
}

fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("system random number generator");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl AppSettings {
    /// Reads `<home>/config.json`. A missing file gives the defaults; a corrupt one is kept as
    /// `config.json.bad` and replaced by the defaults, with a Polish warning. An empty
    /// `sessionSecret` is filled with a random one here, so the first save stores it.
    pub fn load(home: &Path) -> (Self, Option<String>) {
        let path = home.join(CONFIG_FILE);
        let (mut settings, warning) = match std::fs::read(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!(
                    "Nie można odczytać {}: {e}. Użyto ustawień domyślnych.",
                    path.display()
                )),
            ),
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                match serde_json::from_str::<Self>(text.trim_start_matches('\u{feff}')) {
                    Ok(s) => (s, None),
                    Err(e) => {
                        let bad = home.join(format!("{CONFIG_FILE}.bad"));
                        let _ = std::fs::remove_file(&bad);
                        let kept = match std::fs::rename(&path, &bad) {
                            Ok(()) => format!("Uszkodzony plik zachowano jako {}.", bad.display()),
                            Err(_) => "Nie udało się zachować uszkodzonego pliku.".to_string(),
                        };
                        (
                            Self::default(),
                            Some(format!(
                                "Plik {CONFIG_FILE} jest uszkodzony ({e}). {kept} Użyto ustawień domyślnych."
                            )),
                        )
                    }
                }
            }
        };
        if settings.session_secret.is_empty() {
            settings.session_secret = random_secret();
        }
        (settings, warning)
    }

    /// Atomic write: a temporary file next to `config.json`, then a rename over it.
    pub fn save(&self, home: &Path) -> io::Result<()> {
        std::fs::create_dir_all(home)?;
        let tmp = home.join(format!("{CONFIG_FILE}.tmp"));
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, home.join(CONFIG_FILE)).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// The encoder the channel uses: `auto` means the detected one; anything unknown or
    /// undetected falls back to libx264.
    pub fn effective_encoder(&self) -> VideoEncoder {
        match self.video_encoder.as_str() {
            "auto" => self
                .detected_encoder
                .as_deref()
                .and_then(VideoEncoder::parse),
            name => VideoEncoder::parse(name),
        }
        .unwrap_or(VideoEncoder::Libx264)
    }
}

/// The engine configuration for the desktop app: tools in `<home>/tools`, data in
/// `<home>/data`, no media library. `admin_password` stays empty (the app authenticates
/// with `adminPasswordHash`).
pub fn app_config(settings: &AppSettings, home: &Path) -> AppConfig {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let tools = home.join("tools");
    let video_encoder = settings.effective_encoder();
    let deno = tools.join(format!("deno{exe}"));
    AppConfig {
        port: settings.port,
        host: if settings.lan_access {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
        .to_string(),
        admin_password: String::new(),
        session_secret: settings.session_secret.clone(),
        public_url: settings.public_url.trim().trim_end_matches('/').to_string(),
        data_dir: home.join("data"),
        ffmpeg_path: tools.join(format!("ffmpeg{exe}")),
        ffprobe_path: tools.join(format!("ffprobe{exe}")),
        ytdlp_path: tools.join(format!("yt-dlp{exe}")),
        ytdlp_js_runtime: deno.is_file().then(|| format!("deno:{}", deno.display())),
        video_encoder,
        trust_cloudflare: settings.trust_cloudflare,
        trust_proxy: false,
        ytdlp_cookies: settings.ytdlp_cookies.clone(),
        pixeldrain_api_key: settings
            .pixeldrain_api_key
            .clone()
            .filter(|k| !k.trim().is_empty()),
        media_dirs: Vec::new(),
        font_file: PathBuf::from(if cfg!(windows) {
            "C:/Windows/Fonts/arial.ttf"
        } else {
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
        }),
        segment_seconds: 4,
        window_size: 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let d = AppSettings::default();
        assert_eq!(d.port, 8080);
        assert!(!d.lan_access && !d.remote_admin && !d.autostart && !d.trust_cloudflare);
        assert_eq!(d.video_encoder, "auto");
        assert!(!d.first_run_completed && !d.tools_installed);
        assert!(d.admin_password_hash.is_none() && d.skipped_update_version.is_none());
    }

    #[test]
    fn missing_file_gives_defaults_with_random_secret_and_no_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let (s, warning) = AppSettings::load(tmp.path());
        assert!(warning.is_none());
        assert_eq!(s.session_secret.len(), 64);
        let (other, _) = AppSettings::load(tmp.path());
        assert_ne!(s.session_secret, other.session_secret);
    }

    #[test]
    fn save_then_load_round_trips_and_keeps_the_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, _) = AppSettings::load(tmp.path());
        s.port = 9000;
        s.lan_access = true;
        s.pixeldrain_api_key = Some("k".into());
        s.save(tmp.path()).unwrap();
        assert!(!tmp.path().join("config.json.tmp").exists());
        let (back, warning) = AppSettings::load(tmp.path());
        assert!(warning.is_none());
        assert_eq!(back, s);
    }

    #[test]
    fn file_uses_camel_case_and_tolerates_missing_and_unknown_fields() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            "\u{feff}{\"lanAccess\":true,\"port\":8123,\"nieznane\":1}",
        )
        .unwrap();
        let (s, warning) = AppSettings::load(tmp.path());
        assert!(warning.is_none());
        assert!(s.lan_access);
        assert_eq!(s.port, 8123);
        assert_eq!(s.video_encoder, "auto");
        s.save(tmp.path()).unwrap();
        let text = std::fs::read_to_string(tmp.path().join("config.json")).unwrap();
        assert!(text.contains("\"lanAccess\": true") && text.contains("\"firstRunCompleted\""));
    }

    #[test]
    fn corrupt_file_is_kept_as_bad_and_defaults_are_used_with_a_warning() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.json"), "{ nie json").unwrap();
        let (s, warning) = AppSettings::load(tmp.path());
        assert_eq!(s.port, 8080);
        assert!(warning.unwrap().contains("uszkodzony"));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("config.json.bad")).unwrap(),
            "{ nie json"
        );
        assert!(!tmp.path().join("config.json").exists());
    }

    #[test]
    fn app_config_maps_the_fields() {
        let home = Path::new("H");
        let mut s = AppSettings {
            port: 9001,
            public_url: "https://tv.example.com/".into(),
            trust_cloudflare: true,
            session_secret: "sekret".into(),
            pixeldrain_api_key: Some("  ".into()),
            ytdlp_cookies: Some(PathBuf::from("c.txt")),
            ..AppSettings::default()
        };
        let c = app_config(&s, home);
        assert_eq!((c.port, c.host.as_str()), (9001, "127.0.0.1"));
        assert_eq!(c.public_url, "https://tv.example.com");
        assert!(c.trust_cloudflare && !c.trust_proxy);
        assert_eq!(c.session_secret, "sekret");
        assert_eq!(c.data_dir, home.join("data"));
        assert_eq!(c.ffmpeg_path.parent().unwrap(), home.join("tools"));
        assert_eq!(c.ytdlp_path.parent().unwrap(), home.join("tools"));
        assert!(c.media_dirs.is_empty());
        assert!(c.pixeldrain_api_key.is_none());
        assert_eq!(c.ytdlp_cookies, Some(PathBuf::from("c.txt")));
        assert_eq!(c.video_encoder, VideoEncoder::Libx264);
        s.lan_access = true;
        assert_eq!(app_config(&s, home).host, "0.0.0.0");
    }

    #[test]
    fn encoder_auto_uses_the_detected_one() {
        let home = Path::new("H");
        let mut s = AppSettings {
            detected_encoder: Some("h264_nvenc".into()),
            ..AppSettings::default()
        };
        assert_eq!(app_config(&s, home).video_encoder, VideoEncoder::H264Nvenc);
        s.video_encoder = "h264_amf".into();
        assert_eq!(app_config(&s, home).video_encoder, VideoEncoder::H264Amf);
        s.video_encoder = "bzdura".into();
        assert_eq!(app_config(&s, home).video_encoder, VideoEncoder::Libx264);
    }
}
