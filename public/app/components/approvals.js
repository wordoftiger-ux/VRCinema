// "Waiting for approval": videos viewers sent while moderation is on. ✓ lets one into the
// queue, ✕ refuses it (`PATCH /api/admin/items/{id}` with `approve` / `reject`). Videos a
// viewer sent as one list (the engine's `pendingLists`) form a group with ✓ / ✕ for all of
// them (`POST /api/admin/channels/main/lists/{id}`); each video keeps its own buttons.

import { html } from '../html.js';
import { t, tn, videoTitle } from '../i18n.js';
import { Icon } from '../icons.js';
import { approveItem, approveList, channel, rejectItem, rejectList } from '../store.js';
import { ItemText, Thumb, lengthText } from './item.js';

function requester(item) {
  return item.requesterName
    ? t('queue.requested_by', { name: item.requesterName })
    : t('queue.requested_anonymously');
}

function Pending({ item }) {
  const named = (key) => t(key, { title: videoTitle(item.title) });
  // "3:52 · requested by Bartek": the length first, as in the queue.
  const sub = [lengthText(item), requester(item)].filter(Boolean).join(' · ');
  return html`
    <li class="qi qi-pending" data-id=${item.id}>
      <${Thumb} id=${item.id} src=${item.thumb} />
      <${ItemText} item=${item} sub=${sub} />
      <span class="act">
        <button type="button" class="act-button yes" aria-label=${named('queue.approve_label')}
          title=${t('queue.approve')} onClick=${() => approveItem(item.id)}><${Icon} name="check" size=${14} /></button>
        <button type="button" class="act-button no" aria-label=${named('queue.reject_label')}
          title=${t('queue.reject')} onClick=${() => rejectItem(item.id)}><${Icon} name="close" size=${14} /></button>
      </span>
    </li>`;
}

/** A viewer's list: a heading row with ✓ / ✕ for all, then its videos. */
function ListGroup({ list }) {
  const first = list.items[0];
  const sub = [list.title ? `“${videoTitle(list.title)}”` : null, requester(first)].filter(Boolean).join(' · ');
  return html`
    <li class="qi-list" data-list=${list.id}>
      <div class="qi qi-list-head">
        <div class="qi-text">
          <b class="qi-title">${tn('queue.list_heading', list.items.length)}</b>
          <small class="qi-sub">${sub}</small>
        </div>
        <span class="act">
          <button type="button" class="act-button yes" aria-label=${t('queue.approve_list_label')}
            title=${t('queue.approve_list')} onClick=${() => approveList(list)}><${Icon} name="check" size=${14} /></button>
          <button type="button" class="act-button no" aria-label=${t('queue.reject_list_label')}
            title=${t('queue.reject_list')} onClick=${() => rejectList(list)}><${Icon} name="close" size=${14} /></button>
        </span>
      </div>
      <ul class="qlist qi-list-items">${list.items.map((item) => html`<${Pending} key=${item.id} item=${item} />`)}</ul>
    </li>`;
}

/** The waiting items in queue order; a list (two or more still waiting) where its first item is. */
export function groupPending(items, lists) {
  const byId = new Map(items.map((i) => [i.id, i]));
  const groups = (lists ?? [])
    .map((l) => ({ ...l, items: l.itemIds.map((id) => byId.get(id)).filter(Boolean) }))
    .filter((l) => l.items.length >= 2);
  const groupOf = new Map();
  for (const g of groups) for (const i of g.items) groupOf.set(i.id, g);
  const out = [];
  const shown = new Set();
  for (const item of items) {
    const g = groupOf.get(item.id);
    if (!g) out.push({ item });
    else if (!shown.has(g.id)) {
      shown.add(g.id);
      out.push({ list: g });
    }
  }
  return out;
}

export function Approvals({ items }) {
  const rows = groupPending(items, channel.value?.pendingLists);
  return html`
    <section class="approvals">
      <h3 class="sec">${t('queue.section_pending')}</h3>
      <ul class="qlist">${rows.map((r) =>
        r.list
          ? html`<${ListGroup} key=${`list-${r.list.id}`} list=${r.list} />`
          : html`<${Pending} key=${r.item.id} item=${r.item} />`)}</ul>
    </section>`;
}
