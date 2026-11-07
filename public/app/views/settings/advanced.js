// Settings → Advanced: the cookies file for yt-dlp (videos that need a login) and the pixeldrain
// key. The address typed by hand and the trust in Cloudflare are in Internet (they only matter
// while the tunnel is off). Choosing the cookies file stays in the window (the file dialog opens
// on the computer); a remote panel can clear it and set the key. The pixeldrain key is
// write-only: the engine only says whether one is saved; a new one replaces it, "Remove"
// clears it. Below them the broadcast options (transitions, peak limit, subtitles, loudness,
// alarms, journal: `broadcast-options.js`).

import { useState } from '/static/vendor/hooks.mjs';
import { BroadcastOptions } from './broadcast-options.js';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { Note, Row, WindowNote, loadConfig, saveWith, useAccess, useConfig, useNote } from './common.js';

export function Advanced() {
  const { windowed } = useAccess();
  const cfg = useConfig();
  const [note, say] = useNote();
  const [key, setKey] = useState('');
  const hasKey = Boolean(cfg?.hasPixeldrainKey);
  const saveKey = async (value, okText) => {
    if (await saveWith(say, { pixeldrainApiKey: value }, okText)) setKey('');
  };

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
      <${Row} title=${t('settings.advanced.cookies')} hint=${t('settings.advanced.cookies_hint')} block windowOnly>
        <p class="path-value" data-cookies-path>${cfg?.ytdlpCookies ?? t('settings.advanced.cookies_unset')}</p>
        <div class="pl-actions">
          <button type="button" class="btn" disabled=${!cfg || !windowed} onClick=${pick}>${t('settings.advanced.cookies_pick')}</button>
          ${cfg?.ytdlpCookies
            ? html`<button type="button" class="btn" onClick=${() => saveWith(say, { ytdlpCookies: null }, t('settings.advanced.cookies_cleared'))}>
                ${t('settings.advanced.cookies_clear')}</button>`
            : null}
        </div>
      <//>
      <${Row} title=${t('settings.advanced.pixeldrain')} hint=${t('settings.advanced.pixeldrain_hint')}>
        <form class="inline" onSubmit=${(e) => {
          e.preventDefault();
          if (key.trim()) saveKey(key.trim(), t('settings.advanced.pixeldrain_saved'));
        }}>
          <input type="password" class="txt" autocomplete="off" aria-label=${t('settings.advanced.pixeldrain')}
            placeholder=${hasKey ? t('settings.advanced.pixeldrain_placeholder_saved') : ''} data-key-saved=${hasKey ? 'yes' : 'no'}
            value=${key} disabled=${!cfg} onInput=${(e) => setKey(e.target.value)} />
          <button type="submit" class="btn" disabled=${!cfg || !key.trim()}>${hasKey ? t('settings.advanced.pixeldrain_replace') : t('settings.save')}</button>
          ${hasKey
            ? html`<button type="button" class="btn" data-remove-key onClick=${() => saveKey(null, t('settings.advanced.pixeldrain_removed'))}>
                ${t('settings.advanced.pixeldrain_remove')}</button>`
            : null}
        </form>
      <//>
      <${Note} note=${note} />
      <${BroadcastOptions} />
    </div>`;
}
