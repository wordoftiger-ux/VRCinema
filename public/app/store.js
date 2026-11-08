// The state of the channel as the interface sees it: signals filled by polling the engine
// (the panel every second, the program's info every 3 s) and the owner's controls.

import { signal, computed, batch } from '/static/vendor/signals.mjs';
import { api, apiBlob, errorText } from './api.js';
import { chosenLang, lang, loadLang, rememberLang } from './i18n.js';

const PANEL = '/api/admin/channels/main';
const WINDOW_INFO = '/api/desktop/info';
const PANEL_INFO = '/api/admin/info';
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
/**
 * The program's info: the window's (`/api/desktop/info`), or the same for a remote session
 * (`/api/admin/info`); null on a server without a window.
 */
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
 * (`null` without a window). Polled on a server without a window instead of the info.
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

// Where the info is read: the window's route, the panel's (a remote session), or nowhere (a
// server without a window: the environment instead).
let infoRoute = WINDOW_INFO;
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
  // A read that was under way when the owner signed out does not bring the panel back.
  if (session.value === 'signed_out' || mutating > 0 || startedAt < lastMutationEnd) return;
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
  if (infoRoute === null) return refreshEnvironment();
  const route = infoRoute;
  const switches = languageSwitches;
  try {
    const data = await api('GET', route);
    if (session.value === 'signed_out') return 'stop';
    const inWindow = route === WINDOW_INFO;
    batch(() => {
      info.value = data;
      access.value = inWindow ? 'window' : 'remote';
    });
    // The window speaks the program's language; a remote panel too unless this browser picked one.
    const wanted = inWindow ? data?.language : (chosenLang() ?? data?.language);
    const current = !switchingLanguage && switches === languageSwitches;
    if (current && wanted && wanted !== lang.value) await loadLang(wanted);
    return 'ok';
  } catch (e) {
    if (e.status === 401) return 'stop';
    // 403 from the window's route: a remote session (the panel's route next). 404: a server
    // without a window (the environment says so); until one answers, the access stays unknown.
    if (route === WINDOW_INFO && e.status === 403) {
      infoRoute = PANEL_INFO;
      return refreshInfo();
    }
    if (e.status === 403 || e.status === 404) {
      infoRoute = null;
      return refreshEnvironment();
    }
    return 'fail';
  }
}

/** `/api/desktop` + `path` in the window, `/api/admin` + `path` for a remote panel. */
export function appRoute(path) {
  return `${access.value === 'window' ? '/api/desktop' : '/api/admin'}${path}`;
}

/**
 * One read of the environment on a server without a window. The panel follows the server's
 * language unless a language was picked in this browser.
 */
