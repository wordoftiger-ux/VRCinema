// Wizard: what the stream shows when no video plays. The last frame behind the waiting slates,
// the request page address and QR code on the idle slate (automatic once the channel has a
// public address), and the picture of audio files. The own background stays in Settings.

import { useEffect } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { focusOnMount } from '../focus.js';
import { Note, Row, channelSettings, loadChannelSettings, saveChannelWith, useNote } from '../views/settings/common.js';
import { FRAME_STYLES } from '../views/settings/slates.js';
import { AudioLook } from '../views/settings/audio.js';
import { Later, StepNav, settingsPlace } from './common.js';

export function LookStep({ nav }) {
  const settings = channelSettings.value;
  const [note, say] = useNote();
  useEffect(() => {
    loadChannelSettings();
  }, []);
  const frameLabel = t('settings.channel.slate_frame');
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.look_title')}</h1>
    <p>${t('wizard.look_intro')}</p>
    ${settings
      ? html`
          <div class="wz-rows">
            <p class="wz-note" data-idle-qr>${t('wizard.idle_qr')}</p>
            <${Row} title=${frameLabel} hint=${t('settings.channel.slate_frame_hint')}>
              <select class="sel" aria-label=${frameLabel} name="slateFrameStyle" value=${settings.slateFrameStyle ?? 'dim'}
                onChange=${(e) => saveChannelWith(say, { slateFrameStyle: e.target.value }, t('settings.channel.slate_frame_saved'))}>
                ${FRAME_STYLES.map((s) => html`<option value=${s.value}>${t(s.label)}</option>`)}
              </select>
            <//>
            <p class="wz-hint">${t('wizard.look_background', { place: settingsPlace('channel') })}</p>
            <${AudioLook} settings=${settings} say=${say} />
            <${Note} note=${note} />
          </div>`
      : html`<p class="wz-hint">${t('settings.loading')}</p>`}
    <${Later} sections=${['channel']} />
    <${StepNav} nav=${nav} />`;
}
