// Settings → About: the version, checking for a new one, installing it or skipping it.

import { signal } from '/static/vendor/signals.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { appRoute, info, refreshInfo } from '../../store.js';
import { Note, WindowNote, saveConfig, useAccess, useNote } from './common.js';

/** The offer found by a check made here (`undefined`: no check yet, the info's offer counts). */
const checked = signal(undefined);
const checking = signal(false);
const installing = signal(false);

export function About() {
  const { managed } = useAccess();
  const [note, say] = useNote();
  const i = info.value;
  const offer = checked.value === undefined ? (i?.update ?? null) : checked.value;

  const check = async () => {
    checking.value = true;
    say('info', t('settings.about.checking'));
    try {
      const r = await api('GET', appRoute('/update'), undefined, { timeoutMs: 30000 });
      checked.value = r.available;
      say(r.available ? null : 'ok', r.available ? '' : t('settings.about.latest', { version: r.current }));
      refreshInfo();
    } catch (e) {
      say('err', errorText(e));
    } finally {
      checking.value = false;
    }
  };

  const install = async () => {
    installing.value = true;
    say('info', t('settings.about.installing'));
    try {
      const r = await api('POST', appRoute('/update/apply'), undefined, { timeoutMs: 600000 });
      say('ok', t('settings.about.installed', { version: r.version }));
    } catch (e) {
      installing.value = false;
      say('err', t('settings.about.install_failed', { error: errorText(e) }));
    }
  };

  const skip = async () => {
    const r = await saveConfig({ skippedUpdateVersion: offer.version });
    if (!r.ok) return say('err', r.error ?? t('settings.restarting'));
    checked.value = null;
    say('ok', t('settings.about.skipped', { version: offer.version }));
  };

  return html`
    <div class="set-rows">
      <${WindowNote} />
      <p class="about-version" data-version>${i?.version ? t('settings.about.version', { version: i.version }) : 'VRCinema'}</p>
      ${offer
        ? html`<div class="banner" data-banner="update">
            <p>${t('settings.about.available', { version: offer.version })}</p>
            ${offer.notes ? html`<p class="set-sub">${offer.notes}</p>` : null}
            <div class="pl-actions">
              <button type="button" class="btn pri" disabled=${installing.value} onClick=${install}>${t('settings.about.install')}</button>
              <button type="button" class="btn" disabled=${installing.value} onClick=${skip}>${t('settings.about.skip')}</button>
            </div>
          </div>`
        : null}
      <div class="pl-actions">
        <button type="button" class="btn" disabled=${!managed || checking.value} onClick=${check}>${t('settings.about.check')}</button>
      </div>
      <${Note} note=${note} />
    </div>`;
}
