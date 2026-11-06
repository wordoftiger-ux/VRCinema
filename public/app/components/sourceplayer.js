// The producer's Preview as a player of the film itself (spec 2026-11-06-preview-player-design,
// 4.1): the engine serves the film's file (a disk file, or what yt-dlp downloaded into
// data\temp) at /api/desktop/preview/<id>/media and a plain <video> plays it. The Preview's
// position is the film position (`currentTime`), so pause, resume, seeks, ±N, "from the
// beginning" and "play now" change the picture at once; the engine gets the same `atS`/`toS`
// as before and the air follows a few seconds later (a delayed mirror).
//
// Until the file is there the Preview says "Preparing…"; a film with no file (live, too long,
// a viewer's film the air did not download, a failed download) or one this window cannot
// decode falls back to the engine's segments (monitor.js), reported through `onMode`.
//
// The Preview follows the engine where the owner did not act here: another film (a skip, the
// end of a film, the queue), a pause for another reason (no viewers, the tunnel) or from
// another device, a resume elsewhere, a seek made elsewhere (`seekPending` with another
// target), and a drift beyond DRIFT_S (the engine stalled). After an armed pause the resume
// plays on from the engine's resume position (the end of the segment holding the pause), as
// the segment Preview did.

import { signal } from '/static/vendor/signals.mjs';
import { api } from '../api.js';
import { channel, control, controlAt } from '../store.js';

/** `off`, `none` (nothing to play), `preparing`, `ready`, `fallback` (segments instead). */
export const sourceState = signal('off');
/** Why the fallback is used (the engine's reason, `media` or `network`); null otherwise. */
export const sourceReason = signal(null);
/** The film's address while it is ready (the seek bar's thumbnails read it too). */
export const sourceUrl = signal(null);
/** Seconds the air is behind the Preview (whole seconds, steady); null: not known. */
export const airBehindS = signal(null);

const PREPARE_POLL_MS = 1500;
/** After the owner's own action, engine positions do not move the Preview for this long. */
const OWN_ACTION_MS = 4000;
/** "Play now": the Preview waits this long for the engine to switch to the film. */
const PLAY_NOW_MS = 8000;
/** The Preview follows the engine's position when they drift further apart than this. */
const DRIFT_S = 20;
const TICK_MS = 500;

/** The channel's segment length (seconds), from the air clock; 4 s until it is known. */
function segmentS() {
  const s = channel.peek()?.air?.segmentS;
  return Number.isFinite(s) && s > 0 ? s : 4;
}

/**
 * The steady "air is N s behind" figure: whole seconds, changed only when the measure moves by
 * a second or more from what is shown (the air clock steps with the polls).
 */
export function steadyBehind(shown, raw) {
  if (!Number.isFinite(raw)) return null;
  const value = Math.max(0, raw);
  if (shown === null || Math.abs(value - shown) >= 1) return Math.round(value);
  return shown;
}

/**
 * Plays the Preview's film in `video`. `onMode(mode)`: `file`, `fallback` or `none`.
 * Returns the Preview interface of store.js plus `playNow(itemId)`, `playNowAnswered(ok)`,
 * `sync()` and `stop()`.
 */
