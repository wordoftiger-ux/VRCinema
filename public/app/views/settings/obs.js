// Settings → OBS (window only: the stream key never reaches a remote panel): the server
// address(es) and the stream key for OBS with Copy buttons, a new key, the two switches (other
// devices on the network, priority over the queue) and the listener's state.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { copyText } from '../../components/playerbar.js';
import { Block, Note, Row, Switch, WindowNote, saveWith, useAccess, useConfig, useNote } from './common.js';

const OBS = '/api/desktop/obs';
const POLL_MS = 3000;

function CopyField({ value, label, secret = false }) {
  const [copied, setCopied] = useState(false);
  const [shown, setShown] = useState(!secret);
  const copy = async () => {
    await copyText(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return html`
    <div class="obs-copy">
      <code aria-label=${label} data-obs-value=${secret ? 'key' : 'server'}>${shown ? value : '•'.repeat(Math.min(24, value.length))}</code>
      ${secret
        ? html`<button type="button" class="btn" onClick=${() => setShown(!shown)}>
            ${shown ? t('settings.obs.hide_key') : t('settings.obs.show_key')}</button>`
        : null}
      <button type="button" class="btn" data-copy aria-label=${`${t('settings.obs.copy')}: ${label}`} onClick=${copy}>
        <span aria-live="polite">${copied ? t('settings.obs.copied') : t('settings.obs.copy')}</span>
      </button>
    </div>`;
}

const STATUS_KEYS = {
  listening: 'settings.obs.status_listening',
  streaming: 'settings.obs.status_streaming',
  off: 'settings.obs.status_off',
};

export function Obs() {
  const { windowed } = useAccess();
  const cfg = useConfig();
  const [obs, setObs] = useState(null);
  const [note, say] = useNote();

  useEffect(() => {
    if (!windowed) return undefined;
    let stopped = false;
    let timer = 0;
    const read = async () => {
      try {
        const data = await api('GET', OBS);
        if (!stopped) setObs(data);
      } catch {
        // read again on the next round
      }
      if (!stopped) timer = setTimeout(read, POLL_MS);
    };
    read();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [windowed, cfg?.obsLan, cfg?.obsPriority]);

  const newKey = async () => {
    say(null);
    try {
      setObs(await api('POST', `${OBS}/key`, {}));
      say('ok', t('settings.obs.new_key_done'));
    } catch (e) {
      say('err', errorText(e));
    }
  };

  if (!windowed) return html`<div class="set-rows"><${WindowNote} all /></div>`;
  const status = obs?.status === 'failed'
    ? t('settings.obs.status_failed', { error: obs.error ?? '' })
    : obs
      ? t(STATUS_KEYS[obs.status] ?? STATUS_KEYS.off)
      : '';
  return html`
    <div class="set-rows" data-obs-status=${obs?.status ?? ''}>
      <${Block} hint=${t('settings.obs.intro')}>
        <${Row} title=${t('settings.obs.server')} hint=${status}>
          ${obs ? html`<${CopyField} value=${obs.server} label=${t('settings.obs.server')} />` : null}
        <//>
        ${obs?.lanServers?.length
          ? html`<${Row} title=${t('settings.obs.server_lan')} hint=${t('settings.obs.server_lan_hint')}>
              <div class="obs-lan">
                ${obs.lanServers.map((s) => html`<${CopyField} value=${s} label=${t('settings.obs.server_lan')} />`)}
              </div>
            <//>`
          : null}
        <${Row} title=${t('settings.obs.key')} hint=${t('settings.obs.key_hint')}>
          ${obs ? html`<${CopyField} value=${obs.key} label=${t('settings.obs.key')} secret />` : null}
        <//>
        <${Row} title=${t('settings.obs.new_key')} hint=${t('settings.obs.new_key_hint')}>
          <button type="button" class="btn" data-obs-new-key disabled=${!obs} onClick=${newKey}>${t('settings.obs.new_key')}</button>
        <//>
      <//>
      <${Row} title=${t('settings.obs.lan')} hint=${t('settings.obs.lan_hint')}>
        <${Switch} label=${t('settings.obs.lan')} checked=${Boolean(cfg?.obsLan)} disabled=${!cfg}
          onChange=${(on) => saveWith(say, { obsLan: on }, on ? t('settings.obs.lan_on') : t('settings.obs.lan_off'))} />
      <//>
      <${Row} title=${t('settings.obs.priority')} hint=${t('settings.obs.priority_hint')}>
        <${Switch} label=${t('settings.obs.priority')} checked=${cfg?.obsPriority !== false} disabled=${!cfg}
          onChange=${(on) => saveWith(say, { obsPriority: on }, on ? t('settings.obs.priority_on') : t('settings.obs.priority_off'))} />
      <//>
      <${Note} note=${note} />
    </div>`;
}
