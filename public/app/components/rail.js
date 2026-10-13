// The left rail with the four views (tabs at the bottom on a phone), and the router: `#/channel`, `#/history`, `#/share`,
// `#/settings/<section>`.

import { signal } from '/static/vendor/signals.mjs';
import { html } from '../html.js';
import { t, tn } from '../i18n.js';
import { Icon } from '../icons.js';
import { channel } from '../store.js';

export const VIEWS = ['channel', 'history', 'share', 'settings'];
export const SETTINGS_SECTIONS = [
  'general',
  'internet',
  'channel',
  'tools',
  'remote',
  'advanced',
  'log',
  'about',
];

/** `#/settings/tools` → `{view: 'settings', section: 'tools'}`; anything unknown is the channel. */
export function parseRoute(hash) {
  const [view, section] = String(hash ?? '')
    .replace(/^#\/?/, '')
    .split('/');
  if (view === 'settings') {
    return { view, section: SETTINGS_SECTIONS.includes(section) ? section : 'general' };
  }
  return { view: VIEWS.includes(view) ? view : 'channel', section: null };
}

export const route = signal(parseRoute(location.hash));

/** A phone-sized screen (spec section 10: below 700 px the rail becomes tabs at the bottom). */
const PHONE = window.matchMedia('(max-width: 699.98px)');
export const narrow = signal(PHONE.matches);
PHONE.addEventListener('change', (e) => (narrow.value = e.matches));
window.addEventListener('hashchange', () => (route.value = parseRoute(location.hash)));

const ITEMS = [
  { view: 'channel', icon: 'channel', key: 'nav.channel' },
  { view: 'history', icon: 'history', key: 'nav.history' },
  { view: 'share', icon: 'share', key: 'nav.share' },
  { view: 'settings', icon: 'settings', key: 'nav.settings' },
];

export function Rail() {
  const current = route.value.view;
  const pending = channel.value?.pending?.length ?? 0;
  return html`
    <nav class="rail" aria-label=${t('nav.label')}>
      ${ITEMS.map(({ view, icon, key }) => {
        const badge = view === 'channel' && pending > 0;
        return html`
          <a href=${`#/${view}`} class=${`rail-item rail-${view}`}
            aria-current=${current === view ? 'page' : undefined}>
            <${Icon} name=${icon} size=${20} />
            <span class="rail-label">${t(key)}</span>
            ${badge
              ? html`<span class="badge" title=${tn('nav.pending', pending)}>
                  <span aria-hidden="true">${pending}</span>
                  <span class="sr-only">${tn('nav.pending', pending)}</span>
                </span>`
              : null}
          </a>`;
      })}
    </nav>`;
}
