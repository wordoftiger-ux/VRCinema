// The state of the channel as the interface sees it: signals filled by polling the engine
// (the panel every second, the window's info every 3 s) and the owner's controls.

import { signal, computed, batch } from '/static/vendor/signals.mjs';
import { api } from './api.js';
import { lang, loadLang } from './i18n.js';

const PANEL = '/api/admin/channels/main';
const INFO = '/api/desktop/info';
const PANEL_EVERY_MS = 1000;
const INFO_EVERY_MS = 3000;

/** The panel's view of the channel (status, current, paused, pending, settings, links…). */
export const channel = signal(null);
/** The owner's controls: `{positionS, durationS, canPause, canSeek, seekPending, …}`. */
export const control = signal(null);
/** When `control` arrived (`performance.now()`), to move the position on between polls. */
export const controlAt = signal(0);
export const queue = signal([]);
export const history = signal([]);
/** The window's info (`/api/desktop/info`); null in a remote session or without a window. */
export const info = signal(null);
export const tunnel = computed(() => info.value?.tunnel ?? null);
/** `loading`, `ok`, or `signed_out` (no panel session). */
export const session = signal('loading');
/** A short message over the player bar: `{text, kind}` (`error` or `info`). */
export const toast = signal(null);

let infoAvailable = true;
let toastTimer = 0;
// Raised by every language switch made here: an info read that started before it carries the
// old language and must not switch back.
let languageSwitches = 0;
let switchingLanguage = false;

export function showToast(text, kind = 'error') {
  clearTimeout(toastTimer);
  toast.value = { text, kind };
  toastTimer = setTimeout(() => (toast.value = null), 6000);
}

// A panel read that started before the last control answer carries an older position.
let lastControlAnswer = 0;

function applyPanel(data, startedAt) {
  const { control: c, queue: q, history: h, ...rest } = data;
  batch(() => {
    channel.value = rest;
    if (c && startedAt >= lastControlAnswer) setControl(c);
    queue.value = q ?? [];
    history.value = h ?? [];
    session.value = 'ok';
  });
}

function setControl(c) {
  batch(() => {
    control.value = c;
    controlAt.value = performance.now();
  });
}

export async function refreshPanel() {
  const startedAt = performance.now();
  try {
    applyPanel(await api('GET', PANEL), startedAt);
  } catch (e) {
    if (e.status === 401) session.value = 'signed_out';
  }
}

export async function refreshInfo() {
  if (!infoAvailable) return;
  const switches = languageSwitches;
  try {
    const data = await api('GET', INFO);
    info.value = data;
    const current = !switchingLanguage && switches === languageSwitches;
    if (current && data?.language && data.language !== lang.value) {
      await loadLang(data.language);
    }
  } catch (e) {
    // 403: a remote session, 404: the server without a window — no window info there.
    if (e.status === 403 || e.status === 404) {
      infoAvailable = false;
      info.value = null;
    }
  }
}

function loop(fn, everyMs) {
  const run = async () => {
    await fn();
    setTimeout(run, everyMs);
  };
  run();
}

/** Starts polling (once). */
let started = false;
export function startPolling() {
  if (started) return;
  started = true;
  loop(refreshPanel, PANEL_EVERY_MS);
  loop(refreshInfo, INFO_EVERY_MS);
}

/** Reads the window's info once (for the language at start); null when there is none. */
export async function firstInfo() {
  await refreshInfo();
  return info.value;
}

// ---------- The owner's controls ----------

async function send(action, body) {
  try {
    const data = await api('POST', `${PANEL}/${action}`, body);
    if (data?.control) {
      lastControlAnswer = performance.now();
      setControl(data.control);
    }
    refreshPanel();
    return true;
  } catch (e) {
    showToast(e.message);
    return false;
  }
}

/** Manual pause: only the owner's own pause is resumed; other reasons stay. */
export const pause = () => send('pause');
export const resume = () => send('resume');
export function togglePause() {
  const c = control.value;
  if (!c) return Promise.resolve(false);
  return c.pauseReasons?.includes('manual') ? resume() : pause();
}
export const seekBy = (byS) => send('seek', { byS });
export const seekTo = (toS) => send('seek', { toS: Math.max(0, toS) });
export const restart = () => send('restart');
export const playNow = (itemId) => send('play-now', { itemId });
export async function skip() {
  try {
    await api('POST', `${PANEL}/skip`);
    refreshPanel();
    return true;
  } catch (e) {
    showToast(e.message);
    return false;
  }
}

/** Changes the program's language (window only) and switches the interface at once. */
export async function setLanguage(code) {
  switchingLanguage = true;
  languageSwitches += 1;
  try {
    await api('PUT', '/api/desktop/config', { language: code });
    if (info.value) info.value = { ...info.value, language: code };
    await loadLang(code);
  } catch (e) {
    showToast(e.message);
  } finally {
    switchingLanguage = false;
  }
}

// ---------- Derived values ----------

/** The link for the VRChat player: the public one when there is one, else the local one. */
export const channelLink = computed(() => {
  const i = info.value;
  const local = channel.value?.channelUrl ?? null;
  if (!i) return local;
  const link = (base) => `${String(base).replace(/\/+$/, '')}/live/main.m3u8`;
  if (i.tunnel?.state === 'online' && i.tunnel.url) return link(i.tunnel.url);
  if (!i.tunnelEnabled && i.publicUrl) return link(i.publicUrl);
  return local;
});

/** The internet status of the window: `online`, `connecting`, `offline`, `manual`, `local`. */
export const netStatus = computed(() => {
  const i = info.value;
  if (!i) return null;
  const state = i.tunnel?.state ?? 'off';
  if (state === 'online') return 'online';
  if (state === 'connecting' || state === 'reconnecting') return 'connecting';
  if (state === 'error') return 'offline';
  if (i.tunnelEnabled) return 'connecting';
  return i.publicUrl ? 'manual' : 'local';
});
