// The player bar at the bottom: the video on air, the owner's controls (−10 s, ⏮, pause /
// resume, ⏭, +10 s), the timeline (time under the pointer, click or drag to seek), the
// preview volume, the internet status and "Copy link". The position is the producer's Preview's
// (what the owner sees and controls) while it plays or is frozen by a pause, right after the
// owner's seek the target (until the Preview has the new clip); otherwise it comes
// from `control` — the server's position (a few segments ahead of what viewers see), "≈" when a
// seek lands on a keyframe. Controls the engine does not allow now are greyed out.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t, videoTitle } from '../i18n.js';
import { Icon } from '../icons.js';
import { contextMenuHandlers } from './menu.js';
import { monitorRunning, monitorState, previewPosition } from './monitor.js';
import { sourceState, sourceUrl } from './sourceplayer.js';
import { monitorMuted, monitorVolume, previewMuted, previewVolume } from './volume.js';
import { countdownText } from './broadcast.js';
import {
  channel,
  channelLink,
  control,
  abilities,
  controlAt,
  netStatus,
  queue,
  restart,
  seekBy,
  seekTo,
  showToast,
  skip,
  togglePause,
} from '../store.js';

// The volumes live in volume.js (On air's keeps its old names, re-exported here).
export { previewMuted, previewVolume };

// ---------- Thumbnails of the seek bar (the Preview's own file) ----------

const THUMB_W = 160;
const THUMB_H = 90;
/** Thumbnails kept, by 2-second step. */
const THUMB_CACHE = 240;

/**
 * Frames of the Preview's film for the seek bar, taken from a second, hidden <video> of the same
 * file (one seek at a time; the newest wish wins).
 */
function thumbnailer(url) {
  const video = document.createElement('video');
  video.muted = true;
  video.preload = 'auto';
  video.src = url;
  const canvas = document.createElement('canvas');
  canvas.width = THUMB_W;
  canvas.height = THUMB_H;
  const cache = new Map();
  let busy = false;
  let wanted = null;
  let onFrame = null;
  const step = (s) => Math.max(0, Math.round(s / 2) * 2);
  const next = () => {
    if (busy || wanted === null || video.readyState < 1) return;
    const at = wanted;
    wanted = null;
    if (cache.has(at)) {
      onFrame?.(at, cache.get(at));
      next();
      return;
    }
    busy = true;
    const done = () => {
      video.removeEventListener('seeked', done);
      busy = false;
      try {
        canvas.getContext('2d').drawImage(video, 0, 0, THUMB_W, THUMB_H);
        const img = canvas.toDataURL('image/jpeg', 0.7);
        cache.set(at, img);
        if (cache.size > THUMB_CACHE) cache.delete(cache.keys().next().value);
        onFrame?.(at, img);
      } catch {
        // Not drawable (yet): no thumbnail.
      }
      next();
    };
    video.addEventListener('seeked', done);
    video.currentTime = Math.min(at, Math.max(0, (video.duration || at) - 0.1));
  };
  video.addEventListener('loadedmetadata', next);
  return {
    want(s, cb) {
      onFrame = cb;
      const at = step(s);
      if (cache.has(at)) cb(at, cache.get(at));
      else {
        wanted = at;
        next();
      }
    },
    stop() {
      video.removeAttribute('src');
      video.load();
    },
  };
}

/** The thumbnail of the film at `seconds` (only while the Preview plays its own file). */
function Thumb({ seconds, left }) {
  const url = sourceState.value === 'ready' ? sourceUrl.value : null;
  const grabber = useRef(null);
  const [frame, setFrame] = useState(null);
  useEffect(() => {
    if (!url) return undefined;
    grabber.current = thumbnailer(url);
    return () => {
      grabber.current?.stop();
      grabber.current = null;
      setFrame(null);
    };
  }, [url]);
  useEffect(() => {
    grabber.current?.want(seconds, (_at, img) => setFrame(img));
  }, [seconds, url]);
  if (!url || !frame) return null;
  return html`<img class="tl-thumb" alt="" aria-hidden="true" src=${frame} style=${`left:${left}%`} />`;
}