async function refreshEnvironment() {
  const switches = languageSwitches;
  try {
    const env = await api('GET', ENVIRONMENT);
    batch(() => {
      environment.value = env;
      info.value = null;
      access.value = 'headless';
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
  infoRoute = WINDOW_INFO;
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

/**
 * The producer's Preview while it runs (monitor.js): `position()` (the film position it shows,
 * or null), `freeze()` (stops its picture now; that position or null), `pauseAnswered(ok)`,
 * `resumed()`, `expect(toS)` (a seek of the owner: its target is the Preview's position until
 * the new clip plays), `expectAnswered(ok)`, `expecting()` and `playNow(itemId)` /
 * `playNowAnswered(ok)` (the film player switches films at once). The owner's controls count
 * from what it shows.
 */
let preview = null;
export function attachPreview(p) {
  preview = p;
}

/**
 * Manual pause: only the owner's own pause is resumed; other reasons stay. With the Preview the
 * picture stops at once and the engine pauses at that frame's segment end (`atS`).
 */
export async function pause() {
  const c = control.value;
  const atS = c && !c.isLive ? (preview?.freeze() ?? null) : null;
  const ok = await send('pause', atS === null ? undefined : { atS });
  preview?.pauseAnswered(ok);
  return ok;
}
export function resume() {
  preview?.resumed();
  return send('resume');
}
export function togglePause() {
  const c = control.value;
  if (!c) return Promise.resolve(false);
  return c.pauseReasons?.includes('manual') || c.pausePending ? resume() : pause();
}
/**
 * A seek (or restart) the engine accepted: viewers see it after the preview's delay. `toS`:
 * its target, the Preview's position at once (null: not known here).
 */
async function sought(action, body, toS = null) {
  if (toS !== null && !control.value?.isLive) preview?.expect(toS);
  const ok = await send(action, body);
  if (ok) seekBubble.value += 1;
  if (toS !== null) preview?.expectAnswered(ok);
  return ok;
}
// The target of the absolute seek still under way: a step pressed meanwhile counts from it
// (without a Preview; with one, the Preview's position is that target already).
let seekTarget = null;
let seeksInFlight = 0;
/**
 * `byS` seconds from what the Preview shows (an absolute seek; right after a seek, from its
 * target), else from the server's position (the engine joins a burst; also while a seek from
 * elsewhere waits there).
 */
export function seekBy(byS) {
  const c = control.value;
  let from = null;
  if (seeksInFlight > 0) from = seekTarget;
  if (c && !c.isLive && Number.isFinite(c.durationS) && (!c.seekPending || seeksInFlight > 0 || preview?.expecting())) {
    from = preview?.position() ?? from;
  }
  if (from === null) return sought('seek', { byS });
  return seekTo(from + byS);
}
export async function seekTo(toS) {
  const c = control.value;
  // Inside the start and end points (the engine keeps seeks there too).
  const end = Number.isFinite(c?.outS) ? c.outS : c?.durationS;
  const to = Math.max(Number(c?.inS) || 0, Number.isFinite(end) ? Math.min(toS, end) : toS);
  seekTarget = to;
  seeksInFlight += 1;
  try {
    return await sought('seek', { toS: to }, to);
  } finally {
    seeksInFlight -= 1;
  }
}
/** From the beginning: the video's start point when it has one. */
export const restart = () => sought('restart', undefined, Number(control.value?.inS) || 0);
/** "Play now": the Preview switches to the film at once when its file is there. */
export async function playNow(itemId) {
  preview?.playNow?.(itemId);
  const ok = await send('play-now', { itemId });
  preview?.playNowAnswered?.(ok);
  return ok;
}
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

/**
 * Removes every waiting video (`includePlaying`: the one on air too). Resolves to the engine's
 * `{removed, blocked, undo}` (`undo`: the id for `undoRemoval`), or null after a failure (shown).
 */
export async function clearQueue(includePlaying) {
  let answer = null;
  const ok = await mutate(
    async () => (answer = await api('POST', `${PANEL}/clear`, { includePlaying })),
    () => {
      queue.value = [];
      const c = channel.value;
      if (c?.pending) channel.value = { ...c, pending: [] };
    },
  );
  return ok ? answer : null;
}

/**
 * What removing the viewer behind `itemId` would take: `{name, queued, pending, playing,
 * canBlock, blocked}`; throws `ApiError`.
 */
export const requesterPreview = (itemId) => api('POST', `${PANEL}/remove-requester`, { itemId, dryRun: true });

/** Removes the waiting videos of the viewer behind `itemId` (`{includePlaying, block}`); like `clearQueue`. */
export async function removeRequester(itemId, { includePlaying = false, block = false } = {}) {
  let answer = null;
  const ok = await mutate(async () => {
    answer = await api('POST', `${PANEL}/remove-requester`, { itemId, includePlaying, block });
  });
  return ok ? answer : null;
}

/** Puts back what a bulk removal took: resolves to `{restored}`, or null after a failure (shown). */
export async function undoRemoval(undo) {
  let answer = null;
  const ok = await mutate(async () => (answer = await api('POST', `${PANEL}/undo`, { undo })));
  return ok ? answer : null;
}

/** The in and out points of a waiting or playing video (seconds, null: none). Resolves to success. */
export const trimItem = (itemId, inS, outS) =>
  mutate(() => api('POST', `${PANEL}/trim`, { itemId, inS, outS }));

/**
 * When a waiting video starts: `{startAt}` (Unix ms), `{afterS}` (from the engine's clock) or
 * `{}` (no start time). Resolves to success.
 */
export const scheduleItem = (itemId, when) =>
  mutate(() => api('POST', `${PANEL}/schedule`, { itemId, ...when }));

/** A queued video moves first; one from the history is queued again first. Resolves to success. */
export const playNext = (itemId) => mutate(() => api('POST', `${PANEL}/play-next`, { itemId }));

const decide = (id, action) =>
  mutate(() => api('PATCH', `/api/admin/items/${id}`, { action }), () => dropPending(id));
export const approveItem = (id) => decide(id, 'approve');
export const rejectItem = (id) => decide(id, 'reject');

/** Every waiting video of a viewer's list (`{id, itemIds}` of `pendingLists`) at once. */
const decideList = (list, action) =>
  mutate(() => api('POST', `${PANEL}/lists/${list.id}`, { action }), () => list.itemIds.forEach(dropPending));
export const approveList = (list) => decideList(list, 'approve');
export const rejectList = (list) => decideList(list, 'reject');

/** Adds a link (a video or a playlist): resolves to `{added, title}`, throws `ApiError`. */
export async function addLink(url) {
  try {
    return await api('POST', `${PANEL}/items`, { url }, { timeoutMs: 180000 });
  } finally {
    refreshPanel();
  }
}

// ---------- Live capture of this computer (the window only) ----------

const CAPTURE = '/api/desktop/capture';

/** What can be captured now: `{platform, desktop, region, screens, windows, cameras, audio, fast, notes}`. */
export const captureSources = () => api('GET', `${CAPTURE}/sources`, undefined, { timeoutMs: 30000 });

/** One frame of the capture `spec` (a JPEG `Blob`); throws `ApiError`. */
export const capturePreview = (spec) => apiBlob(`${CAPTURE}/preview`, spec, { timeoutMs: 30000 });

/** Adds the capture `spec` (with its `title`) to the queue: `{itemId, title}`; throws `ApiError`. */
export async function addCapture(spec) {
  try {
    return await api('POST', CAPTURE, spec);
  } finally {
    refreshPanel();
    refreshInfo();
  }
}

/** Stop: every capture ends and the waiting ones leave the queue. Resolves to success (a failure is shown). */
export async function stopCapture() {
  try {
    await api('POST', `${CAPTURE}/stop`);
    if (info.value?.capture) info.value = { ...info.value, capture: { running: [], queued: 0 } };
    return true;
  } catch (e) {
    showToast(errorText(e));
    return false;
  } finally {
    refreshPanel();
    refreshInfo();
  }
}

/** A window, screen, camera or microphone of this computer is being captured now (window only). */
export const capturing = computed(() => (info.value?.capture?.running?.length ?? 0) > 0);

/**
 * Changes the program's language and switches the interface at once (a remote panel only when
 * this browser did not pick a language of its own).
 */
export async function setLanguage(code) {
  switchingLanguage = true;
  languageSwitches += 1;
  try {
    await api('PUT', appRoute('/config'), { language: code });
    if (info.value) info.value = { ...info.value, language: code };
    if (access.value === 'window' || !chosenLang()) await loadLang(code);
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

/** Signs a password session out (remote panel, server without a window): the sign-in screen. */
export async function signOut() {
  try {
    await api('POST', '/api/admin/logout');
  } catch (e) {
    showToast(errorText(e));
    return;
  }
  batch(() => {
    session.value = 'signed_out';
    info.value = null;
    environment.value = null;
    channel.value = null;
    queue.value = [];
    history.value = [];
    access.value = 'unknown';
  });
}

// ---------- Derived values ----------

/** The OBS stream: `off`, `waiting` (connected, after the video on air), `live` or `brb`. */
export const obsState = computed(() => channel.value?.obs?.state ?? 'off');

/** What the owner's controls may do now (the keys and the buttons follow the same rules). */
export const abilities = computed(() => {
  const c = control.value;
  const current = Boolean(channel.value?.current);
  // The OBS stream on air is ended in OBS: no skip here (it cannot be paused or sought either).
  const obs = obsState.value === 'live';
  // A pause waiting for the Preview's position is the owner's pause too (the button resumes).
  const manual = Boolean(c?.pauseReasons?.includes('manual') || c?.pausePending);
  return {
    current,
    manual,
    obs,
    canSkip: current && !obs,
    canSeek: Boolean(current && c?.canSeek),
    // The owner's pause can always be resumed, also with no film left on air (the film ended
    // before the pause point, or was skipped while paused: the next one waits for the resume).
    canToggle: Boolean(c && (manual || (current && c.canPause))),
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
