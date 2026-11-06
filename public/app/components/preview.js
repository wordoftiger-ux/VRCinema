// The live preview on the Channel view: the channel's own stream (/live/main.m3u8, as the
// viewers get it) played with hls.js, so the owner sees what VRChat sees and how far behind
// the server it is. The delay is measured here once a second (`previewDelayS`) and drives the
// caption of the On air screen and (with how far the Preview is ahead) the bubble after a seek.
// A right click opens the preview menu;
// the "Stats for nerds" overlay sits on top of the picture. While the window is hidden
// (minimised, in the tray) the preview stops loading, so it does not count as a viewer.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import Hls from '/static/vendor/hls.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { contextMenuHandlers } from './menu.js';
import { monitorAheadS, previewMode } from './monitor.js';
import { copyText, previewMuted, previewVolume } from './playerbar.js';
import { airBehindS } from './sourceplayer.js';
import { StatsOverlay, previewShown, previewStats, statsOpen, toggleStats } from './stats.js';
import {
  abilities,
  channel,
  channelLink,
  previewDelayS,
  seekBubble,
  seekBy,
  showToast,
  togglePause,
} from '../store.js';

/**
 * On air stands where a typical viewer is: moved when it is further off than half a segment
 * (at least 1.5 s) for OFF_MEASURES measures in a row.
 */
function latencyTolerance() {
  const seg = channel.peek()?.air?.segmentS;
  return Math.max(1.5, Number.isFinite(seg) ? seg / 2 : 2);
}
const OFF_MEASURES = 3;

/** How far behind the stream's edge a typical viewer plays (the engine's air clock); null: not known. */
function airLatency() {
  const s = channel.peek()?.air?.latencyS;
  return Number.isFinite(s) && s > 0 ? s : null;
}

const SOURCE = '/live/main.m3u8';
const MEASURE_EVERY_MS = 1000;
const RETRY_AFTER_MS = 3000;
const BUBBLE_MS = 5000;
/** Immediate recoveries from a media error, then the stream is rebuilt. */
const MAX_MEDIA_RECOVERIES = 2;
/** Rebuilds in a row (without healthy playback in between) before the preview gives up. */
const MAX_RETRIES = 4;

/** The On air screen gave up after several tries: shows "On air unavailable" and a retry button. */
export const previewFailed = signal(false);

/** The last delay that was really measured (the bubble falls back to it). */
let lastGoodDelayS = null;

/** The seconds buffered ahead of the playing position (0 with nothing buffered there). */
function bufferedAhead(video) {
  const at = video.currentTime;
  for (let i = 0; i < video.buffered.length; i += 1) {
    if (video.buffered.start(i) <= at + 0.1 && video.buffered.end(i) >= at) {
      return video.buffered.end(i) - at;
    }
  }
  return 0;
}

/**
 * Plays the channel in `video`. Returns `{stop, retry}`. A stream that cannot be read (the
 * channel is starting, the network dropped) is tried again every few seconds, a few times;
 * then `previewFailed` is raised until `retry()`.
 */
