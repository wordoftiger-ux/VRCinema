// The left rail with the five views (tabs at the bottom on a phone), and the router: `#/channel`, `#/search`,
// `#/search/<type>/<id>[?for=<item>]`, `#/history`, `#/share`, `#/settings/<section>`.

import { signal } from '/static/vendor/signals.mjs';
import { html } from '../html.js';
import { t, tn } from '../i18n.js';
import { Icon } from '../icons.js';
import { channel } from '../store.js';

export const VIEWS = ['channel', 'search', 'history', 'share', 'settings'];
export const SETTINGS_SECTIONS = [
  'general',
  'internet',
  'channel',
  'addons',
  'obs',
  'tools',
  'remote',
  'advanced',
  'log',
  'about',
];

const decoded = (text) => {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
};

/**
 * `#/settings/tools` → `{view: 'settings', section: 'tools'}`; `#/search/series/tt2:1:2?for=14` →
 * `{view: 'search', type: 'series', id: 'tt2:1:2', forItem: 14}` (no type and id: the search page);
 * anything unknown is the channel.
 */
export function parseRoute(hash) {
  const [path, query = ''] = String(hash ?? '')
    .replace(/^#\/?/, '')
    .split('?');
  const [view, a, b] = path.split('/');
  if (view === 'settings') {
    return { view, section: SETTINGS_SECTIONS.includes(a) ? a : 'general' };
  }
  if (view === 'search') {
    const has = (a === 'movie' || a === 'series') && b;
    const forItem = Number(new URLSearchParams(query).get('for')) || null;
    return { view, section: null, type: has ? a : null, id: has ? decoded(b) : null, forItem: has ? forItem : null };
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
  { view: 'search', icon: 'search', key: 'nav.search' },
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
