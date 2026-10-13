// Settings → General: the program's language, starting with Windows, access from the home
// network and the port. A new port or network access restarts the server.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { lang, t } from '../../i18n.js';
import { setLanguage } from '../../store.js';
import { LanguageOptions, Note, Row, Switch, WindowNote, saveWith, useAccess, useConfig, useNote } from './common.js';

export function General() {
  const { mode, windowed } = useAccess();
  const cfg = useConfig();
  const [note, say] = useNote();
  const [port, setPort] = useState('');
  useEffect(() => {
    if (cfg) setPort(String(cfg.port));
  }, [cfg?.port]);

  const savePort = async (e) => {
    e.preventDefault();
    const value = Number(port);
    if (!Number.isInteger(value) || value < 1024 || value > 65535) {
      return say('err', t('errors.invalid_port'));
    }
    if (value === cfg.port) return say('info', t('settings.general.port_same'));
    await saveWith(say, { port: value }, (r) =>
      r.restarted ? t('settings.general.port_restarted', { port: r.port }) : t('settings.general.port_saved', { port: r.port }));
  };

  return html`
    <div class="set-rows">
      <${WindowNote} />
      ${mode === 'headless'
        ? html`<${Row} title=${t('settings.general.language')} hint=${t('settings.general.language_env')} />`
        : html`<${Row} title=${t('settings.general.language')} hint=${t('settings.general.language_hint')}>
            <select class="sel" aria-label=${t('settings.general.language')} value=${lang.value}
              disabled=${!windowed} onChange=${(e) => setLanguage(e.target.value)}>
              <${LanguageOptions} />
            </select>
          <//>`}
      <${Row} title=${t('settings.general.autostart')} hint=${t('settings.general.autostart_hint')}>
        <${Switch} label=${t('settings.general.autostart')} checked=${Boolean(cfg?.autostart)} disabled=${!cfg}
          onChange=${(on) => saveWith(say, { autostart: on }, on ? t('settings.general.autostart_on') : t('settings.general.autostart_off'))} />
      <//>
      <${Row} title=${t('settings.general.lan')} hint=${t('settings.general.lan_hint')}>
        <${Switch} label=${t('settings.general.lan')} checked=${Boolean(cfg?.lanAccess)} disabled=${!cfg}
          onChange=${(on) => saveWith(say, { lanAccess: on }, on ? t('settings.general.lan_on') : t('settings.general.lan_off'))} />
      <//>
      <${Row} title=${t('settings.general.port')} hint=${t('settings.general.port_hint')}>
        <form class="inline" onSubmit=${savePort}>
          <input type="number" class="num" min="1024" max="65535" step="1" required aria-label=${t('settings.general.port')}
            value=${port} disabled=${!cfg} onInput=${(e) => setPort(e.target.value)} />
          <button type="submit" class="btn" disabled=${!cfg}>${t('settings.general.port_save')}</button>
        </form>
      <//>
      <${Note} note=${note} />
    </div>`;
}
