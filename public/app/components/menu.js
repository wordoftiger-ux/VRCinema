// Context menus: one open at a time, at the pointer (or under the element for the keyboard).
// Items name their text by dictionary key, so an open menu follows a language change.
// Keyboard: ↑/↓/Home/End move, Enter/Space choose, Esc or Tab close; focus returns to the
// element that opened the menu.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { Icon } from '../icons.js';

/** The open menu: `{x, y, labelKey, items, opener}` or null. */
export const menu = signal(null);

/**
 * Opens a menu. `items`: `{key, icon?, danger?, disabled?, action}` or `'separator'`.
 * `at`: a mouse event (its position) or an element (the menu opens under it).
 */
export function openMenu(at, labelKey, items) {
  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  let x;
  let y;
  if (at instanceof Element) {
    const r = at.getBoundingClientRect();
    x = r.left;
    y = r.bottom + 4;
  } else {
    x = at.clientX;
    y = at.clientY;
  }
  menu.value = { x, y, labelKey, items, opener };
}

export function closeMenu({ restoreFocus = true } = {}) {
  const open = menu.value;
  if (!open) return;
  menu.value = null;
  if (restoreFocus && open.opener?.isConnected) open.opener.focus();
}

// Esc closes the open menu wherever the focus is — also when it is pressed before the menu has
// moved the focus in (its effect runs after the paint) or after the focus left it — and goes to
// nothing else while a menu is open. One listener for the page, so no key is missed between
// the menu's render and its effect.
document.addEventListener(
  'keydown',
  (e) => {
    if (e.key !== 'Escape' || !menu.value) return;
    e.preventDefault();
    e.stopPropagation();
    closeMenu();
  },
  true,
);

/** Opens `items` for the context-menu gesture of an element: right click, Menu key, Shift+F10. */
export function contextMenuHandlers(labelKey, items) {
  return {
    onContextMenu: (e) => {
      e.preventDefault();
      openMenu(e.clientX || e.clientY ? e : e.currentTarget, labelKey, items());
    },
  };
}

export function MenuLayer() {
  const open = menu.value;
  const ref = useRef(null);

  useEffect(() => {
    // Closed before this effect ran (Esc right after the render): Preact still runs the effect
    // ahead of the next render, and focusing the old menu then would lose the restored focus.
    if (!open || menu.value !== open) return undefined;
    const root = ref.current;
    // Keep the menu inside the window.
    const r = root.getBoundingClientRect();
    const left = Math.max(4, Math.min(open.x, window.innerWidth - r.width - 4));
    const top = open.y + r.height > window.innerHeight - 4 ? Math.max(4, open.y - r.height) : open.y;
    root.style.left = `${left}px`;
    root.style.top = `${top}px`;
    root.querySelector('[role=menuitem]:not([aria-disabled=true])')?.focus();
    const away = (e) => {
      if (!root.contains(e.target)) closeMenu({ restoreFocus: false });
    };
    const blur = () => closeMenu({ restoreFocus: false });
    document.addEventListener('pointerdown', away, true);
    window.addEventListener('blur', blur);
    window.addEventListener('resize', blur);
    return () => {
      document.removeEventListener('pointerdown', away, true);
      window.removeEventListener('blur', blur);
      window.removeEventListener('resize', blur);
    };
  }, [open]);

  if (!open) return null;

  const choose = (item) => {
    if (item.disabled) return;
    closeMenu();
    item.action();
  };

  const onKeyDown = (e) => {
    const items = [...ref.current.querySelectorAll('[role=menuitem]:not([aria-disabled=true])')];
    const at = items.indexOf(document.activeElement);
    const move = (i) => items[(i + items.length) % items.length]?.focus();
    switch (e.key) {
      case 'ArrowDown':
        move(at + 1);
        break;
      case 'ArrowUp':
        move(at < 0 ? -1 : at - 1);
        break;
      case 'Home':
        move(0);
        break;
      case 'End':
        move(-1);
        break;
      case 'Tab':
        closeMenu();
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  return html`
    <div class="menu" role="menu" aria-label=${t(open.labelKey)} ref=${ref} onKeyDown=${onKeyDown}
      onContextMenu=${(e) => e.preventDefault()}>
      ${open.items.map((item) =>
        item === 'separator'
          ? html`<div class="menu-sep" role="separator"></div>`
          : html`<button type="button" role="menuitem" tabindex="-1"
              class=${item.danger ? 'menu-item danger' : 'menu-item'}
              aria-disabled=${item.disabled ? 'true' : 'false'}
              onClick=${() => choose(item)}>
              <span class="menu-icon">${item.icon ? html`<${Icon} name=${item.icon} size=${16} />` : null}</span>
              <span class="menu-label">${t(item.key)}</span>
            </button>`,
      )}
    </div>`;
}
