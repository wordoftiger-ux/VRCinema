// Wizard: the optional extras. OBS (address and stream key with Copy, the home network and the
// priority: the Settings → OBS component), the video on air in the VRChat chatbox (the switch;
// the address and the text stay in Settings → Channel), and one line about what Settings →
// Advanced holds (it is only named here, nothing there is changed).

import { useEffect } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { focusOnMount } from '../focus.js';
import { Note, Row, Switch, channelSettings, loadChannelSettings, saveChannelWith, useNote } from '../views/settings/common.js';
import { Obs } from '../views/settings/obs.js';
import { Later, StepNav, settingsPlace } from './common.js';

export function ExtrasStep({ nav }) {
  const settings = channelSettings.value;
  const [note, say] = useNote();
  useEffect(() => {
    loadChannelSettings();
  }, []);
  const oscLabel = t('settings.channel.osc_enabled');
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.extras_title')}</h1>
    <p>${t('wizard.extras_intro')}</p>
    <h2>${t('settings.nav.obs')}</h2>
    <div class="wz-rows" data-block="obs"><${Obs} /></div>
    <h2>${t('settings.channel.osc_block')}</h2>
    <div class="wz-rows" data-block="osc">
      <${Row} title=${oscLabel} hint=${t('settings.channel.osc_enabled_hint')}>
        <${Switch} label=${oscLabel} checked=${Boolean(settings?.oscEnabled)} disabled=${!settings}
          onChange=${(v) => saveChannelWith(say, { oscEnabled: v }, t(v ? 'settings.channel.osc_on' : 'settings.channel.osc_off'))} />
      <//>
      <${Note} note=${note} />
    </div>
    <h2>${t('settings.nav.advanced')}</h2>
    <p class="wz-hint" data-advanced>${t('wizard.advanced_pointer', { place: settingsPlace('advanced') })}</p>
    <${Later} sections=${['obs', 'channel']} />
    <${StepNav} nav=${nav} />`;
}
