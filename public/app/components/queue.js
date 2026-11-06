// The queue panel of the Channel view: header with the count and total length, the link
// field, the videos waiting for approval, and the "Up next" list. The list is reordered by
// dragging a row with the mouse (pointer events: the window's native file-drop handler leaves
// no HTML5 drag and drop to the page) or with Alt+↑/↓; the context menu (right click, Menu
// key, Shift+F10, a long press on a touch screen, or the ⋮ button on a phone) plays a video
// now, moves it up, down or to the top, copies its link, removes it or everything of its
// viewer (asked first; optionally blocking the device). "Clear queue" in the heading removes
// every waiting video; both bulk removals can be undone for a few seconds and touch the video
// on air only when the owner ticks it in the dialog. In the program's window a tip under the
// list says that video files can be dropped on it (a remote panel cannot take a drop).

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { Icon } from '../icons.js';
import { sentences, t, tn, videoTitle } from '../i18n.js';
import { ItemText, Thumb, isWeb } from './item.js';
import { contextMenuHandlers, openMenu } from './menu.js';
import { AddLink } from './addlink.js';
import { Library } from './library.js';
import { Approvals } from './approvals.js';
import { RequesterBadge } from './badge.js';
import { openDialog } from './dialog.js';
import { RowMarks, openSchedule, openTrim } from './broadcast.js';
import { copyText, formatTime } from './playerbar.js';
import {
  access,
  channel,
  clearQueue,
  moveItem,
  playNow,
  queue,
  removeItem,
  removeRequester,
  requesterPreview,
  showToast,
  undoRemoval,
} from '../store.js';
import { errorText } from '../api.js';

/** A pointer must move this far before a press on a row becomes a drag. */
const DRAG_THRESHOLD_PX = 5;
/** Near the top or bottom edge of the list a drag scrolls it. */
const EDGE_PX = 32;
/** A touch held this long on a row opens its menu. */
const LONG_PRESS_MS = 500;

/** How long the "Undo" of a bulk removal stays (the engine keeps it longer). */
const UNDO_SHOW_MS = 8000;

/** Said to screen readers after a move made with the keyboard. */
const announcement = signal('');

/** The last bulk removal while it can be undone here: `{text, undo}` or null. */
const removal = signal(null);
let removalTimer = 0;
/** The ticks of the open bulk-removal dialog. */
const includeOnAir = signal(false);
const alsoBlock = signal(false);

function showRemoval(answer) {
  if (!answer) return;
  clearTimeout(removalTimer);
  const text = sentences(
    tn('queue.removed_count', answer.removed),
    answer.blocked ? t('queue.removed_blocked') : '',
  );
  removal.value = answer.undo == null ? null : { text, undo: answer.undo };
  if (answer.undo == null) showToast(text, 'info');
  else removalTimer = setTimeout(() => (removal.value = null), UNDO_SHOW_MS);
}

async function undoLast() {
  const r = removal.value;
  if (!r) return;
  clearTimeout(removalTimer);
  removal.value = null;
  const answer = await undoRemoval(r.undo);
  if (answer) showToast(tn('queue.restored_count', answer.restored), 'info');
}

/** A video a viewer added (with a nick or a device), not the owner's. */
export const hasRequester = (item) =>
  item.requesterBadge !== 'host' &&
  (Boolean(item.requesterName) || (item.requesterBadge !== null && typeof item.requesterBadge === 'object'));

function OnAirTick({ title }) {
  return html`
    <label class="dialog-check">
      <input type="checkbox" data-include-on-air checked=${includeOnAir.value}
        onChange=${(e) => (includeOnAir.value = e.currentTarget.checked)} />
      <span>${t('queue.include_on_air', { title: videoTitle(title) })}</span>
    </label>`;
}

