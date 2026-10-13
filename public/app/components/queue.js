// The queue panel of the Channel view: header with the count and total length, the link
// field, the videos waiting for approval, and the "Up next" list. The list is reordered by
// dragging a row (pointer events: the window's native file-drop handler leaves no HTML5 drag
// and drop to the page) or with Alt+↑/↓; the context menu (right click, Menu key, Shift+F10)
// plays a video now, moves it to the top, copies its link or removes it.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t, tn, videoTitle } from '../i18n.js';
import { ItemText, Thumb, isWeb } from './item.js';
import { contextMenuHandlers, openMenu } from './menu.js';
import { AddLink } from './addlink.js';
import { Approvals } from './approvals.js';
import { copyText, formatTime } from './playerbar.js';
import { channel, moveItem, playNow, queue, removeItem, showToast } from '../store.js';
import { errorText } from '../api.js';

/** A pointer must move this far before a press on a row becomes a drag. */
const DRAG_THRESHOLD_PX = 5;
/** Near the top or bottom edge of the list a drag scrolls it. */
const EDGE_PX = 32;

/** Said to screen readers after a move made with the keyboard. */
const announcement = signal('');

function summary(items) {
  let total = 0;
  let partial = false;
  for (const i of items) {
    if (i.isLive || !Number.isFinite(i.durationS)) partial = true;
    else total += i.durationS;
  }
  const count = tn('queue.count', items.length);
  if (!items.length || (partial && total === 0)) return { text: count, title: '' };
  return {
    text: `${count} · ${formatTime(total)}${partial ? '+' : ''}`,
    title: partial ? t('queue.unknown_length') : '',
  };
}

// ---------- The list ----------

function menuFor(item, index) {
  return [
    { key: 'queue.play_now', icon: 'play', action: () => playNow(item.id) },
    { key: 'queue.to_top', icon: 'top', disabled: index === 0, action: () => moveItem(item.id, 0) },
    {
      key: 'queue.copy_link',
      icon: 'copy',
      disabled: !isWeb(item),
      action: () => copyText(item.sourceUrl).catch((e) => showToast(errorText(e))),
    },
    'separator',
    { key: 'queue.remove', icon: 'close', danger: true, action: () => removeItem(item.id) },
  ];
}

function QueueList({ items }) {
  const list = useRef(null);
  const press = useRef(null); // a pressed row: {id, x, y, active}
  const refocus = useRef(null); // the row to give focus back to after a keyboard move
  const [drag, setDrag] = useState(null); // {id, to} while a row is being dragged

  useEffect(() => {
    if (refocus.current === null) return;
    const row = list.current?.querySelector(`li[data-id="${refocus.current}"]`);
    refocus.current = null;
    row?.focus();
  }, [items]);

  const dragging = drag !== null;
  useEffect(() => {
    if (!dragging) return undefined;
    const cancel = (e) => {
      if (e.key !== 'Escape') return;
      e.stopPropagation();
      press.current = null;
      setDrag(null);
    };
    document.addEventListener('keydown', cancel, true);
    return () => document.removeEventListener('keydown', cancel, true);
  }, [dragging]);

  /** Where the dragged row would land: the number of other rows above the pointer. */
  const targetFor = (id, y) => {
    let to = 0;
    for (const row of list.current.querySelectorAll('li[data-id]')) {
      if (Number(row.dataset.id) === id) continue;
      const r = row.getBoundingClientRect();
      if (y > r.top + r.height / 2) to += 1;
    }
    return to;
  };

  const onPointerDown = (e, item) => {
    if (e.button !== 0 || e.target.closest('button, a, input')) return;
    press.current = { id: item.id, x: e.clientX, y: e.clientY, active: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e) => {
    const p = press.current;
    if (!p) return;
    if (!p.active) {
      if (Math.hypot(e.clientX - p.x, e.clientY - p.y) < DRAG_THRESHOLD_PX) return;
      p.active = true;
    }
    const box = list.current.closest('.qscroll');
    const r = box.getBoundingClientRect();
    if (e.clientY < r.top + EDGE_PX) box.scrollTop -= 14;
    else if (e.clientY > r.bottom - EDGE_PX) box.scrollTop += 14;
    setDrag({ id: p.id, to: targetFor(p.id, e.clientY) });
  };
  const finish = (e, commit) => {
    const p = press.current;
    press.current = null;
    if (!p) return;
    setDrag(null);
    if (!p.active || !commit) return;
    const from = items.findIndex((i) => i.id === p.id);
    const to = targetFor(p.id, e.clientY);
    if (from >= 0 && to !== from) moveItem(p.id, to);
  };

  const onKeyDown = (e, item, index) => {
    if (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) {
      e.preventDefault();
      openMenu(e.currentTarget, 'queue.menu', menuFor(item, index));
    } else if (e.altKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
      const to = index + (e.key === 'ArrowUp' ? -1 : 1);
      e.preventDefault();
      if (to < 0 || to >= items.length) return;
      refocus.current = item.id;
      announcement.value = t('queue.moved', {
        title: videoTitle(item.title),
        position: to + 1,
        total: items.length,
      });
      moveItem(item.id, to);
    }
  };

  const others = items.filter((i) => i.id !== drag?.id);
  const marks = new Map();
  if (drag) {
    if (drag.to < others.length) marks.set(others[drag.to].id, 'drop-before');
    else if (others.length) marks.set(others[others.length - 1].id, 'drop-after');
  }

  return html`
    <ul class="qlist" ref=${list}>
      ${items.map(
        (item, index) => html`
          <li key=${item.id} data-id=${item.id} tabindex="0" aria-describedby="queue-hint"
            class=${['qi', 'qi-queued', drag?.id === item.id ? 'dragging' : '', marks.get(item.id) ?? ''].join(' ').trim()}
            onPointerDown=${(e) => onPointerDown(e, item)} onPointerMove=${onPointerMove}
            onPointerUp=${(e) => finish(e, true)} onPointerCancel=${(e) => finish(e, false)}
            onKeyDown=${(e) => onKeyDown(e, item, index)}
            ...${contextMenuHandlers('queue.menu', () => menuFor(item, index))}>
            <${Thumb} id=${item.id} />
            <${ItemText} item=${item} />
          </li>`,
      )}
    </ul>`;
}

// ---------- The panel ----------

export function QueuePanel() {
  const items = queue.value;
  const pending = channel.value?.pending ?? [];
  const { text, title } = summary(items);
  return html`
    <aside class="q" aria-labelledby="queue-title">
      <h2 class="q-head"><span id="queue-title">${t('queue.title')}</span><span class="q-count" title=${title}>${text}</span></h2>
      <${AddLink} />
      <div class="qscroll">
        ${pending.length ? html`<${Approvals} items=${pending} />` : null}
        ${pending.length ? html`<h3 class="sec">${t('queue.section_queued')}</h3>` : null}
        ${items.length
          ? html`<${QueueList} items=${items} />`
          : html`<p class="q-empty">${t('queue.empty')}</p>`}
      </div>
      <p id="queue-hint" class="sr-only">${t('queue.reorder_hint')}</p>
      <div class="sr-only" role="status" aria-live="polite">${announcement.value}</div>
    </aside>`;
}
