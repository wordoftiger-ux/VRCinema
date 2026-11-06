// Level meters beside the Preview and On air (spec 2026-11-06-broadcast-design, 4): peak bars
// for left and right (dBFS, a short peak hold), the momentary loudness (LUFS: ITU-R BS.1770
// K-weighting over 400 ms) and a clipping light (a sample at −0.1 dBFS or above, lit for 3 s; a
// click puts it out).
//
// Web Audio taps the screen's own <video>. Once tapped, the element's sound goes only through
// the graph: the element plays at full level and the host's volume becomes a gain node after
// the meters, so they read the signal, not what the host listens at (the element's `volume` and
// `muted` then read and set that gain, so the players' code works as before). A tap is made only after
// the first click or key in the window: before that the browser keeps an AudioContext
// suspended, which would silence the screen.

import { useEffect, useRef } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';

const FLOOR_DB = -60;
const CLIP_LEVEL = 0.989; // −0.1 dBFS
const CLIP_HOLD_MS = 3000;
const PEAK_HOLD_MS = 1500;
const EVERY_MS = 100;
const MOMENTARY_S = 0.4;

let ctx = null;
/** Per element: `{gain, raw: [AnalyserNode×2], weighted: [AnalyserNode×2]}`. */
const taps = new WeakMap();
/** Per element: the host's `{volume, muted}` (applied to the element until it is tapped). */
const wanted = new WeakMap();

/** The BS.1770 K-weighting (shelf, then high-pass) for `fs`, as IIR coefficients. */
export function kWeighting(fs) {
  let f0 = 1681.974450955533;
  const G = 3.999843853973347;
  let Q = 0.7071752369554196;
  let K = Math.tan((Math.PI * f0) / fs);
  const Vh = 10 ** (G / 20);
  const Vb = Vh ** 0.4996667741545416;
  let a0 = 1 + K / Q + K * K;
  const shelf = {
    feedforward: [(Vh + (Vb * K) / Q + K * K) / a0, (2 * (K * K - Vh)) / a0, (Vh - (Vb * K) / Q + K * K) / a0],
    feedback: [1, (2 * (K * K - 1)) / a0, (1 - K / Q + K * K) / a0],
  };
  f0 = 38.13547087602444;
  Q = 0.5003270373238773;
  K = Math.tan((Math.PI * f0) / fs);
  a0 = 1 + K / Q + K * K;
  const highpass = {
    feedforward: [1, -2, 1],
    feedback: [1, (2 * (K * K - 1)) / a0, (1 - K / Q + K * K) / a0],
  };
  return [shelf, highpass];
}

/** Loudness (LUFS) of mean squares per channel (left and right weigh 1). */
export function loudness(meanSquares) {
  const sum = meanSquares.reduce((a, b) => a + b, 0);
  return sum > 0 ? -0.691 + 10 * Math.log10(sum) : -Infinity;
}

/** The AudioContext, made at the first click or key (and resumed then if suspended). */
function startAudio() {
  const AC = window.AudioContext ?? window.webkitAudioContext;
  if (!AC) return;
  if (!ctx) ctx = new AC();
  if (ctx.state === 'suspended') ctx.resume().catch(() => {});
}
for (const name of ['pointerdown', 'keydown']) document.addEventListener(name, startAudio, { capture: true, passive: true });

const MEDIA = HTMLMediaElement.prototype;
const VOLUME = Object.getOwnPropertyDescriptor(MEDIA, 'volume');
const MUTED = Object.getOwnPropertyDescriptor(MEDIA, 'muted');

function apply(video) {
  const tapped = taps.get(video);
  const w = wanted.get(video) ?? { volume: VOLUME.get.call(video), muted: MUTED.get.call(video) };
  if (tapped) {
    VOLUME.set.call(video, 1);
    MUTED.set.call(video, false);
    tapped.gain.gain.value = w.muted ? 0 : w.volume;
  } else {
    VOLUME.set.call(video, w.volume);
    MUTED.set.call(video, w.muted);
  }
}

/**
 * Once tapped, the element's own `volume` and `muted` stay at full level; reading or setting
 * them (the players' code, a muted autoplay) works on what the host hears instead.
 */
function redirectVolume(video) {
  const listening = () => wanted.get(video) ?? { volume: 1, muted: false };
  Object.defineProperty(video, 'volume', {
    configurable: true,
    get: () => listening().volume,
    set: (v) => setListening(video, Number(v), listening().muted),
  });
  Object.defineProperty(video, 'muted', {
    configurable: true,
    get: () => listening().muted,
    set: (m) => setListening(video, listening().volume, Boolean(m)),
  });
}

/** What the host hears from `video`: its volume (0–1) and mute switch. */
export function setListening(video, volume, muted) {
  if (!video) return;
  wanted.set(video, { volume, muted });
  apply(video);
}

