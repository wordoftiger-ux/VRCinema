// Wizard: the picture. Which graphics card can convert it (the engine tests it; a run again
// with a saved result shows that result and offers the test), the encoder choice, the note for
// a light computer converting on its processor (Raspberry Pi 5) and the highest resolution.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { focusOnMount } from '../focus.js';
import { ENCODER_OPTIONS, ENCODER_RESULT_KEYS } from '../views/settings/tools.js';
import { HEIGHTS } from '../views/settings/broadcast-options.js';
import { Note, Row, channelSettings, loadChannelSettings, loadConfig, saveChannelWith, saveWith, useNote } from '../views/settings/common.js';
import { Later, StepNav } from './common.js';

export function EncoderStep({ nav, again }) {
  // {phase: 'loading'|'testing'|'done'|'failed', encoder, error}
  const [state, setState] = useState({ phase: 'loading' });
  const [attempt, setAttempt] = useState(0);
  const [cfg, setCfg] = useState(null);
  const [light, setLight] = useState(false);
  const [note, say] = useNote();
  const settings = channelSettings.value;

  useEffect(() => {
    loadChannelSettings();
    api('GET', '/api/admin/environment').then(
      (env) => setLight(Boolean(env?.lightProfile)),
      () => {},
    );
  }, []);

  useEffect(() => {
    let alive = true;
    (async () => {
      const saved = await loadConfig();
      if (!alive) return;
      setCfg(saved);
      // A run again keeps the last result; the first run (or "Test the card") tests now.
      if (attempt === 0 && again && saved?.detectedEncoder) {
        setState({ phase: 'done', encoder: saved.detectedEncoder });
        return;
      }
      setState({ phase: 'testing' });
      try {
        const r = await api('POST', '/api/desktop/encoder/detect', undefined, { timeoutMs: 60000 });
        if (!alive) return;
        setState({ phase: 'done', encoder: r.detectedEncoder ?? r.encoder });
        const fresh = await loadConfig();
        if (alive && fresh) setCfg(fresh);
      } catch (e) {
        if (alive) setState({ phase: 'failed', error: errorText(e) });
      }
    })();
    return () => {
      alive = false;
    };
  }, [attempt]);

  const chooseEncoder = async (value) => {
    const saved = await saveWith(say, { videoEncoder: value }, value === 'auto' ? t('settings.tools.encoder_auto') : t('settings.tools.encoder_saved'));
    if (saved) setCfg(saved);
  };

  const { phase, encoder, error } = state;
  const gpu = encoder && encoder !== 'libx264';
  const encoderLabel = t('settings.tools.encoder');
  const heightLabel = t('settings.advanced.max_height');
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.encoder_title')}</h1>
    <p>${t('wizard.encoder_intro')}</p>
    ${phase === 'testing' ? html`<p class="wz-note" role="status">${t('wizard.encoder_testing')}</p>` : null}
    ${phase === 'done'
      ? html`<div class="wz-result" data-encoder=${encoder}>
          <strong>${t(ENCODER_RESULT_KEYS[encoder] ?? 'settings.tools.result_cpu')}</strong>
          <span>${gpu ? t('wizard.encoder_gpu') : t('wizard.encoder_cpu')}</span></div>`
      : null}
    ${phase === 'failed' ? html`<p class="wz-note err" role="alert">${t('wizard.encoder_failed', { error })}</p>` : null}
    ${phase === 'done' || phase === 'failed'
      ? html`<div class="wz-inline-actions">
          <button type="button" class="btn" data-retry onClick=${() => setAttempt(attempt + 1)}>
            ${phase === 'failed' ? t('wizard.retry') : t('settings.tools.test')}</button></div>`
      : null}
    ${light ? html`<p class="wz-note" data-light-profile>${t('settings.advanced.light_profile_note')}</p>` : null}
    <div class="wz-rows">
      <${Row} title=${encoderLabel} hint=${t('settings.tools.encoder_hint')}>
        <select class="sel" aria-label=${encoderLabel} name="videoEncoder" value=${cfg?.videoEncoder ?? 'auto'} disabled=${!cfg}
          onChange=${(e) => chooseEncoder(e.target.value)}>
          ${ENCODER_OPTIONS.map((o) => html`<option value=${o.value}>${t(o.key)}</option>`)}
        </select>
      <//>
      <${Row} title=${heightLabel} hint=${t('settings.advanced.max_height_hint')}>
        <select class="sel" aria-label=${heightLabel} name="maxHeight" value=${String(settings?.maxHeight ?? 0)} disabled=${!settings}
          onChange=${(e) => saveChannelWith(say, { maxHeight: Number(e.target.value) }, t('settings.advanced.saved'))}>
          ${HEIGHTS.map((h) => html`<option value=${String(h)}>${h ? t('settings.advanced.max_height_value', { n: h }) : t('settings.advanced.max_height_auto')}</option>`)}
        </select>
      <//>
      <${Note} note=${note} />
    </div>
    <${Later} sections=${['tools', 'advanced']} />
    <${StepNav} nav=${nav} />`;
}
