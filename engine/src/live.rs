//! Settings the desktop app changes while the server runs. Readers take a copy at the moment
//! they need the value (next request, next resolve, next clip), so no restart is needed.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use crate::config::{AppConfig, VideoEncoder};
use crate::i18n::Lang;
use crate::tunnel::TunnelState;

/// A shared value: every clone sees the latest `set`.
#[derive(Debug, Default)]
pub struct LiveValue<T>(Arc<RwLock<T>>);

impl<T> Clone for LiveValue<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Clone> LiveValue<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(RwLock::new(value)))
    }

    pub fn get(&self) -> T {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set(&self, value: T) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = value;
    }
}

/// The server settings that take effect without a restart. Cloning shares the values.
#[derive(Debug, Clone)]
pub struct LiveSettings {
    /// In the app: on while the tunnel is enabled, see [`apply_tunnel`].
    pub trust_cloudflare: LiveValue<bool>,
    /// The Cloudflare trust comes from the built-in tunnel: `CF-Connecting-IP` counts only on
    /// requests from a loopback peer (cloudflared connects from 127.0.0.1), so a LAN client
    /// cannot forge it. False for the manual `trustCloudflare` (stage-2 behaviour).
    pub cloudflare_from_loopback_only: LiveValue<bool>,
    /// Links in the API; in the app the tunnel's address while it is enabled (the slates get a
    /// new address through `SlateSwapper`).
    pub public_url: LiveValue<String>,
    /// Encoder of the next clip.
    pub encoder: LiveValue<VideoEncoder>,
    pub ytdlp_cookies: LiveValue<Option<PathBuf>>,
    pub pixeldrain_api_key: LiveValue<Option<String>>,
    /// The app's language (answers to requests that name none they can read).
    pub language: LiveValue<Lang>,
    /// The OBS stream key (the listener compares every `publish` with the current one).
    pub obs_key: LiveValue<String>,
    /// The OBS listener takes connections from the home network (it moves over at once).
    pub obs_lan: LiveValue<bool>,
    /// The OBS stream takes the air at once (read by the channel).
    pub obs_priority: LiveValue<bool>,
}

impl LiveSettings {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            trust_cloudflare: LiveValue::new(config.trust_cloudflare),
            cloudflare_from_loopback_only: LiveValue::new(false),
            public_url: LiveValue::new(config.public_url.clone()),
            encoder: LiveValue::new(config.video_encoder),
            ytdlp_cookies: LiveValue::new(config.ytdlp_cookies.clone()),
            pixeldrain_api_key: LiveValue::new(config.pixeldrain_api_key.clone()),
            language: LiveValue::new(config.language),
            obs_key: LiveValue::new(config.obs.key.clone()),
            obs_lan: LiveValue::new(config.obs.lan),
            obs_priority: LiveValue::new(config.obs.priority),
        }
    }

    /// Takes over the values of `config`; everyone holding a clone sees them from now on.
    pub fn apply(&self, config: &AppConfig) {
        self.trust_cloudflare.set(config.trust_cloudflare);
        self.public_url.set(config.public_url.clone());
        self.encoder.set(config.video_encoder);
        self.ytdlp_cookies.set(config.ytdlp_cookies.clone());
        self.pixeldrain_api_key
            .set(config.pixeldrain_api_key.clone());
        self.language.set(config.language);
        self.obs_key.set(config.obs.key.clone());
        self.obs_lan.set(config.obs.lan);
        self.obs_priority.set(config.obs.priority);
    }
}

/// Where the public address and the Cloudflare trust come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicAccess<'a> {
    /// The tunnel is switched off: the manual `publicUrl` and `trustCloudflare` apply.
    Manual,
    /// The tunnel is enabled; `address`: its public address in effect (see
    /// [`tunnel_address`]), `None` while it has none.
    Tunnel { address: Option<&'a str> },
}

/// Puts the public address and the Cloudflare trust in effect into `config` (spec section 7).
/// While the tunnel is enabled the address is the tunnel's (none while it has none) and
/// `CF-Connecting-IP` is trusted (every request through the tunnel carries it; the app trusts
/// it only from loopback peers, see [`LiveSettings::cloudflare_from_loopback_only`]). Returns
/// whether the tunnel is enabled but has no address (the idle slate then says "Łączenie z
/// internetem…").
pub fn apply_tunnel(config: &mut AppConfig, access: PublicAccess<'_>) -> bool {
    let PublicAccess::Tunnel { address } = access else {
        return false;
    };
    let url = address.unwrap_or_default().trim_end_matches('/');
    config.public_url = url.to_string();
    config.trust_cloudflare = true;
    url.is_empty()
}

