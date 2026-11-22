// Settings → Addons: the Stremio-compatible addons the catalog and the sources come from. Add one by the
// address of its manifest.json, switch it on or off, change the order, refresh its description or remove it.
// These work in a remote session too (the engine does the asking).

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { Icon } from '../../icons.js';
import { api, errorText, listAddons } from '../../api.js';
import { openDialog } from '../../components/dialog.js';
import { Block, Note, Switch, useNote } from './common.js';

const ADDONS = '/api/admin/addons';

const PROVIDES = [
  ['catalogSearch', 'settings.addons.provides_catalog'],
  ['meta', 'settings.addons.provides_meta'],
  ['stream', 'settings.addons.provides_stream'],
];

const typeName = (type) => (type === 'movie' || type === 'series' ? t(`search.type_${type}`) : type);

function AddonRow({ addon, first, last, busy, run }) {
  const named = (key) => t(key, { name: addon.name });
  const confirmRemove = () =>
    openDialog({
      title: () => t('settings.addons.remove_title'),
      body: () => html`<p class="dialog-intro">${t('settings.addons.remove_body', { name: addon.name })}</p>`,
      buttons: [
        {
          key: 'settings.addons.remove_button',
          danger: true,
          action: () => run(() => api('DELETE', `${ADDONS}/${addon.id}`), t('settings.addons.removed', { name: addon.name })),
        },
        { key: 'settings.cancel' },
      ],
    });
  const post = (body, text) => run(() => api('POST', `${ADDONS}/${addon.id}`, body, { timeoutMs: 20000 }), text);
  return html`
    <li class=${addon.enabled ? 'ad-row' : 'ad-row off'} data-addon=${addon.id}>
      ${addon.logo
        ? html`<img class="ad-logo" src=${addon.logo} alt="" loading="lazy" referrerpolicy="no-referrer"
            onError=${(e) => (e.currentTarget.hidden = true)} />`
        : html`<span class="ad-logo ad-logo-none" aria-hidden="true"></span>`}
      <div class="ad-text">
        <b class="ad-name">${addon.name}${addon.version ? html` <small>${t('settings.addons.version', { version: addon.version })}</small>` : null}</b>
        ${addon.description ? html`<small class="ad-desc">${addon.description}</small>` : null}
        <span class="ad-chips">
          ${PROVIDES.filter(([key]) => addon.provides?.[key]).map(([, label]) => html`<span class="chip-tag">${t(label)}</span>`)}
          ${(addon.types ?? []).map((type) => html`<span class="chip-tag chip-type">${typeName(type)}</span>`)}
        </span>
      </div>
      <span class="ad-actions">
        <button type="button" class="act-button ad-up" disabled=${busy || first} aria-label=${named('settings.addons.up')}
          title=${named('settings.addons.up')} onClick=${() => post({ move: 'up' })}><${Icon} name="up" size=${14} /></button>
        <button type="button" class="act-button ad-down" disabled=${busy || last} aria-label=${named('settings.addons.down')}
          title=${named('settings.addons.down')} onClick=${() => post({ move: 'down' })}><${Icon} name="down" size=${14} /></button>
        <button type="button" class="act-button ad-refresh" disabled=${busy} aria-label=${named('settings.addons.refresh')}
          title=${named('settings.addons.refresh')}
          onClick=${() => post({ refresh: true }, t('settings.addons.refreshed', { name: addon.name }))}><${Icon} name="sync" size=${14} /></button>
        <button type="button" class="act-button no ad-remove" disabled=${busy} aria-label=${named('settings.addons.remove')}
          title=${named('settings.addons.remove')} onClick=${confirmRemove}><${Icon} name="close" size=${14} /></button>
        <${Switch} label=${named('settings.addons.toggle')} checked=${addon.enabled} disabled=${busy}
          onChange=${(on) => post({ enabled: on }, t(on ? 'settings.addons.on' : 'settings.addons.off', { name: addon.name }))} />
      </span>
    </li>`;
}

export function Addons() {
  const [addons, setAddons] = useState(null);
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [note, say] = useNote();

  const load = async () => {
    try {
      setAddons(await listAddons());
    } catch (e) {
      setAddons((old) => old ?? []);
      say('err', errorText(e));
    }
  };
  useEffect(() => {
    load();
  }, []);

  /** Runs a change, says `okText` and reads the list again (also after a failure: the engine's list is the truth). */
  const run = async (job, okText) => {
    if (busy) return;
    setBusy(true);
    say(null);
    try {
      await job();
      if (okText) say('ok', okText);
    } catch (e) {
      say('err', errorText(e));
    } finally {
      await load();
      setBusy(false);
    }
  };

  const add = (e) => {
    e.preventDefault();
    const text = url.trim();
    if (!text || busy) return;
    run(async () => {
      const added = await api('POST', ADDONS, { url: text }, { timeoutMs: 20000 });
      setUrl('');
      say('ok', t('settings.addons.added', { name: added.name }));
    });
  };

  return html`
    <div class="set-rows addons-settings">
      <p class="set-sub addons-note" data-note="responsibility">${t('settings.addons.note')}</p>
      <p class="set-sub addons-note" data-note="expiry">${t('settings.addons.expiry_hint')}</p>
      <${Block} title=${t('settings.addons.add_block')}>
        <form class="inline wide ad-add" onSubmit=${add}>
          <input type="text" class="txt" name="addonUrl" inputmode="url" autocomplete="off" spellcheck="false"
            aria-label=${t('settings.addons.url_label')} placeholder="https://addons.example/manifest.json"
            value=${url} onInput=${(e) => setUrl(e.currentTarget.value)} />
          <button type="submit" class="btn pri" disabled=${busy || !url.trim()}>${busy ? t('settings.addons.adding') : t('settings.addons.add')}</button>
        </form>
        <${Note} note=${note} />
      <//>
      <${Block} title=${t('settings.addons.list_block')}>
        ${addons === null
          ? html`<p class="set-sub">${t('settings.loading')}</p>`
          : addons.length
            ? html`<ul class="ad-list">${addons.map((a, i) => html`<${AddonRow} key=${a.id} addon=${a} first=${i === 0} last=${i === addons.length - 1} busy=${busy} run=${run} />`)}</ul>`
            : html`<p class="q-empty">${t('settings.addons.empty')}</p>`}
      <//>
    </div>`;
}
