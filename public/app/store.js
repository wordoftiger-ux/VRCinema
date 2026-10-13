// The state of the channel as the interface sees it: signals filled by polling the engine
// (the panel every second, the window's info every 3 s) and the owner's controls.

import { signal, computed, batch } from '/static/vendor/signals.mjs';
import { api, errorText } from './api.js';
import { lang, loadLang } from './i18n.js';

const PANEL = '/api/admin/channels/main';
const INFO = '/api/desktop/info';
const PANEL_EVERY_MS = 1000;
const INFO_EVERY_MS = 3000;
// After failed reads the next one waits longer (doubling), up to this.
const MAX_BACKOFF_MS = 30000;

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
// Panel changes made here (move, remove, approve) that have not finished, and when the last
// one did: a read that started before that shows the old state and must not overwrite ours.
let mutating = 0;
let lastMutationEnd = 0;
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
  if (mutating > 0 || startedAt < lastMutationEnd) return;
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

/** One panel read: `ok`, `fail` (try again later) or `stop` (no session: polling ends). */
async function readPanel() {
  const startedAt = performance.now();
  try {
    applyPanel(await api('GET', PANEL), startedAt);
    return 'ok';
  } catch (e) {
    if (e.status === 401) {
      session.value = 'signed_out';
      return 'stop';
    }
    return 'fail';
  }
}

// Reads are serialised (the polling loop and the refresh after an action share one): while
// one runs, a request for another only marks that a fresh one must follow, so an older answer
// never arrives after a newer one.
let panelRun = null;
let panelAgain = false;

export function refreshPanel() {
  if (panelRun) {
    panelAgain = true;
    return panelRun;
  }
  panelRun = (async () => {
    let result;
    do {
      panelAgain = false;
      result = await readPanel();
    } while (panelAgain);
    panelRun = null; // no await between the check above and this: nothing can slip in
    return result;
  })();
  return panelRun;
}

export async function refreshInfo() {
  if (!infoAvailable) return 'stop';
  const switches = languageSwitches;
  try {
    const data = await api('GET', INFO);
    info.value = data;
    const current = !switchingLanguage && switches === languageSwitches;
    if (current && data?.language && data.language !== lang.value) {
      await loadLang(data.language);
    }
    return 'ok';
  } catch (e) {
    // 403: a remote session, 404: the server without a window — no window info there.
    if (e.status === 403 || e.status === 404) {
      infoAvailable = false;
      info.value = null;
      return 'stop';
    }
    return e.status === 401 ? 'stop' : 'fail';
  }
}

/**
 * Runs `fn` every `everyMs` (it answers `ok`, `fail` or `stop`): after failures the wait
 * doubles up to MAX_BACKOFF_MS, `stop` ends the loop until `restart()`.
 */
function loop(fn, everyMs) {
  let fails = 0;
  let running = false;
  const run = async () => {
    running = true;
    const result = await fn();
    if (result === 'stop') {
      running = false;
      return;
    }
    fails = result === 'fail' ? fails + 1 : 0;
    setTimeout(run, Math.min(MAX_BACKOFF_MS, everyMs * 2 ** Math.min(fails, 6)));
  };
  run();
  return {
    restart() {
      if (running) return;
      fails = 0;
      run();
    },
  };
}

/** Starts polling (once). */
let loops = null;
export function startPolling() {
  if (loops) return;
  loops = { panel: loop(refreshPanel, PANEL_EVERY_MS), info: loop(refreshInfo, INFO_EVERY_MS) };
}

/**
 * Takes polling up again after it stopped for lack of a session (the sign-in screen calls this
 * once the owner has signed in).
 */
export function resumePolling() {
  session.value = 'loading';
  infoAvailable = true;
  loops?.panel.restart();
  loops?.info.restart();
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
    showToast(errorText(e));
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
    showToast(errorText(e));
    return false;
  }
}

// ---------- The queue ----------

/** Runs a change of the queue: `optimistic` shows it at once, a failure shows the error. */
async function mutate(request, optimistic) {
  mutating += 1;
  optimistic?.();
  try {
    await request();
    return true;
  } catch (e) {
    showToast(errorText(e));
    return false;
  } finally {
    mutating -= 1;
    lastMutationEnd = performance.now();
    refreshPanel(); // the engine's state, also after a failure
  }
}

function dropPending(id) {
  const c = channel.value;
  if (c?.pending) channel.value = { ...c, pending: c.pending.filter((i) => i.id !== id) };
}

/** Moves the queued item `id` to `to` (0 = first) in the list without it. */
export function moveItem(id, to) {
  return mutate(
    () => api('PATCH', `/api/admin/items/${id}`, { position: to }),
    () => {
      const moved = queue.value.find((i) => i.id === id);
      if (!moved) return;
      const rest = queue.value.filter((i) => i.id !== id);
      queue.value = [...rest.slice(0, to), moved, ...rest.slice(to)];
    },
  );
}

export const removeItem = (id) =>
  mutate(
    () => api('DELETE', `/api/admin/items/${id}`),
    () => {
      queue.value = queue.value.filter((i) => i.id !== id);
      dropPending(id);
    },
  );

const decide = (id, action) =>
  mutate(() => api('PATCH', `/api/admin/items/${id}`, { action }), () => dropPending(id));
export const approveItem = (id) => decide(id, 'approve');
export const rejectItem = (id) => decide(id, 'reject');

/** Adds a link (a video or a playlist): resolves to `{added, title}`, throws `ApiError`. */
export async function addLink(url) {
  try {
    return await api('POST', `${PANEL}/items`, { url }, { timeoutMs: 180000 });
  } finally {
    refreshPanel();
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
    showToast(errorText(e));
  } finally {
    switchingLanguage = false;
  }
}

// ---------- Derived values ----------

/** What the owner's controls may do now (the keys and the buttons follow the same rules). */
export const abilities = computed(() => {
  const c = control.value;
  const current = Boolean(channel.value?.current);
  const manual = Boolean(c?.pauseReasons?.includes('manual'));
  return {
    current,
    manual,
    canSeek: Boolean(current && c?.canSeek),
    canToggle: Boolean(current && c && (manual || c.canPause)),
  };
});

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
