// Small formatting helpers of the request page.

import { t } from '../i18n.js';

/** `3:52`, `1:42:10`; empty for an unknown length. */
export function formatTime(seconds) {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return '';
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
}

/** `2 h` / `30 min`: the longest video the channel accepts. */
export function formatLimit(seconds) {
  const hours = seconds / 3600;
  return hours >= 1
    ? t('viewer.limit_hours', { n: +hours.toFixed(1) })
    : t('viewer.limit_minutes', { n: Math.max(1, Math.round(seconds / 60)) });
}
