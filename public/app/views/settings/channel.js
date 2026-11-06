// Settings → Channel: the links viewers may send, the limits, moderation, viewer lists, the language of the
// slates, conversion, the info ticker and the viewers' nicks. These are the channel's own
// settings: they work in a remote session too.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { Block, LanguageOptions, Note, Row, Switch, channelSettings, loadChannelSettings, saveChannelWith, useNote } from './common.js';
import { Ticker } from './ticker.js';
import { Nicks } from './nicks.js';

const MODES = [
  { value: 'all', label: 'settings.channel.mode_all', hint: 'settings.channel.mode_all_hint' },
  { value: 'allowlist', label: 'settings.channel.mode_allowlist', hint: 'settings.channel.mode_allowlist_hint' },
  { value: 'blocklist', label: 'settings.channel.mode_blocklist', hint: 'settings.channel.mode_blocklist_hint' },
];

/** A site as the engine stores it: lower case, without the scheme, `www.` and path. */
export function normalizeDomain(text) {
  const s = String(text).trim().toLowerCase().replace(/^https?:\/\//, '').replace(/^www\./, '');
  return s.split('/')[0];
}

/** Sites typed in a field (separated by spaces, commas or lines); `bad` holds what is not a site. */
export function parseDomains(text) {
  const domains = [];
  const bad = [];
  for (const part of String(text).split(/[\s,;]+/).filter(Boolean)) {
    const domain = normalizeDomain(part);
    if (/^[^\s/@:?#]+\.[^\s/@:?#]+$/.test(domain)) domains.push(domain);
    else bad.push(part);
  }
  return { domains, bad };
}

/** One list of sites as chips, with a field to add more; `extra` are more buttons. */
function DomainList({ id, title, hint, domains, active, onChange, extra }) {
  const [text, setText] = useState('');
  const [error, setError] = useState(null);
  const add = (e) => {
    e.preventDefault();
    const { domains: added, bad } = parseDomains(text);
    if (bad.length) return setError(t('settings.channel.domain_invalid', { text: bad[0] }));
    setError(null);
    if (!added.length) return;
    setText('');
    onChange([...new Set([...domains, ...added])]);
  };
  return html`
    <div class=${active ? 'domains' : 'domains idle'} data-list=${id}>
      <b id=${`list-${id}`}>${title}</b>
      <small>${hint}</small>
      <ul class="chips" aria-labelledby=${`list-${id}`}>
        ${domains.length
          ? domains.map((d) => html`<li class="chip"><span>${d}</span>
              <button type="button" aria-label=${t('settings.channel.domain_remove', { domain: d })} title=${t('settings.channel.domain_remove', { domain: d })}
                onClick=${() => onChange(domains.filter((x) => x !== d))}>×</button></li>`)
          : html`<li class="chips-empty">${t('settings.channel.domains_empty')}</li>`}
      </ul>
      <form class="inline wide" onSubmit=${add}>
        <input type="text" class="txt" aria-label=${t('settings.channel.domain_add_label', { list: title })}
          placeholder="example.com" value=${text} onInput=${(e) => setText(e.target.value)} />
        <button type="submit" class="btn">${t('settings.channel.domain_add')}</button>
        ${extra}
      </form>
      ${error ? html`<p class="set-note err" role="alert">${error}</p>` : null}
    </div>`;
}

function LinkModes({ settings, say }) {
  const save = (patch, text) => saveChannelWith(say, patch, text);
  const popular = settings.popularDomains ?? [];
  return html`
    <${Block} title=${t('settings.channel.links')} hint=${t('settings.channel.links_hint')}>
      <div class="radios" role="radiogroup" aria-label=${t('settings.channel.links')}>
        ${MODES.map(
          (m) => html`<label class="radio">
            <input type="radio" name="linkMode" value=${m.value} checked=${settings.linkMode === m.value}
              onChange=${() => save({ linkMode: m.value }, t('settings.channel.mode_saved'))} />
            <span><b>${t(m.label)}</b><small>${t(m.hint)}</small></span>
          </label>`,
        )}
      </div>
      <${DomainList} id="allowed" title=${t('settings.channel.allowed')} hint=${t('settings.channel.allowed_hint')}
        domains=${settings.allowedDomains} active=${settings.linkMode === 'allowlist'}
        onChange=${(list) => save({ allowedDomains: list }, t('settings.channel.domains_saved'))}
        extra=${html`<button type="button" class="btn" data-action="popular"
          onClick=${() => save({ allowedDomains: [...new Set([...settings.allowedDomains, ...popular])] }, t('settings.channel.domains_saved'))}>
          ${t('settings.channel.popular')}</button>`} />
      <${DomainList} id="blocked" title=${t('settings.channel.blocked')} hint=${t('settings.channel.blocked_hint')}
        domains=${settings.blockedDomains} active=${settings.linkMode === 'blocklist'}
        onChange=${(list) => save({ blockedDomains: list }, t('settings.channel.domains_saved'))} />
      ${settings.linkMode === 'allowlist' && !settings.allowedDomains.length
        ? html`<p class="banner warn" role="status" data-warning="allowlist_empty">${t('settings.channel.allowed_empty_warning')}</p>`
        : null}
      ${settings.linkMode === 'blocklist' && !settings.blockedDomains.length
        ? html`<p class="banner warn" role="status" data-warning="blocklist_empty">${t('settings.channel.blocked_empty_warning')}</p>`
        : null}
      <p class="set-sub">${t('settings.channel.home_network')}</p>
    <//>`;
}

function Limits({ settings, say }) {
  const fromSettings = () => ({
    maxDurationMin: String(Math.round(settings.maxDurationS / 60)),
    maxActivePerRequester: String(settings.maxActivePerRequester),
    requestLimitCount: String(settings.requestLimitCount),
    requestLimitWindowMin: String(Math.round(settings.requestLimitWindowS / 60)),
  });
  const [form, setForm] = useState(fromSettings);
  const key = JSON.stringify([settings.maxDurationS, settings.maxActivePerRequester, settings.requestLimitCount, settings.requestLimitWindowS]);
  useEffect(() => setForm(fromSettings()), [key]);
  const field = (name, label) => html`
    <label class="limit"><span>${label}</span>
      <input type="number" class="num" min="1" step="1" name=${name} value=${form[name]}
        onInput=${(e) => setForm({ ...form, [name]: e.target.value })} /></label>`;
  const submit = (e) => {
    e.preventDefault();
    const numbers = Object.fromEntries(Object.entries(form).map(([k, v]) => [k, Number(v)]));
    if (!Object.values(numbers).every((n) => Number.isInteger(n) && n >= 1)) {
      return say('err', t('settings.channel.limits_invalid'));
    }
    saveChannelWith(say, {
      maxDurationS: numbers.maxDurationMin * 60,
      maxActivePerRequester: numbers.maxActivePerRequester,
      requestLimitCount: numbers.requestLimitCount,
      requestLimitWindowS: numbers.requestLimitWindowMin * 60,
    }, t('settings.channel.limits_saved'));
  };
  return html`
    <${Block} title=${t('settings.channel.limits')}>
      <form class="limits" onSubmit=${submit}>
        ${field('maxDurationMin', t('settings.channel.max_duration'))}
        ${field('maxActivePerRequester', t('settings.channel.max_active'))}
        ${field('requestLimitCount', t('settings.channel.request_count'))}
        ${field('requestLimitWindowMin', t('settings.channel.request_window'))}
        <button type="submit" class="btn pri">${t('settings.save')}</button>
      </form>
    <//>`;
}

/** Viewer lists: the switch and the most videos one list adds (1–50). */
function ViewerLists({ settings, say }) {
  const current = String(settings.viewerListMax ?? 10);
  const [max, setMax] = useState(current);
  useEffect(() => setMax(current), [current]);
  const submit = (e) => {
    e.preventDefault();
    const n = Number(max);
    if (!Number.isInteger(n) || n < 1 || n > 50) return say('err', t('settings.channel.viewer_list_max_invalid'));
    saveChannelWith(say, { viewerListMax: n }, t('settings.channel.viewer_list_max_saved'));
  };
  const label = t('settings.channel.viewer_lists');
  return html`
    <${Block} title=${t('settings.channel.viewer_lists_block')}>
      <${Row} title=${label} hint=${t('settings.channel.viewer_lists_hint')}>
        <${Switch} label=${label} checked=${Boolean(settings.viewerLists)}
          onChange=${(v) => saveChannelWith(say, { viewerLists: v }, t(v ? 'settings.channel.viewer_lists_on' : 'settings.channel.viewer_lists_off'))} />
      <//>
      <form class="limits" data-form="viewer-lists" onSubmit=${submit}>
        <label class="limit"><span>${t('settings.channel.viewer_list_max')}</span>
          <input type="number" class="num" min="1" max="50" step="1" name="viewerListMax" value=${max}
            disabled=${!settings.viewerLists} onInput=${(e) => setMax(e.target.value)} /></label>
        <button type="submit" class="btn pri" disabled=${!settings.viewerLists}>${t('settings.save')}</button>
      </form>
      <p class="set-sub">${t('settings.channel.viewer_list_max_hint')}</p>
    <//>`;
}

/** The owner's upload speed (Mbit/s, 0: not given): the panel warns when viewers need about that. */
function Upload({ settings, say }) {
  const current = String(settings.uploadMbps ?? 0);
  const [mbps, setMbps] = useState(current);
  useEffect(() => setMbps(current), [current]);
  const submit = (e) => {
    e.preventDefault();
    const n = Number(String(mbps).replace(',', '.'));
    if (!Number.isFinite(n) || n < 0) return say('err', t('settings.channel.upload_invalid'));
    saveChannelWith(say, { uploadMbps: n }, t('settings.channel.upload_saved'));
  };
  return html`
    <form class="limits" data-form="upload" onSubmit=${submit}>
      <label class="limit"><span>${t('settings.channel.upload')}</span>
        <input type="number" class="num" min="0" step="0.1" name="uploadMbps" value=${mbps}
          onInput=${(e) => setMbps(e.target.value)} /></label>
      <button type="submit" class="btn pri">${t('settings.save')}</button>
    </form>
    <p class="set-sub">${t('settings.channel.upload_hint')}</p>`;
}

export function ChannelSettings() {
  const settings = channelSettings.value;
  const [note, say] = useNote();
  useEffect(() => {
    loadChannelSettings();
  }, []);
  if (!settings) return html`<div class="set-rows"><p class="set-sub">${t('settings.loading')}</p></div>`;
  const toggle = (key, label, hint, on, off) => html`
    <${Row} title=${label} hint=${hint}>
      <${Switch} label=${label} checked=${Boolean(settings[key])}
        onChange=${(v) => saveChannelWith(say, { [key]: v }, v ? on : off)} />
    <//>`;
  return html`
    <div class="set-rows channel-settings">
      <${LinkModes} settings=${settings} say=${say} />
      <${Limits} settings=${settings} say=${say} />
      <${Block} title=${t('settings.channel.moderation_block')}>
        ${toggle('moderation', t('settings.channel.moderation'), t('settings.channel.moderation_hint'), t('settings.channel.moderation_on'), t('settings.channel.moderation_off'))}
      <//>
      <${ViewerLists} settings=${settings} say=${say} />
      <${Block} title=${t('settings.channel.stream')}>
        <${Row} title=${t('settings.channel.slate_language')} hint=${t('settings.channel.slate_language_hint')}>
          <select class="sel" aria-label=${t('settings.channel.slate_language')} value=${settings.slateLanguage}
            onChange=${(e) => saveChannelWith(say, { slateLanguage: e.target.value }, settings.tickerEnabled ? t('settings.channel.slate_language_saved_restart') : t('settings.channel.slate_language_saved'))}>
            <${LanguageOptions} />
          </select>
        <//>
        ${toggle('forceTranscode', t('settings.channel.force_transcode'), t('settings.channel.force_transcode_hint'), t('settings.channel.force_transcode_on'), t('settings.channel.force_transcode_off'))}
        ${toggle('discontinuityTags', t('settings.channel.discontinuity'), t('settings.channel.discontinuity_hint'), t('settings.channel.discontinuity_on'), t('settings.channel.discontinuity_off'))}
        <${Upload} settings=${settings} say=${say} />
      <//>
      <${Note} note=${note} />
      <${Ticker} />
      <${Nicks} />
    </div>`;
}
