// Broadcasting like TV, in the queue (spec 2026-11-06-broadcast-design): the start and end
// points of a video ("Trim…"), a start time ("Start at…", the stream counts down to it), the
// marks of both on a queue row, and how ready the next video is.

import { signal } from '/static/vendor/signals.mjs';
import { html } from '../html.js';
import { t, videoTitle } from '../i18n.js';
import { closeDialog, openDialog } from './dialog.js';
import { formatTime } from './playerbar.js';
import { previewPosition } from './monitor.js';
import { channel, control, scheduleItem, showToast, trimItem } from '../store.js';

/** `1:30` / `1:02:30` / `90` → seconds; `''` → null; anything else → NaN. */
export function parseTime(text) {
  const s = String(text ?? '').trim().replace(',', '.');
  if (!s) return null;
  if (!/^\d+(\.\d+)?$|^\d+:\d{1,2}(\.\d+)?$|^\d+:\d{1,2}:\d{1,2}(\.\d+)?$/.test(s)) return NaN;
  const parts = s.split(':').map(Number);
  if (parts.slice(1).some((p) => p >= 60)) return NaN;
  return parts.reduce((total, p) => total * 60 + p, 0);
}

/** The next time `HH:MM` (local) comes: today, or tomorrow when it has passed. */
export function nextOccurrence(hhmm, now = new Date()) {
  const m = /^(\d{1,2}):(\d{2})$/.exec(String(hhmm ?? '').trim());
  if (!m) return null;
  const at = new Date(now);
  at.setHours(Number(m[1]), Number(m[2]), 0, 0);
  if (at.getTime() <= now.getTime()) at.setDate(at.getDate() + 1);
  return at.getTime();
}

/** `20:30` in the interface's time format. */
export function clockTime(ms) {
  return new Intl.DateTimeFormat(document.documentElement.lang || undefined, { hour: '2-digit', minute: '2-digit' }).format(new Date(ms));
}

// ---------- Trim ----------

const trimIn = signal('');
const trimOut = signal('');

function TimeField({ name, labelKey, value, placeholder, item }) {
  const usable = item.id === control.value?.itemId && previewPosition() !== null;
  return html`
    <label class="dialog-field">
      <span>${t(labelKey)}</span>
      <span class="dialog-field-row">
        <input type="text" class="txt" name=${name} inputmode="decimal" autocomplete="off" placeholder=${placeholder}
          value=${value.value} onInput=${(e) => (value.value = e.currentTarget.value)} />
        ${usable
          ? html`<button type="button" class="btn btn-small" data-use-preview=${name}
              onClick=${() => (value.value = formatTime(previewPosition()))}>${t('trim.use_preview')}</button>`
          : null}
      </span>
    </label>`;
}

/** "Trim…": where the video starts and ends on air. */
export function openTrim(item) {
  trimIn.value = Number.isFinite(item.inS) ? formatTime(item.inS) : '';
  trimOut.value = Number.isFinite(item.outS) ? formatTime(item.outS) : '';
  const save = () => {
    const inS = parseTime(trimIn.value);
    const outS = parseTime(trimOut.value);
    if (Number.isNaN(inS) || Number.isNaN(outS)) {
      showToast(t('trim.bad_time'));
      return;
    }
    trimItem(item.id, inS, outS);
  };
  openDialog({
    title: () => t('trim.title'),
    body: () => html`
      <p class="dialog-intro">${videoTitle(item.title)}</p>
      <div class="dialog-fields" data-dialog="trim">
        <${TimeField} name="inS" labelKey="trim.in" value=${trimIn} placeholder="0:00" item=${item} />
        <${TimeField} name="outS" labelKey="trim.out" value=${trimOut}
          placeholder=${Number.isFinite(item.durationS) ? formatTime(item.durationS) : ''} item=${item} />
      </div>
      <p class="dialog-note">${t('trim.hint')}</p>`,
    buttons: [
      { key: 'settings.save', primary: true, action: save },
      ...(Number.isFinite(item.inS) || Number.isFinite(item.outS)
        ? [{ key: 'trim.clear', action: () => trimItem(item.id, null, null) }]
        : []),
      { key: 'settings.cancel' },
    ],
  });
}

