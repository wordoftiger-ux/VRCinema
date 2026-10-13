// Settings → Log: the last entries of the program's log. The engine writes them in the language
// of the request, so the log follows the interface's language when it is read again.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { lang, t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { copyText } from '../../components/playerbar.js';
import { Note, WindowNote, useAccess, useNote } from './common.js';

const SHOWN = 200;

/** The lines of the log; null until read. */
const lines = signal(null);

export function Log() {
  const { windowed } = useAccess();
  const [note, say] = useNote();
  const [copied, setCopied] = useState(false);
  const view = useRef(null);

  const load = async () => {
    try {
      const r = await api('GET', '/api/desktop/log');
      lines.value = r.lines.slice(-SHOWN);
      say(null);
    } catch (e) {
      say('err', errorText(e));
    }
  };

  // Read on entering and again when the language changes (the engine writes the log per language).
  useEffect(() => {
    if (windowed) load();
  }, [windowed, lang.value]);
  useEffect(() => {
    if (view.current) view.current.scrollTop = view.current.scrollHeight;
  }, [lines.value]);

  const copy = async () => {
    try {
      await copyText((lines.value ?? []).join('\n'));
    } catch (e) {
      return say('err', errorText(e));
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const shown = windowed ? lines.value : null;
  return html`
    <div class="set-rows">
      <${WindowNote} />
      <pre class="log" ref=${view} tabindex="0" aria-label=${t('settings.log.label')} data-log>${shown ? (shown.join('\n') || t('settings.log.empty')) : ''}</pre>
      <div class="pl-actions">
        <button type="button" class="btn" disabled=${!windowed} onClick=${load}>${t('settings.log.refresh')}</button>
        <button type="button" class="btn" disabled=${!windowed || !shown?.length} onClick=${copy}>
          <span aria-live="polite">${copied ? t('share.copied') : t('settings.log.copy')}</span></button>
      </div>
      <${Note} note=${note} />
    </div>`;
}
