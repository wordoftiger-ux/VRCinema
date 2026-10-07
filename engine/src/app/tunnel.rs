//! The built-in tunnel inside the app (spec sections 7–9): which cloudflared runs for the
//! settings and the server's port, the state the window sees, and the public address in effect
//! (the tunnel's while it is enabled, see [`crate::live::apply_tunnel`]).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{AppCore, lock};
use crate::app_settings::{AppSettings, TunnelModeSetting, app_config};
use crate::config::AppConfig;
use crate::live::{PublicAccess, apply_tunnel, tunnel_address};
use crate::pipeline::slates::SlateAddress;
use crate::tools::Tool;
use crate::tunnel::{NamedTunnel, TunnelConfig, TunnelMode, TunnelState};

/// Folder of the named tunnel's files (`cert.pem`, `<id>.json`) in the app's home.
pub(crate) const TUNNEL_DIR: &str = "tunnel";

const MISSING: &str =
    "Brak programu cloudflared — pobierz brakujące narzędzia w ustawieniach (Narzędzia).";
const UPDATING: &str = "Trwa aktualizacja programu cloudflared — tunel wróci za chwilę.";
const NAMED_FALLBACK: &str = "Brak plików stałego linku — działa szybki tunel (jego adres zmienia się po każdym uruchomieniu). Ustaw stały link ponownie.";

/// What the tunnel does besides its state, for the window.
#[derive(Debug, Clone, Default)]
pub(crate) struct TunnelNotes {
    /// Why the enabled tunnel cannot run (shown as its error).
    blocked: Option<String>,
    /// Shown next to a running tunnel (the named mode fell back to quick).
    warning: Option<String>,
    /// Mode of the cloudflared asked to run.
    mode: Option<TunnelModeSetting>,
}

/// The tunnel as `GET /api/desktop/tunnel` shows it. Never the credential files.
#[derive(Debug, Clone)]
pub(crate) struct TunnelInfo {
    pub state: TunnelState,
    /// Mode in use (the setting while nothing runs).
    pub mode: TunnelModeSetting,
    pub hostname: Option<String>,
    pub warning: Option<String>,
}

/// What the settings ask for.
enum Plan {
    Off,
    /// Enabled, but it cannot run (why).
    Blocked(String),
    Run {
        config: TunnelConfig,
        warning: Option<String>,
    },
}

/// The named tunnel set up by the wizard: credentials `<home>/tunnel/<id>.json` for `hostname`.
/// `None` without a host name or without the file (the quick tunnel runs instead).
pub(crate) fn find_named_tunnel(home: &Path, hostname: Option<&str>) -> Option<NamedTunnel> {
    let hostname = hostname.map(str::trim).filter(|h| !h.is_empty())?;
    let dir = home.join(TUNNEL_DIR);
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let id = name.strip_suffix(".json")?;
            is_tunnel_id(id).then(|| id.to_string())
        })
        .collect();
    ids.sort();
    let tunnel_id = ids.into_iter().next()?;
    Some(NamedTunnel {
        hostname: hostname.to_string(),
        credentials_file: dir.join(format!("{tunnel_id}.json")),
        origin_cert: dir.join("cert.pem"),
        tunnel_id,
    })
}

/// A tunnel UUID as cloudflared names its credentials file (`8-4-4-4-12` hex digits).
fn is_tunnel_id(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, len)| g.len() == len && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn mode_setting(mode: &TunnelMode) -> TunnelModeSetting {
    match mode {
        TunnelMode::Quick => TunnelModeSetting::Quick,
        TunnelMode::Named(_) => TunnelModeSetting::Named,
    }
}

impl AppCore {
    /// `keep_port`: the port of the running tunnel, used while no server is published.
    fn tunnel_plan(&self, settings: &AppSettings, keep_port: Option<u16>) -> Plan {
        if self.closing.load(Ordering::SeqCst) || !settings.tunnel_enabled {
            return Plan::Off;
        }
        if self.tunnel_held.load(Ordering::SeqCst) {
            return Plan::Blocked(UPDATING.to_string());
        }
        let cloudflared = self.tools.tools_dir().join(Tool::Cloudflared.exe());
        if !cloudflared.is_file() {
            return Plan::Blocked(MISSING.to_string());
        }
        let (mode, warning) = match settings.tunnel_mode {
            TunnelModeSetting::Quick => (TunnelMode::Quick, None),
            TunnelModeSetting::Named => {
                match find_named_tunnel(&self.home, settings.named_tunnel_hostname.as_deref()) {
                    Some(named) => (TunnelMode::Named(named), None),
                    None => (TunnelMode::Quick, Some(NAMED_FALLBACK.to_string())),
                }
            }
        };
        Plan::Run {
            config: TunnelConfig {
                cloudflared,
                port: self.live_port().or(keep_port).unwrap_or(settings.port),
                mode,
            },
            warning,
        }
    }

    /// Brings cloudflared in line with the settings and the server's port. A running tunnel
    /// whose configuration is unchanged is left alone (a restart would give a quick tunnel a
    /// new address); a new port or mode re-points it, one process at a time.
    pub(crate) async fn sync_tunnel(self: &Arc<Self>) {
        self.run_tunnel_plan(false).await;
    }

    /// A new cloudflared even for an unchanged configuration (`POST tunnel/restart`). False
    /// when the tunnel is switched off.
    pub(crate) async fn restart_tunnel(self: &Arc<Self>) -> bool {
        if !self.settings().tunnel_enabled {
            return false;
        }
        self.run_tunnel_plan(true).await;
        true
    }

