// Wizard: the end. The link for the VRChat player with "Copy link for VRChat" (the public one,
// or the one of this computer with a note), a short test, the Settings sections (each ends
// the wizard there) and "Open the program".

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { errorText } from '../api.js';
import { focusOnMount } from '../focus.js';
import { copyText } from '../components/playerbar.js';
import { info, publicAddress, refreshInfo } from '../store.js';
import { settingsPlace } from './common.js';

const SECTIONS = ['general', 'channel', 'internet', 'obs', 'remote', 'tools', 'advanced'];

export function SummaryStep({ nav, finish }) {
  const [copied, setCopied] = useState(false);
  const [failure, setFailure] = useState(null);
  useEffect(() => {
    refreshInfo();
  }, []);
  const address = publicAddress.value;
  const base = address ? String(address).replace(/\/+$/, '') : `${location.protocol}//127.0.0.1:${info.value?.port ?? 8080}`;
  const link = `${base}/live/main.m3u8`;

  const copy = async () => {
    try {
      await copyText(link);
    } catch {
      return;
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  const leave = async (hash) => {
    try {
      await finish(hash);
    } catch (e) {
      setFailure(errorText(e));
    }
  };
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.summary_title')}</h1>
    <p>${t('wizard.summary_intro')}</p>
    <div class="wz-vrchat" data-link="vrchat" data-public=${address ? 'true' : 'false'}>
      <code class="field-value">${link}</code>
      <button type="button" class="btn pri" data-copy-vrchat onClick=${copy}>
        <span aria-live="polite">${copied ? t('share.copied') : t('wizard.copy_vrchat')}</span></button>
    </div>
    ${address ? null : html`<p class="wz-note" data-local>${t('wizard.summary_local', { place: settingsPlace('internet') })}</p>`}
    <h2>${t('wizard.test_title')}</h2>
    <ol class="wz-check" data-checklist>
      <li>${t('wizard.test_add')}</li>
      <li>${t('wizard.test_preview')}</li>
      <li>${t('wizard.test_vrchat')}</li>
    </ol>
    <h2>${t('wizard.summary_settings')}</h2>
    <ul class="wz-places">
      ${SECTIONS.map((s) => html`<li><a href=${`/admin#/settings/${s}`} data-settings=${s}
        onClick=${(e) => { e.preventDefault(); leave(`#/settings/${s}`); }}>${settingsPlace(s)}</a></li>`)}
    </ul>
    ${failure ? html`<p class="wz-note err" role="alert">${failure}</p>` : null}
    <div class="wz-actions">
      ${nav.back ? html`<button type="button" class="btn" data-back onClick=${nav.back}>${t('wizard.back')}</button>` : null}
      <button type="button" class="btn pri" data-finish onClick=${() => leave()}>${t('wizard.open')}</button>
    </div>`;
}
