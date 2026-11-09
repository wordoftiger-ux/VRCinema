// The Preview of an audio-only film (mp3, flac, a SoundCloud link…; spec 2026-11-08-audio-design,
// section 4). The file has no picture of its own (the picture on air is made by the engine), so
// over the Preview's empty player this screen shows the title, the artist and the cover (what
// the engine found for its picture on air: /api/desktop/audio/<id>, there once the film airs;
// until then the queue's title), bars that move with the film's own sound (Web Audio: the
// level meters' tap of the same <video>, see meters.js; a still row of bars when the sound
// cannot be read yet: no click in the window, or reduced motion), the elapsed and total time and
// "Up next: <title>" (the next film of the queue; nothing without one).

import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api } from '../api.js';
import { channel, control, queue } from '../store.js';
import { sourceState } from './sourceplayer.js';
import { sampleRate, spectrum } from './meters.js';

const BARS = 32;
const LOW_HZ = 40;
const HIGH_HZ = 8000;
const FRAME_MS = 33;
const META_RETRY_MS = 3000;
const META_TRIES = 20;
/** The bars when the sound cannot be read: a still, gentle row. */
const STILL = Array.from({ length: BARS }, (_, i) => 0.12 + 0.08 * Math.abs(Math.sin(i * 0.7)));

const reducedMotion = () => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;

/** A player with a file but no picture (an audio-only film). */
export function hasNoPicture(v) {
  return Boolean(v && v.getAttribute('src') && v.readyState >= 1 && v.videoWidth === 0 && v.videoHeight === 0);
}

/** The title of the film after `currentId` in the queue (the first other one); null: none. */
export function nextTitle(items, currentId) {
  return (items ?? []).find((i) => i.id !== currentId)?.title ?? null;
}

/** `m:ss`, or `h:mm:ss` from an hour; null for an unknown time. */
export function clock(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return null;
  const s = Math.floor(seconds);
  const pad = (n) => String(n).padStart(2, '0');
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

/**
 * `count` bar heights (0–1) from frequency bins (`bins[i]` 0–255 at `i × rate / 2 / binCount`
 * Hz): bands spaced on a log scale from 40 Hz to 8 kHz, each the loudest bin in it.
 */
export function bandsOf(bins, rate, count, binCount = 2048) {
  const hzPerBin = rate / 2 / binCount;
  const out = new Array(count).fill(0);
  const ratio = HIGH_HZ / LOW_HZ;
  for (let b = 0; b < count; b += 1) {
    const from = Math.floor((LOW_HZ * ratio ** (b / count)) / hzPerBin);
    const to = Math.max(from + 1, Math.floor((LOW_HZ * ratio ** ((b + 1) / count)) / hzPerBin));
    let max = 0;
    for (let i = from; i < to && i < bins.length; i += 1) max = Math.max(max, bins[i]);
    out[b] = max / 255;
  }
  return out;
}

/** Draws the bars into the canvas (sized to its box; the accent colour of the theme). */
function paint(canvas, heights) {
  const ratio = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(canvas.clientWidth * ratio));
  const h = Math.max(1, Math.round(canvas.clientHeight * ratio));
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== h) canvas.height = h;
  const g = canvas.getContext('2d');
  if (!g) return;
  g.clearRect(0, 0, w, h);
  g.fillStyle = getComputedStyle(canvas).color;
  const pitch = w / heights.length;
  const bar = Math.max(1, pitch * 0.72);
  heights.forEach((v, i) => {
    const bh = Math.max(ratio * 2, Math.min(1, v) * h);
    g.fillRect(i * pitch + (pitch - bar) / 2, h - bh, bar, bh);
  });
}

/** Over the Preview's player (`video()`): shown only while it plays an audio-only film. */
export function AudioScreen({ video, compact = false }) {
  const [audio, setAudio] = useState(false);
  const [time, setTime] = useState({ at: 0, total: null });
  const [meta, setMeta] = useState(null);
  const canvas = useRef(null);
  const ready = sourceState.value === 'ready';
  const itemId = control.value?.itemId ?? null;
  const shown = audio && ready;

  useEffect(() => {
    const v = video();
    if (!v) return undefined;
    const check = () => setAudio(hasNoPicture(v));
    const tick = () => setTime({ at: v.currentTime, total: Number.isFinite(v.duration) ? v.duration : null });
    const kinds = ['loadedmetadata', 'loadeddata', 'emptied', 'resize'];
    for (const k of kinds) v.addEventListener(k, check);
    for (const k of ['timeupdate', 'seeked', 'durationchange']) v.addEventListener(k, tick);
    check();
    tick();
    return () => {
      for (const k of kinds) v.removeEventListener(k, check);
      for (const k of ['timeupdate', 'seeked', 'durationchange']) v.removeEventListener(k, tick);
    };
  }, []);

  // The engine's title, artist and cover (its clip folder appears once the film airs).
  useEffect(() => {
    setMeta(null);
    if (!shown || itemId === null) return undefined;
    let stopped = false;
    let tries = 0;
    let timer = 0;
    const ask = async () => {
      try {
        const m = await api('GET', `/api/desktop/audio/${encodeURIComponent(itemId)}`);
        if (!stopped && m?.itemId === itemId) setMeta(m);
      } catch {
        tries += 1;
        if (!stopped && tries < META_TRIES) timer = setTimeout(ask, META_RETRY_MS);
      }
    };
    ask();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [shown, itemId]);

  useEffect(() => {
    if (!shown) return undefined;
    const bins = new Uint8Array(2048);
    let frame = 0;
    let last = -Infinity;
    const draw = (now) => {
      frame = requestAnimationFrame(draw);
      if (document.hidden || now - last < FRAME_MS || !canvas.current) return;
      last = now;
      const v = video();
      const live = !reducedMotion() && v && spectrum(v, bins);
      canvas.current.dataset.live = live ? 'yes' : 'no';
      paint(canvas.current, live ? bandsOf(bins, sampleRate(), BARS, bins.length) : STILL);
    };
    frame = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(frame);
  }, [shown]);

  if (!shown) return null;
  const title = meta?.title || channel.value?.current?.title || '';
  const artist = meta?.artist || '';
  const total = time.total ?? control.value?.durationS ?? null;
  const elapsed = clock(time.at);
  const next = nextTitle(queue.value, itemId);
  return html`
    <div class=${`audio-screen${compact ? ' is-compact' : ''}`} data-audio-screen>
      <div class="as-art" aria-hidden="true">
        ${meta?.cover ? html`<img src=${meta.cover} alt="" />` : html`<span class="as-note">♪</span>`}
      </div>
      <div class="as-text">
        <span class="as-badge">${t('preview.audio')}</span>
        <b class="as-title" title=${title}>${title}</b>
        ${artist ? html`<span class="as-artist">${artist}</span>` : null}
        <span class="as-time" data-as-time>${elapsed}${clock(total) ? ` / ${clock(total)}` : ''}</span>
        ${next ? html`<span class="as-next" data-as-next>${t('visual.up_next', { title: next })}</span>` : null}
      </div>
      <canvas class="as-bars" ref=${canvas} aria-hidden="true"></canvas>
    </div>`;
}
