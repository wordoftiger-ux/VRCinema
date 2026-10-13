// Settings → Internet: the built-in tunnel (on/off, quick or permanent link with the wizard),
// the pauses, and — only while the tunnel is off — the address typed by hand and the trust in
// Cloudflare's headers.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { NamedLink } from '../../components/named-wizard.js';
import { refreshInfo, tunnel } from '../../store.js';
import { Block, Note, Row, Switch, WindowNote, saveWith, useConfig, useNote } from './common.js';

const STATE_KEYS = {
  off: 'settings.internet.state_off',
  connecting: 'settings.internet.state_connecting',
  online: 'settings.internet.state_online',
  reconnecting: 'settings.internet.state_reconnecting',
  error: 'settings.internet.state_error',
};

const reconnecting = signal(false);

export function Internet() {
  const cfg = useConfig();
  const tun = tunnel.value;
  const [note, say] = useNote();
  const [address, setAddress] = useState('');
  useEffect(() => {
    if (cfg) setAddress(cfg.publicUrl ?? '');
  }, [cfg?.publicUrl]);
  const on = Boolean(cfg?.tunnelEnabled);

  const reconnect = async () => {
    reconnecting.value = true;
    say(null);
    try {
      await api('POST', '/api/desktop/tunnel/restart');
      say('ok', t('share.reconnected'));
      await refreshInfo();
    } catch (e) {
      say('err', errorText(e));
    } finally {
      reconnecting.value = false;
    }
  };

  return html`
    <div class="set-rows">
      <${WindowNote} />
      <${Row} title=${t('settings.internet.tunnel')} hint=${t('settings.internet.tunnel_hint')}>
        <${Switch} label=${t('settings.internet.tunnel')} checked=${on} disabled=${!cfg}
          onChange=${(v) => saveWith(say, { tunnelEnabled: v }, v ? t('settings.internet.tunnel_on') : t('settings.internet.tunnel_off'))} />
      <//>
      ${on && tun?.warning ? html`<p class="set-note err" role="alert">${tun.warning}</p>` : null}
      ${on
        ? html`
            <${Block}>
              <div class="kv" data-tunnel-state=${tun?.state ?? 'off'}>
                <span class="name">${t(STATE_KEYS[tun?.state ?? 'off'] ?? STATE_KEYS.off)}</span>
                <button type="button" class="btn" disabled=${reconnecting.value} onClick=${reconnect}>
                  ${reconnecting.value ? t('share.reconnecting') : t('share.reconnect')}</button>
              </div>
              ${tun?.url
                ? html`<p class="set-sub">${t('settings.internet.address', { url: tun.url })}</p>`
                : tun?.message
                  ? html`<p class="set-sub">${t('share.reason', { reason: tun.message })}</p>`
                  : null}
            <//>
            <${Block} title=${t('settings.internet.link_type')}>
              <${NamedLink} />
            <//>
            <${Block} title=${t('settings.internet.pauses')} hint=${t('settings.internet.pauses_hint')}>
              <${Row} title=${t('settings.internet.pause_loss')}>
                <${Switch} label=${t('settings.internet.pause_loss')} checked=${Boolean(cfg?.pauseOnTunnelLoss)}
                  onChange=${(v) => saveWith(say, { pauseOnTunnelLoss: v }, v ? t('settings.internet.pause_loss_on') : t('settings.internet.pause_loss_off'))} />
              <//>
              <${Row} title=${t('settings.internet.pause_viewers')}>
                <${Switch} label=${t('settings.internet.pause_viewers')} checked=${Boolean(cfg?.pauseWhenNoViewers)}
                  onChange=${(v) => saveWith(say, { pauseWhenNoViewers: v }, v ? t('settings.internet.pause_viewers_on') : t('settings.internet.pause_viewers_off'))} />
              <//>
            <//>`
        : null}
      ${cfg && !on
        ? html`
            <${Block} title=${t('settings.internet.manual')} hint=${t('settings.internet.manual_hint')}>
              <form class="inline wide" onSubmit=${(e) => {
                e.preventDefault();
                saveWith(say, { publicUrl: address }, t('settings.internet.manual_saved'));
              }}>
                <input type="url" class="txt" aria-label=${t('settings.internet.manual')} placeholder="https://example.trycloudflare.com"
                  value=${address} onInput=${(e) => setAddress(e.target.value)} />
                <button type="submit" class="btn">${t('settings.save')}</button>
              </form>
              <${Row} title=${t('settings.internet.trust')} hint=${t('settings.internet.trust_hint')}>
                <${Switch} label=${t('settings.internet.trust')} checked=${Boolean(cfg.trustCloudflare)}
                  onChange=${(v) => saveWith(say, { trustCloudflare: v }, v ? t('settings.internet.trust_on') : t('settings.internet.trust_off'))} />
              <//>
            <//>`
        : null}
      <${Note} note=${note} />
    </div>`;
}
