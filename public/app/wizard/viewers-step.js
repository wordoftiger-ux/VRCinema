// Wizard: who can add videos and how. The links viewers may send (all, allowed sites, blocked
// sites; the lists themselves are edited in Settings → Channel), approval, the limits per
// viewer, viewer lists and files from viewers (off unless switched on, with a security note).
// Every control saves at once as its twin in Settings → Channel does.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { focusOnMount } from '../focus.js';
import { Note, Row, Switch, channelSettings, loadChannelSettings, saveChannelWith, useNote } from '../views/settings/common.js';
import { Later, StepNav, settingsPlace } from './common.js';

const MODES = [
  { value: 'all', label: 'settings.channel.mode_all', hint: 'settings.channel.mode_all_hint' },
  { value: 'allowlist', label: 'settings.channel.mode_allowlist', hint: 'settings.channel.mode_allowlist_hint' },
  { value: 'blocklist', label: 'settings.channel.mode_blocklist', hint: 'settings.channel.mode_blocklist_hint' },
];

/** The longest video and the videos in the queue per person, saved together. */
function Limits({ settings, say }) {
  const fromSettings = () => ({
    maxDurationMin: String(Math.round(settings.maxDurationS / 60)),
    maxActivePerRequester: String(settings.maxActivePerRequester),
  });
  const [form, setForm] = useState(fromSettings);
  useEffect(() => setForm(fromSettings()), [settings.maxDurationS, settings.maxActivePerRequester]);
  const field = (name, label) => html`
    <label class="limit"><span>${label}</span>
      <input type="number" class="num" min="1" step="1" name=${name} value=${form[name]}
        onInput=${(e) => setForm({ ...form, [name]: e.target.value })} /></label>`;
  const submit = (e) => {
    e.preventDefault();
    const minutes = Number(form.maxDurationMin);
    const active = Number(form.maxActivePerRequester);
    if (![minutes, active].every((n) => Number.isInteger(n) && n >= 1)) return say('err', t('settings.channel.limits_invalid'));
    saveChannelWith(say, { maxDurationS: minutes * 60, maxActivePerRequester: active }, t('settings.channel.limits_saved'));
  };
  return html`
    <form class="limits" data-form="limits" onSubmit=${submit}>
      ${field('maxActivePerRequester', t('settings.channel.max_active'))}
      ${field('maxDurationMin', t('settings.channel.max_duration'))}
      <button type="submit" class="btn pri">${t('settings.save')}</button>
    </form>`;
}

export function ViewersStep({ nav }) {
  const settings = channelSettings.value;
  const [note, say] = useNote();
  useEffect(() => {
    loadChannelSettings();
  }, []);
  const save = (patch, text) => saveChannelWith(say, patch, text);
  const toggle = (key, label, hint, on, off) => html`
    <${Row} title=${label} hint=${hint}>
      <${Switch} label=${label} checked=${Boolean(settings[key])} onChange=${(v) => save({ [key]: v }, v ? on : off)} />
    <//>`;
  const popular = settings?.popularDomains ?? [];
  return html`
    <h1 tabindex="-1" ref=${focusOnMount}>${t('wizard.viewers_title')}</h1>
    <p>${t('wizard.viewers_intro')}</p>
    ${settings
      ? html`
          <div class="wz-rows">
            <h2 id="wz-links">${t('settings.channel.links')}</h2>
            <div class="radios" role="radiogroup" aria-labelledby="wz-links">
              ${MODES.map((m) => html`<label class="radio">
                <input type="radio" name="linkMode" value=${m.value} checked=${settings.linkMode === m.value}
                  onChange=${() => save({ linkMode: m.value }, t('settings.channel.mode_saved'))} />
                <span><b>${t(m.label)}</b><small>${t(m.hint)}</small></span>
              </label>`)}
            </div>
            ${settings.linkMode === 'allowlist' && !settings.allowedDomains.length
              ? html`<p class="banner warn" role="status" data-warning="allowlist_empty">${t('settings.channel.allowed_empty_warning')}
                  <button type="button" class="btn" data-action="popular"
                    onClick=${() => save({ allowedDomains: [...new Set([...settings.allowedDomains, ...popular])] }, t('settings.channel.domains_saved'))}>
                    ${t('settings.channel.popular')}</button></p>`
              : null}
            ${settings.linkMode === 'blocklist' && !settings.blockedDomains.length
              ? html`<p class="banner warn" role="status" data-warning="blocklist_empty">${t('settings.channel.blocked_empty_warning')}</p>`
              : null}
            ${settings.linkMode !== 'all' ? html`<p class="wz-hint" data-lists-later>${t('wizard.viewers_lists_later', { place: settingsPlace('channel') })}</p>` : null}
            ${toggle('moderation', t('settings.channel.moderation'), t('settings.channel.moderation_hint'), t('settings.channel.moderation_on'), t('settings.channel.moderation_off'))}
            <${Limits} settings=${settings} say=${say} />
            ${toggle('viewerLists', t('settings.channel.viewer_lists'), t('settings.channel.viewer_lists_hint'), t('settings.channel.viewer_lists_on'), t('settings.channel.viewer_lists_off'))}
            ${toggle('viewerUploads', t('settings.channel.uploads'), t('settings.channel.uploads_hint'), t('settings.channel.uploads_on'), t('settings.channel.uploads_off'))}
            <p class="wz-hint wz-security" data-security-note>${t('wizard.uploads_security')}</p>
            <${Note} note=${note} />
          </div>`
      : html`<p class="wz-hint">${t('settings.loading')}</p>`}
    <${Later} sections=${['channel']} />
    <${StepNav} nav=${nav} />`;
}
