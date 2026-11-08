// Wizard: the channel on the internet, and the remote panel. On the first run the program
// connects by itself (a quick link); on a run again a tunnel switched off stays off until
// "Connect". No connection never blocks: Next reads "Skip" until the channel is online.
// "I want a permanent link" ends the wizard in Settings → Internet. Below it the remote panel
// (switch and password: the Settings → Remote panel component).

import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { focusOnMount } from '../focus.js';
import { copyText } from '../components/playerbar.js';
import { Remote } from '../views/settings/remote.js';
import { Later, StepNav } from './common.js';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function LinkRow({ label, value, name }) {
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

/** Switches the tunnel on in the program's settings when it is off. */
async function ensureTunnelOn() {
  const cfg = await api('GET', '/api/desktop/config');
  if (!cfg.tunnelEnabled) await api('PUT', '/api/desktop/config', { tunnelEnabled: true });
}

const sameTunnel = (a, b) => a.state === b.state && a.url === b.url && a.message === b.message;

export function InternetStep({ nav, again, finish }) {
  const [tun, setTun] = useState({ state: again ? 'loading' : 'connecting' });
  const [failure, setFailure] = useState(null);
  const [retrying, setRetrying] = useState(false);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    (async () => {
      if (!again) {
        try {
          await ensureTunnelOn();
        } catch (e) {
          if (alive.current) setTun({ state: 'error', message: errorText(e), local: true });
        }
      }
      while (alive.current) {
        try {
          const v = await api('GET', '/api/desktop/tunnel');
          // A failure of ours stays until Retry; a poll with nothing new changes nothing (no re-render).
          if (alive.current) setTun((old) => (old.local || sameTunnel(old, v) ? old : v));
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
  const connect = async (restart) => {
    setRetrying(true);
    setTun({ state: 'connecting' });
    try {
      await ensureTunnelOn();
      if (restart) await api('POST', '/api/desktop/tunnel/restart');
    } catch (e) {
      setTun({ state: 'error', message: errorText(e), local: true });
    } finally {
      setRetrying(false);
    }
  };

  const online = tun.state === 'online' && tun.url;
  const bad = tun.state === 'error' || tun.state === 'reconnecting';
  const off = tun.state === 'off';
  const shown = online ? 'online' : bad ? tun.state : off ? 'off' : 'connecting';
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.net_title')}</h1>
    <p>${t('wizard.net_intro')}</p>
    <div class="wz-stack" aria-live="polite" data-tunnel=${shown}>
      ${online
        ? html`
            <p class="wz-note ok" role="status">${t('wizard.net_online')}</p>
            <${LinkRow} name="channel" label=${t('wizard.net_channel_link')} value=${`${tun.url}/live/main.m3u8`} />
            <${LinkRow} name="request" label=${t('wizard.net_request_link')} value=${`${tun.url}/`} />
            <div class="wz-qr">
              <div class="qr"><img class="qr-img" src=${`/api/desktop/tunnel/qr?for=${encodeURIComponent(tun.url)}`} alt=${t('wizard.net_qr_alt')} width="196" height="196" /></div>
              <p class="wz-hint">${t('wizard.net_qr_hint')}</p>
            </div>
            <p class="wz-hint">${t('wizard.net_changes')}</p>
            <div class="wz-inline-actions">
              <button type="button" class="btn" data-permanent onClick=${() => leave('#/settings/internet')}>${t('wizard.permanent')}</button>
            </div>`
        : bad
          ? html`
              <p class="wz-note err" role="alert">${tun.message || (tun.state === 'reconnecting' ? t('wizard.net_retrying') : t('wizard.net_failed'))}</p>
              <p class="wz-hint">${t('wizard.net_skip_hint')}</p>
              <div class="wz-inline-actions">
                <button type="button" class="btn pri" data-retry disabled=${retrying} onClick=${() => connect(true)}>${t('wizard.retry')}</button>
              </div>`
          : off
            ? html`
                <p class="wz-note" role="status">${t('wizard.net_off')}</p>
                <div class="wz-inline-actions">
                  <button type="button" class="btn pri" data-connect disabled=${retrying} onClick=${() => connect(false)}>${t('wizard.net_connect')}</button>
                </div>`
            : html`
                <p class="wz-note" role="status">${t('wizard.net_connecting')}</p>
                <div class="bar indeterminate" role="progressbar" aria-label=${t('wizard.net_connecting')}><span></span></div>`}
      ${failure ? html`<p class="wz-note err" role="alert">${failure}</p>` : null}
    </div>
    <h2>${t('settings.nav.remote')}</h2>
    <p class="wz-hint">${t('wizard.remote_intro')}</p>
    <div class="wz-rows" data-block="remote"><${Remote} /></div>
    <${Later} sections=${['internet', 'remote']} />
    <${StepNav} nav=${nav} nextLabel=${online ? null : t('wizard.skip')} />`;
}