/** The tap of `video`, made when the AudioContext runs; null before that. */
function tap(video) {
  if (!video) return null;
  const known = taps.get(video);
  if (known) return known;
  if (!ctx || ctx.state !== 'running') return null;
  let source;
  try {
    source = ctx.createMediaElementSource(video);
  } catch {
    return null;
  }
  const gain = ctx.createGain();
  source.connect(gain).connect(ctx.destination);
  // Analysers render only when pulled: a silent path to the output.
  const sink = ctx.createGain();
  sink.gain.value = 0;
  sink.connect(ctx.destination);
  const split = ctx.createChannelSplitter(2);
  source.connect(split);
  const [shelf, highpass] = kWeighting(ctx.sampleRate);
  const raw = [];
  const weighted = [];
  for (let ch = 0; ch < 2; ch += 1) {
    const r = ctx.createAnalyser();
    r.fftSize = 4096;
    split.connect(r, ch);
    r.connect(sink);
    const k = ctx.createAnalyser();
    k.fftSize = 32768;
    const f1 = ctx.createIIRFilter(shelf.feedforward, shelf.feedback);
    const f2 = ctx.createIIRFilter(highpass.feedforward, highpass.feedback);
    split.connect(f1, ch);
    f1.connect(f2).connect(k);
    k.connect(sink);
    raw.push(r);
    weighted.push(k);
  }
  const tapped = { gain, raw, weighted, buf: new Float32Array(32768) };
  if (!wanted.has(video)) wanted.set(video, { volume: VOLUME.get.call(video), muted: MUTED.get.call(video) });
  taps.set(video, tapped);
  redirectVolume(video);
  apply(video);
  return tapped;
}

/** Peaks (0–1) per channel and the momentary loudness of a tap. */
function read(tapped) {
  const peaks = tapped.raw.map((a) => {
    const buf = tapped.buf.subarray(0, a.fftSize);
    a.getFloatTimeDomainData(buf);
    let max = 0;
    for (let i = 0; i < buf.length; i += 1) max = Math.max(max, Math.abs(buf[i]));
    return max;
  });
  const n = Math.min(32768, Math.round(MOMENTARY_S * ctx.sampleRate));
  const squares = tapped.weighted.map((a) => {
    a.getFloatTimeDomainData(tapped.buf);
    let sum = 0;
    for (let i = tapped.buf.length - n; i < tapped.buf.length; i += 1) sum += tapped.buf[i] * tapped.buf[i];
    return sum / n;
  });
  return { peaks, lufs: loudness(squares) };
}

const toDb = (v) => (v > 0 ? 20 * Math.log10(v) : -Infinity);
const share = (db) => Math.max(0, Math.min(1, (db - FLOOR_DB) / -FLOOR_DB));

/** The meter of a screen; `video()` gives the element whose sound it reads now. */
export function Meter({ video }) {
  const box = useRef(null);
  const state = useRef({ hold: [0, 0], holdAt: [0, 0], clipAt: -Infinity });

  useEffect(() => {
    const timer = setInterval(() => {
      const el = box.current;
      if (!el) return;
      const tapped = tap(video());
      const s = state.current;
      const now = performance.now();
      const { peaks, lufs } = tapped && ctx?.state === 'running' ? read(tapped) : { peaks: [0, 0], lufs: -Infinity };
      el.dataset.active = tapped ? 'yes' : 'no';
      peaks.forEach((p, ch) => {
        if (p >= s.hold[ch] || now - s.holdAt[ch] > PEAK_HOLD_MS) {
          s.hold[ch] = p;
          s.holdAt[ch] = now;
        }
        if (p >= CLIP_LEVEL) s.clipAt = now;
        const bar = el.querySelector(`[data-ch="${ch}"]`);
        bar.style.setProperty('--level', String(share(toDb(p))));
        bar.style.setProperty('--hold', String(share(toDb(s.hold[ch]))));
      });
      const clipping = now - s.clipAt < CLIP_HOLD_MS;
      el.classList.toggle('clipping', clipping);
      el.querySelector('.meter-lufs').dataset.lufs = Number.isFinite(lufs) && lufs > -70 ? `${Math.round(lufs)} LUFS` : '— LUFS';
      const peakDb = toDb(Math.max(...peaks));
      el.title = t('meter.title', {
        peak: Number.isFinite(peakDb) ? peakDb.toFixed(1) : '−∞',
        lufs: Number.isFinite(lufs) ? lufs.toFixed(1) : '−∞',
      });
    }, EVERY_MS);
    return () => clearInterval(timer);
  }, []);

  const reset = (e) => {
    e.stopPropagation();
    state.current.clipAt = -Infinity;
    box.current?.classList.remove('clipping');
  };
  return html`
    <span class="meter" ref=${box} role="img" aria-label=${t('meter.label')} data-meter="">
      <span class="meter-bars" aria-hidden="true"><i data-ch="0"></i><i data-ch="1"></i></span>
      <span class="meter-lufs" aria-hidden="true" data-lufs="— LUFS"></span>
      <button type="button" class="meter-clip" aria-label=${t('meter.clip')} title=${t('meter.clip')} onClick=${reset}></button>
    </span>`;
}
