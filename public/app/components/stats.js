// "Stats for nerds": an overlay on the preview with the technical numbers of the channel
// (the engine's stats route) and of the preview itself. It reads the route every second, and
// only while it is open; "Copy" puts all of it on the clipboard as text.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { api } from '../api.js';
import { lang, t, tn } from '../i18n.js';
import { Icon } from '../icons.js';
import { control, showToast, tunnel } from '../store.js';
import { copyText, formatTime } from './playerbar.js';

const STATS = '/api/admin/channels/main/stats';
const EVERY_MS = 1000;

/** Whether the overlay is open. */
export const statsOpen = signal(false);
/** The preview's own numbers: `{bufferS, dropped}` (the delay is `previewDelayS`). */
export const previewStats = signal({ bufferS: null, dropped: null });
/** A preview is on the screen (the key I does nothing without one). */
export const previewShown = signal(false);

export function toggleStats() {
  if (previewShown.value) statsOpen.value = !statsOpen.value;
}

// ---------- Formatting ----------

const num = (n, digits = 1) =>
  new Intl.NumberFormat(lang.value, { maximumFractionDigits: digits }).format(n);
const present = (v) => v !== null && v !== undefined && v !== '';

/** `—` for the parts that are missing, `—` alone when all are. */
function join(parts) {
  if (parts.every((p) => !present(p))) return '—';
  return parts.map((p) => (present(p) ? p : '—')).join(' · ');
}

/** `82.4` → `1:22.4`. */
function position(s) {
  if (!Number.isFinite(s)) return null;
  return `${formatTime(s)}.${Math.floor((s % 1) * 10)}`;
}

function duration(s) {
  if (!Number.isFinite(s)) return null;
  const total = Math.floor(s);
  if (total >= 3600) {
    return t('stats.v_hours_minutes', { h: Math.floor(total / 3600), m: Math.floor((total % 3600) / 60) });
  }
  if (total >= 60) return t('stats.v_minutes', { m: Math.floor(total / 60) });
  return t('stats.v_seconds', { s: total });
}

const seconds = (s, key) => (Number.isFinite(s) ? t(key, { seconds: num(s) }) : null);

function host(source) {
  try {
    return new URL(source).hostname.replace(/^www\./, '');
  } catch {
    return source;
  }
}

function sentRate(bytesPerS) {
  if (!Number.isFinite(bytesPerS)) return null;
  return bytesPerS >= 1e6
    ? t('stats.v_mb_s', { n: num(bytesPerS / 1e6) })
    : t('stats.v_kb_s', { n: num(bytesPerS / 1e3) });
}

const REASON_KEYS = {
  manual: 'stats.reason_manual',
  tunnel: 'stats.reason_tunnel',
  no_viewers: 'stats.reason_no_viewers',
};
const NEXT_KEYS = {
  preparing: 'stats.v_next_preparing',
  ready: 'stats.v_next_ready',
  failed: 'stats.v_next_failed',
};
const TUNNEL_KEYS = {
  connecting: 'stats.tunnel_connecting',
  online: 'stats.tunnel_online',
  reconnecting: 'stats.tunnel_reconnecting',
  error: 'stats.tunnel_error',
};

// ---------- The rows ----------

/**
 * The groups of rows to show: `{titleKey, rows: [{labelKey, text, tone?, bars?}]}`. Missing
 * values (`null`) are `—`. `data` is the stats route's answer (null before the first one).
 */
