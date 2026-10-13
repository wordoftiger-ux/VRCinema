// The History view: what has already been on the channel, with its outcome, and the buttons
// that put a video on air again or back in the queue.

import { useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { lang, t } from '../i18n.js';
import { ItemText, Thumb, isWeb, itemDetails } from '../components/item.js';
import { queueLink } from '../components/addlink.js';
import { history, playNow } from '../store.js';

/**
 * How the video ended: `failed` is an error; `played` with a reason is a skipped one (the
 * owner skipped it, or another video was put on air over it); anything else was played.
 */
export function outcome(item) {
  if (item.status === 'failed') return 'failed';
  return item.error ? 'skipped' : 'played';
}

const STATUS_KEYS = {
  played: 'history.status_played',
  skipped: 'history.status_skipped',
  failed: 'history.status_failed',
};

/** `21:04` for today, `Oct 12, 21:04` for another day; empty when the time is unknown. */
export function finishedText(ms, now = Date.now()) {
  if (!Number.isFinite(ms)) return '';
  const at = new Date(ms);
  const sameDay = at.toDateString() === new Date(now).toDateString();
  const options = sameDay
    ? { hour: '2-digit', minute: '2-digit' }
    : { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' };
  return new Intl.DateTimeFormat(lang.value, options).format(at);
}

function details(item, kind) {
  const parts = [itemDetails(item), finishedText(item.finishedAt)];
  if (kind === 'failed' && item.error) parts.push(item.error);
  return parts.filter(Boolean).join(' · ');
}

function Row({ item }) {
  const [busy, setBusy] = useState(false);
  const kind = outcome(item);
  const again = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await playNow(item.id);
    } finally {
      setBusy(false);
    }
  };
  const addable = isWeb(item);
  return html`
    <li class="qi hist-row" data-id=${item.id} data-outcome=${kind}>
      <${Thumb} id=${item.id} />
      <${ItemText} item=${item} sub=${details(item, kind)} />
      <span class=${`stt s-${kind}`} title=${kind === 'skipped' && item.error ? item.error : undefined}>${t(STATUS_KEYS[kind])}</span>
      <span class="hist-actions">
        <button type="button" class="btn hist-play" disabled=${busy} onClick=${again}>
          ${kind === 'failed' ? t('history.retry') : t('history.play_now')}
        </button>
        <button type="button" class="btn hist-queue" disabled=${!addable}
          title=${addable ? undefined : t('history.to_queue_local')} onClick=${() => queueLink(item.sourceUrl)}>
          ${t('history.to_queue')}
        </button>
      </span>
    </li>`;
}

export function HistoryView() {
  const items = history.value;
  return html`
    <main class="view history" id="view" data-view="history">
      <div class="page">
        <h1>${t('history.title')}</h1>
        <p class="page-sub">${t('history.sub')}</p>
        ${items.length
          ? html`<ul class="qlist hlist">${items.map((item) => html`<${Row} key=${item.id} item=${item} />`)}</ul>`
          : html`<p class="q-empty">${t('history.empty')}</p>`}
      </div>
    </main>`;
}
