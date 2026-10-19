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

export async function refresh() {
  try {
    channel.value = await api('GET', '/api/channels/main/state', undefined, { timeoutMs: 5000 });
    unreachable.value = false;
  } catch {
    unreachable.value = true;
  }
}

export function startPolling() {
  refresh();
  setInterval(() => {
    if (!document.hidden) refresh();
  }, POLL_MS);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) refresh();
  });
}
