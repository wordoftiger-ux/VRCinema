// Settings → Channel → VRChat: the video on air in the owner's VRChat chatbox over OSC (the
// engine sends `/chatbox/input`; spec 2026-11-08-ctl-osc). Channel settings, so a remote panel
// changes them too; they reach VRChat only on the PC that runs it.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { Block, Note, Row, Switch, saveChannelWith, useNote } from './common.js';

const REPEATS = [0, 10, 30, 60, 120, 300];
const MAX_TEMPLATE = 200;

/** An IP address or a host name, as the engine accepts it. */
export function validHost(text) {
  const h = String(text).trim();
  if (/^\[?[0-9a-fA-F:.]+\]?$/.test(h) && h.includes(':')) return true;
  return h.length > 0 && h.length <= 253 && h.split('.').every((l) => /^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(l));
}

export function Osc({ settings }) {
  const [note, say] = useNote();
  const saved = { host: settings.oscHost ?? '127.0.0.1', port: String(settings.oscPort ?? 9000) };
  const [target, setTarget] = useState(saved);
  useEffect(() => setTarget(saved), [saved.host, saved.port]);
  const [template, setTemplate] = useState(settings.oscTemplate ?? '');
  useEffect(() => setTemplate(settings.oscTemplate ?? ''), [settings.oscTemplate]);
  const on = Boolean(settings.oscEnabled);

  const saveTarget = (e) => {
    e.preventDefault();
    const port = Number(target.port);
    if (!validHost(target.host) || !Number.isInteger(port) || port < 1 || port > 65535) {
      return say('err', t('settings.channel.osc_target_invalid'));
    }
    saveChannelWith(say, { oscHost: target.host.trim(), oscPort: port }, t('settings.channel.osc_target_saved'));
  };
  const saveTemplate = (e) => {
    e.preventDefault();
    saveChannelWith(say, { oscTemplate: template }, t('settings.channel.osc_template_saved'));
  };
  const label = t('settings.channel.osc_enabled');
  return html`
    <${Block} title=${t('settings.channel.osc_block')} hint=${t('settings.channel.osc_block_hint')}>
      <div class="osc-settings" data-block="osc">
        <${Row} title=${label} hint=${t('settings.channel.osc_enabled_hint')}>
          <${Switch} label=${label} checked=${on}
            onChange=${(v) => saveChannelWith(say, { oscEnabled: v }, t(v ? 'settings.channel.osc_on' : 'settings.channel.osc_off'))} />
        <//>
        <form class="limits" data-form="osc-target" onSubmit=${saveTarget}>
          <label class="limit"><span>${t('settings.channel.osc_host')}</span>
            <input type="text" class="txt" name="oscHost" spellcheck="false" autocomplete="off" value=${target.host}
              onInput=${(e) => setTarget({ ...target, host: e.target.value })} /></label>
          <label class="limit"><span>${t('settings.channel.osc_port')}</span>
            <input type="number" class="num" min="1" max="65535" step="1" name="oscPort" value=${target.port}
              onInput=${(e) => setTarget({ ...target, port: e.target.value })} /></label>
          <button type="submit" class="btn pri">${t('settings.save')}</button>
        </form>
        <p class="set-sub">${t('settings.channel.osc_target_hint')}</p>
        <${Row} title=${t('settings.channel.osc_template')} hint=${t('settings.channel.osc_template_hint')} block>
          <form class="inline wide" data-form="osc-template" onSubmit=${saveTemplate}>
            <input type="text" class="txt" name="oscTemplate" maxlength=${MAX_TEMPLATE} spellcheck="false"
              aria-label=${t('settings.channel.osc_template')} placeholder="▶ {title} — {added}" value=${template}
              onInput=${(e) => setTemplate(e.target.value)} />
            <button type="submit" class="btn pri">${t('settings.save')}</button>
          </form>
        <//>
        <${Row} title=${t('settings.channel.osc_repeat')} hint=${t('settings.channel.osc_repeat_hint')}>
          <select class="sel" aria-label=${t('settings.channel.osc_repeat')} value=${String(settings.oscRepeatS ?? 0)}
            onChange=${(e) => saveChannelWith(say, { oscRepeatS: Number(e.target.value) }, t('settings.channel.osc_repeat_saved'))}>
            ${REPEATS.map((s) => html`<option value=${String(s)}>${s
              ? t('settings.channel.osc_repeat_every', { seconds: s })
              : t('settings.channel.osc_repeat_off')}</option>`)}
          </select>
        <//>
        <${Note} note=${note} />
      </div>
    <//>`;
}