// ---------- Start at ----------

const scheduleAt = signal('');
const QUICK_MINUTES = [1, 2, 5, 10, 15];

/** "Start at…": a time today or tomorrow, or in a few minutes; the stream counts down to it. */
export function openSchedule(item) {
  scheduleAt.value = Number.isFinite(item.startAt)
    ? new Date(item.startAt).toTimeString().slice(0, 5)
    : '';
  const save = () => {
    const startAt = nextOccurrence(scheduleAt.value);
    if (startAt === null) {
      showToast(t('schedule.bad_time'));
      return;
    }
    scheduleItem(item.id, { startAt });
  };
  const quick = (minutes) => {
    closeDialog({ dismissed: false });
    scheduleItem(item.id, { afterS: minutes * 60 });
  };
  openDialog({
    title: () => t('schedule.title'),
    body: () => html`
      <p class="dialog-intro">${videoTitle(item.title)}</p>
      <div class="dialog-fields" data-dialog="schedule">
        <label class="dialog-field"><span>${t('schedule.at')}</span>
          <input type="time" class="txt" name="startAt" value=${scheduleAt.value}
            onInput=${(e) => (scheduleAt.value = e.currentTarget.value)} /></label>
        <div class="dialog-field"><span>${t('schedule.in')}</span>
          <span class="dialog-field-row">
            ${QUICK_MINUTES.map(
              (m) => html`<button type="button" class="btn btn-small" data-after-min=${m} onClick=${() => quick(m)}>
                ${t('schedule.minutes', { minutes: m })}</button>`,
            )}
          </span>
        </div>
      </div>
      <p class="dialog-note">${t('schedule.hint')}</p>`,
    buttons: [
      { key: 'settings.save', primary: true, action: save },
      ...(Number.isFinite(item.startAt) ? [{ key: 'schedule.clear', action: () => scheduleItem(item.id, {}) }] : []),
      { key: 'settings.cancel' },
    ],
  });
}

// ---------- Marks on a row ----------

const READY_KEYS = {
  resolving: 'queue.next_preparing',
  resolved: 'queue.next_preparing',
  preparing: 'queue.next_preparing',
  ready: 'queue.next_ready',
  failed: 'queue.next_failed',
};

/** The start time, the trim and (for the next video) how ready it is. */
export function RowMarks({ item }) {
  const marks = [];
  if (Number.isFinite(item.startAt)) {
    marks.push(html`<span class="qi-mark mark-time" data-mark="start" title=${t('queue.starts_at_hint')}>
      ${t('queue.starts_at', { time: clockTime(item.startAt) })}</span>`);
  }
  if (Number.isFinite(item.inS) || Number.isFinite(item.outS)) {
    const from = formatTime(item.inS ?? 0);
    const to = Number.isFinite(item.outS) ? formatTime(item.outS) : Number.isFinite(item.durationS) ? formatTime(item.durationS) : '…';
    marks.push(html`<span class="qi-mark mark-trim" data-mark="trim" title=${t('queue.trimmed_hint')}>${from}–${to}</span>`);
  }
  const next = channel.value?.next;
  if (next && next.itemId === item.id && READY_KEYS[next.state]) {
    marks.push(html`<span class=${`qi-mark mark-ready ready-${next.state}`} data-mark="ready" role="status">${t(READY_KEYS[next.state])}</span>`);
  }
  return marks.length ? html`<span class="qi-marks">${marks}</span>` : null;
}

/** "Starts in 2:30" for the player bar while the stream counts down (null otherwise). */
export function countdownText(queueItems) {
  const head = queueItems?.[0];
  if (channel.value?.status !== 'countdown' || !head || !Number.isFinite(head.startAt)) return null;
  const left = Math.max(0, Math.ceil((head.startAt - Date.now()) / 1000));
  return { title: head.title, text: t('player.countdown', { time: formatTime(left) }) };
}
