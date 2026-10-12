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
    /// Built-in Cloudflare tunnel (spec section 7).
    pub tunnel_enabled: bool,
    pub tunnel_mode: TunnelModeSetting,
    /// Host name of the named tunnel (`kanal.example.com`), set by the permanent-link wizard.
    pub named_tunnel_hostname: Option<String>,
    /// UUID of the named tunnel the wizard set up (its credentials are `tunnel\<id>.json`).
    pub named_tunnel_id: Option<String>,
    /// Pause the channel when the tunnel is lost for 10 s (spec section 8).
    pub pause_on_tunnel_loss: bool,
    /// Pause the channel when nobody fetched a segment for 30 s.
    pub pause_when_no_viewers: bool,
}

/// `tunnelMode` in `config.json`: `"quick"` or `"named"` (anything else reads as quick).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelModeSetting {
    /// A random `trycloudflare.com` address, new on every start.
    #[default]
    Quick,
    /// The permanent link on the owner's own domain.
    Named,
}

impl TunnelModeSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Named => "named",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "quick" => Some(Self::Quick),
            "named" => Some(Self::Named),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for TunnelModeSetting {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Ok(Self::parse(&text).unwrap_or_default())
    }
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
            tunnel_enabled: true,
            tunnel_mode: TunnelModeSetting::Quick,
            named_tunnel_hostname: None,
            named_tunnel_id: None,
            pause_on_tunnel_loss: true,
            pause_when_no_viewers: false,
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
        // Polish, like the window's pages, until the app gets its `language` setting.
        language: crate::i18n::Lang::Pl,
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
        // Spec section 7: the tunnel is on by default, quick, pausing when it is lost.
        assert!(d.tunnel_enabled);
        assert_eq!(d.tunnel_mode, TunnelModeSetting::Quick);
        assert!(d.named_tunnel_hostname.is_none() && d.named_tunnel_id.is_none());
        assert!(d.pause_on_tunnel_loss);
        assert!(!d.pause_when_no_viewers);
    }

    #[test]
    fn tunnel_fields_round_trip_in_camel_case_and_default_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        // A stage-2 file has none of them.
        std::fs::write(tmp.path().join("config.json"), "{\"port\": 8123}").unwrap();
        let (mut s, warning) = AppSettings::load(tmp.path());
        assert!(warning.is_none());
        assert!(s.tunnel_enabled && s.pause_on_tunnel_loss && !s.pause_when_no_viewers);
        s.tunnel_enabled = false;
        s.tunnel_mode = TunnelModeSetting::Named;
        s.named_tunnel_hostname = Some("kanal.example.com".into());
        s.named_tunnel_id = Some("6ff42ae2-765d-4adf-8112-31c55c1551ef".into());
        s.pause_on_tunnel_loss = false;
        s.pause_when_no_viewers = true;
        s.save(tmp.path()).unwrap();
        let text = std::fs::read_to_string(tmp.path().join("config.json")).unwrap();
        for field in [
            "\"tunnelEnabled\": false",
            "\"tunnelMode\": \"named\"",
            "\"namedTunnelHostname\": \"kanal.example.com\"",
            "\"namedTunnelId\": \"6ff42ae2-765d-4adf-8112-31c55c1551ef\"",
            "\"pauseOnTunnelLoss\": false",
            "\"pauseWhenNoViewers\": true",
        ] {
            assert!(text.contains(field), "{field} missing in {text}");
        }
        assert_eq!(AppSettings::load(tmp.path()).0, s);
    }

    #[test]
    fn an_unknown_tunnel_mode_falls_back_to_quick() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            "{\"port\": 8123, \"tunnelMode\": \"inny\"}",
        )
        .unwrap();
        let (s, warning) = AppSettings::load(tmp.path());
        assert!(warning.is_none(), "{warning:?}");
        assert_eq!(s.port, 8123);
        assert_eq!(s.tunnel_mode, TunnelModeSetting::Quick);
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
