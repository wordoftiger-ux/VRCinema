// The producer's monitor ("Preview", left of "On air" in the Channel view): the newest clip
// segments the engine has (/api/desktop/monitor.m3u8, the window only), played with hls.js in
// real time, muted by default. After a seek, "play now", "from the beginning" or a skip the
// picture changes as soon as ffmpeg wrote the first segment of the new clip (2–4 s), not after
// the viewers' delay. "Sync with on-air" (key S) shows what the viewers see now.
//
// The list is read here once a second, one read at a time; a new clip in it starts the player
// again at that clip's first segment (`?from=<row>`: the engine puts EXT-X-START there). Rows
// carry the film position (`#VRCINEMA-AT`), the list names the row on air (`#VRCINEMA-ANTENNA`)
// and the newest aired one (`#VRCINEMA-AIRED`). While the channel is paused the picture stands
// still on the segment the film resumes from. While the window is hidden nothing is read and
// the player is gone; errors are retried a few times, then a retry button is offered.
//
// The Preview is the owner's reference (store.js asks it through `attachPreview`): a pause
// freezes it at once on the frame shown and sends that film position (`atS`); the engine airs
// up to the end of the segment holding it (at most one segment further) and pauses there.
// Until the resume the Preview keeps the frozen frame; the resume plays the film's relaunched
// clip from its start (the engine's resume position). ±10 s / ±60 s count from the Preview's
// position, and the player bar shows it. After the owner's seek (or ⏮) the Preview's position
// is the target until the new clip plays (its old picture may still run for a few seconds),
// so the bar and a second step count from where the film goes, not from the old picture; a
// seek while frozen moves the frozen position (the engine resumes from there).

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef } from '/static/vendor/hooks.mjs';
import Hls from '/static/vendor/hls.mjs';
import { html } from '../html.js';
import { lang, t } from '../i18n.js';
import { Icon } from '../icons.js';
import { access, attachPreview, control, showToast } from '../store.js';

const FEED = '/api/desktop/monitor.m3u8';
const READ_EVERY_MS = 1000;
const READ_TIMEOUT_MS = 5000;
const RETRY_AFTER_MS = 3000;
/** Immediate recoveries from a media error, then the player is rebuilt. */
const MAX_MEDIA_RECOVERIES = 2;
/** Rebuilds in a row (without healthy playback in between) before the monitor gives up. */
const MAX_RETRIES = 4;
/** The longest a seek target stands in for the Preview's position without its clip arriving. */
const EXPECT_MAX_MS = 60000;

/** `off` (not running), `none` (nothing to preview), `playing`, `paused` (still), `failed`. */
export const monitorState = signal('off');
/** How many seconds the monitor plays ahead of on-air; null: not known. */
export const monitorAheadS = signal(null);
/** The monitor's own sound switch (it starts muted; the volume of the player bar is On air's). */
export const monitorMuted = signal(true);

let current = null;

/** The monitor runs (the window's Channel view on a wide screen): the S key syncs it. */
export function monitorRunning() {
  return current !== null;
}

/** "Sync with on-air": the monitor plays from the segment the viewers see now. */
export function syncMonitor() {
  return current?.sync() ?? false;
}

/** The film position of the frame the Preview shows now (playing or frozen); null: not known. */
export function previewPosition() {
  return current?.position() ?? null;
}

/**
 * The list as the window reads it: `rows` [{seq, clip, name, itemId, positionS, duration,
 * start (seconds from the top of this list), discontinuity}], `antenna` and `aired` (row
 * numbers or null), `target` (the segment length the list announces).
 */
