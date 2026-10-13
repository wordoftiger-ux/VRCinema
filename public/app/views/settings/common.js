// Shared parts of the Settings sections: rows, switches, segmented choices, notes, and the
// program's settings (`/api/desktop/config`, window only) that several sections read and save.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { lang, t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { access, refreshInfo } from '../../store.js';

const CONFIG = '/api/desktop/config';

/** The program's settings as the window sees them; null until read. */
export const config = signal(null);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Whether the engine is (still) restarting: it did not answer, or said it is going away. */
const restarting = (e) => e.code === 'network' || e.status === 503;

/** Reads the settings, retrying while the engine restarts. Resolves to them or null. */
export async function loadConfig() {
  for (let attempt = 1; attempt <= 6; attempt += 1) {
    try {
      config.value = await api('GET', CONFIG);
      return config.value;
    } catch (e) {
      if (!restarting(e)) return null;
      await sleep(700);
    }
  }
  return null;
}

/**
 * Saves a part of the settings: `{ok: true, data}`, `{ok: false, error}` (the view goes back to
 * the saved values) or `{ok: false, restarting: true}` (the engine restarts for a new port or
 * network access; the settings are read again once it answers).
 */
export async function saveConfig(patch) {
  // Only a new port or network access restarts the server; for anything else a dropped
  // connection is an ordinary error.
  const restarts = 'port' in patch || 'lanAccess' in patch;
  try {
    const data = await api('PUT', CONFIG, patch);
    config.value = data;
    refreshInfo();
    return { ok: true, data };
  } catch (e) {
    if (restarts && restarting(e)) {
      await sleep(1500);
      await loadConfig();
      return { ok: false, restarting: true };
    }
    await loadConfig();
    return { ok: false, error: errorText(e) };
  }
}

/** What the section may do: window-only parts work in the window only (not while unknown). */
export function useAccess() {
  const mode = access.value;
  return { mode, windowed: mode === 'window', known: mode !== 'unknown' };
}

/** The program's settings for a window-only section: read on mount, null until then. */
export function useConfig() {
  const { windowed } = useAccess();
  useEffect(() => {
    if (windowed) loadConfig();
  }, [windowed]);
  return windowed ? config.value : null;
}

/** A note under a control: `[note, say]`; `say(kind, text)` with `ok`, `info` or `err`, `say(null)` clears. */
export function useNote() {
  const [note, setNote] = useState(null);
  // A note is text in the language it was made in: a language switch clears it.
  useEffect(() => setNote(null), [lang.value]);
  const say = (kind, text) => setNote(kind ? { kind, text } : null);
  return [note, say];
}

export function Note({ note }) {
  if (!note) return null;
  return html`<p class=${`set-note ${note.kind}`} role=${note.kind === 'err' ? 'alert' : 'status'}>${note.text}</p>`;
}

/**
 * Saves `patch` and says what happened in `say`. Resolves to the saved settings or null.
 * `okText` is the text for the note on success (a function of the answer, or text).
 */
export async function saveWith(say, patch, okText) {
  say(null);
  const r = await saveConfig(patch);
  if (r.ok) {
    say('ok', typeof okText === 'function' ? okText(r.data) : okText);
    return r.data;
  }
  if (r.restarting) say('info', t('settings.restarting'));
  else say('err', r.error);
  return null;
}

/** A line of a section: title and hint on the left, the control on the right. */
export function Row({ title, hint, id, block = false, children }) {
  return html`
    <div class=${block ? 'row2 block' : 'row2'}>
      <div class="row2-text">
        <b id=${id}>${title}</b>
        ${hint ? html`<small>${hint}</small>` : null}
      </div>
      ${children}
    </div>`;
}

/** An on/off switch; `label` names it for screen readers. */
export function Switch({ checked, onChange, disabled = false, label }) {
  return html`<button type="button" role="switch" aria-checked=${checked ? 'true' : 'false'} aria-label=${label}
    class=${checked ? 'sw on' : 'sw'} disabled=${disabled} onClick=${() => onChange(!checked)}></button>`;
}

/** A choice of one of a few: `options` are `{value, label}`. */
export function Segmented({ value, options, onChange, disabled = false, label }) {
  return html`
    <div class="seg" role="radiogroup" aria-label=${label}>
      ${options.map(
        (o) => html`<button type="button" role="radio" aria-checked=${value === o.value ? 'true' : 'false'}
          class=${value === o.value ? 'on' : ''} disabled=${disabled} data-value=${o.value}
          onClick=${() => value !== o.value && onChange(o.value)}>${o.label}</button>`,
      )}
    </div>`;
}

/** The note that a part works in the program's window only (shown outside it, once known). */
export function WindowNote() {
  const { windowed, known } = useAccess();
  return known && !windowed ? html`<p class="window-only set-window-only">${t('ui.window_only')}</p>` : null;
}

/** A group of rows with a heading. */
export function Block({ title, hint, children }) {
  return html`
    <section class="sblock">
      ${title ? html`<h3>${title}</h3>` : null}
      ${hint ? html`<p class="sblock-hint">${hint}</p>` : null}
      ${children}
    </section>`;
}

/** The languages of the program, by their own names (never translated). */
export const LANGUAGES = ['en', 'pl', 'es', 'de', 'fr', 'ja'];
export const LANGUAGE_NAME_KEYS = {
  en: 'settings.language_name.en',
  pl: 'settings.language_name.pl',
  es: 'settings.language_name.es',
  de: 'settings.language_name.de',
  fr: 'settings.language_name.fr',
  ja: 'settings.language_name.ja',
};

/** A list of the program's languages for a `<select>`. */
export function LanguageOptions() {
  return LANGUAGES.map((code) => html`<option value=${code}>${t(LANGUAGE_NAME_KEYS[code])}</option>`);
}

// ---------- The channel's settings (`/api/admin/channels/main/settings`, also in a remote session) ----------

const CHANNEL_SETTINGS = '/api/admin/channels/main/settings';

/** The channel's settings (with `popularDomains`); null until read. */
export const channelSettings = signal(null);

export async function loadChannelSettings() {
  try {
    channelSettings.value = await api('GET', CHANNEL_SETTINGS);
  } catch {
    // the next visit reads it again
  }
  return channelSettings.value;
}

/** Saves a part of the channel's settings: `{ok: true}` or `{ok: false, error}`. */
export async function saveChannel(patch) {
  try {
    const saved = await api('PUT', CHANNEL_SETTINGS, patch);
    // The answer holds the settings but not the list of popular sites the read brought.
    channelSettings.value = { popularDomains: channelSettings.value?.popularDomains ?? [], ...saved };
    return { ok: true };
  } catch (e) {
    await loadChannelSettings();
    return { ok: false, error: errorText(e) };
  }
}

/** Saves `patch` and says what happened in `say` (see `saveWith`). */
export async function saveChannelWith(say, patch, okText) {
  say(null);
  const r = await saveChannel(patch);
  if (r.ok) say('ok', okText);
  else say('err', r.error);
  return r.ok;
}
