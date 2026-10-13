// The Share view: the channel's link for the VRChat player, the page where viewers add
// videos with its QR code, and the internet connection (quick or permanent link).

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { NamedLink } from '../components/named-wizard.js';
import { copyText } from '../components/playerbar.js';
import {
  access,
  channelLink,
  info,
  netStatus,
  publicAddress,
  refreshInfo,
  requestLink,
  showToast,
  tunnel,
} from '../store.js';

const NET_KEYS = {
  online: 'net.online',
  connecting: 'net.connecting',
  offline: 'net.offline',
  manual: 'net.manual',
  local: 'net.local',
};

function CopyButton({ text, primary = false }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    if (!text) return;
    try {
      await copyText(text);
    } catch (e) {
      return showToast(errorText(e));
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return html`<button type="button" class=${primary ? 'btn pri' : 'btn'} disabled=${!text} onClick=${copy}>
    <span aria-live="polite">${copied ? t('share.copied') : t('share.copy')}</span></button>`;
}

function LinkCard({ id, title, hint, link, children }) {
  return html`
    <section class="card2" aria-labelledby=${id}>
      <h2 id=${id}>${title}</h2>
      <p>${hint}</p>
      <div class="field"><code class="field-value" data-link=${id}>${link ?? ''}</code>${children}</div>
    </section>`;
}

/** Reconnect: a new cloudflared (a quick link gets a new address). */
const reconnecting = signal(false);
async function reconnect() {
  reconnecting.value = true;
  try {
    await api('POST', '/api/desktop/tunnel/restart');
    showToast(t('share.reconnected'), 'info');
    await refreshInfo();
  } catch (e) {
    showToast(errorText(e));
  } finally {
    reconnecting.value = false;
  }
}

function InternetCard() {
  const net = netStatus.value;
  const tun = tunnel.value;
  const windowed = access.value === 'window';
  const known = access.value !== 'unknown';
  return html`
    <section class="card2" aria-labelledby="share-internet">
      <h2 id="share-internet">${t('share.internet_title')}
        ${net ? html`<span class=${`pill net-${net}`} role="status"><span class="dot" aria-hidden="true"></span>${t(NET_KEYS[net])}</span>` : null}
      </h2>
      ${windowed
        ? html`
            ${tun?.state === 'error' && tun.message ? html`<p class="pl-small">${t('share.reason', { reason: tun.message })}</p>` : null}
            ${info.value && !info.value.tunnelEnabled ? html`<p>${t('share.tunnel_off')}</p>` : html`<${NamedLink} />`}
            ${info.value?.tunnelEnabled
              ? html`<div class="pl-actions"><button type="button" class="btn" disabled=${reconnecting.value} onClick=${reconnect}>
                  ${reconnecting.value ? t('share.reconnecting') : t('share.reconnect')}</button></div>`
              : null}`
        : known
          ? html`<p class="window-only">${t('ui.window_only')}</p>`
          : null}
    </section>`;
}

/** The QR code of the request page. It is drawn again whenever the address changes. */
function QrCard() {
  const mode = access.value;
  const isRemote = mode !== 'window';
  const address = requestLink.value;
  // The window knows whether there is a public address; a remote session just asks. Nothing
  // is asked before it is known which of the two this is.
  const has = mode === 'unknown' ? false : isRemote ? Boolean(address) : Boolean(publicAddress.value);
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [address, has]);
  const route = isRemote ? '/api/admin/qr' : '/api/desktop/tunnel/qr';
  const src = has ? `${route}?for=${encodeURIComponent(address ?? '')}` : null;
  return html`
    <section class="card2 qr-card" aria-labelledby="share-qr">
      <h2 id="share-qr">${t('share.qr_title')}</h2>
      <p>${t('share.qr_hint')}</p>
      <div class="qr">
        ${src && !failed
          ? html`<img key=${src} class="qr-img" src=${src} alt=${t('share.qr_alt', { url: address })} width="196" height="196"
              onError=${() => setFailed(true)} />`
          : html`<p class="qr-none">${t('share.qr_none')}</p>`}
      </div>
      <p class="qr-note">${t('share.qr_note')}</p>
    </section>`;
}

export function ShareView() {
  const request = requestLink.value;
  return html`
    <main class="view share" id="view" data-view="share">
      <div class="page">
        <h1>${t('share.title')}</h1>
        <p class="page-sub">${t('share.sub')}</p>
        <div class="share-grid">
          <div class="stack">
            <${LinkCard} id="share-channel" title=${t('share.channel_title')} hint=${t('share.channel_hint')} link=${channelLink.value}>
              <${CopyButton} text=${channelLink.value} primary />
            <//>
            <${LinkCard} id="share-request" title=${t('share.request_title')} hint=${t('share.request_hint')} link=${request}>
              <${CopyButton} text=${request} />
              <a class="btn" href=${request ?? undefined} target="_blank" rel="noopener" aria-disabled=${request ? undefined : 'true'}>${t('share.open')}</a>
            <//>
            <${InternetCard} />
          </div>
          <${QrCard} />
        </div>
      </div>
    </main>`;
}
