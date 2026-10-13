// Wizard step 3: which graphics card can convert the picture (the engine tests it).

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { focusOnMount } from '../focus.js';
import { ENCODER_RESULT_KEYS } from '../views/settings/tools.js';

export function EncoderStep({ next }) {
  // {phase: 'testing'|'done'|'failed', encoder, error}
  const [state, setState] = useState({ phase: 'testing' });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let alive = true;
    setState({ phase: 'testing' });
    api('POST', '/api/desktop/encoder/detect', undefined, { timeoutMs: 60000 }).then(
      (r) => alive && setState({ phase: 'done', encoder: r.detectedEncoder ?? r.encoder }),
      (e) => alive && setState({ phase: 'failed', error: errorText(e) }),
    );
    return () => {
      alive = false;
    };
  }, [attempt]);

  const { phase, encoder, error } = state;
  const gpu = encoder && encoder !== 'libx264';
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
    <div class="wz-actions">
      ${phase === 'done' ? html`<button type="button" class="btn pri" data-next ref=${focusOnMount} onClick=${next}>${t('wizard.next')}</button>` : null}
      ${phase === 'failed'
        ? html`<button type="button" class="btn pri" data-retry onClick=${() => setAttempt(attempt + 1)}>${t('wizard.retry')}</button>
            <button type="button" class="btn" data-skip onClick=${next}>${t('wizard.encoder_skip')}</button>`
        : null}
    </div>`;
}
