// Wizard: the info ticker burned into the stream. On/off, from time to time or continuous, how
// often, where its QR code shows, and the owner's messages (the list of Settings → Channel).
// Speed, the next video, the address and the progress line stay in Settings.

import { useEffect } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { focusOnMount } from '../focus.js';
import { Note, Row, Segmented, Switch, channelSettings, loadChannelSettings, saveChannelWith, useNote } from '../views/settings/common.js';
import { INTERVALS_S, Messages } from '../views/settings/ticker.js';
import { Later, StepNav } from './common.js';

export function TickerStep({ nav }) {
  const settings = channelSettings.value;
  const [note, say] = useNote();
  useEffect(() => {
    loadChannelSettings();
  }, []);
  const save = (patch, text) => saveChannelWith(say, patch, text);
  const enabled = Boolean(settings?.tickerEnabled);
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.ticker_title')}</h1>
    <p>${t('wizard.ticker_intro')}</p>
    ${settings
      ? html`
          <div class="wz-rows" data-ticker=${enabled ? 'on' : 'off'}>
            <${Row} title=${t('settings.ticker.enabled')} hint=${t('settings.ticker.enabled_hint')}>
              <${Switch} label=${t('settings.ticker.enabled')} checked=${enabled}
                onChange=${(v) => save({ tickerEnabled: v }, v ? t('settings.ticker.enabled_on') : t('settings.ticker.enabled_off_restart'))} />
            <//>
            <${Row} title=${t('settings.ticker.mode')} hint=${t('settings.ticker.mode_hint')}>
              <${Segmented} label=${t('settings.ticker.mode')} value=${settings.tickerMode} disabled=${!enabled}
                options=${[
                  { value: 'interval', label: t('settings.ticker.mode_interval') },
                  { value: 'continuous', label: t('settings.ticker.mode_continuous') },
                ]}
                onChange=${(v) => save({ tickerMode: v }, t('settings.ticker.saved_restart'))} />
            <//>
            <${Row} title=${t('settings.ticker.interval')} hint=${t('settings.ticker.interval_hint')}>
              <${Segmented} label=${t('settings.ticker.interval')} value=${settings.tickerIntervalS}
                disabled=${!enabled || settings.tickerMode !== 'interval'}
                options=${INTERVALS_S.map((s) => ({ value: s, label: t('units.minutes', { m: s / 60 }) }))}
                onChange=${(v) => save({ tickerIntervalS: v }, t('settings.ticker.saved_restart'))} />
            <//>
            <${Row} title=${t('settings.ticker.qr')} hint=${t('settings.ticker.qr_hint')}>
              <${Segmented} label=${t('settings.ticker.qr')} value=${settings.tickerQr} disabled=${!enabled}
                options=${[
                  { value: 'off', label: t('settings.ticker.qr_off') },
                  { value: 'with_ticker', label: t('settings.ticker.qr_with_ticker') },
                  { value: 'always', label: t('settings.ticker.qr_always') },
                ]}
                onChange=${(v) => save({ tickerQr: v }, t('settings.ticker.saved_restart'))} />
            <//>
            <${Messages} settings=${settings} say=${say} />
            <p class="wz-hint">${t('settings.ticker.restart_note')}</p>
            <${Note} note=${note} />
          </div>`
      : html`<p class="wz-hint">${t('settings.loading')}</p>`}
    <${Later} sections=${['channel']} />
    <${StepNav} nav=${nav} />`;
}