export function sourcePlayer(video, { onMode, onState }) {
  let stopped = false;
  /** The film loaded (or being prepared) here. */
  let item = null;
  let asking = 0;
  let pollTimer = 0;
  /** The owner's pause: `{at, confirmed}`. */
  let frozen = null;
  /** Paused by the engine (another reason or another device). */
  let held = false;
  let ownUntil = 0;
  let pendingItem = null;
  let pendingUntil = 0;
  let lastSeek = null;
  let mode = 'none';
  let shownBehind = null;
  /** Films this window could not decode: the segments for them. */
  const undecodable = new Set();

  const ready = () => sourceState.peek() === 'ready' && video.readyState >= 1;
  const state = () => onState?.(ready() ? (video.paused ? 'paused' : 'playing') : sourceState.peek());

  const setMode = (m) => {
    if (m === mode) return;
    mode = m;
    onMode?.(m);
  };

  const play = () => {
    video.play().catch(() => {
      // A window may refuse sound before the first click: muted rather than not at all.
      video.muted = true;
      video.play().catch(() => {});
    });
  };

  const clear = () => {
    clearTimeout(pollTimer);
    asking += 1;
    frozen = null;
    held = false;
    sourceUrl.value = null;
    if (video.getAttribute('src')) {
      video.removeAttribute('src');
      video.load();
    }
  };

  const idle = () => {
    if (item === null && mode === 'none') return;
    item = null;
    clear();
    sourceState.value = 'none';
    sourceReason.value = null;
    setMode('none');
    state();
  };

  const fallback = (itemId, reason) => {
    item = itemId;
    clear();
    sourceState.value = 'fallback';
    sourceReason.value = reason;
    setMode('fallback');
    state();
  };

  /** Where a film starts when it arrives: the engine's position of it, else its start. */
  const engineStart = (itemId) => {
    const c = control.peek();
    if (c && c.itemId === itemId && !c.isLive) return { at: Number(c.positionS) || 0, paused: Boolean(c.paused) };
    return { at: 0, paused: false };
  };

  function load(url, itemId, start) {
    sourceReason.value = null;
    setMode('file');
    const at = Math.max(0, start.at);
    const begin = () => {
      if (stopped || item !== itemId) return;
      if (Math.abs(video.currentTime - at) > 0.05) video.currentTime = at;
      sourceState.value = 'ready';
      sourceUrl.value = url;
      if (start.paused) {
        video.pause();
        held = true;
      } else play();
      state();
    };
    if (video.getAttribute('src') === url && video.readyState >= 1) {
      begin();
      return;
    }
    sourceState.value = 'preparing';
    video.setAttribute('src', url);
    video.addEventListener('loadedmetadata', begin, { once: true });
    video.load();
    state();
  }

  async function ask(itemId, start = null) {
    item = itemId;
    const token = ++asking;
    clearTimeout(pollTimer);
    frozen = null;
    held = false;
    if (sourceState.peek() !== 'preparing') {
      sourceState.value = 'preparing';
      setMode('file');
      state();
    }
    let s;
    try {
      s = await api('GET', `/api/desktop/preview?item=${encodeURIComponent(itemId)}`);
    } catch (e) {
      if (token === asking && !stopped) fallback(itemId, e?.status === 404 ? 'unavailable' : 'network');
      return;
    }
    if (token !== asking || stopped) return;
    if (s?.state === 'ready' && s.url) load(s.url, itemId, start ?? engineStart(itemId));
    else if (s?.state === 'preparing') {
      sourceUrl.value = null;
      pollTimer = setTimeout(() => ask(itemId, start), PREPARE_POLL_MS);
    } else fallback(itemId, s?.reason ?? 'unavailable');
  }

  const onError = () => {
    if (!video.getAttribute('src') || item === null) return;
    // This window cannot play the file (a codec or a container it does not know).
    undecodable.add(item);
    fallback(item, 'media');
  };
  const onEvent = () => state();
  video.addEventListener('error', onError);
  for (const name of ['play', 'pause', 'playing', 'seeked', 'ended']) video.addEventListener(name, onEvent);

  function reconcile(c) {
    if (stopped) return;
    const now = performance.now();
    if (pendingItem !== null) {
      if (c?.itemId === pendingItem || now > pendingUntil) pendingItem = null;
      else return;
    }
    if (!c || c.itemId === null || c.itemId === undefined) {
      idle();
      return;
    }
    if (c.isLive || undecodable.has(c.itemId)) {
      if (item !== c.itemId || mode !== 'fallback') fallback(c.itemId, c.isLive ? 'live' : 'media');
      return;
    }
    if (c.itemId !== item) {
      ask(c.itemId);
      return;
    }
    if (mode === 'fallback' || !ready()) return;
    const pos = video.currentTime;
    const target = Number(c.positionS) || 0;
    if (c.paused && !frozen) {
      // Paused by the engine: the picture stands where the film resumes.
      if (!video.paused) video.pause();
      if (Math.abs(pos - target) > 0.3 && (!held || Math.abs(pos - target) > 1)) video.currentTime = target;
      held = true;
      return;
    }
    if (frozen) {
      if (!c.paused && !c.pausePending && frozen.confirmed) {
        // Resumed on another device.
        frozen = null;
        play();
        return;
      }
      // A seek made elsewhere during the pause moves where the film resumes.
      if (c.paused && frozen.confirmed && now > ownUntil && Math.abs(frozen.at - target) > segmentS() + 1) {
        frozen.at = target;
        video.currentTime = target;
      }
      return;
    }
    if (held) {
      held = false;
      if (Math.abs(pos - target) > 1) video.currentTime = target;
      play();
      return;
    }
    if (now < ownUntil) return;
    if (c.seekPending && Math.abs(pos - target) > 1) {
      // A seek made elsewhere (another device, the remote panel).
      video.currentTime = target;
      return;
    }
    if (!c.pausePending && !video.ended && Math.abs(pos - target) > DRIFT_S) video.currentTime = target;
    if (video.paused && !video.ended && !c.pausePending) play();
  }

  /** "Air is N s behind the Preview": the Preview's position minus the typical viewer's. */
  const measure = () => {
    const a = channel.peek()?.air;
    const c = control.peek();
    let raw = null;
    if (ready() && a && a.itemId === item && Number.isFinite(a.positionS) && c?.itemId === item) {
      const moving = !c.paused && channel.peek()?.status === 'playing';
      const airPos = a.positionS + (moving ? (performance.now() - controlAt.peek()) / 1000 : 0);
      raw = (frozen ? frozen.at : video.currentTime) - airPos;
    }
    shownBehind = raw === null ? null : steadyBehind(shownBehind, raw);
    airBehindS.value = shownBehind;
  };
  const ticker = setInterval(measure, TICK_MS);
  const unsubscribe = control.subscribe((c) => reconcile(c));

  const position = () => {
    if (!ready() || item !== control.peek()?.itemId) return null;
    return frozen ? frozen.at : video.currentTime;
  };

  return {
    position,
    /** The Preview's position counts for every control while its film plays here. */
    expecting: () => ready(),
    freeze() {
      const at = position();
      if (at === null) return null;
      video.pause();
      frozen = { at, confirmed: false };
      ownUntil = performance.now() + OWN_ACTION_MS;
      state();
      return at;
    },
    pauseAnswered(ok) {
      if (!frozen) return;
      if (ok) frozen.confirmed = true;
      else {
        frozen = null;
        play();
      }
    },
    resumed() {
      const c = control.peek();
      const was = frozen;
      frozen = null;
      held = false;
      ownUntil = performance.now() + OWN_ACTION_MS;
      if (!ready()) return;
      // After a pause the engine resumes from the end of the segment holding the pause point.
      if (was && c?.paused && c.itemId === item && !c.isLive) {
        const resumeAt = Number(c.positionS);
        if (resumeAt >= was.at - 0.05 && resumeAt - was.at <= segmentS() + 1) video.currentTime = resumeAt;
      }
      play();
    },
    expect(toS) {
      if (!ready()) return;
      lastSeek = { from: position(), at: performance.now(), paused: Boolean(frozen) };
      ownUntil = performance.now() + OWN_ACTION_MS;
      if (frozen) frozen.at = toS;
      video.currentTime = toS;
    },
    expectAnswered(ok) {
      if (ok || !lastSeek || lastSeek.from === null || !ready()) return;
      // Refused: back to where the Preview was.
      const back = lastSeek.from + (lastSeek.paused ? 0 : (performance.now() - lastSeek.at) / 1000);
      if (frozen) frozen.at = back;
      video.currentTime = back;
      lastSeek = null;
    },
    /** "Play now": the Preview switches to the film at once (if its file is there). */
    playNow(itemId) {
      pendingItem = itemId;
      pendingUntil = performance.now() + PLAY_NOW_MS;
      ownUntil = performance.now() + OWN_ACTION_MS;
      if (undecodable.has(itemId)) return;
      ask(itemId, { at: 0, paused: false });
    },
    playNowAnswered(ok) {
      if (ok) return;
      pendingItem = null;
      reconcile(control.peek());
    },
    /** S: the Preview to the channel's position (nothing is sent to the engine). */
    sync() {
      const c = control.peek();
      if (!ready() || !c || c.isLive || c.itemId !== item || frozen) return false;
      video.currentTime = Number(c.positionS) || 0;
      ownUntil = performance.now() + OWN_ACTION_MS;
      return true;
    },
    stop() {
      stopped = true;
      unsubscribe();
      clearInterval(ticker);
      clear();
      video.removeEventListener('error', onError);
      for (const name of ['play', 'pause', 'playing', 'seeked', 'ended']) video.removeEventListener(name, onEvent);
      sourceState.value = 'off';
      sourceReason.value = null;
      airBehindS.value = null;
    },
  };
}
