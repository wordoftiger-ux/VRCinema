//! The built-in tunnel inside the app (spec sections 7–9): which cloudflared runs for the
//! settings and the server's port, the state the window sees, and the public address in effect
//! (the tunnel's while it is enabled, see [`crate::live::apply_tunnel`]).

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::oneshot;

use super::{AppCore, lock};
use crate::app_settings::{AppSettings, TunnelModeSetting, app_config};
use crate::config::AppConfig;
use crate::i18n::Msg;
use crate::live::{PublicAccess, apply_tunnel, tunnel_address};
use crate::pipeline::slates::SlateAddress;
use crate::tools::Tool;
use crate::tunnel::named::{
    self, CERT_FILE, CertSource, ErrorCode, LoginEvent, NamedError, NamedPaths, Operation,
    Progress, Step, default_user_dir, is_tunnel_id, no_cloudflared,
};
use crate::tunnel::{NamedTunnel, TunnelConfig, TunnelMode, TunnelState};

/// Folder of the named tunnel's files (`cert.pem`, `<id>.json`) in the app's home.
pub(crate) const TUNNEL_DIR: &str = "tunnel";

/// The tunnel's public address in effect and the cloudflared run that learnt it.
#[derive(Debug, Clone)]
pub(crate) struct KeptAddress {
    pub url: String,
    /// [`crate::tunnel::TunnelManager::runs_ended`] when it was learnt: a quick tunnel's
    /// address is dead once that count has grown.
    pub runs_ended: u64,
}

/// What the tunnel does besides its state, for the window.
#[derive(Debug, Clone, Default)]
pub(crate) struct TunnelNotes {
    /// Why the enabled tunnel cannot run (shown as its error).
    blocked: Option<Msg>,
    /// Shown next to a running tunnel (the named mode fell back to quick).
    warning: Option<Msg>,
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
    pub warning: Option<Msg>,
}

/// What the settings ask for.
enum Plan {
    Off,
    /// Enabled, but it cannot run (why).
    Blocked(Msg),
    Run {
        config: TunnelConfig,
        warning: Option<Msg>,
    },
}

