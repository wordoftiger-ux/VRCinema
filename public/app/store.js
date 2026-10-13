// The state of the channel as the interface sees it: signals filled by polling the engine
// (the panel every second, the window's info every 3 s) and the owner's controls.

import { signal, computed, batch } from '/static/vendor/signals.mjs';
import { api, errorText } from './api.js';
import { chosenLang, lang, loadLang, rememberLang } from './i18n.js';

const PANEL = '/api/admin/channels/main';
const INFO = '/api/desktop/info';
const ENVIRONMENT = '/api/admin/environment';
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
/**
 * What the interface runs in: `unknown` until the first answer, then `window` (the program's
 * own window: the window-only routes work), `remote` (a password session of the program on
 * another device) or `headless` (the server without a window). Window-only parts wait for
 * the answer: they send no window-only request while it is unknown.
 */
export const access = signal('unknown');
/** True in a remote session or on a server without a window: the window-only routes answer 403. */
export const remote = computed(() => access.value === 'remote' || access.value === 'headless');
/**
 * `/api/admin/environment` (open to a remote session too): `{window, encoder, tickerSlow,
 * language, sessionViaTunnel, publicUrl, tunnelEnabled, tunnel, channelRunning, channelError}`
 * (the last four `null` without a window). Polled outside the window instead of the info.
 */
export const environment = signal(null);
/** Where the internet state comes from: the window's info, else the environment. */
const net = computed(() => info.value ?? environment.value ?? null);
export const tunnel = computed(() => net.value?.tunnel ?? null);
/** This session came through the built-in tunnel: switching it off ends the session. */
export const sessionViaTunnel = computed(() => Boolean(net.value?.sessionViaTunnel));
/** `loading`, `ok`, or `signed_out` (no panel session). */
export const session = signal('loading');
/** A short message over the player bar: `{text, kind}` (`error` or `info`). */
export const toast = signal(null);
/** How far behind the server the preview plays, in seconds (measured by the preview); null: unknown. */
export const previewDelayS = signal(null);
/** Raised by every successful seek: the preview shows a bubble for a few seconds. */
export const seekBubble = signal(0);

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
  if (!infoAvailable) return refreshEnvironment();
  const switches = languageSwitches;
  try {
    const data = await api('GET', INFO);
    info.value = data;
    access.value = 'window';
    const current = !switchingLanguage && switches === languageSwitches;
    if (current && data?.language && data.language !== lang.value) {
      await loadLang(data.language);
    }
    return 'ok';
  } catch (e) {
    // 403: a remote session; the server without a window answers 403 or 404 as well. The
    // environment says which one it is (and is read from now on instead); until it answers,
    // the access stays unknown.
    if (e.status === 403 || e.status === 404) {
      const result = await refreshEnvironment();
      if (result === 'ok') infoAvailable = false;
      return result;
    }
    return e.status === 401 ? 'stop' : 'fail';
  }
}

/**
 * One read of the environment outside the window. The panel follows the program's language
 * unless a language was picked in this browser.
 */
async function refreshEnvironment() {
  const switches = languageSwitches;
  try {
    const env = await api('GET', ENVIRONMENT);
    batch(() => {
      environment.value = env;
      info.value = null;
      access.value = env.window ? 'remote' : 'headless';
    });
    const current = !switchingLanguage && switches === languageSwitches;
    if (current && !chosenLang() && env.language && env.language !== lang.value) {
      await loadLang(env.language);
    }
    return 'ok';
  } catch (e) {
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

/**
 * Reads the window's info once (for the language at start); null when there is none (then the
 * environment may have answered: a remote session or a server without a window).
 */
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
/** A seek (or restart) the engine accepted: viewers see it after the preview's delay. */
async function sought(action, body) {
  const ok = await send(action, body);
  if (ok) seekBubble.value += 1;
  return ok;
}
export const seekBy = (byS) => sought('seek', { byS });
export const seekTo = (toS) => sought('seek', { toS: Math.max(0, toS) });
export const restart = () => sought('restart');
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

/** Picks the panel's language in this browser (remote panel, sign-in screen) and switches. */
export async function choosePanelLanguage(code) {
  languageSwitches += 1;
  rememberLang(code);
  await loadLang(code);
}

/**
 * Switches the built-in tunnel from the panel (`PUT /api/admin/tunnel`, also in a remote
 * session). Resolves to true when it was saved; a failure is thrown as `ApiError`.
 */
export async function setTunnelEnabled(enabled) {
  const r = await api('PUT', '/api/admin/tunnel', { enabled });
  const env = environment.value;
  if (env) environment.value = { ...env, tunnelEnabled: r.tunnelEnabled, tunnel: r.tunnel };
  refreshInfo();
  return true;
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
  const i = net.value;
  const local = channel.value?.channelUrl ?? null;
  if (!i) return local;
  const link = (base) => `${String(base).replace(/\/+$/, '')}/live/main.m3u8`;
  if (i.tunnel?.state === 'online' && i.tunnel.url) return link(i.tunnel.url);
  if (!i.tunnelEnabled && i.publicUrl) return link(i.publicUrl);
  return local;
});

/** The page where viewers add videos: the public one when there is one, else the local one. */
export const requestLink = computed(() => {
  const i = net.value;
  const local = channel.value?.requestUrl ?? null;
  if (!i) return local;
  const base = publicAddress.value;
  return base ? `${String(base).replace(/\/+$/, '')}/` : local;
});

/** The public address of the channel (tunnel or manual); null while there is none or unknown. */
export const publicAddress = computed(() => {
  const i = net.value;
  if (!i) return null;
  if (i.tunnel?.state === 'online' && i.tunnel.url) return i.tunnel.url;
  if (!i.tunnelEnabled && i.publicUrl) return i.publicUrl;
  return null;
});

/** The internet status: `online`, `connecting`, `offline`, `manual`, `local` (null: unknown). */
export const netStatus = computed(() => {
  const i = net.value;
  if (!i) return null;
  const state = i.tunnel?.state ?? 'off';
  if (state === 'online') return 'online';
  if (state === 'connecting' || state === 'reconnecting') return 'connecting';
  if (state === 'error') return 'offline';
  if (i.tunnelEnabled) return 'connecting';
  return i.publicUrl ? 'manual' : 'local';
});