export function parseFeed(text) {
  const rows = [];
  let seq = 0;
  let antenna = null;
  let aired = null;
  let target = null;
  let at = null;
  let duration = null;
  let discontinuity = false;
  let start = 0;
  for (const raw of String(text).split('\n')) {
    const line = raw.trim();
    if (!line) continue;
    const value = (prefix) => line.slice(prefix.length);
    if (line.startsWith('#EXT-X-MEDIA-SEQUENCE:')) seq = Number(value('#EXT-X-MEDIA-SEQUENCE:'));
    else if (line.startsWith('#EXT-X-TARGETDURATION:')) target = Number(value('#EXT-X-TARGETDURATION:'));
    else if (line.startsWith('#VRCINEMA-ANTENNA:')) antenna = Number(value('#VRCINEMA-ANTENNA:'));
    else if (line.startsWith('#VRCINEMA-AIRED:')) aired = Number(value('#VRCINEMA-AIRED:'));
    else if (line === '#EXT-X-DISCONTINUITY') discontinuity = true;
    else if (line.startsWith('#VRCINEMA-AT:')) {
      const [item, position] = value('#VRCINEMA-AT:').split(',');
      at = { itemId: Number(item), positionS: Number(position) };
    } else if (line.startsWith('#EXTINF:')) duration = parseFloat(value('#EXTINF:'));
    else if (!line.startsWith('#')) {
      const [, clip = '', name = ''] = line.split('/');
      const d = Number.isFinite(duration) ? duration : 0;
      rows.push({ seq, clip, name, itemId: at?.itemId ?? null, positionS: at?.positionS ?? null, duration: d, start, discontinuity });
      start += d;
      seq += 1;
      at = null;
      duration = null;
      discontinuity = false;
    }
  }
  return { rows, antenna, aired, target: Number.isFinite(target) && target > 0 ? target : null };
}

/**
 * The ahead time to show (0: in sync) from the one shown (`shown`, null: none yet) and a new
 * measure (`raw`: from the middle of the antenna's segment). The antenna moves on a whole
 * segment at a time, so `raw` saws over one segment around the real value (±½ segment); what
 * is shown only changes when `raw` leaves that band (hysteresis), so the caption does not flicker.
 */
export function steadyAhead(shown, raw, segment) {
  if (shown === null) return raw < segment ? 0 : raw;
  // In sync → ahead: the real value is a whole segment or more (the saw's top 1.5 segments).
  if (shown === 0) return raw >= 1.5 * segment ? raw : 0;
  // Ahead → in sync: the real value is below ¾ of a segment (the saw's low end below ¼).
  if (raw < segment / 4) return 0;
  // Another number only when the real value moved (beyond the saw's span, with a margin).
  return Math.abs(raw - shown) > 1.25 * segment ? raw : shown;
}

/** Where a fresh player starts: the newest aired row if it is of the newest clip, else that clip's start. */
function freshStart(feed) {
  const clip = feed.rows.at(-1).clip;
  const aired = feed.rows.find((r) => r.seq === feed.aired);
  if (aired && aired.clip === clip) return aired.seq;
  return feed.rows.find((r) => r.clip === clip).seq;
}

