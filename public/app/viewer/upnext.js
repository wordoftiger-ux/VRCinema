// The "Up next" list: title, LIVE tag, length and the requester with their icon.

import { html } from '../html.js';
import { t, tn, videoTitle } from '../i18n.js';
import { RequesterBadge } from '../components/badge.js';
import { formatTime } from './format.js';
import { requesterText } from './nowplaying.js';

function Row({ item }) {
  const title = videoTitle(item.title);
  const length = !item.isLive && Number.isFinite(item.durationS) ? formatTime(item.durationS) : '';
  return html`
    <li class="v-item">
      <div class="v-item-text">
        <b class="v-item-title" title=${title}><span class="v-ellipsis">${title}</span>
          ${item.isLive ? html`<span class="live">${t('player.live')}</span>` : null}</b>
        <small class="v-sub"><${RequesterBadge} badge=${item.requesterBadge} />
          <span class="v-ellipsis">${[length, requesterText(item)].filter(Boolean).join(' · ')}</span></small>
      </div>
    </li>`;
}

export function UpNext({ state }) {
  const queue = state?.queue ?? [];
  return html`
    <section class="v-next" aria-labelledby="v-next-h">
      <h2 id="v-next-h" class="v-eyebrow">${t('queue.title')}
        ${queue.length ? html`<span class="v-count"> · ${tn('queue.count', queue.length)}</span>` : null}</h2>
      ${queue.length
        ? html`<ol class="v-list">${queue.map((item) => html`<${Row} key=${item.id} item=${item} />`)}</ol>`
        : html`<p class="v-empty">${t('viewer.queue_empty')}</p>`}
    </section>`;
}