// ---------- Time ----------

/** `75` → `1:15`, `3725` → `1:02:05`. */
export function formatTime(s) {
  if (s === null || s === undefined || !Number.isFinite(s)) return '';
  const total = Math.max(0, Math.floor(s));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const sec = String(total % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
}

/** Ticks while something plays, so the position moves on between two polls. */
const now = signal(performance.now());
setInterval(() => (now.value = performance.now()), 250);

/** The position to show: the server's, moved on by the time since it arrived while playing. */
export function shownPosition(c, at, status) {
  if (!c) return null;
  let pos = Number(c.positionS) || 0;
  if (!c.paused && !c.seekPending && status === 'playing' && !c.isLive) {
    // Read only here: the bar re-renders with the clock only while the position moves.
    pos += Math.max(0, now.value - at) / 1000;
  }
  if (c.durationS !== null && c.durationS !== undefined) pos = Math.min(pos, c.durationS);
  return pos;
}

/** The Preview's film position while it plays or stands frozen (moves with the clock); else null. */
function shownPreviewPosition(c) {
  const state = monitorState.value;
  if (!c || c.isLive || (state !== 'playing' && state !== 'paused')) return null;
  // Read only here: the bar re-renders with the clock while the Preview runs.
  void now.value;
  return previewPosition();
}

const PAUSE_REASON_KEYS = {
  manual: 'player.paused_manual',
  tunnel: 'player.paused_tunnel',
  no_viewers: 'player.paused_no_viewers',
};

// ---------- Parts ----------

function NowPlaying({ current, c }) {
  let sub = '';
  // Paused with no film on air (it ended before the pause point): the pause still shows.
  if (c?.pausePending && current) sub = t('player.pause_pending');
  else if (c?.paused && c.pauseReasons?.length) {
    sub = c.pauseReasons.map((r) => (PAUSE_REASON_KEYS[r] ? t(PAUSE_REASON_KEYS[r]) : r)).join(' · ');
  } else if (!current) sub = t('player.nothing_playing_hint');
  else if (current.requesterName) sub = t('player.added_by', { name: current.requesterName });
  // The stream counts down to a video's start time.
  const countdown = current ? null : countdownText(queue.value);
  if (countdown && !c?.paused) sub = countdown.text;
  const title = current
    ? videoTitle(current.title)
    : countdown
      ? videoTitle(countdown.title)
      : t('player.nothing_playing');
  return html`
    <div class="nowp">
      <span class="thumb" aria-hidden="true"></span>
      <div class="nowp-text">
        <b class="nowp-title" title=${title}>${title}</b>
        <small class="nowp-sub" title=${sub}>${sub}</small>
      </div>
    </div>`;
}

function Timeline({ pos, dur, approx, seekable, live }) {
  const track = useRef(null);
  const [hover, setHover] = useState(null);
  const [drag, setDrag] = useState(null);
  const known = Number.isFinite(dur) && dur > 0;
  const active = seekable && known;

  const fraction = (e) => {
    const r = track.current.getBoundingClientRect();
    return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
  };
  const onPointerMove = (e) => {
    if (!known) return;
    const f = fraction(e);
    setHover(f);
    if (drag !== null) setDrag(f);
  };
  const onPointerDown = (e) => {
    if (!active || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag(fraction(e));
  };
  const onPointerUp = (e) => {
    if (drag === null) return;
    const f = fraction(e);
    setDrag(null);
    seekTo(f * dur);
  };
  const cancel = () => setDrag(null);
  const onKeyDown = (e) => {
    if (!seekable) return;
    const step = e.shiftKey ? 60 : 5;
    const keys = {
      ArrowLeft: () => seekBy(-step),
      ArrowRight: () => seekBy(step),
      PageDown: () => seekBy(-60),
      PageUp: () => seekBy(60),
      Home: () => restart(),
    };
    const run = keys[e.key];
    if (!run) return;
    e.preventDefault();
    e.stopPropagation();
    run();
  };

  const shown = drag !== null ? drag * dur : pos;
  const played = known ? Math.min(100, (shown / dur) * 100) : 0;
  const posText = formatTime(shown);
  const valueText = live
    ? t('player.live')
    : t('player.position_of', {
        position: approx ? t('player.about_time', { time: posText }) : posText,
        duration: known ? formatTime(dur) : '—',
      });

  return html`
    <div class="tl">
      <span class="tl-time tl-pos">${live ? '' : `${approx && drag === null ? '≈ ' : ''}${posText}`}</span>
      <div class=${`track-hit ${active ? '' : 'disabled'}`}
        onPointerMove=${onPointerMove} onPointerLeave=${() => setHover(null)}
        onPointerDown=${onPointerDown} onPointerUp=${onPointerUp}
        onPointerCancel=${cancel} onLostPointerCapture=${cancel}>
        <div class="track" ref=${track} role="slider" tabindex=${seekable ? 0 : -1}
          aria-label=${t('player.timeline')} aria-disabled=${active ? 'false' : 'true'}
          aria-valuemin="0" aria-valuemax=${known ? Math.round(dur) : 0}
          aria-valuenow=${Math.round(shown ?? 0)} aria-valuetext=${valueText}
          onKeyDown=${onKeyDown}>
          <i class="played" style=${`width:${live ? 100 : played}%`}></i>
          ${known && !live ? html`<em class="knob" style=${`left:${played}%`}></em>` : null}
          ${hover !== null && known && !live
            ? html`<span class="hv" style=${`left:${hover * 100}%`}></span>
                <${Thumb} seconds=${hover * dur} left=${hover * 100} />
                <span class="tip" style=${`left:${hover * 100}%`}>${formatTime(hover * dur)}</span>`
            : null}
        </div>
      </div>
      <span class="tl-time tl-dur">${live ? html`<span class="live-tag">${t('player.live')}</span>` : known ? formatTime(dur) : ''}</span>
    </div>`;
}

/**
 * One volume: `level`/`muted` signals, the slider's name, the mute button's names and a short
 * tag shown before it (two sliders side by side: which is which).
 */
function VolumeSlider({ level, muted: mutedSig, label, mute, unmute, tag, cls }) {
  const muted = mutedSig.value || level.value === 0;
  const buttonLabel = muted ? unmute : mute;
  const value = mutedSig.value ? 0 : level.value;
  return html`
    <span class=${`vol ${cls}`}>
      ${tag ? html`<small class="vol-tag" aria-hidden="true">${tag}</small>` : null}
      <button type="button" class="icon-button" aria-label=${buttonLabel} title=${buttonLabel}
        onClick=${() => (mutedSig.value = !mutedSig.value)}>
        <${Icon} name=${muted ? 'muted' : 'volume'} size=${18} />
      </button>
      <input type="range" min="0" max="1" step="0.05" aria-label=${label} title=${label}
        value=${value} style=${`--fill:${value * 100}%`}
        onInput=${(e) => {
          level.value = Number(e.currentTarget.value);
          mutedSig.value = false;
        }} />
    </span>`;
}

/** On air's volume, for the air's own window too. */
export function AirVolume() {
  return html`<${VolumeSlider} level=${previewVolume} muted=${previewMuted} cls="vol-air"
    label=${t('player.volume')} mute=${t('player.mute')} unmute=${t('player.unmute')} />`;
}

function Volume() {
  // With the Preview in this window: two sliders (Preview, On air), each with its tag.
  const both = monitorRunning() || monitorState.value !== 'off';
  if (!both) return html`<${AirVolume} />`;
  return html`
    <span class="vols">
      <${VolumeSlider} level=${monitorVolume} muted=${monitorMuted} cls="vol-preview" tag=${t('volume.preview_tag')}
        label=${t('volume.preview')} mute=${t('monitor.mute')} unmute=${t('monitor.unmute')} />
      <${VolumeSlider} level=${previewVolume} muted=${previewMuted} cls="vol-air" tag=${t('volume.air_tag')}
        label=${t('player.volume')} mute=${t('player.mute')} unmute=${t('player.unmute')} />
    </span>`;
}

const NET_KEYS = {
  online: 'net.online',
  connecting: 'net.connecting',
  offline: 'net.offline',
  manual: 'net.manual',
  local: 'net.local',
};

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const field = document.createElement('textarea');
    field.value = text;
    field.style.cssText = 'position:fixed;opacity:0';
    document.body.append(field);
    field.select();
    document.execCommand('copy');
    field.remove();
  }
}