/** Plays the monitor in `video`. Returns `{stop, retry, sync}`. */
function watch(video) {
  let stopped = false;
  let hls = null;
  let readTimer = 0;
  let retryTimer = 0;
  let reading = null;
  let feed = null;
  /** The clip of the newest row seen (a new one restarts the player at its start). */
  let newest = null;
  /** The first row number of the last list (a lower one: the engine started anew). */
  let first = null;
  /** The ahead time shown (changes by at least one segment: no saw-tooth). */
  let shownAhead = null;
  /** The row a still picture (the channel paused) stands on; -1: waiting for the next clip. */
  let still = null;
  /**
   * The owner's pause froze the picture: `{at}` (its film position), `confirmed` once the
   * engine took the pause, `parked` once it has paused (not only armed).
   */
  let frozen = null;
  /**
   * The owner's seek on its way to the picture: `{at, clip, itemId, since}`. Its target is the
   * Preview's position until a newer clip than `clip` plays (or the film changes).
   */
  let expected = null;
  /** The last position worked out from the picture: held through a stall or a gap. */
  let lastShown = null;
  let mediaRecoveries = 0;
  let retries = 0;
  let lastTime = -1;

  const destroy = () => {
    hls?.destroy();
    hls = null;
    still = null;
    frozen = null;
    expected = null;
    lastShown = null;
    monitorAheadS.value = null;
  };

  /**
   * The fragment and the row of the list playing now, or null. The picture stays on the last
   * fragment that started (a gap between two, the end of what is buffered).
   */
  const playing = () => {
    if (!hls || !feed || still !== null || video.readyState < 2) return null;
    const level = hls.levels?.[Math.max(0, hls.currentLevel)];
    const time = video.currentTime;
    const frag = level?.details?.fragments?.filter((f) => f.start <= time + 0.05).at(-1);
    const row = frag && feed.rows.find((r) => r.seq === frag.sn);
    return row ? { frag, row, time } : null;
  };

  /**
   * The seek on its way is over: its clip plays, the film changed, or the channel is paused and
   * the engine answered (the player bar then shows the engine's paused position).
   */
  const settleExpected = (c, p) => {
    if (!expected) return;
    const gone =
      !c ||
      c.isLive ||
      c.itemId !== expected.itemId ||
      (c.paused && expected.answered) ||
      performance.now() - expected.since > EXPECT_MAX_MS ||
      (p && p.row.clip !== expected.clip && feed.rows.at(-1)?.clip === p.row.clip);
    if (gone) expected = null;
  };

  /**
   * The film position shown: the frozen one, the target of the owner's seek on its way, else
   * that of the playing row of the film the controls act on (null while the Preview is still
   * on an older clip after a seek from elsewhere). A stall or a gap holds the last one.
   */
  const position = () => {
    if (frozen) return frozen.at;
    const state = monitorState.peek();
    const c = control.peek();
    const p = state === 'playing' ? playing() : null;
    settleExpected(c, p);
    if (expected && (state === 'playing' || state === 'paused')) return expected.at;
    if (state !== 'playing') return null;
    const valid =
      p && c && !c.isLive && p.row.itemId === c.itemId && Number.isFinite(p.row.positionS) && p.row.clip === feed.rows.at(-1)?.clip;
    if (!valid) return lastShown && c && lastShown.itemId === c.itemId && !c.isLive ? lastShown.at : null;
    const at = p.row.positionS + Math.min(Math.max(0, p.time - p.frag.start), p.frag.duration);
    const shown = Number.isFinite(c.durationS) ? Math.min(at, c.durationS) : at;
    lastShown = { at: shown, itemId: c.itemId };
    return shown;
  };

  /**
   * The pause has ended: an armed one plays on (towards a seek made meanwhile, if any), after
   * a real one the relaunched clip plays.
   */
  const thaw = () => {
    const was = frozen;
    frozen = null;
    if (was?.parked) {
      still = -1;
      return;
    }
    if (was?.moved) {
      expect(was.at);
      if (expected) expected.answered = true;
    }
    video.play().catch(() => {});
    monitorState.value = 'playing';
  };

  /**
   * The owner sought to `at`: frozen, the pause moves there; else the Preview counts from it
   * (only while it shows a picture: otherwise the server's position counts, as before).
   */
  function expect(at) {
    const c = control.peek();
    lastShown = null;
    const state = monitorState.peek();
    if (state !== 'playing' && state !== 'paused') return;
    if (frozen) {
      frozen.at = at;
      frozen.moved = true;
      return;
    }
    expected = { at, clip: feed?.rows.at(-1)?.clip ?? null, itemId: c?.itemId ?? null, since: performance.now(), answered: false };
  }

  /** The engine's answer to the seek: refused, the Preview counts from its picture again. */
  function expectAnswered(ok) {
    if (!expected) return;
    if (ok) expected.answered = true;
    else expected = null;
  }

  const onError = (_event, error) => {
    if (!error.fatal || !hls) return;
    if (error.type === Hls.ErrorTypes.MEDIA_ERROR && mediaRecoveries < MAX_MEDIA_RECOVERIES) {
      mediaRecoveries += 1;
      hls.recoverMediaError();
      return;
    }
    destroy();
    if (retries >= MAX_RETRIES) {
      monitorState.value = 'failed';
      return;
    }
    retries += 1;
    // The next read starts a fresh player.
    newest = null;
  };

  const startAt = (seq, { pause = false } = {}) => {
    hls?.destroy();
    hls = new Hls({
      lowLatencyMode: false,
      liveSyncDurationCount: 1,
      maxBufferLength: 12,
      backBufferLength: 30,
      // Like every request of the interface to /api: the language it is shown in.
      xhrSetup: (xhr) => xhr.setRequestHeader('X-VRCinema-Lang', lang.value),
    });
    still = pause ? seq : null;
    lastShown = null;
    hls.on(Hls.Events.MANIFEST_PARSED, () => {
      if (!pause) video.play().catch(() => {});
    });
    hls.on(Hls.Events.ERROR, onError);
    hls.loadSource(`${FEED}?from=${seq}`);
    hls.attachMedia(video);
    monitorState.value = pause ? 'paused' : 'playing';
  };

  const apply = (f) => {
    feed = f;
    // A restarted engine numbers anew: a player of the old numbers starts again.
    const top = f.rows[0]?.seq ?? null;
    if (top !== null && first !== null && top < first && hls) {
      destroy();
      newest = null;
    }
    if (top !== null) first = top;
    if (monitorState.peek() === 'failed') return;
    if (!f.rows.length) {
      destroy();
      newest = null;
      monitorState.value = 'none';
      return;
    }
    const clip = f.rows.at(-1).clip;
    const c = control.peek();
    if (frozen) {
      const manual = Boolean(c?.pauseReasons?.includes('manual'));
      if (manual && c.paused) frozen.parked = true;
      if (!frozen.confirmed || manual || c?.pausePending) {
        // The frame the owner paused at stays until the resume.
        newest = clip;
        return;
      }
      // Resumed elsewhere (another device): like a resume here.
      thaw();
    }
    if (c?.paused && c.itemId != null && !c.isLive) {
      // Paused: the picture stands on the segment the film resumes from.
      const at = f.rows.filter((r) => r.itemId === c.itemId && Math.abs(r.positionS - c.positionS) < 0.5).at(-1);
      if (at && still !== at.seq) startAt(at.seq, { pause: true });
      else if (!at) {
        video.pause();
        monitorState.value = 'paused';
      }
      newest = clip;
      return;
    }
    if (!hls) startAt(newest === null ? freshStart(f) : f.rows.find((r) => r.clip === clip).seq);
    else if (clip !== newest) startAt(f.rows.find((r) => r.clip === clip).seq);
    else if (still !== null) {
      // Resumed: the still picture waits for the film's next clip (it starts at its start).
    } else if (monitorState.peek() !== 'playing') monitorState.value = 'playing';
    newest = clip;
  };

  /** Seconds ahead of on-air: the playing row and the antenna row, both in this list. */
  const measure = () => {
    if (!hls || !feed || still !== null || frozen || video.readyState < 2) {
      monitorAheadS.value = null;
      shownAhead = null;
      return;
    }
    const time = video.currentTime;
    const { frag, row } = playing() ?? {};
    const antenna = feed.rows.find((r) => r.seq === feed.antenna);
    if (row && antenna) {
      // The viewers are somewhere in the antenna's segment: measured from its middle.
      const segment = antenna.duration || feed.target || 1;
      const raw = row.start + (time - frag.start) - (antenna.start + segment / 2);
      shownAhead = steadyAhead(shownAhead, raw, segment);
      monitorAheadS.value = shownAhead;
    } else {
      shownAhead = null;
      monitorAheadS.value = null;
    }
    // Healthy playback: the counters start again.
    if (!video.paused && time > lastTime + 0.5) {
      mediaRecoveries = 0;
      retries = 0;
    }
    lastTime = time;
  };

  const schedule = () => {
    clearTimeout(readTimer);
    if (!stopped && !document.hidden) readTimer = setTimeout(read, READ_EVERY_MS);
  };

  async function read() {
    if (stopped || document.hidden || reading) return;
    const abort = new AbortController();
    reading = abort;
    const timeout = setTimeout(() => abort.abort(), READ_TIMEOUT_MS);
    try {
      const res = await fetch(FEED, {
        headers: { 'X-VRCinema-Lang': lang.value },
        cache: 'no-store',
        credentials: 'same-origin',
        signal: abort.signal,
      });
      const text = await res.text();
      if (stopped || reading !== abort) return;
      if (res.ok) apply(parseFeed(text));
      else {
        // No monitor here (no window session, a server without a window).
        destroy();
        monitorState.value = 'none';
      }
    } catch {
      // No answer: the next read tries again.
    } finally {
      clearTimeout(timeout);
      if (reading === abort) reading = null;
    }
    measure();
    schedule();
  }

  const onVisibility = () => {
    if (document.hidden) {
      clearTimeout(readTimer);
      reading?.abort();
      reading = null;
      destroy();
      newest = null;
      video.pause();
    } else {
      read();
    }
  };
  document.addEventListener('visibilitychange', onVisibility);
  monitorState.value = 'none';
  read();

  return {
    stop() {
      stopped = true;
      clearTimeout(readTimer);
      clearTimeout(retryTimer);
      reading?.abort();
      document.removeEventListener('visibilitychange', onVisibility);
      destroy();
      monitorState.value = 'off';
    },
    retry() {
      destroy();
      mediaRecoveries = 0;
      retries = 0;
      newest = null;
      monitorState.value = 'none';
      clearTimeout(retryTimer);
      retryTimer = setTimeout(read, 0);
    },
    sync() {
      // Only a playing monitor syncs (not idle, paused, failed or for a live stream).
      if (control.peek()?.isLive || monitorState.peek() !== 'playing') return false;
      const antenna = feed?.rows.find((r) => r.seq === feed.antenna);
      if (!antenna) {
        showToast(t('monitor.sync_unavailable'), 'info');
        return false;
      }
      startAt(antenna.seq);
      return true;
    },
    position,
    expect,
    expectAnswered,
    /** A seek of the owner stands in for the picture's position (see `expect`). */
    expecting: () => expected !== null || Boolean(frozen?.moved),
    /** The owner pauses: the picture stops now; its film position, or null (not known). */
    freeze() {
      const at = position();
      if (at === null) return null;
      video.pause();
      // Paused right after a seek: the pause stands on its target (the old picture stops).
      frozen = { at, confirmed: false, parked: false, moved: expected !== null };
      expected = null;
      monitorState.value = 'paused';
      monitorAheadS.value = null;
      shownAhead = null;
      return at;
    },
    /** The engine's answer to the pause: refused, the Preview plays on. */
    pauseAnswered(ok) {
      if (!frozen) return;
      if (ok) frozen.confirmed = true;
      else thaw();
    },
    /** The owner resumes. */
    resumed() {
      if (frozen) thaw();
    },
  };
}