function play(video) {
  let hls = null;
  let timer = 0;
  let stopped = false;
  let waitingForVisible = false;
  let mediaRecoveries = 0;
  let retries = 0;
  let lastTime = -1;
  let offCount = 0;

  const start = () => {
    if (stopped) return;
    if (document.hidden) {
      waitingForVisible = true;
      return;
    }
    // Not hls.js's own start point: where the engine says a typical viewer is (VRChat's
    // players), kept there by `measure` below. The window's own requests are marked so the
    // engine does not count them as a viewer's position.
    const latency = airLatency();
    hls = new Hls({
      lowLatencyMode: false,
      ...(latency ? { liveSyncDuration: latency, liveMaxLatencyDuration: latency + 30 } : {}),
      xhrSetup: (xhr) => xhr.setRequestHeader('X-VRCinema-Air', '1'),
    });
    hls.on(Hls.Events.MANIFEST_PARSED, () => {
      // A browser may refuse sound before the first click: play muted rather than not at all.
      video.play().catch(() => {
        previewMuted.value = true;
        video.muted = true;
        video.play().catch(() => {});
      });
    });
    hls.on(Hls.Events.ERROR, (_event, error) => {
      if (!error.fatal) return;
      if (error.type === Hls.ErrorTypes.MEDIA_ERROR && mediaRecoveries < MAX_MEDIA_RECOVERIES) {
        mediaRecoveries += 1;
        hls.recoverMediaError();
        return;
      }
      hls.destroy();
      hls = null;
      if (retries >= MAX_RETRIES) {
        previewFailed.value = true;
        return;
      }
      retries += 1;
      timer = setTimeout(start, RETRY_AFTER_MS);
    });
    hls.loadSource(SOURCE);
    hls.attachMedia(video);
  };
  start();

  const onVisibility = () => {
    if (document.hidden) {
      hls?.stopLoad();
      video.pause();
    } else if (waitingForVisible) {
      waitingForVisible = false;
      start();
    } else if (hls) {
      hls.startLoad();
      video.play().catch(() => {});
    }
  };
  document.addEventListener('visibilitychange', onVisibility);

  const measure = setInterval(() => {
    const alive = Boolean(hls) && video.readyState >= 2 && !document.hidden;
    if (!alive) {
      previewDelayS.value = null;
      previewStats.value = { bufferS: null, dropped: null };
      return;
    }
    const advancing = !video.paused && !video.seeking;
    if (advancing) {
      // hls.js reports 0 (or NaN) while it does not know: that is no measurement.
      const latency = hls.latency;
      if (Number.isFinite(latency) && latency > 0) {
        lastGoodDelayS = latency;
        previewDelayS.value = latency;
        // Off the typical viewer's position by more than the tolerance for a few measures in a
        // row (hls.js sees the stream's edge only when it reloads the list): moved there.
        const target = airLatency();
        const off = target === null ? 0 : latency - target;
        offCount = Math.abs(off) > latencyTolerance() ? offCount + 1 : 0;
        if (offCount >= OFF_MEASURES) {
          offCount = 0;
          const to = video.currentTime + off;
          const range = video.seekable;
          const start = range.length ? range.start(0) : 0;
          const end = range.length ? range.end(range.length - 1) : to;
          video.currentTime = Math.min(Math.max(to, start), end);
        }
      } else {
        previewDelayS.value = null;
      }
      // Healthy playback: the counters start again.
      if (video.currentTime > lastTime + 0.5) {
        mediaRecoveries = 0;
        retries = 0;
      }
    }
    lastTime = video.currentTime;
    previewStats.value = {
      bufferS: bufferedAhead(video),
      dropped: video.getVideoPlaybackQuality?.().droppedVideoFrames ?? null,
    };
  }, MEASURE_EVERY_MS);

  return {
    retry() {
      clearTimeout(timer);
      hls?.destroy();
      hls = null;
      mediaRecoveries = 0;
      retries = 0;
      previewFailed.value = false;
      start();
    },
    stop() {
      stopped = true;
      clearInterval(measure);
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibility);
      hls?.destroy();
      previewDelayS.value = null;
      previewStats.value = { bufferS: null, dropped: null };
      previewFailed.value = false;
      lastGoodDelayS = null;
    },
  };
}

