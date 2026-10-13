// Settings → Advanced: the cookies file for yt-dlp (videos that need a login) and the pixeldrain
// key. The address typed by hand and the trust in Cloudflare are in Internet (they only matter
// while the tunnel is off).

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { Note, Row, WindowNote, loadConfig, saveWith, useConfig, useNote } from './common.js';

export function Advanced() {
  const cfg = useConfig();
  const [note, say] = useNote();
  const [key, setKey] = useState('');
  useEffect(() => {
    if (cfg) setKey(cfg.pixeldrainApiKey ?? '');
  }, [cfg?.pixeldrainApiKey]);

  const pick = async () => {
    say('info', t('settings.advanced.cookies_picking'));
    try {
      const r = await api('POST', '/api/desktop/pick-cookies', undefined, { timeoutMs: 600000 });
      say(r.picked ? 'ok' : 'info', r.picked ? t('settings.advanced.cookies_saved') : t('settings.advanced.cookies_none_picked'));
      await loadConfig();
    } catch (e) {
      say('err', errorText(e));
    }
  };

  return html`
    <div class="set-rows">
      <${WindowNote} />
      <${Row} title=${t('settings.advanced.cookies')} hint=${t('settings.advanced.cookies_hint')} block>
        <p class="path-value" data-cookies-path>${cfg?.ytdlpCookies ?? t('settings.advanced.cookies_unset')}</p>
        <div class="pl-actions">
          <button type="button" class="btn" disabled=${!cfg} onClick=${pick}>${t('settings.advanced.cookies_pick')}</button>
          ${cfg?.ytdlpCookies
            ? html`<button type="button" class="btn" onClick=${() => saveWith(say, { ytdlpCookies: null }, t('settings.advanced.cookies_cleared'))}>
                ${t('settings.advanced.cookies_clear')}</button>`
            : null}
        </div>
      <//>
      <${Row} title=${t('settings.advanced.pixeldrain')} hint=${t('settings.advanced.pixeldrain_hint')}>
        <form class="inline" onSubmit=${(e) => {
          e.preventDefault();
          saveWith(say, { pixeldrainApiKey: key.trim() }, key.trim() ? t('settings.advanced.pixeldrain_saved') : t('settings.advanced.pixeldrain_removed'));
        }}>
          <input type="password" class="txt" autocomplete="off" aria-label=${t('settings.advanced.pixeldrain')}
            value=${key} disabled=${!cfg} onInput=${(e) => setKey(e.target.value)} />
          <button type="submit" class="btn" disabled=${!cfg}>${t('settings.save')}</button>
        </form>
      <//>
      <${Note} note=${note} />
    </div>`;
}
