// The first-run wizard (window only): language, the tools, the graphics card, the internet.
// Served at /wizard while the first run is not complete; at the end it goes to the interface.
// The window has no frame: the wizard brings the interface's title bar.

import { render } from '/static/vendor/preact.mjs';
import { signal } from '/static/vendor/signals.mjs';
import { html } from '../html.js';
import { browserLang, lang, loadLang, t } from '../i18n.js';
import { api } from '../api.js';
import { access, firstInfo, setLanguage, toast } from '../store.js';
import { Icon } from '../icons.js';
import { Titlebar, inWindow } from '../components/titlebar.js';
import { focusOnMount } from '../focus.js';
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
  location.href = `/admin${hash}`;
}

function LanguageStep() {
  const choose = (code) => {
    if (code !== lang.value) setLanguage(code);
  };
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.welcome_title')}</h1>
    <p>${t('wizard.welcome_intro')}</p>
    <p>${t('wizard.welcome_setup')}</p>
    <h2 id="wizard-language">${t('wizard.language_title')}</h2>
    <div class="wz-langs" role="group" aria-labelledby="wizard-language">
      ${LANGUAGES.map((code) => html`
        <button type="button" class="wz-lang" data-lang=${code} aria-pressed=${lang.value === code}
          onClick=${() => choose(code)}>${t(LANGUAGE_NAME_KEYS[code])}</button>`)}
    </div>
    <p class="wz-hint">${t('wizard.language_hint')}</p>
    <div class="wz-actions">
      <button type="button" class="btn pri" data-next onClick=${() => (step.value = 2)}>${t('wizard.start')}</button>
    </div>`;
}

/** A message over the steps (a failed save of the language), as the interface shows it. */
function Toast() {
  const msg = toast.value;
  if (!msg) return null;
  return html`
    <div class=${`toast toast-${msg.kind}`} role=${msg.kind === 'error' ? 'alert' : 'status'}>
      <span>${msg.text}</span>
      <button type="button" class="icon-button" aria-label=${t('ui.close')} title=${t('ui.close')}
        onClick=${() => (toast.value = null)}><${Icon} name="close" size=${14} /></button>
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
    ${inWindow() ? html`<${Titlebar} />` : null}
    <div class="wz-scroll">
      <main class="wz" data-step=${n}>
        <ol class="wz-steps" aria-label=${t('wizard.progress')}>
          ${Array.from({ length: STEPS }, (_, i) => html`<li class=${i + 1 < n ? 'done' : i + 1 === n ? 'current' : ''}></li>`)}
        </ol>
        <p class="wz-no">${t('wizard.step', { n, total: STEPS })}</p>
        <div class="wz-body" key=${n}>${body}</div>
        <${Toast} />
      </main>
    </div>`;
}

/** The window's info; the engine may still be starting, so a few tries before giving up. */
async function readInfo() {
  for (let attempt = 1; ; attempt += 1) {
    const info = await firstInfo();
    if (info || attempt >= 6) return info;
    await new Promise((r) => setTimeout(r, 700));
  }
}

async function start() {
  const info = await readInfo();
  if (!info || access.value !== 'window') {
    // Not the program's window (or the session is gone): the panel signs in or says so.
    location.replace('/admin');
    return;
  }
  if (info.firstRunCompleted) {
    location.replace('/admin');
    return;
  }
  await loadLang(info.language ?? browserLang());
  render(html`<${Wizard} />`, document.getElementById('wizard'));
}

start();
