// The "Queue" view: every queued film with its picture, length, who added it and when it
// starts (about); my own are marked; "Your requests" lists mine that wait for approval.

import { html } from '../html.js';
import { t, tn, videoTitle } from '../i18n.js';
import { RequesterBadge } from '../components/badge.js';
import { formatStart, formatTime } from './format.js';
import { requesterText } from './nowplaying.js';
import { Pic } from './pic.js';

function Row({ item }) {
  const title = videoTitle(item.title);
  const length = !item.isLive && Number.isFinite(item.durationS) ? formatTime(item.durationS) : '';
  const when = formatStart(item.startsInS);
  return html`
    <li class=${item.mine ? 'v-item v-mine' : 'v-item'} data-mine=${item.mine ? 'yes' : 'no'}>
      <${Pic} src=${item.thumb} seed=${item.id} />
      <div class="v-item-text">
        <b class="v-item-title" title=${title}><span class="v-ellipsis">${title}</span>
          ${item.isLive ? html`<span class="live">${t('player.live')}</span>` : null}
          ${item.mine ? html`<span class="v-chip">${t('viewer.yours')}</span>` : null}</b>
        <small class="v-sub"><${RequesterBadge} badge=${item.requesterBadge} />
          <span class="v-ellipsis">${[length, requesterText(item)].filter(Boolean).join(' · ')}</span></small>
        ${when ? html`<small class="v-eta">${when}</small>` : null}
      </div>
    </li>`;
}

function Pending({ items }) {
  if (!items.length) return null;
  return html`
    <section class="v-requests" aria-labelledby="v-req-h">
      <h2 id="v-req-h" class="v-eyebrow">${t('viewer.your_requests')}</h2>
      <ul class="v-list">
        ${items.map((item) => html`
          <li key=${item.id} class="v-item" data-pending="yes">
            <${Pic} src=${item.thumb} seed=${item.id} />
            <div class="v-item-text">
              <b class="v-item-title" title=${videoTitle(item.title)}><span class="v-ellipsis">${videoTitle(item.title)}</span></b>
              <small class="v-eta">${t('viewer.awaiting')}</small>
            </div>
          </li>`)}
      </ul>
    </section>`;
}

export function QueueView({ state }) {
  const queue = state?.queue ?? [];
  return html`
    <div class="v-queue">
      <section aria-labelledby="v-next-h">
        <h2 id="v-next-h" class="v-eyebrow">${t('viewer.queue_heading')}
          ${queue.length ? html`<span class="v-count"> · ${tn('queue.count', queue.length)}</span>` : null}</h2>
        ${queue.length
          ? html`<ol class="v-list">${queue.map((item) => html`<${Row} key=${item.id} item=${item} />`)}</ol>`
          : html`<p class="v-empty">${t('viewer.queue_empty')}</p>`}
      </section>
      <${Pending} items=${state?.myRequests ?? []} />
    </div>`;
}