    async fn run_tunnel_plan(self: &Arc<Self>, force: bool) {
        let mut running = self.tunnel_running.lock().await;
        // While the server restarts (none published) a running tunnel keeps its port: the
        // restart syncs again once the new server is up.
        let keep_port = running.as_ref().map(|c| c.port);
        let (wanted, notes) = match self.tunnel_plan(&self.settings(), keep_port) {
            Plan::Off => (None, TunnelNotes::default()),
            Plan::Blocked(why) => (
                None,
                TunnelNotes {
                    blocked: Some(why),
                    ..TunnelNotes::default()
                },
            ),
            Plan::Run { config, warning } => {
                let mode = Some(mode_setting(&config.mode));
                (
                    Some(config),
                    TunnelNotes {
                        warning,
                        mode,
                        ..TunnelNotes::default()
                    },
                )
            }
        };
        if let Some(warning) = &notes.warning
            && lock(&self.tunnel_notes).warning.as_ref() != Some(warning)
        {
            self.log(&format!("Tunel: {warning}"));
        }
        match (running.take(), wanted.clone()) {
            (None, None) => {}
            (Some(_), None) => self.tunnel.stop().await,
            (None, Some(new)) => self.tunnel.start(new).await,
            (Some(old), Some(new)) => {
                let port = old.port != new.port;
                let mode = old.mode != new.mode;
                if force || old.cloudflared != new.cloudflared || (port && mode) {
                    self.tunnel.start(new).await;
                } else if port {
                    self.tunnel.restart(new.port).await;
                } else if mode {
                    self.tunnel.set_mode(new.mode).await;
                }
            }
        }
        *running = wanted;
        *lock(&self.tunnel_notes) = notes;
        drop(running);
        self.update_tunnel_view();
    }

    /// Publishes the state the window sees (and the automatic pause follows); a change puts
    /// the address in effect and refreshes the slates.
    pub(crate) fn update_tunnel_view(self: &Arc<Self>) {
        // Worked out under the view's lock, so a stale view never overwrites a newer one.
        let changed = self.tunnel_view.send_if_modified(|view| {
            let fresh = if !self.settings().tunnel_enabled {
                TunnelState::Off
            } else if let Some(why) = lock(&self.tunnel_notes).blocked.clone() {
                TunnelState::Error { message: why }
            } else {
                self.tunnel.state().borrow().clone()
            };
            // The address in effect follows the state (a short loss keeps it).
            let mut address = lock(&self.tunnel_address);
            *address = tunnel_address(address.as_deref(), &fresh);
            let changed = *view != fresh;
            *view = fresh;
            changed
        });
        if changed {
            self.apply_live();
            self.refresh_slates();
        }
    }

    pub(crate) fn tunnel_state(&self) -> TunnelState {
        self.tunnel_view.borrow().clone()
    }

    pub(crate) fn tunnel_info(&self) -> TunnelInfo {
        let settings = self.settings();
        let notes = lock(&self.tunnel_notes).clone();
        TunnelInfo {
            state: self.tunnel_state(),
            mode: notes.mode.unwrap_or(settings.tunnel_mode),
            hostname: settings.named_tunnel_hostname,
            warning: notes.warning,
        }
    }

    /// The engine configuration with the public address and Cloudflare trust in effect, and
    /// whether the tunnel is enabled but has no address yet.
    pub(crate) fn effective_config(&self, settings: &AppSettings) -> (AppConfig, bool) {
        let mut config = app_config(settings, &self.home);
        let address = lock(&self.tunnel_address).clone();
        let access = if settings.tunnel_enabled {
            PublicAccess::Tunnel {
                address: address.as_deref(),
            }
        } else {
            PublicAccess::Manual
        };
        let connecting = apply_tunnel(&mut config, access);
        (config, connecting)
    }

    /// What the idle slate should show right now.
    pub(crate) fn slate_address(&self) -> SlateAddress {
        let (config, connecting) = self.effective_config(&self.settings());
        SlateAddress {
            public_url: config.public_url,
            connecting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "6ff42ae2-765d-4adf-8112-31c55c1551ef";

    #[test]
    fn tunnel_ids_are_uuids() {
        assert!(is_tunnel_id(ID));
        assert!(!is_tunnel_id("cert"));
        assert!(!is_tunnel_id("6ff42ae2-765d-4adf-8112-31c55c1551eZ"));
        assert!(!is_tunnel_id("6ff42ae2765d-4adf-8112-31c55c1551ef0"));
    }

    #[test]
    fn the_named_tunnel_needs_a_host_name_and_its_credentials_file() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(TUNNEL_DIR);
        assert!(find_named_tunnel(home.path(), Some("kanal.example.com")).is_none());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cert.pem"), "x").unwrap();
        std::fs::write(dir.join("inny.json"), "{}").unwrap();
        assert!(find_named_tunnel(home.path(), Some("kanal.example.com")).is_none());
        std::fs::write(dir.join(format!("{ID}.json")), "{}").unwrap();
        assert!(find_named_tunnel(home.path(), None).is_none());
        assert!(find_named_tunnel(home.path(), Some(" ")).is_none());
        let named = find_named_tunnel(home.path(), Some("kanal.example.com")).unwrap();
        assert_eq!(named.hostname, "kanal.example.com");
        assert_eq!(named.tunnel_id, ID);
        assert_eq!(named.credentials_file, dir.join(format!("{ID}.json")));
        assert_eq!(named.origin_cert, dir.join("cert.pem"));
    }
}
