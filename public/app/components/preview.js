// The live preview on the Channel view: the channel's own stream (/live/main.m3u8, as the
// viewers get it) played with hls.js, so the owner sees what VRChat sees and how far behind
// the server it is. The delay is measured here once a second (`previewDelayS`) and drives the
// caption under the preview and the bubble after a seek. A right click opens the preview menu;
// the "Stats for nerds" overlay sits on top of the picture.

import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import Hls from '/static/vendor/hls.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { contextMenuHandlers } from './menu.js';
import { copyText, previewMuted, previewVolume } from './playerbar.js';
import { StatsOverlay, previewShown, previewStats, statsOpen, toggleStats } from './stats.js';
import {
  abilities,
  channelLink,
  previewDelayS,
  seekBubble,
  seekBy,
  showToast,
  togglePause,
} from '../store.js';

const SOURCE = '/live/main.m3u8';
const MEASURE_EVERY_MS = 1000;
const RETRY_AFTER_MS = 3000;
const BUBBLE_MS = 5000;

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
 * Plays the channel in `video` until the returned function is called. A stream that cannot be
 * read (the channel is starting, the network dropped) is tried again every few seconds.
 */
function play(video) {
  let hls = null;
  let retry = 0;
  let stopped = false;

  const start = () => {
    if (stopped) return;
    hls = new Hls({ lowLatencyMode: false });
    hls.on(Hls.Events.MANIFEST_PARSED, () => {
      // A browser may refuse sound before the first click: play muted rather than not at all.
      video.play().catch(() => {
        video.muted = true;
        video.play().catch(() => {});
      });
    });
    hls.on(Hls.Events.ERROR, (_event, error) => {
      if (!error.fatal) return;
      if (error.type === Hls.ErrorTypes.MEDIA_ERROR) {
        hls.recoverMediaError();
        return;
      }
      hls.destroy();
      hls = null;
      retry = setTimeout(start, RETRY_AFTER_MS);
    });
    hls.loadSource(SOURCE);
    hls.attachMedia(video);
  };
  start();

  const measure = setInterval(() => {
    const ready = hls && video.readyState >= 2;
    previewDelayS.value = ready && Number.isFinite(hls.latency) ? hls.latency : null;
    previewStats.value = {
      bufferS: ready ? bufferedAhead(video) : null,
      dropped: ready ? (video.getVideoPlaybackQuality?.().droppedVideoFrames ?? null) : null,
    };
  }, MEASURE_EVERY_MS);

  return () => {
    stopped = true;
    clearInterval(measure);
    clearTimeout(retry);
    hls?.destroy();
    previewDelayS.value = null;
    previewStats.value = { bufferS: null, dropped: null };
  };
}

/** The bubble that follows a seek: when the viewers will see it. */
function SeekBubble() {
  const count = seekBubble.value;
  const seen = useRef(count);
  const [shown, setShown] = useState(false);
  useEffect(() => {
    if (count === seen.current) return undefined;
    seen.current = count;
    setShown(true);
    const timer = setTimeout(() => setShown(false), BUBBLE_MS);
    return () => clearTimeout(timer);
  }, [count]);
  if (!shown) return null;
  const delay = previewDelayS.value;
  const text = Number.isFinite(delay)
    ? t('preview.seek_bubble', { seconds: Math.max(1, Math.round(delay)) })
    : t('preview.seek_bubble_unknown');
  return html`<div class="seek-bubble" role="status">⏩ ${text}</div>`;
}

export function Preview() {
  const video = useRef(null);
  const volume = previewVolume.value;
  const muted = previewMuted.value || volume === 0;
  const delay = previewDelayS.value;

  useEffect(() => {
    previewShown.value = true;
    const stop = Hls.isSupported() ? play(video.current) : () => {};
    return () => {
      stop();
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

  return html`
    <div class="video" tabindex="0" aria-label=${t('preview.label')}
      ...${contextMenuHandlers('preview.menu', menuItems)}>
      <video ref=${video} playsinline></video>
      <${SeekBubble} />
      ${statsOpen.value ? html`<${StatsOverlay} delayS=${delay} />` : null}
    </div>
    <p class="stage-caption">${Number.isFinite(delay)
      ? t('preview.caption', { seconds: Math.max(1, Math.round(delay)) })
      : t('preview.caption_plain')}</p>`;
}