export function statGroups(data, own) {
  const d = data ?? {};
  const { onAir, hls, next, ffmpeg, viewers, tunnel: tun } = d;
  const probe = onAir?.probe;
  const reasons = control.value?.pauseReasons;
  const addr = tunnel.value?.url ?? null;
  const readyS = onAir && hls ? onAir.readySegments * hls.segmentS : null;

  const row = (labelKey, parts, extra = {}) => ({ labelKey, text: join(parts), ...extra });
  return [
    {
      titleKey: 'stats.group_on_air',
      rows: [
        row('stats.row_video', [onAir?.title, onAir ? host(onAir.source) : null]),
        row('stats.row_position', [
          onAir ? `${position(onAir.positionS)} / ${position(probe?.durationS) ?? '—'}` : null,
          onAir ? t('stats.v_attempt', { n: onAir.attempt }) : null,
        ]),
        row('stats.row_format', probe
          ? [
              Number.isFinite(probe.height) ? `${probe.height}p` : null,
              Number.isFinite(probe.fps) ? t('stats.v_fps', { fps: num(probe.fps) }) : null,
              probe.codec,
              probe.pixFmt,
              probe.hasAudio ? t('stats.v_audio') : t('stats.v_no_audio'),
            ]
          : [null]),
        row('stats.row_mode', [
          onAir?.mode ? t(onAir.mode === 'copy' ? 'stats.v_mode_copy' : 'stats.v_mode_transcode') : null,
        ]),
        row('stats.row_buffer', [
          onAir ? tn('stats.v_ready', onAir.readySegments, { seconds: Number.isFinite(readyS) ? num(readyS) : '—' }) : null,
        ]),
      ],
    },
    {
      titleKey: 'stats.group_hls',
      rows: [
        row('stats.row_sequence', hls
          ? [
              `#${num(hls.sequence, 0)}`,
              t('stats.v_window', { n: hls.window }),
              t('stats.v_segment', { seconds: num(hls.segmentS, 2) }),
              t('stats.v_discontinuities', { n: hls.discontinuities }),
            ]
          : [null]),
        row('stats.row_timestamps', hls
          ? [
              hls.lastPts === null ? null : t('stats.v_pts', { seconds: num(hls.lastPts, 0) }),
              t(hls.dtsContinuous ? 'stats.v_continuous' : 'stats.v_not_continuous'),
            ]
          : [null], { tone: hls ? (hls.dtsContinuous ? 'ok' : 'warn') : undefined }),
        row('stats.row_pause', [
          reasons?.length ? reasons.map((r) => (REASON_KEYS[r] ? t(REASON_KEYS[r]) : r)).join(', ') : control.value ? t('stats.v_no') : null,
        ]),
      ],
    },
    {
      titleKey: 'stats.group_next',
      rows: [
        row('stats.row_video', [next?.title]),
        row('stats.row_next_state', [
          next ? t(NEXT_KEYS[next.state] ?? 'stats.v_next_failed') : null,
          next ? tn('stats.v_ready', next.readySegments, { seconds: hls ? num(next.readySegments * hls.segmentS) : '—' }) : null,
        ]),
      ],
    },
    {
      titleKey: 'stats.group_ffmpeg',
      rows: [
        row('stats.row_process', ffmpeg
          ? [
              t('stats.v_pid', { pid: ffmpeg.pid }),
              Number.isFinite(ffmpeg.speed) ? t('stats.v_speed', { speed: num(ffmpeg.speed) }) : null,
              Number.isFinite(ffmpeg.cpuPercent) ? t('stats.v_cpu', { percent: num(ffmpeg.cpuPercent, 0) }) : null,
              Number.isFinite(ffmpeg.rssBytes) ? t('stats.v_ram', { mb: num(ffmpeg.rssBytes / 1048576, 0) }) : null,
            ]
          : [null]),
        row('stats.row_encoder', [onAir ? (onAir.encoder ?? null) : null]),
      ],
    },
    {
      titleKey: 'stats.group_viewers',
      rows: [
        row('stats.row_watching', [viewers ? t('stats.v_about', { n: viewers.watching }) : null]),
        row('stats.row_fetches', viewers
          ? [t('stats.v_per_min', { n: viewers.fetchesPerMin }), sentRate(viewers.bytesPerS)]
          : [null], { bars: viewers?.history ?? null }),
      ],
    },
    {
      titleKey: 'stats.group_tunnel',
      rows: [
        row('stats.row_tunnel_state', tun
          ? [
              t(TUNNEL_KEYS[tun.state] ?? 'stats.tunnel_error'),
              tun.onlineS === null ? null : t('stats.v_online_for', { time: duration(tun.onlineS) }),
            ]
          : [null], { tone: tun ? (tun.state === 'online' ? 'ok' : 'warn') : undefined }),
        row('stats.row_connections', tun
          ? [
              `${t('stats.v_connections', { n: tun.connections })}${tun.locations.length ? ` (${tun.locations.map((l) => l.toUpperCase()).join(', ')})` : ''}`,
              t('stats.v_reconnects', { n: tun.reconnects }),
            ]
          : [null]),
        row('stats.row_address', [tun ? addr : null]),
      ],
    },
    {
      titleKey: 'stats.group_preview',
      rows: [
        row('stats.row_delay', [
          seconds(own.delayS, 'stats.v_behind'),
          seconds(own.bufferS, 'stats.v_buffered'),
          Number.isFinite(own.dropped) ? t('stats.v_dropped', { n: own.dropped }) : null,
        ]),
      ],
    },
  ];
}

/** All the groups as plain text (for "Copy"). */
export function statsText(groups) {
  return groups
    .map((g) => `${t(g.titleKey)}\n${g.rows.map((r) => `  ${t(r.labelKey)}: ${r.text}${r.bars ? ` [${r.bars.join(' ')}]` : ''}`).join('\n')}`)
    .join('\n\n');
}

// ---------- The overlay ----------

function Bars({ values }) {
  const max = Math.max(1, ...values);
  return html`<span class="stats-bars" aria-hidden="true">${values.map(
    (v) => html`<i style=${`height:${Math.max(v > 0 ? 8 : 3, Math.round((v / max) * 100))}%`}></i>`,
  )}</span>`;
}

/** Reads the stats route once a second while mounted (never two reads at once). */
function useStats() {
  const [data, setData] = useState(null);
  useEffect(() => {
    let alive = true;
    let timer = 0;
    const read = async () => {
      let again = true;
      try {
        const answer = await api('GET', STATS);
        if (alive) setData(answer);
      } catch (e) {
        again = e.status !== 401; // no session: nothing to poll
      }
      if (alive && again) timer = setTimeout(read, EVERY_MS);
    };
    read();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, []);
  return data;
}

export function StatsOverlay({ delayS }) {
  const data = useStats();
  const [copied, setCopied] = useState(false);
  const groups = statGroups(data, { delayS, ...previewStats.value });
  const timer = useRef(0);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = async () => {
    try {
      await copyText(statsText(groups));
    } catch (e) {
      showToast(e.message);
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
  };
  return html`
    <section class="stats" role="region" aria-label=${t('stats.title')}>
      <div class="stats-head">
        <b>${t('stats.title')}</b>
        <span class="stats-actions">
          <button type="button" class="stats-copy" onClick=${copy}><span aria-live="polite">${copied ? t('player.copied') : t('stats.copy')}</span></button>
          <button type="button" class="icon-button" aria-label=${t('ui.close')} title=${t('ui.close')}
            onClick=${() => (statsOpen.value = false)}><${Icon} name="close" size=${14} /></button>
        </span>
      </div>
      ${groups.map(
        (g) => html`
          <h5>${t(g.titleKey)}</h5>
          ${g.rows.map(
            (r) => html`
              <div class="stats-row">
                <span>${t(r.labelKey)}</span>
                <span class=${r.tone ?? ''}>${r.text}${r.bars ? html`<${Bars} values=${r.bars} />` : null}</span>
              </div>`,
          )}`,
      )}
    </section>`;
}
