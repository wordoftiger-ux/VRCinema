// The request page (`/`): what is playing, the queue, and a form to add a video. Phones get
// three tabs at the bottom (Now / Queue / Add); from about 900 px wide two columns (Now on the
// left, the queue on the right) and "Add a film" in a side drawer. Starts in the browser's
// language (the first of navigator.languages that the program has, else English); the
// viewer's own choice is kept in this browser.

import { render } from '/static/vendor/preact.mjs';
import { html } from '../html.js';
import { LANGS, browserLang, chosenLang, lang, loadLang, rememberLang, t, tn } from '../i18n.js';
import { NowPlaying } from './nowplaying.js';
import { QueueView } from './queueview.js';
import { RequestForm } from './requestform.js';
import { Drawer } from './drawer.js';
import { HowTo } from './howto.js';
import { channel, drawerOpen, requestDone, startPolling, tab, toast, unreachable, wide } from './store.js';

// A language is named in itself (the same text in every dictionary), so a viewer who cannot read
// the current language can still find theirs.
const LANGUAGE_NAME_KEYS = {
  en: 'settings.language_name.en',
  pl: 'settings.language_name.pl',
  es: 'settings.language_name.es',
  de: 'settings.language_name.de',
  fr: 'settings.language_name.fr',
  ja: 'settings.language_name.ja',
};

function chooseLanguage(code) {
  rememberLang(code);
  loadLang(code);
}

function LanguageSwitch() {
  return html`
    <label class="v-lang">
      <span class="sr-only">${t('viewer.language')}</span>
      <select class="sel" value=${lang.value} onChange=${(e) => chooseLanguage(e.currentTarget.value)}>
        ${LANGS.map((code) => html`<option value=${code} selected=${code === lang.value}>${t(LANGUAGE_NAME_KEYS[code])}</option>`)}
      </select>
    </label>`;
}

const TABS = [
  { id: 'now', key: 'viewer.tab_now', icon: 'M8 5v14l11-7z' },
  { id: 'queue', key: 'viewer.tab_queue', icon: 'M4 6h16v2H4zm0 5h16v2H4zm0 5h16v2H4z' },
  { id: 'add', key: 'viewer.tab_add', icon: 'M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6z' },
];

function TabBar({ count }) {
  return html`
    <nav class="v-tabs" aria-label=${t('viewer.tabs')}>
      ${TABS.map(
        (item) => html`
          <button type="button" class=${tab.value === item.id ? 'v-tab on' : 'v-tab'} data-tab=${item.id}
            aria-current=${tab.value === item.id ? 'page' : undefined} onClick=${() => (tab.value = item.id)}>
            <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true" focusable="false"><path fill="currentColor" d=${item.icon} /></svg>
            <span>${t(item.key)}</span>
            ${item.id === 'queue' && count > 0 ? html`<b class="v-badge" aria-label=${tn('queue.count', count)}>${count}</b>` : null}
          </button>`,
      )}
    </nav>`;
}

function App() {
  const state = channel.value;
  const count = state?.queue?.length ?? 0;
  const isWide = wide.value;
  document.title = t('viewer.title');
  return html`
    <main class=${isWide ? 'v-page v-wide' : 'v-page'}>
      <header class="v-head">
        <h1>${t('viewer.title')}</h1>
        ${isWide
          ? html`<button type="button" class="btn pri v-addfilm" data-action="add-film" onClick=${() => (drawerOpen.value = true)}>
              + ${t('viewer.add_film')}</button>`
          : null}
        <${LanguageSwitch} />
      </header>
      ${unreachable.value ? html`<p class="v-offline" role="status">${t('viewer.offline')}</p>` : null}
      ${isWide
        ? html`
            <div class="v-cols">
              <div class="v-col-now"><${NowPlaying} state=${state} /><${HowTo} /></div>
              <div class="v-col-queue"><${QueueView} state=${state} /></div>
            </div>
            ${drawerOpen.value ? html`<${Drawer}><${RequestForm} bare onDone=${requestDone} /><//>` : null}`
        : html`
            <div class="v-view" data-view=${tab.value}>
              ${tab.value === 'now' ? html`<${NowPlaying} state=${state} /><${HowTo} />` : null}
              ${tab.value === 'queue' ? html`<${QueueView} state=${state} />` : null}
              ${tab.value === 'add' ? html`<${RequestForm} onDone=${requestDone} />` : null}
            </div>
            <${TabBar} count=${count} />`}
      <div class="v-toast-slot" role="status" aria-live="polite">
        ${toast.value ? html`<p class="v-toast">${toast.value.text}</p>` : null}
      </div>
    </main>`;
}

await loadLang(chosenLang() ?? browserLang());
render(html`<${App} />`, document.getElementById('root'));
startPolling();