function caption(state, ahead) {
  if (state !== 'playing' || !Number.isFinite(ahead)) return t('monitor.title');
  return ahead >= 1 ? t('monitor.ahead', { seconds: Math.round(ahead) }) : t('monitor.in_sync');
}

/** The left screen of the Channel view. In a remote panel it stays grey with a note. */
export function MonitorScreen() {
  const video = useRef(null);
  const inWindow = access.value === 'window';
  const known = access.value !== 'unknown';
  const state = monitorState.value;
  const muted = monitorMuted.value;
  const live = Boolean(control.value?.isLive);

  useEffect(() => {
    if (!inWindow || !Hls.isSupported()) return undefined;
    current = watch(video.current);
    attachPreview(current);
    return () => {
      attachPreview(null);
      current?.stop();
      current = null;
    };
  }, [inWindow]);

  useEffect(() => {
    if (video.current) video.current.muted = muted;
  }, [muted]);

  const supported = Hls.isSupported();
  const canSync = inWindow && supported && !live && state === 'playing';
  const muteLabel = muted ? t('monitor.unmute') : t('monitor.mute');
  return html`
    <div class=${`screen screen-monitor${inWindow || !known ? '' : ' is-off'}`} data-state=${inWindow ? state : 'remote'}>
      <div class="screen-cap">
        <span class="monitor-caption">${inWindow ? caption(state, monitorAheadS.value) : t('monitor.title')}</span>
        ${inWindow
          ? html`<button type="button" class="icon-button monitor-mute" aria-label=${muteLabel} title=${muteLabel}
              onClick=${() => (monitorMuted.value = !monitorMuted.value)}>
              <${Icon} name=${muted ? 'muted' : 'volume'} size=${16} />
            </button>`
          : null}
      </div>
      <div class="monitor-box" role="region" aria-label=${t('monitor.label')}>
        <video ref=${video} muted playsinline></video>
        ${!inWindow && known
          ? html`<div class="monitor-note">${t('ui.window_only')}</div>`
          : inWindow && !supported
            ? html`<div class="monitor-note" role="alert">${t('monitor.unsupported')}</div>`
            : state === 'none'
            ? html`<div class="monitor-note">${t('monitor.none')}</div>`
            : state === 'paused'
              ? html`<div class="monitor-badge" role="status">
                  ${control.value?.pausePending ? t('player.pause_pending') : t('monitor.paused')}
                </div>`
              : state === 'failed'
                ? html`<div class="preview-unavailable" role="alert">
                    <span>${t('monitor.unavailable')}</span>
                    <button type="button" class="btn" onClick=${() => current?.retry()}>${t('preview.retry')}</button>
                  </div>`
                : null}
      </div>
      <div class="screen-actions">
        <button type="button" class="btn monitor-sync" disabled=${!canSync} aria-keyshortcuts="S"
          title=${live ? t('monitor.sync_live') : undefined} onClick=${syncMonitor}>
          <${Icon} name="sync" size=${16} /><span>${t('monitor.sync')}</span>
        </button>
      </div>
    </div>`;
}