function CopyLink() {
  const [copied, setCopied] = useState(false);
  const link = channelLink.value;
  const copy = async () => {
    if (!link) return;
    await copyText(link);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return html`
    <button type="button" class="btn copy-link" disabled=${!link} title=${t('player.copy_link_label')}
      onClick=${copy}>
      <span aria-live="polite">${copied ? t('player.copied') : t('player.copy_link')}</span>
    </button>`;
}

// ---------- The bar ----------

export function PlayerBar({ onShortcuts }) {
  const ch = channel.value;
  const c = control.value;
  const current = ch?.current ?? null;
  const status = ch?.status ?? 'idle';
  const previewPos = shownPreviewPosition(c);
  const pos = previewPos ?? shownPosition(c, controlAt.value, status);
  const dur = c?.durationS ?? current?.durationS ?? null;
  const live = Boolean(c?.isLive ?? current?.isLive);
  const { canSeek, manual, canToggle } = abilities.value;
  const net = netStatus.value;
  const videoLink = ch?.playing?.[0]?.sourceUrl ?? null;

  const menuItems = () => [
    { key: 'player.restart', icon: 'prev', disabled: !canSeek, action: restart },
    { key: 'player.next', icon: 'next', disabled: !current, action: skip },
    {
      key: 'player.copy_video_link',
      icon: 'copy',
      disabled: !videoLink,
      action: () => copyText(videoLink).catch((e) => showToast(e.message)),
    },
    ...(onShortcuts ? ['separator', { key: 'player.shortcuts', icon: 'keyboard', action: onShortcuts }] : []),
  ];

  const ctl = (icon, labelKey, action, enabled, cls = 'ctl-button') => html`
    <button type="button" class=${cls} aria-label=${t(labelKey)} title=${t(labelKey)}
      disabled=${!enabled} onClick=${action}><${Icon} name=${icon} size=${20} /></button>`;

  return html`
    <section class="player" aria-label=${t('player.label')} ...${contextMenuHandlers('player.menu', menuItems)}>
      <${NowPlaying} current=${current} c=${c} />
      <div class="mid">
        <div class="ctl">
          <button type="button" class="sm" disabled=${!canSeek} aria-label=${t('player.back_10_label')}
            title=${t('player.back_10_label')} onClick=${() => seekBy(-10)}>${t('player.back_10')}</button>
          ${ctl('prev', 'player.restart', restart, canSeek)}
          <button type="button" class="play" disabled=${!canToggle}
            aria-label=${manual ? t('player.resume') : t('player.pause')}
            title=${manual ? t('player.resume') : t('player.pause')} onClick=${togglePause}>
            <${Icon} name=${manual ? 'play' : 'pause'} size=${20} />
          </button>
          ${ctl('next', 'player.next', skip, Boolean(current))}
          <button type="button" class="sm" disabled=${!canSeek} aria-label=${t('player.forward_10_label')}
            title=${t('player.forward_10_label')} onClick=${() => seekBy(10)}>${t('player.forward_10')}</button>
        </div>
        <${Timeline} pos=${current ? pos : null} dur=${current ? dur : null}
          approx=${previewPos === null && Boolean(c?.positionApprox)} seekable=${canSeek} live=${live && Boolean(current)} />
      </div>
      <div class="right">
        <${Volume} />
        ${net ? html`<span class=${`pill net-${net}`} role="status"><span class="dot" aria-hidden="true"></span>${t(NET_KEYS[net])}</span>` : null}
        <${CopyLink} />
      </div>
    </section>`;
}
