// The setup wizard (window only): the program's main options in short steps, each with the
// engine's defaults already chosen, each skippable and each saved at once as in Settings
// (spec 2026-11-08-wizard-design). Served at /wizard; on the first run until it is finished,
// later again from Settings → General ("Run setup again": /wizard?again=1). At the end it
// goes to the interface. The window has no frame: the wizard brings the interface's title bar.

import { render } from '/static/vendor/preact.mjs';
import { signal } from '/static/vendor/signals.mjs';
import { useEffect } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { browserLang, lang, loadLang, t } from '../i18n.js';
import { api } from '../api.js';
import { access, firstInfo, setLanguage, toast } from '../store.js';
import { blockNativeMenu } from '../native-menu.js';
import { Icon } from '../icons.js';
import { Titlebar, inWindow } from '../components/titlebar.js';
import { focusOnMount } from '../focus.js';
import { LANGUAGES, LANGUAGE_NAME_KEYS, LanguageOptions, Note, Row, channelSettings, loadChannelSettings, saveChannelWith, useNote } from '../views/settings/common.js';
import { Later } from './common.js';
import { ToolsStep } from './tools-step.js';
import { EncoderStep } from './encoder-step.js';
import { ViewersStep } from './viewers-step.js';
import { TickerStep } from './ticker-step.js';
import { LookStep } from './look-step.js';
import { InternetStep } from './internet-step.js';
import { ExtrasStep } from './extras-step.js';
import { SummaryStep } from './summary-step.js';

/** Opened again from Settings (`/wizard?again=1`): the options are already set once. */
const again = new URLSearchParams(location.search).has('again');
/** The steps of this run (the tools are left out on a run again with every tool there). */
const steps = signal([]);
/** The step on show (index in `steps`). */
const index = signal(0);

/** Finishes the wizard (the first run is complete) and opens the interface at `hash`. */
export async function finish(hash = '#/channel') {
  await api('POST', '/api/desktop/wizard/complete');
  location.href = `/admin${hash}`;
}

/** Back / Next / Skip to the end for the step at `i`. */
function navFor(i) {
  const list = steps.value;
  const last = list.length - 1;
  const go = (n) => {
    index.value = Math.max(0, Math.min(last, n));
  };
  // Skipping to the end makes sense once the tools are behind (they are needed to broadcast).
  const tools = list.indexOf('tools');
  return {
    back: i > 0 ? () => go(list[i - 1] === 'tools' ? i - 2 : i - 1) : null,
    next: () => go(i + 1),
    toEnd: i < last && i > tools ? () => go(last) : null,
  };
}

function SlateLanguage() {
  const settings = channelSettings.value;
  const [note, say] = useNote();
  useEffect(() => {
    loadChannelSettings();
  }, []);
  if (!settings) return null;
  const label = t('settings.channel.slate_language');
  return html`
    <div class="wz-rows">
      <${Row} title=${label} hint=${t('settings.channel.slate_language_hint')}>
        <select class="sel" aria-label=${label} data-slate-language value=${settings.slateLanguage}
          onChange=${(e) => saveChannelWith(say, { slateLanguage: e.target.value }, t('settings.channel.slate_language_saved'))}>
          <${LanguageOptions} />
        </select>
      <//>
      <${Note} note=${note} />
    </div>`;
}

function LanguageStep({ nav }) {
  const choose = (code) => {
    if (code !== lang.value) setLanguage(code);
  };
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${again ? t('wizard.again_title') : t('wizard.welcome_title')}</h1>
    ${again
      ? html`<p>${t('wizard.again_intro')}</p>`
      : html`<p>${t('wizard.welcome_intro')}</p><p>${t('wizard.welcome_setup')}</p>`}
    <h2 id="wizard-language">${t('wizard.language_title')}</h2>
    <div class="wz-langs" role="group" aria-labelledby="wizard-language">
      ${LANGUAGES.map((code) => html`
        <button type="button" class="wz-lang" data-lang=${code} aria-pressed=${lang.value === code}
          onClick=${() => choose(code)}>${t(LANGUAGE_NAME_KEYS[code])}</button>`)}
    </div>
    <p class="wz-hint">${t('wizard.language_hint')}</p>
    <${SlateLanguage} />
    <${Later} sections=${['general', 'channel']} />
    <div class="wz-actions">
      <button type="button" class="btn pri" data-next onClick=${nav.next}>${again ? t('wizard.next') : t('wizard.start')}</button>
      ${nav.toEnd ? html`<button type="button" class="btn wz-to-end" data-to-end onClick=${nav.toEnd}>${t('wizard.to_end')}</button>` : null}
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

const STEP_VIEWS = {
  language: LanguageStep,
  tools: ToolsStep,
  picture: EncoderStep,
  viewers: ViewersStep,
  ticker: TickerStep,
  look: LookStep,
  internet: InternetStep,
  extras: ExtrasStep,
  summary: SummaryStep,
};

function Wizard() {
  const list = steps.value;
  const i = index.value;
  const id = list[i];
  const View = STEP_VIEWS[id];
  const total = list.length;
  return html`
    ${inWindow() ? html`<${Titlebar} />` : null}
    <div class="wz-scroll">
      <main class="wz" data-step=${id} data-again=${again ? 'true' : undefined}>
        <ol class="wz-steps" aria-label=${t('wizard.progress')}>
          ${list.map((_, n) => html`<li class=${n < i ? 'done' : n === i ? 'current' : ''}></li>`)}
        </ol>
        <p class="wz-no">${t('wizard.step', { n: i + 1, total })}</p>
        <div class="wz-body" key=${id}><${View} nav=${navFor(i)} again=${again} finish=${finish} /></div>
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
  if (info.firstRunCompleted && !again) {
    location.replace('/admin');
    return;
  }
  await loadLang(info.language ?? browserLang());
  const withTools = !(again && info.toolsOk);
  steps.value = ['language', ...(withTools ? ['tools'] : []), 'picture', 'viewers', 'ticker', 'look', 'internet', 'extras', 'summary'];
  blockNativeMenu();
  render(html`<${Wizard} />`, document.getElementById('wizard'));
}

start();
