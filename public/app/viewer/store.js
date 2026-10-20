// The channel as the request page sees it: the public state, read every few seconds, and the
// short message shown after a request.

import { signal } from '/static/vendor/signals.mjs';
import { api } from '../api.js';

const POLL_MS = 2500;

/** The last public state of the channel (`null` until the first answer). */
export const channel = signal(null);
/** True while the channel does not answer. */
export const unreachable = signal(false);
/** A message of the page: `{ text }` for a few seconds, or null. */
export const toast = signal(null);

let toastTimer = null;

export function showToast(text) {
  clearTimeout(toastTimer);
  toast.value = { text };
  toastTimer = setTimeout(() => (toast.value = null), 6000);
}

/** The state request on its way: a slow channel never gets a second one meanwhile. */
let inFlight = null;
/** Read once more when the one on its way is done (it may predate a request just added). */
let again = false;

function poll() {
  inFlight ??= (async () => {
    try {
      do {
        again = false;
        try {
          channel.value = await api('GET', '/api/channels/main/state', undefined, { timeoutMs: 5000 });
          unreachable.value = false;
        } catch {
          unreachable.value = true;
        }
      } while (again);
    } finally {
      inFlight = null;
    }
  })();
  return inFlight;
}

/** Reads the state now (or right after the read on its way, which may be older). */
export function refresh() {
  if (inFlight) again = true;
  return poll();
}

export function startPolling() {
  poll();
  setInterval(() => {
    if (!document.hidden) poll();
  }, POLL_MS);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) poll();
  });
}
