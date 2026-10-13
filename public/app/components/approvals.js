// "Waiting for approval": videos viewers sent while moderation is on. ✓ lets one into the
// queue, ✕ refuses it (`PATCH /api/admin/items/{id}` with `approve` / `reject`).

import { html } from '../html.js';
import { t, videoTitle } from '../i18n.js';
import { Icon } from '../icons.js';
import { approveItem, rejectItem } from '../store.js';
import { ItemText, Thumb, lengthText } from './item.js';

function Pending({ item }) {
  const who = item.requesterName
    ? t('queue.requested_by', { name: item.requesterName })
    : t('queue.requested_anonymously');
  const named = (key) => t(key, { title: videoTitle(item.title) });
  // "3:52 · requested by Bartek": the length first, as in the queue.
  const sub = [lengthText(item), who].filter(Boolean).join(' · ');
  return html`
    <li class="qi qi-pending" data-id=${item.id}>
      <${Thumb} id=${item.id} />
      <${ItemText} item=${item} sub=${sub} />
      <span class="act">
        <button type="button" class="act-button yes" aria-label=${named('queue.approve_label')}
          title=${t('queue.approve')} onClick=${() => approveItem(item.id)}><${Icon} name="check" size=${14} /></button>
        <button type="button" class="act-button no" aria-label=${named('queue.reject_label')}
          title=${t('queue.reject')} onClick=${() => rejectItem(item.id)}><${Icon} name="close" size=${14} /></button>
      </span>
    </li>`;
}

export function Approvals({ items }) {
  return html`
    <section class="approvals">
      <h3 class="sec">${t('queue.section_pending')}</h3>
      <ul class="qlist">${items.map((item) => html`<${Pending} key=${item.id} item=${item} />`)}</ul>
    </section>`;
}
