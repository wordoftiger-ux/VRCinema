// Files dropped on the program's window (only there: the window's own drop handler adds them
// to the queue and tells the page with `vrcinema:drag-enter`, `vrcinema:drag-leave` and
// `vrcinema:files-added`; a browser sends none of these). Shows the drop target while files are
// over the window and what came of the drop.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t, tn } from '../i18n.js';
import { refreshPanel, showToast } from '../store.js';

const over = signal(false);

/** The words for a drop's result: `{added, skipped, error}`. */
export function dropMessage({ added = 0, skipped = 0, error = null }) {
  if (error) return { text: error, kind: 'error' };
  const parts = [];
  if (added > 0) parts.push(tn('queue.added_count', added));
  if (skipped > 0) parts.push(tn('drop.skipped', skipped));
  if (!parts.length) parts.push(t('drop.none'));
  return { text: parts.join(' '), kind: added > 0 ? 'info' : 'error' };
}

export function DropZone() {
  useEffect(() => {
    const enter = () => (over.value = true);
    const leave = () => (over.value = false);
    const added = (e) => {
      over.value = false;
      const { text, kind } = dropMessage(e.detail ?? {});
      showToast(text, kind);
      refreshPanel();
    };
    window.addEventListener('vrcinema:drag-enter', enter);
    window.addEventListener('vrcinema:drag-leave', leave);
    window.addEventListener('vrcinema:files-added', added);
    return () => {
      window.removeEventListener('vrcinema:drag-enter', enter);
      window.removeEventListener('vrcinema:drag-leave', leave);
      window.removeEventListener('vrcinema:files-added', added);
    };
  }, []);

  if (!over.value) return null;
  return html`<div class="drop-overlay" aria-hidden="true"><div>${t('drop.hint')}</div></div>`;
}
