// A part of the interface that fails to draw (an error while rendering) shows a message with
// "Try again" (draws it anew) and "Reload" instead of a frozen or empty window; the error goes
// to the console. Wrapped around the view on show (main.js), keyed by the route, so moving to
// another view starts afresh, and around the whole app as the last resort.

import { useErrorBoundary } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';

export function Guard({ children, whole = false }) {
  const [error, reset] = useErrorBoundary((e) => console.error('VRCinema: the interface failed to draw:', e));
  if (!error) return children;
  return html`
    <div class=${whole ? 'crash crash-whole' : 'crash'} role="alert" data-crash>
      <b class="crash-title">${t('ui.crash_title')}</b>
      <p class="crash-hint">${t('ui.crash_hint')}</p>
      <div class="crash-actions">
        <button type="button" class="btn pri" data-crash-retry onClick=${reset}>${t('ui.crash_retry')}</button>
        <button type="button" class="btn" onClick=${() => location.reload()}>${t('ui.crash_reload')}</button>
      </div>
    </div>`;
}