/** "Clear queue": every waiting video (the queue and the approvals); the one on air only when ticked. */
export function confirmClear() {
  const waiting = queue.value.length + (channel.value?.pending?.length ?? 0);
  const onAir = channel.value?.current ?? null;
  includeOnAir.value = false;
  openDialog({
    title: () => t('queue.clear_title'),
    body: () => html`
      <p class="dialog-intro">${tn('queue.clear_body', waiting)}</p>
      ${onAir ? html`<${OnAirTick} title=${onAir.title} />` : null}
      <p class="dialog-note">${t('queue.undo_hint')}</p>`,
    buttons: [
      { key: 'queue.clear', danger: true, action: async () => showRemoval(await clearQueue(onAir ? includeOnAir.value : false)) },
      { key: 'settings.cancel' },
    ],
  });
}

/** "Remove all from this viewer": asks the engine what would go, then asks the owner. */
async function confirmRemoveRequester(item) {
  let preview;
  try {
    preview = await requesterPreview(item.id);
  } catch (e) {
    showToast(errorText(e));
    return;
  }
  includeOnAir.value = false;
  alsoBlock.value = false;
  const count = preview.queued + preview.pending;
  openDialog({
    title: () => t('queue.remove_requester_title'),
    body: () => html`
      <p class="dialog-who"><${RequesterBadge} badge=${item.requesterBadge} /><b>${preview.name ?? t('queue.anonymous')}</b></p>
      <p class="dialog-intro">${tn('queue.remove_requester_body', count)}</p>
      ${preview.playing ? html`<${OnAirTick} title=${preview.playing.title} />` : null}
      ${preview.canBlock && !preview.blocked
        ? html`<label class="dialog-check">
            <input type="checkbox" data-also-block checked=${alsoBlock.value}
              onChange=${(e) => (alsoBlock.value = e.currentTarget.checked)} />
            <span>${t('queue.also_block')}</span>
          </label>`
        : null}
      ${preview.blocked ? html`<p class="dialog-note">${t('queue.already_blocked')}</p>` : null}
      <p class="dialog-note">${t('queue.undo_hint')}</p>`,
    buttons: [
      {
        key: 'queue.remove_confirm',
        danger: true,
        action: async () =>
          showRemoval(
            await removeRequester(item.id, {
              includePlaying: Boolean(preview.playing) && includeOnAir.value,
              block: preview.canBlock && !preview.blocked && alsoBlock.value,
            }),
          ),
      },
      { key: 'settings.cancel' },
    ],
  });
}

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

function menuFor(item, index, count) {
  return [
    { key: 'queue.play_now', icon: 'play', action: () => playNow(item.id) },
    { key: 'queue.to_top', icon: 'top', disabled: index === 0, action: () => moveItem(item.id, 0) },
    { key: 'queue.move_up', icon: 'up', disabled: index === 0, action: () => moveItem(item.id, index - 1) },
    { key: 'queue.move_down', icon: 'down', disabled: index >= count - 1, action: () => moveItem(item.id, index + 1) },
    {
      key: 'queue.copy_link',
      icon: 'copy',
      disabled: !isWeb(item),
      action: () => copyText(item.sourceUrl).catch((e) => showToast(errorText(e))),
    },
    'separator',
    { key: 'queue.trim', icon: 'trim', disabled: Boolean(item.isLive), action: () => openTrim(item) },
    { key: 'queue.schedule', icon: 'clock', action: () => openSchedule(item) },
    'separator',
    { key: 'queue.remove', icon: 'close', danger: true, action: () => removeItem(item.id) },
    ...(hasRequester(item)
      ? [{ key: 'queue.remove_requester', icon: 'close', danger: true, action: () => confirmRemoveRequester(item) }]
      : []),
  ];
}

