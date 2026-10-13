// The first-run wizard (window only): language, the tools, the graphics card, the internet.
// Served at /wizard while the first run is not complete; at the end it goes to the interface.

import { render } from '/static/vendor/preact.mjs';
import { signal } from '/static/vendor/signals.mjs';
import { html } from '../html.js';
import { browserLang, lang, loadLang, t } from '../i18n.js';
import { api } from '../api.js';
import { firstInfo, setLanguage } from '../store.js';
import { LANGUAGES, LANGUAGE_NAME_KEYS } from '../views/settings/common.js';
import { ToolsStep } from './tools-step.js';
import { EncoderStep } from './encoder-step.js';
import { InternetStep } from './internet-step.js';

const STEPS = 4;
/** The step on show (1..4). */
const step = signal(1);

/** Finishes the wizard (the first run is complete) and opens the interface at `hash`. */
export async function finish(hash = '#/channel') {
  await api('POST', '/api/desktop/wizard/complete');
  location.href = `/app${hash}`;
}

function LanguageStep() {
  const choose = (code) => {
    if (code !== lang.value) setLanguage(code);
  };
  return html`
    <h1 tabindex="-1" ref=${(el) => el?.focus()}>${t('wizard.welcome_title')}</h1>
    <p>${t('wizard.welcome_intro')}</p>
    <p>${t('wizard.welcome_setup')}</p>
    <h2 id="wizard-language">${t('wizard.language_title')}</h2>
    <div class="wz-langs" role="radiogroup" aria-labelledby="wizard-language">
      ${LANGUAGES.map((code) => html`
        <button type="button" role="radio" class="wz-lang" data-lang=${code} aria-checked=${lang.value === code}
          onClick=${() => choose(code)}>${t(LANGUAGE_NAME_KEYS[code])}</button>`)}
    </div>
    <p class="wz-hint">${t('wizard.language_hint')}</p>
    <div class="wz-actions">
      <button type="button" class="btn pri" data-next onClick=${() => (step.value = 2)}>${t('wizard.start')}</button>
    </div>`;
}

function Wizard() {
  const n = step.value;
  const body =
    n === 1
      ? html`<${LanguageStep} />`
      : n === 2
        ? html`<${ToolsStep} next=${() => (step.value = 3)} finish=${finish} />`
        : n === 3
          ? html`<${EncoderStep} next=${() => (step.value = 4)} />`
          : html`<${InternetStep} finish=${finish} />`;
  return html`
    <main class="wz" data-step=${n}>
      <ol class="wz-steps" aria-label=${t('wizard.progress')}>
        ${Array.from({ length: STEPS }, (_, i) => html`<li class=${i + 1 < n ? 'done' : i + 1 === n ? 'current' : ''}></li>`)}
      </ol>
      <p class="wz-no">${t('wizard.step', { n, total: STEPS })}</p>
      <div class="wz-body" key=${n}>${body}</div>
    </main>`;
}

async function start() {
  const info = await firstInfo();
  if (!info) {
    // Not the program's window (or the session is gone): the panel signs in or says so.
    location.replace('/app');
    return;
  }
  if (info.firstRunCompleted) {
    location.replace('/app');
    return;
  }
  await loadLang(info.language ?? browserLang());
  render(html`<${Wizard} />`, document.getElementById('wizard'));
}

start();