/** The bubble that follows a seek: when the viewers will see it (fixed when the seek happens). */
function SeekBubble() {
  const count = seekBubble.value;
  const seen = useRef(count);
  const [bubble, setBubble] = useState(null);
  useEffect(() => {
    if (count === seen.current) return undefined;
    seen.current = count;
    // On air is a delayed copy of the Preview: the change reaches the viewers about as far
    // behind as the Preview is ahead of the air (its caption), never sooner than the air's own
    // delay (a Preview in sync with the air jumps ahead to the new clip).
    const latency = previewDelayS.peek() ?? lastGoodDelayS;
    const ahead = previewMode.peek() === 'file' ? airBehindS.peek() : monitorAheadS.peek();
    const known = [latency, ahead].filter(Number.isFinite);
    const delay = known.length ? Math.max(...known) : null;
    setBubble({ seconds: delay === null ? null : Math.max(1, Math.round(delay)) });
    const timer = setTimeout(() => setBubble(null), BUBBLE_MS);
    return () => clearTimeout(timer);
  }, [count]);
  if (!bubble) return null;
  const text = bubble.seconds === null
    ? t('preview.seek_bubble_unknown')
    : t('preview.seek_bubble', { seconds: bubble.seconds });
  return html`<div class="seek-bubble" role="status">⏩ ${text}</div>`;
}

/** The air's caption: how far behind the Preview (the window), else behind the server. */
function airCaption(delay) {
  const behind = previewMode.value === 'file' ? airBehindS.value : null;
  if (Number.isFinite(behind)) return t('monitor.air_behind', { seconds: behind });
  const seconds = Number.isFinite(delay) ? Math.max(1, Math.round(delay)) : null;
  return seconds === null ? t('monitor.on_air') : t('monitor.on_air_caption', { seconds });
}

/**
 * The channel as the viewers get it. `dual`: a screen of the Channel view with its caption
 * above it (`role`: `main`, `pip` or `side`, `tools`: the layout buttons, `onActivate`: a click
 * on the small screen); otherwise the only screen (phone, the air's own window), caption below.
 */
export function Preview({ dual = false, role = 'side', tools = null, onActivate = null }) {
  const video = useRef(null);
  const player = useRef(null);
  const volume = previewVolume.value;
  const muted = previewMuted.value || volume === 0;
  const delay = previewDelayS.value;

  useEffect(() => {
    previewShown.value = true;
    if (Hls.isSupported()) player.current = play(video.current);
    return () => {
      player.current?.stop();
      player.current = null;
      previewShown.value = false;
    };
  }, []);

  useEffect(() => {
    video.current.volume = volume;
    video.current.muted = muted;
  }, [volume, muted]);

  const menuItems = () => {
    const { canSeek, manual, canToggle } = abilities.value;
    const link = channelLink.value;
    return [
      { key: manual ? 'player.resume' : 'player.pause', icon: manual ? 'play' : 'pause', disabled: !canToggle, action: togglePause },
      { key: 'player.back_10_label', disabled: !canSeek, action: () => seekBy(-10) },
      { key: 'player.forward_10_label', disabled: !canSeek, action: () => seekBy(10) },
      'separator',
      { key: 'stats.title', action: toggleStats },
      { key: 'preview.copy_channel_link', icon: 'copy', disabled: !link, action: () => copyText(link).catch((e) => showToast(e.message)) },
    ];
  };

  // Beside the monitor the screen is narrow: the stats overlay then spreads over the stage.
  const stats = statsOpen.value ? html`<${StatsOverlay} delayS=${delay} />` : null;
  const pip = dual && role === 'pip';
  const box = html`
    <div class="video" tabindex="0" aria-label=${t('preview.label')}
      onClick=${pip && onActivate ? onActivate : undefined}
      ...${contextMenuHandlers('preview.menu', menuItems)}>
      <video ref=${video} playsinline></video>
      ${previewFailed.value
        ? html`<div class="preview-unavailable" role="alert">
            <span>${t('preview.unavailable')}</span>
            <button type="button" class="btn" onClick=${() => player.current?.retry()}>${t('preview.retry')}</button>
          </div>`
        : null}
      ${pip ? null : html`<${SeekBubble} />`}
      ${dual ? tools : stats}
    </div>`;
  const caption = pip ? t('monitor.on_air') : airCaption(delay);
  if (dual) {
    return html`
      <div class=${`screen screen-air role-${role}`}>
        <p class="stage-caption screen-cap">${caption}</p>
        ${box}
        ${stats}
      </div>`;
  }
  return html`
    ${box}
    <p class="stage-caption">${caption}</p>`;
}
