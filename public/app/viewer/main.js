// The request page (`/`): what is playing, the queue, and a form to add a video. Starts in the
// browser's language (the first of navigator.languages that the program has, else English);
// the viewer's own choice is kept in this browser.

import { render } from '/static/vendor/preact.mjs';
import { html } from '../html.js';
import { LANGS, browserLang, chosenLang, lang, loadLang, rememberLang, t } from '../i18n.js';
import { NowPlaying } from './nowplaying.js';
import { UpNext } from './upnext.js';
import { RequestForm } from './requestform.js';
import { HowTo } from './howto.js';
import { channel, startPolling, toast, unreachable } from './store.js';

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

function App() {
  const state = channel.value;
  document.title = t('viewer.title');
  return html`
    <main class="v-page">
      <header class="v-head">
        <h1>${t('viewer.title')}</h1>
        <${LanguageSwitch} />
      </header>
      ${unreachable.value ? html`<p class="v-offline" role="status">${t('viewer.offline')}</p>` : null}
      <${NowPlaying} state=${state} />
      <${RequestForm} />
      <${UpNext} state=${state} />
      <${HowTo} />
      <div class="v-toast-slot" role="status" aria-live="polite">
        ${toast.value ? html`<p class="v-toast">${toast.value.text}</p>` : null}
      </div>
    </main>`;
}

await loadLang(chosenLang() ?? browserLang());
render(html`<${App} />`, document.getElementById('root'));
startPolling();