/// The tunnel's public address in effect after a change of its state, given the one in effect
/// before: online → its URL; `Reconnecting` → the previous address stays when it `still_leads`
/// to the channel (the same cloudflared lost its connection for a moment, or a named tunnel's
/// fixed host name), so a short loss changes neither the links nor the slates; a quick tunnel
/// whose cloudflared exited has a dead address, none until the new one is online. A fresh
/// start (`Connecting`: switched on, new port or mode), an error or off → none.
pub fn tunnel_address(
    previous: Option<&str>,
    state: &TunnelState,
    still_leads: bool,
) -> Option<String> {
    match state {
        TunnelState::Online { url } => Some(url.clone()),
        TunnelState::Reconnecting => previous.filter(|_| still_leads).map(str::to_string),
        TunnelState::Connecting | TunnelState::Off | TunnelState::Error { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_settings::{AppSettings, app_config};

    fn manual() -> AppConfig {
        let settings = AppSettings {
            public_url: "https://reczny.example.com".into(),
            trust_cloudflare: false,
            ..AppSettings::default()
        };
        app_config(&settings, std::path::Path::new("H"))
    }

    const URL: &str = "https://abc.trycloudflare.com";

    #[test]
    fn a_tunnel_address_is_the_public_address_and_cloudflare_is_trusted() {
        let mut config = manual();
        let access = PublicAccess::Tunnel {
            address: Some("https://abc.trycloudflare.com/"),
        };
        assert!(!apply_tunnel(&mut config, access));
        assert_eq!(config.public_url, URL);
        assert!(config.trust_cloudflare);
    }

    #[test]
    fn an_enabled_tunnel_without_address_gives_none_and_says_connecting() {
        let mut config = manual();
        assert!(apply_tunnel(
            &mut config,
            PublicAccess::Tunnel { address: None }
        ));
        assert_eq!(config.public_url, "");
        assert!(config.trust_cloudflare);
    }

    #[test]
    fn a_disabled_tunnel_leaves_the_manual_settings() {
        let mut config = manual();
        assert!(!apply_tunnel(&mut config, PublicAccess::Manual));
        assert_eq!(config.public_url, "https://reczny.example.com");
        assert!(!config.trust_cloudflare);
    }

    /// Fix round 1: a short loss keeps the address; a fresh start, an error or off clear it.
    #[test]
    fn the_address_survives_reconnecting_only() {
        let online = TunnelState::Online { url: URL.into() };
        assert_eq!(tunnel_address(None, &online, true).as_deref(), Some(URL));
        assert_eq!(
            tunnel_address(Some("https://old.example.com"), &online, false).as_deref(),
            Some(URL)
        );
        assert_eq!(
            tunnel_address(Some(URL), &TunnelState::Reconnecting, true).as_deref(),
            Some(URL)
        );
        assert_eq!(tunnel_address(None, &TunnelState::Reconnecting, true), None);
        // Fix round 2: a quick tunnel whose cloudflared exited has a dead address.
        assert_eq!(
            tunnel_address(Some(URL), &TunnelState::Reconnecting, false),
            None
        );
        for state in [
            TunnelState::Connecting,
            TunnelState::Off,
            TunnelState::Error {
                message: crate::i18n::Msg::new("x"),
            },
        ] {
            assert_eq!(tunnel_address(Some(URL), &state, true), None, "{state:?}");
        }
    }

    #[test]
    fn clones_share_the_value() {
        let a = LiveValue::new(1);
        let b = a.clone();
        b.set(2);
        assert_eq!(a.get(), 2);
    }

    #[test]
    fn apply_reaches_every_clone() {
        let home = std::path::Path::new("H");
        let live = LiveSettings::from_config(&app_config(&AppSettings::default(), home));
        let held = live.clone();
        assert_eq!(held.encoder.get(), VideoEncoder::Libx264);
        let changed = AppSettings {
            trust_cloudflare: true,
            public_url: "https://tv.example.com/".into(),
            video_encoder: "h264_qsv".into(),
            ytdlp_cookies: Some(PathBuf::from("c.txt")),
            pixeldrain_api_key: Some("k".into()),
            ..AppSettings::default()
        };
        live.apply(&app_config(&changed, home));
        assert!(held.trust_cloudflare.get());
        assert_eq!(held.public_url.get(), "https://tv.example.com");
        assert_eq!(held.encoder.get(), VideoEncoder::H264Qsv);
        assert_eq!(held.ytdlp_cookies.get(), Some(PathBuf::from("c.txt")));
        assert_eq!(held.pixeldrain_api_key.get(), Some("k".to_string()));
    }
}
