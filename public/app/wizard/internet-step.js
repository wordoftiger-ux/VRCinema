// Wizard step 4: the channel on the internet. The program connects by itself (a quick link);
// "Skip" is there in every state but a working connection, so no network never blocks the end.

import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { copyText } from '../components/playerbar.js';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function LinkRow({ label, value, name }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await copyText(value);
    } catch {
      return;
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return html`
    <div class="wz-link" data-link=${name}>
      <div class="wz-hint">${label}</div>
      <div class="field"><code class="field-value">${value}</code>
        <button type="button" class="btn" onClick=${copy}><span aria-live="polite">${copied ? t('share.copied') : t('share.copy')}</span></button></div>
    </div>`;
}

export function InternetStep({ finish }) {
  const [tun, setTun] = useState({ state: 'connecting' });
  const [failure, setFailure] = useState(null);
  const [retrying, setRetrying] = useState(false);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    (async () => {
      try {
        const cfg = await api('GET', '/api/desktop/config');
        if (!cfg.tunnelEnabled) await api('PUT', '/api/desktop/config', { tunnelEnabled: true });
      } catch (e) {
        if (alive.current) setTun({ state: 'error', message: errorText(e) });
      }
      while (alive.current) {
        try {
          const v = await api('GET', '/api/desktop/tunnel');
          if (alive.current) setTun((old) => (old.state === 'error' && old.local && v.state === 'connecting' ? old : v));
        } catch {
          // the engine does not answer for a moment: the next try
        }
        await sleep(1000);
      }
    })();
    return () => {
      alive.current = false;
    };
  }, []);

  const leave = async (hash) => {
    try {
      await finish(hash);
    } catch (e) {
      setFailure(errorText(e));
    }
  };
  const again = async () => {
    setRetrying(true);
    setTun({ state: 'connecting' });
    try {
      await api('POST', '/api/desktop/tunnel/restart');
    } catch (e) {
      setTun({ state: 'error', message: errorText(e), local: true });
    } finally {
      setRetrying(false);
    }
  };

  const online = tun.state === 'online' && tun.url;
  const bad = tun.state === 'error' || tun.state === 'reconnecting';
  return html`
    <h1 tabindex="-1" ref=${(el) => el?.focus()}>${t('wizard.net_title')}</h1>
    <p>${t('wizard.net_intro')}</p>
    <div class="wz-stack" aria-live="polite" data-tunnel=${online ? 'online' : bad ? tun.state : 'connecting'}>
      ${online
        ? html`
            <p class="wz-note ok" role="status">${t('wizard.net_online')}</p>
            <${LinkRow} name="channel" label=${t('wizard.net_channel_link')} value=${`${tun.url}/live/main.m3u8`} />
            <${LinkRow} name="request" label=${t('wizard.net_request_link')} value=${`${tun.url}/`} />
            <div class="wz-qr">
              <div class="qr"><img class="qr-img" src=${`/api/desktop/tunnel/qr?for=${encodeURIComponent(tun.url)}`} alt=${t('wizard.net_qr_alt')} width="196" height="196" /></div>
              <p class="wz-hint">${t('wizard.net_qr_hint')}</p>
            </div>
            <p class="wz-hint">${t('wizard.net_changes')}</p>`
        : bad
          ? html`
              <p class="wz-note err" role="alert">${tun.message || (tun.state === 'reconnecting' ? t('wizard.net_retrying') : t('wizard.net_failed'))}</p>
              <p class="wz-hint">${t('wizard.net_skip_hint')}</p>`
          : html`
              <p class="wz-note" role="status">${t('wizard.net_connecting')}</p>
              <div class="bar indeterminate" role="progressbar" aria-label=${t('wizard.net_connecting')}><span></span></div>`}
      ${failure ? html`<p class="wz-note err" role="alert">${failure}</p>` : null}
    </div>
    <div class="wz-actions">
      ${online
        ? html`<button type="button" class="btn pri" data-finish onClick=${() => leave()}>${t('wizard.done')}</button>
            <button type="button" class="btn" data-permanent onClick=${() => leave('#/settings/internet')}>${t('wizard.permanent')}</button>`
        : html`${bad ? html`<button type="button" class="btn pri" data-retry disabled=${retrying} onClick=${again}>${t('wizard.retry')}</button>` : null}
            <button type="button" class="btn" data-skip onClick=${() => leave()}>${t('wizard.skip')}</button>`}
    </div>`;
}