/// The named tunnel the wizard set up: the tunnel `id` it stored, with its credentials
/// `<home>/tunnel/<id>.json`, for `hostname`. `None` without a host name, without an id or
/// without the file (the quick tunnel runs instead).
pub(crate) fn find_named_tunnel(
    home: &Path,
    hostname: Option<&str>,
    id: Option<&str>,
) -> Option<NamedTunnel> {
    let hostname = hostname.map(str::trim).filter(|h| !h.is_empty())?;
    let tunnel_id = id.filter(|id| is_tunnel_id(id))?.to_string();
    let dir = home.join(TUNNEL_DIR);
    let credentials_file = dir.join(format!("{tunnel_id}.json"));
    if !credentials_file.is_file() {
        return None;
    }
    Some(NamedTunnel {
        hostname: hostname.to_string(),
        credentials_file,
        origin_cert: dir.join(CERT_FILE),
        tunnel_id,
    })
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
            return Plan::Blocked(Msg::new("tunnel.updating"));
        }
        let cloudflared = self.tools.tools_dir().join(Tool::Cloudflared.exe());
        if !cloudflared.is_file() {
            // Not `tools_job_running()`: that is still true in the job's own final sync.
            if self.cloudflared_fetching.load(Ordering::SeqCst) {
                return Plan::Blocked(Msg::new("tunnel.fetching"));
            }
            return Plan::Blocked(no_cloudflared());
        }
        let (mode, warning) = match settings.tunnel_mode {
            TunnelModeSetting::Quick => (TunnelMode::Quick, None),
            TunnelModeSetting::Named => {
                match find_named_tunnel(
                    &self.home,
                    settings.named_tunnel_hostname.as_deref(),
                    settings.named_tunnel_id.as_deref(),
                ) {
                    Some(named) => (TunnelMode::Named(named), None),
                    None => (TunnelMode::Quick, Some(Msg::new("tunnel.named_fallback"))),
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
            self.log(Msg::new("log.tunnel_warning").arg_msg("warning", warning.clone()));
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
        // The address in effect can change without the state (a quick cloudflared dies while
        // already `Reconnecting`): that is a change for the links and the slates too.
        let mut address_changed = false;
        // Worked out under the view's lock, so a stale view never overwrites a newer one.
        let view_changed = self.tunnel_view.send_if_modified(|view| {
            // The count first, then the state: the manager raises the count before it sends
            // the state that follows a process's end, so a state read after the count is never
            // older than it. (Read the other way round, an `Online` read just before an exit
            // could be stamped with the count after it, and its dead address kept.) A count
            // that is older than the state read is corrected by the notification that follows.
            let ended = self.tunnel.runs_ended();
            let fresh = if !self.settings().tunnel_enabled {
                TunnelState::Off
            } else if let Some(why) = lock(&self.tunnel_notes).blocked.clone() {
                TunnelState::Error { message: why }
            } else {
                self.tunnel.state().borrow().clone()
            };
            // The address in effect follows the state: a short loss keeps it while it still
            // leads to the channel (the same cloudflared, or a named tunnel's fixed host).
            let named = lock(&self.tunnel_notes).mode == Some(TunnelModeSetting::Named);
            let mut kept = lock(&self.tunnel_address);
            let still_leads = named || kept.as_ref().is_some_and(|k| k.runs_ended == ended);
            let url = tunnel_address(kept.as_ref().map(|k| k.url.as_str()), &fresh, still_leads);
            address_changed = kept.as_ref().map(|k| k.url.as_str()) != url.as_deref();
            let since = match &fresh {
                TunnelState::Online { .. } => ended,
                _ => kept.as_ref().map_or(ended, |k| k.runs_ended),
            };
            *kept = url.map(|url| KeptAddress {
                url,
                runs_ended: since,
            });
            let changed = *view != fresh;
            *view = fresh;
            changed
        });
        if view_changed || address_changed {
            self.apply_live();
            self.refresh_slates();
        }
    }

    pub(crate) fn tunnel_state(&self) -> TunnelState {
        self.tunnel_view.borrow().clone()
    }

    /// The tunnel for the nerd stats (`None`: switched off).
    pub(crate) fn tunnel_stats(&self) -> Option<crate::stats::TunnelStatsView> {
        crate::stats::TunnelStatsView::of(&self.tunnel_state(), &self.tunnel.stats())
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
        let address = lock(&self.tunnel_address).clone().map(|kept| kept.url);
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

    /// What the slates should show right now (address and the channel's slate language).
    pub(crate) fn slate_address(&self) -> SlateAddress {
        let (config, connecting) = self.effective_config(&self.settings());
        SlateAddress {
            public_url: config.public_url,
            connecting,
            lang: self
                .live()
                .map_or_else(Default::default, |l| l.state.slate_language()),
            look: self
                .live()
                .map_or_else(Default::default, |l| l.state.slate_look()),
        }
    }
}

/// The permanent-link wizard (spec section 9): the management commands of
/// [`crate::tunnel::named`] and what they change in the settings. A failure never changes the
/// mode in use (spec section 11).
impl AppCore {
    pub(crate) fn named_paths(&self) -> NamedPaths {
        NamedPaths {
            cloudflared: self.tools.tools_dir().join(Tool::Cloudflared.exe()),
            dir: self.home.join(TUNNEL_DIR),
            user_dir: self
                .options
                .named_tunnel
                .user_dir
                .clone()
                .or_else(default_user_dir)
                .unwrap_or_else(|| self.home.join(".cloudflared")),
        }
    }

    /// The wizard's last (or running) operation, and whether the certificate is there.
    pub(crate) fn named_progress(&self) -> (Progress, bool) {
        (self.named.progress(), self.named_paths().logged_in())
    }

    /// Starts `tunnel login` in the background (its progress: [`Self::named_progress`]).
    pub(crate) fn start_named_login(self: &Arc<Self>) -> Result<(), NamedError> {
        let busy = self.named.begin(Operation::Login)?;
        let paths = self.named_paths();
        if !paths.cloudflared.is_file() {
            let error = NamedError::new(ErrorCode::NoCloudflared, no_cloudflared());
            self.named.finish(&Err(error.clone()));
            return Err(error);
        }
        let (cancel, cancelled) = oneshot::channel();
        self.named.set_cancel(cancel);
        self.named.step(Step::Starting, Step::Starting.message(""));
        let core = Arc::clone(self);
        let task = tokio::spawn(async move {
            let log = core.log.log_fn();
            let named = &core.named;
            let on_event = |event: LoginEvent| match event {
                LoginEvent::Url(url) => {
                    named.set_login_url(url);
                    named.step(Step::WaitingForBrowser, Step::WaitingForBrowser.message(""));
                }
                LoginEvent::BrowserOpened(opened) => named.set_browser_opened(opened),
            };
            let timeout = core.options.named_tunnel.login_timeout;
            let result = named::login(&paths, timeout, cancelled, on_event, &log)
                .await
                .map(|source| match source {
                    CertSource::Moved => Msg::new("named.logged_in"),
                    CertSource::Copied => Msg::new("named.logged_in_before"),
                });
            named.finish(&result);
            drop(busy);
        });
        // Aborted when the app quits (the login's cloudflared dies with its handle).
        let mut background = lock(&self.background);
        background.retain(|t| !t.is_finished());
        background.push(task);
        Ok(())
    }

    /// Cancels a running login; false when none runs.
    pub(crate) fn cancel_named_login(&self) -> bool {
        self.named.cancel_login()
    }

    /// Creates (or reuses) the tunnel, routes `hostname` to it and switches to it.
    pub(crate) async fn create_named(
        self: &Arc<Self>,
        hostname: String,
        overwrite: bool,
    ) -> Result<Msg, NamedError> {
        self.named_operation(Operation::Create, move |core| async move {
            let paths = core.named_paths();
            let log = core.log.log_fn();
            let timeout = core.options.named_tunnel.command_timeout;
            let on_step = |step: Step| core.named.step(step, step.message(&hostname));
            let id = named::create(&paths, &hostname, overwrite, timeout, on_step, &log).await?;
            let url = format!("https://{hostname}");
            core.switch_tunnel_mode(move |s| {
                s.tunnel_mode = TunnelModeSetting::Named;
                s.named_tunnel_hostname = Some(hostname);
                s.named_tunnel_id = Some(id);
            })
            .await?;
            Ok(Msg::new("named.ready").arg("url", url))
        })
        .await
    }

    /// Back to the quick tunnel; the permanent link's files stay.
    pub(crate) async fn named_to_quick(self: &Arc<Self>) -> Result<Msg, NamedError> {
        self.named_operation(Operation::Quick, |core| async move {
            core.switch_tunnel_mode(|s| s.tunnel_mode = TunnelModeSetting::Quick)
                .await?;
            Ok(Msg::new("named.back_to_quick"))
        })
        .await
    }

    /// Deletes the tunnel in Cloudflare, then (the quick tunnel running) its files here.
    pub(crate) async fn delete_named(self: &Arc<Self>) -> Result<Msg, NamedError> {
        self.named_operation(Operation::Delete, |core| async move {
            let paths = core.named_paths();
            let log = core.log.log_fn();
            let host = core.settings().named_tunnel_hostname;
            core.named.step(Step::Deleting, Step::Deleting.message(""));
            named::delete(&paths, core.options.named_tunnel.command_timeout, &log).await?;
            // The tunnel is gone in Cloudflare: from here on the quick tunnel is the only
            // way out, whatever else fails.
            core.switch_tunnel_mode(|s| {
                s.tunnel_mode = TunnelModeSetting::Quick;
                s.named_tunnel_hostname = None;
                s.named_tunnel_id = None;
            })
            .await?;
            // The named cloudflared has exited by now: its files can go.
            if let Err(e) = named::remove_files(&paths.dir) {
                core.log(
                    Msg::new("log.named_remove_files_failed")
                        .arg("path", paths.dir.display())
                        .arg("error", &e),
                );
                return Err(NamedError::new(
                    ErrorCode::Failed,
                    Msg::error("named_files_left").arg("detail", e),
                ));
            }
            core.log(Msg::new("log.named_files_removed").arg("path", paths.dir.display()));
            Ok(match host {
                Some(host) => Msg::new("named.deleted").arg("hostname", host),
                None => Msg::new("named.deleted_no_host"),
            })
        })
        .await
    }

    /// Runs one operation of the wizard (others are refused meanwhile) as its own task, so a
    /// request that gives up waiting never leaves the settings half changed; its outcome goes
    /// to the progress.
    async fn named_operation<F, Fut>(
        self: &Arc<Self>,
        operation: Operation,
        work: F,
    ) -> Result<Msg, NamedError>
    where
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Msg, NamedError>> + Send + 'static,
    {
        let busy = self.named.begin(operation)?;
        let core = Arc::clone(self);
        tokio::spawn(async move {
            let result = work(Arc::clone(&core)).await;
            core.named.finish(&result);
            drop(busy);
            result
        })
        .await
        .unwrap_or_else(|e| {
            Err(NamedError::new(
                ErrorCode::Failed,
                Msg::error("failed").arg("detail", e),
            ))
        })
    }

    /// Saves the tunnel mode and switches the running tunnel to it (only after a successful
    /// command: a failure never changes the mode, spec section 11).
    async fn switch_tunnel_mode(
        self: &Arc<Self>,
        change: impl FnOnce(&mut AppSettings),
    ) -> Result<(), NamedError> {
        self.named
            .step(Step::Switching, Step::Switching.message(""));
        let _guard = self.settings_change.lock().await;
        self.update_settings(change)
            .map_err(|e| NamedError::new(ErrorCode::Failed, self.save_failed(&e)))?;
        self.apply_live();
        self.sync_tunnel().await;
        self.refresh_slates();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "6ff42ae2-765d-4adf-8112-31c55c1551ef";
    /// Sorts before [`ID`]: never chosen in its place.
    const OTHER: &str = "0aa42ae2-765d-4adf-8112-31c55c1551ef";
    const HOST: Option<&str> = Some("kanal.example.com");

    #[test]
    fn the_named_tunnel_needs_a_host_name_its_stored_id_and_its_credentials_file() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(TUNNEL_DIR);
        assert!(find_named_tunnel(home.path(), HOST, Some(ID)).is_none());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cert.pem"), "x").unwrap();
        std::fs::write(dir.join(format!("{OTHER}.json")), "{}").unwrap();
        assert!(
            find_named_tunnel(home.path(), HOST, Some(ID)).is_none(),
            "another tunnel's credentials are not the stored one's"
        );
        std::fs::write(dir.join(format!("{ID}.json")), "{}").unwrap();
        assert!(find_named_tunnel(home.path(), None, Some(ID)).is_none());
        assert!(find_named_tunnel(home.path(), Some(" "), Some(ID)).is_none());
        assert!(find_named_tunnel(home.path(), HOST, None).is_none());
        assert!(find_named_tunnel(home.path(), HOST, Some("..\\cert")).is_none());
        let named = find_named_tunnel(home.path(), HOST, Some(ID)).unwrap();
        assert_eq!(named.hostname, "kanal.example.com");
        assert_eq!(named.tunnel_id, ID);
        assert_eq!(named.credentials_file, dir.join(format!("{ID}.json")));
        assert_eq!(named.origin_cert, dir.join("cert.pem"));
        let other = find_named_tunnel(home.path(), HOST, Some(OTHER)).unwrap();
        assert_eq!(other.tunnel_id, OTHER);
    }
}
