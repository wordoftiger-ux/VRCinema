// Dialogs: one at a time, modal. Focus moves into the dialog and stays there (Tab cycles),
// Esc or the close button closes it, and focus returns to where it was. Title and body are
// rendered from functions, so an open dialog follows a language change.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { Icon } from '../icons.js';

/** The open dialog: `{title, body, buttons, opener}` or null. */
export const dialog = signal(null);

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * Opens a dialog. `title` and `body` are functions returning text / elements; `buttons`:
 * `{key, primary?, action?}` (a button without `action` only closes).
 */
export function openDialog({ title, body, buttons = [] }) {
  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  dialog.value = { title, body, buttons, opener };
}

export function closeDialog() {
  const open = dialog.value;
  if (!open) return;
  dialog.value = null;
  if (open.opener?.isConnected) open.opener.focus();
}

export function DialogLayer() {
  const open = dialog.value;
  const ref = useRef(null);

  useEffect(() => {
    if (!open) return;
    const box = ref.current;
    (box.querySelector('[data-autofocus]') ?? box.querySelector(FOCUSABLE) ?? box).focus();
  }, [open]);

  if (!open) return null;

  const onKeyDown = (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeDialog();
      return;
    }
    if (e.key === 'Tab') {
      const all = [...ref.current.querySelectorAll(FOCUSABLE)];
      if (!all.length) return;
      const first = all[0];
      const last = all[all.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
    // Other keys stay in the dialog: the window's shortcuts do not act behind it.
    e.stopPropagation();
  };

  const press = (button) => {
    closeDialog();
    button.action?.();
  };

  return html`
    <div class="dialog-backdrop" onPointerDown=${(e) => e.target === e.currentTarget && closeDialog()}>
      <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="dialog-title" tabindex="-1"
        ref=${ref} onKeyDown=${onKeyDown}>
        <header class="dialog-head">
          <h2 id="dialog-title">${open.title()}</h2>
          <button type="button" class="icon-button" aria-label=${t('ui.close')} title=${t('ui.close')}
            onClick=${closeDialog}><${Icon} name="close" size=${18} /></button>
        </header>
        <div class="dialog-body">${open.body()}</div>
        ${open.buttons.length
          ? html`<footer class="dialog-foot">
              ${open.buttons.map(
                (b, i) => html`<button type="button" class=${b.primary ? 'btn pri' : 'btn'}
                  data-autofocus=${i === 0 ? '' : undefined} onClick=${() => press(b)}>${t(b.key)}</button>`,
              )}
            </footer>`
          : null}
      </div>
    </div>`;
}
