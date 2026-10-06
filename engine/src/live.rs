//! Settings the desktop app changes while the server runs. Readers take a copy at the moment
//! they need the value (next request, next resolve, next clip), so no restart is needed.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use crate::config::{AppConfig, VideoEncoder};

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
    pub trust_cloudflare: LiveValue<bool>,
    /// Links in the API (the slates keep the value they were drawn with until a restart).
    pub public_url: LiveValue<String>,
    /// Encoder of the next clip.
    pub encoder: LiveValue<VideoEncoder>,
    pub ytdlp_cookies: LiveValue<Option<PathBuf>>,
    pub pixeldrain_api_key: LiveValue<Option<String>>,
}

impl LiveSettings {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            trust_cloudflare: LiveValue::new(config.trust_cloudflare),
            public_url: LiveValue::new(config.public_url.clone()),
            encoder: LiveValue::new(config.video_encoder),
            ytdlp_cookies: LiveValue::new(config.ytdlp_cookies.clone()),
            pixeldrain_api_key: LiveValue::new(config.pixeldrain_api_key.clone()),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_settings::{AppSettings, app_config};

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
