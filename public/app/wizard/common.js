// Shared parts of the wizard's steps: the line that says where an option lives in Settings,
// and the buttons under a step (Back, Next, Skip to the end).

import { html } from '../html.js';
import { t } from '../i18n.js';

/** "Settings → <section>" in the program's language (soft hyphens of the rail label dropped). */
export function settingsPlace(section) {
  return `${t('nav.settings').replace(/­/g, '')} → ${t(`settings.nav.${section}`)}`;
}

/** "You can change this later in Settings → …" for one or more sections. */
export function Later({ sections }) {
  const place = sections.map(settingsPlace).join(', ');
  return html`<p class="wz-later" data-later=${sections.join(' ')}>${t('wizard.later', { place })}</p>`;
}

/**
 * The buttons under a step: Back (when there is a step before), Next (primary) and Skip to
 * the end (to the summary). `nav` comes from the wizard: `{back, next, toEnd}`.
 */
export function StepNav({ nav, nextLabel }) {
  return html`
    <div class="wz-actions">
      ${nav.back ? html`<button type="button" class="btn" data-back onClick=${nav.back}>${t('wizard.back')}</button>` : null}
      <button type="button" class="btn pri" data-next onClick=${nav.next}>${nextLabel ?? t('wizard.next')}</button>
      ${nav.toEnd ? html`<button type="button" class="btn wz-to-end" data-to-end onClick=${nav.toEnd}>${t('wizard.to_end')}</button>` : null}
    </div>`;
}