function QueueList({ items }) {
  const list = useRef(null);
  const press = useRef(null); // a row pressed with the mouse: {id, x, y, active}
  const touch = useRef(null); // a row held on a touch screen: {x, y, timer}
  const longPressAt = useRef(-Infinity); // when a long press last opened a menu
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

  const endTouch = () => {
    clearTimeout(touch.current?.timer);
    touch.current = null;
  };
  useEffect(() => endTouch, []);

  const onPointerDown = (e, item, index) => {
    if (e.button !== 0 || e.target.closest('button, a, input')) return;
    if (e.pointerType === 'touch') {
      // A touch scrolls the list; held still it opens the row's menu (dragging is for the mouse).
      endTouch();
      const row = e.currentTarget;
      const timer = setTimeout(() => {
        touch.current = null;
        longPressAt.current = performance.now();
        openMenu(row, 'queue.menu', menuFor(item, index, items.length));
      }, LONG_PRESS_MS);
      touch.current = { x: e.clientX, y: e.clientY, timer };
      return;
    }
    press.current = { id: item.id, x: e.clientX, y: e.clientY, active: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e) => {
    const held = touch.current;
    if (held && Math.hypot(e.clientX - held.x, e.clientY - held.y) >= DRAG_THRESHOLD_PX) endTouch();
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
    endTouch();
    const p = press.current;
    press.current = null;
    if (!p) return;
    setDrag(null);
    if (!p.active || !commit) return;
    const from = items.findIndex((i) => i.id === p.id);
    const to = targetFor(p.id, e.clientY);
    if (from >= 0 && to !== from) moveItem(p.id, to);
  };

  // A touch browser may open its own long-press gesture (`contextmenu`) too: whichever comes
  // first opens the menu, the other one is dropped.
  const onContextMenu = (e, item, index) => {
    if (touch.current) endTouch();
    else if (performance.now() - longPressAt.current < LONG_PRESS_MS * 3) {
      e.preventDefault();
      return;
    }
    contextMenuHandlers('queue.menu', () => menuFor(item, index, items.length)).onContextMenu(e);
  };

  const onKeyDown = (e, item, index) => {
    if (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) {
      e.preventDefault();
      openMenu(e.currentTarget, 'queue.menu', menuFor(item, index, items.length));
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
            onPointerDown=${(e) => onPointerDown(e, item, index)} onPointerMove=${onPointerMove}
            onPointerUp=${(e) => finish(e, true)} onPointerCancel=${(e) => finish(e, false)}
            onKeyDown=${(e) => onKeyDown(e, item, index)}
            onContextMenu=${(e) => onContextMenu(e, item, index)}>
            <${Thumb} id=${item.id} />
            <${ItemText} item=${item} />
            <${RowMarks} item=${item} />
            <button type="button" class="icon-button qi-more" aria-label=${t('queue.more', { title: videoTitle(item.title) })}
              title=${t('queue.more_short')} aria-haspopup="menu"
              onClick=${(e) => openMenu(e.currentTarget, 'queue.menu', menuFor(item, index, items.length))}>
              <${Icon} name="more" size=${20} /></button>
          </li>`,
      )}
    </ul>`;
}

// ---------- The panel ----------

export function QueuePanel() {
  const items = queue.value;
  const pending = channel.value?.pending ?? [];
  const { text, title } = summary(items);
  const undo = removal.value;
  return html`
    <aside class="q" aria-labelledby="queue-title">
      <h2 class="q-head"><span id="queue-title">${t('queue.title')}</span><span class="q-count" title=${title}>${text}</span></h2>
      <${AddLink} />
      ${undo
        ? html`<div class="q-undo" role="status">
            <span>${undo.text}</span>
            <button type="button" class="btn" data-undo onClick=${undoLast}>${t('queue.undo')}</button>
          </div>`
        : null}
      <div class="qscroll">
        <${Library} />
        ${pending.length ? html`<${Approvals} items=${pending} />` : null}
        ${pending.length ? html`<h3 class="sec">${t('queue.section_queued')}</h3>` : null}
        ${items.length
          ? html`<${QueueList} items=${items} />`
          : html`<p class="q-empty">${t('queue.empty')}</p>`}
        ${items.length || pending.length
          ? html`<div class="q-tools">
              <button type="button" class="btn q-clear" data-clear-queue onClick=${confirmClear}>${t('queue.clear')}</button>
            </div>`
          : null}
        ${access.value === 'window' ? html`<p class="q-drop-hint" data-drop-hint>${t('queue.drop_hint')}</p>` : null}
      </div>
      <p id="queue-hint" class="sr-only">${t('queue.reorder_hint')}</p>
      <div class="sr-only" role="status" aria-live="polite">${announcement.value}</div>
    </aside>`;
}
