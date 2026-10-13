// Settings → Internet: the built-in tunnel (on/off, quick or permanent link with the wizard),
// the pauses, and — only while the tunnel is off — the address typed by hand and the trust in
// Cloudflare's headers. A remote session may switch the tunnel (spec section 2) — and is asked
// first when it came through the tunnel, which switching it off would end; the rest is the
// window's.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { NamedLink } from '../../components/named-wizard.js';
import { openDialog } from '../../components/dialog.js';
import { environment, refreshInfo, sessionViaTunnel, setTunnelEnabled, tunnel } from '../../store.js';
import { Block, Note, Row, Switch, WindowNote, saveWith, useAccess, useConfig, useNote } from './common.js';

/** Asks before the tunnel goes off under a session that came through it; resolves to the answer. */
export function confirmLosingAccess() {
  return new Promise((resolve) => {
    openDialog({
      title: () => t('settings.internet.lose_access_title'),
      body: () => html`<p class="dialog-intro" data-lose-access>${t('settings.internet.lose_access')}</p>`,
      buttons: [
        { key: 'settings.internet.lose_access_cancel', action: () => resolve(false) },
        { key: 'settings.internet.lose_access_yes', danger: true, action: () => resolve(true) },
      ],
      onClose: () => resolve(false),
    });
  });
}

/** Switches the tunnel from here: the window saves its settings, a remote session the switch. */
async function switchTunnel(on, { windowed, say }) {
  if (!on && sessionViaTunnel.value && !(await confirmLosingAccess())) return;
  const okText = on ? t('settings.internet.tunnel_on') : t('settings.internet.tunnel_off');
  if (windowed) {
    await saveWith(say, { tunnelEnabled: on }, okText);
    return;
  }
  say(null);
  try {
    await setTunnelEnabled(on);
    say('ok', okText);
  } catch (e) {
    say('err', errorText(e));
  }
}

const STATE_KEYS = {
  off: 'settings.internet.state_off',
  connecting: 'settings.internet.state_connecting',
  online: 'settings.internet.state_online',
  reconnecting: 'settings.internet.state_reconnecting',
  error: 'settings.internet.state_error',
};

const reconnecting = signal(false);

export function Internet() {
  const { windowed, mode } = useAccess();
  const cfg = useConfig();
  const env = environment.value;
  const tun = tunnel.value;
  const [note, say] = useNote();
  const [address, setAddress] = useState('');
  useEffect(() => {
    if (cfg) setAddress(cfg.publicUrl ?? '');
  }, [cfg?.publicUrl]);
  // The window reads its settings; a remote session the environment (a server without a
  // window has no tunnel).
  const remoteSwitch = mode === 'remote' && typeof env?.tunnelEnabled === 'boolean';
  const on = windowed ? Boolean(cfg?.tunnelEnabled) : Boolean(remoteSwitch && env.tunnelEnabled);
  const canSwitch = windowed ? Boolean(cfg) : remoteSwitch;

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
      <${Row} title=${t('settings.internet.tunnel')} hint=${t('settings.internet.tunnel_hint')}>
        <${Switch} label=${t('settings.internet.tunnel')} checked=${on} disabled=${!canSwitch}
          onChange=${(v) => switchTunnel(v, { windowed, say })} />
      <//>
      ${on && sessionViaTunnel.value ? html`<p class="set-sub" data-via-tunnel>${t('settings.internet.via_tunnel')}</p>` : null}
      <${WindowNote} />
      ${on && tun?.warning ? html`<p class="set-note err" role="alert">${tun.warning}</p>` : null}
      ${on
        ? html`
            <${Block}>
              <div class="kv" data-tunnel-state=${tun?.state ?? 'off'}>
                <span class="name">${t(STATE_KEYS[tun?.state ?? 'off'] ?? STATE_KEYS.off)}</span>
                <button type="button" class="btn" disabled=${!windowed || reconnecting.value} onClick=${reconnect}>
                  ${reconnecting.value ? t('share.reconnecting') : t('share.reconnect')}</button>
              </div>
              ${tun?.url
                ? html`<p class="set-sub">${t('settings.internet.address', { url: tun.url })}</p>`
                : tun?.message
                  ? html`<p class="set-sub">${t('share.reason', { reason: tun.message })}</p>`
                  : null}
            <//>
            ${windowed
              ? html`<${Block} title=${t('settings.internet.link_type')}>
                  <${NamedLink} />
                <//>`
              : null}
            <${Block} title=${t('settings.internet.pauses')} hint=${t('settings.internet.pauses_hint')}>
              <${Row} title=${t('settings.internet.pause_loss')}>
                <${Switch} label=${t('settings.internet.pause_loss')} checked=${Boolean(cfg?.pauseOnTunnelLoss)} disabled=${!cfg}
                  onChange=${(v) => saveWith(say, { pauseOnTunnelLoss: v }, v ? t('settings.internet.pause_loss_on') : t('settings.internet.pause_loss_off'))} />
              <//>
              <${Row} title=${t('settings.internet.pause_viewers')}>
                <${Switch} label=${t('settings.internet.pause_viewers')} checked=${Boolean(cfg?.pauseWhenNoViewers)} disabled=${!cfg}
                  onChange=${(v) => saveWith(say, { pauseWhenNoViewers: v }, v ? t('settings.internet.pause_viewers_on') : t('settings.internet.pause_viewers_off'))} />
              <//>
            <//>`
        : null}
      ${cfg && !on
        ? html`
            <${Block} title=${t('settings.internet.manual')} hint=${t('settings.internet.manual_hint')}>
              <form class="inline wide" onSubmit=${(e) => {
                e.preventDefault();
                saveWith(say, { publicUrl: address.trim() }, t('settings.internet.manual_saved'));
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
