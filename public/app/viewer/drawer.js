// The side panel of wide screens ("Add a film"): Esc, the close button or a click on the
// backdrop closes it; Tab stays inside while it is open and the focus goes back to the button
// that opened it.

import { useEffect, useRef } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { drawerOpen } from './store.js';

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]):not([type=hidden]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

function visible(el) {
  return el.getClientRects().length > 0;
}

export function Drawer({ children }) {
  const panel = useRef(null);
  useEffect(() => {
    const opener = document.activeElement;
    const close = () => (drawerOpen.value = false);
    const first = panel.current?.querySelector('input:not([type=hidden]), textarea');
    (first ?? panel.current)?.focus();
    const onKey = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        close();
        return;
      }
      if (e.key !== 'Tab' || !panel.current) return;
      const items = [...panel.current.querySelectorAll(FOCUSABLE)].filter(visible);
      if (!items.length) return;
      const [head, tail] = [items[0], items[items.length - 1]];
      if (!panel.current.contains(document.activeElement)) {
        e.preventDefault();
        head.focus();
      } else if (e.shiftKey && document.activeElement === head) {
        e.preventDefault();
        tail.focus();
      } else if (!e.shiftKey && document.activeElement === tail) {
        e.preventDefault();
        head.focus();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('keydown', onKey);
      if (opener instanceof HTMLElement && document.contains(opener)) opener.focus();
    };
  }, []);
  return html`
    <div class="v-drawer-wrap">
      <div class="v-backdrop" data-action="drawer-backdrop" onClick=${() => (drawerOpen.value = false)}></div>
      <aside class="v-drawer" role="dialog" aria-modal="true" aria-labelledby="v-drawer-h" tabindex="-1" ref=${panel}>
        <div class="v-drawer-head">
          <h2 id="v-drawer-h">${t('viewer.add_film')}</h2>
          <button type="button" class="btn" data-action="drawer-close" aria-label=${t('viewer.close')} title=${t('viewer.close')}
            onClick=${() => (drawerOpen.value = false)}>×</button>
        </div>
        ${children}
      </aside>
    </div>`;
}
