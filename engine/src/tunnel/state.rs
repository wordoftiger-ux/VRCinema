//! What the tunnel is doing right now, and the wait before the next start of cloudflared.

use std::time::Duration;

use crate::i18n::Msg;

/// State of the tunnel as shown to the rest of the app (spec section 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelState {
    /// Not started, or stopped.
    Off,
    /// First start of cloudflared after `start` / `restart` / `set_mode`.
    Connecting,
    /// The channel is reachable at `url` (`https://…`, no trailing slash).
    Online { url: String },
    /// cloudflared failed (exited, or gave no address in time) and is being started again, or
    /// it is running but has had no connection to Cloudflare for longer than the grace period
    /// (`/ready` not 200); it then reconnects on its own and the state returns to `Online`.
    Reconnecting,
    /// A failure worth a readable explanation; a new start follows after a wait.
    Error { message: Msg },
}

impl TunnelState {
    /// The public address while online.
    pub fn url(&self) -> Option<&str> {
        match self {
            TunnelState::Online { url } => Some(url),
            _ => None,
        }
    }
}

/// Waits between restarts: `initial`, doubling up to `max` (1 → 2 → 4 … ≤ 30 s).
#[derive(Debug, Clone)]
pub struct Backoff {
    initial: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(initial: Duration, max: Duration) -> Self {
        Self {
            initial,
            max,
            next: initial.min(max),
        }
    }

    /// The wait before the next start; every call doubles the following one (capped).
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = delay.saturating_mul(2).min(self.max);
        delay
    }

    /// Back to `initial` (after a long enough time online).
    pub fn reset(&mut self) {
        self.next = self.initial.min(self.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let mut b = Backoff::new(secs(1), secs(30));
        let delays: Vec<u64> = (0..8).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn backoff_reset_starts_over() {
        let mut b = Backoff::new(secs(1), secs(30));
        b.next_delay();
        b.next_delay();
        b.next_delay();
        b.reset();
        assert_eq!(b.next_delay(), secs(1));
        assert_eq!(b.next_delay(), secs(2));
    }

    #[test]
    fn url_only_while_online() {
        assert_eq!(
            TunnelState::Online {
                url: "https://a.example".into()
            }
            .url(),
            Some("https://a.example")
        );
        assert_eq!(TunnelState::Connecting.url(), None);
        assert_eq!(TunnelState::Off.url(), None);
    }
}
