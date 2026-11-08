// Wizard: downloads the tools the channel runs on, with progress. A failed download can be
// retried or skipped (the next step; Settings → Tools repairs it later).

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { focusOnMount } from '../focus.js';
import { TOOL_NAME_KEYS, TOOL_ORDER, rowState } from '../views/settings/tools.js';
import { settingsPlace } from './common.js';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Asks again while the engine does not answer yet (it may be starting). */
async function retrying(method, path) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await api(method, path);
    } catch (e) {
      if (attempt >= 6 || (e.code !== 'network' && e.status !== 503)) throw e;
      await sleep(700);
    }
  }
}

function Rows({ names, progress, all }) {
  return html`
    <div class="tool-list" data-job="running">
      ${names.map((name) => {
        const s = all ? { kind: 'ready', text: t('settings.tools.done') } : rowState(name, names, progress);
        const pct = s.kind === 'ready' ? 100 : s.kind === 'active' ? (s.ratio === null ? null : Math.round(Math.min(1, Math.max(0, s.ratio)) * 100)) : 0;
        const label = t(TOOL_NAME_KEYS[name] ?? 'settings.tools.name_other', { name });
        return html`<div class="tool-row" data-kind=${s.kind} data-tool=${name}>
          <div class="head"><span>${label}</span><span class="state">${s.text}</span></div>
          <div class=${pct === null ? 'bar indeterminate' : 'bar'} role="progressbar" aria-label=${label} aria-valuenow=${pct ?? undefined}><span style=${pct === null ? '' : `width:${pct}%`}></span></div>
        </div>`;
      })}
    </div>`;
}

export function ToolsStep({ nav }) {
  // state: running | done | failed
  const [state, setState] = useState({ phase: 'running', names: TOOL_ORDER, progress: {}, installed: false, error: null });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let alive = true;
    const set = (patch) => alive && setState((s) => ({ ...s, ...patch }));
    (async () => {
      try {
        set({ phase: 'running', error: null });
        const list = await retrying('GET', '/api/desktop/tools');
        const names = list.missing?.length ? list.missing : TOOL_ORDER;
        if (list.ok) return set({ phase: 'done', names: TOOL_ORDER, installed: true });
        set({ names, progress: {} });
        try {
          await api('POST', '/api/desktop/tools/install');
        } catch (e) {
          if (e.status !== 409) throw e; // already running: only watch it
        }
        let bad = 0;
        while (alive) {
          await sleep(500);
          let p;
          try {
            p = await api('GET', '/api/desktop/tools/progress');
            bad = 0;
          } catch (e) {
            if (++bad > 40) throw e;
            continue;
          }
          set({ progress: p });
          if (!p.running && p.done) {
            if (p.error) return set({ phase: 'failed', error: p.error });
            const after = await retrying('GET', '/api/desktop/tools');
            return after.ok ? set({ phase: 'done', progress: p }) : set({ phase: 'failed', error: t('wizard.tools_incomplete') });
          }
        }
      } catch (e) {
        set({ phase: 'failed', error: errorText(e) });
      }
    })();
    return () => {
      alive = false;
    };
  }, [attempt]);

  const { phase, names, progress, installed, error } = state;
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.tools_title')}</h1>
    <p>${t('wizard.tools_intro')}</p>
    <${Rows} names=${names} progress=${progress} all=${phase === 'done'} />
    ${phase === 'done'
      ? html`<p class="wz-note ok" role="status">${installed ? t('wizard.tools_present') : t('wizard.tools_ready')}</p>`
      : null}
    ${phase === 'failed'
      ? html`<p class="wz-note err" role="alert">${t('wizard.tools_failed', { error })}</p>
          <p class="wz-hint">${t('wizard.tools_skip_hint', { place: settingsPlace('tools') })}</p>`
      : null}
    <div class="wz-actions">
      ${phase === 'done' ? html`<button type="button" class="btn pri" data-next ref=${focusOnMount} onClick=${nav.next}>${t('wizard.next')}</button>` : null}
      ${phase === 'failed'
        ? html`<button type="button" class="btn pri" data-retry onClick=${() => setAttempt(attempt + 1)}>${t('wizard.retry')}</button>
            <button type="button" class="btn" data-skip onClick=${nav.next}>${t('wizard.skip')}</button>`
        : null}
    </div>`;
}
