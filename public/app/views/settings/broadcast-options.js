// Settings → Advanced, the broadcast options (spec 2026-11-06-advanced-design): transitions
// between videos, the bitrate peak limit, burned-in subtitles, loudness normalisation, the
// black picture / silence alarms and the broadcast journal with its export. These are channel
// settings (`/api/admin/channels/main/settings`), so a remote panel has them too; the journal
// is `/api/admin/channels/main/journal`.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { lang, t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { Block, Note, Row, Segmented, Switch, channelSettings, loadChannelSettings, saveChannelWith, useNote } from './common.js';

const JOURNAL = '/api/admin/channels/main/journal';
const FADES = [0.5, 1, 1.5, 2, 3];
const TARGETS = [-23, -18, -16, -14];

const num = (v) => Number(String(v).replace(',', '.'));

/** Number fields saved together: `fields` are `{key, label, min, max, step}`. */
function Numbers({ settings, say, name, fields, okText, disabled = false }) {
  const current = fields.map((f) => String(settings[f.key]));
  const [values, setValues] = useState(current);
  useEffect(() => setValues(current), [current.join('|')]);
  const submit = (e) => {
    e.preventDefault();
    const patch = {};
    for (const [i, f] of fields.entries()) {
      const n = num(values[i]);
      if (!Number.isFinite(n) || n < f.min || n > f.max) {
        return say('err', t('settings.advanced.number_invalid', { label: f.label, min: f.min, max: f.max }));
      }
      patch[f.key] = n;
    }
    saveChannelWith(say, patch, okText);
  };
  return html`
    <form class="limits" data-form=${name} novalidate onSubmit=${submit}>
      ${fields.map(
        (f, i) => html`<label class="limit"><span>${f.label}</span>
          <input type="number" class="num" name=${f.key} min=${f.min} max=${f.max} step=${f.step} value=${values[i]}
            disabled=${disabled} onInput=${(e) => setValues(values.map((v, j) => (j === i ? e.target.value : v)))} /></label>`,
      )}
      <button type="submit" class="btn pri" disabled=${disabled}>${t('settings.save')}</button>
    </form>`;
}

function Subtitles({ settings, say }) {
  const [code, setCode] = useState(settings.subtitlesLanguage);
  useEffect(() => setCode(settings.subtitlesLanguage), [settings.subtitlesLanguage]);
  const submit = (e) => {
    e.preventDefault();
    const c = code.trim();
    if (!/^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})?$/.test(c)) return say('err', t('settings.advanced.subtitles_language_invalid'));
    saveChannelWith(say, { subtitlesLanguage: c }, t('settings.advanced.saved'));
  };
  return html`
    <form class="limits" data-form="subtitles" onSubmit=${submit}>
      <label class="limit"><span>${t('settings.advanced.subtitles_language')}</span>
        <input type="text" class="num" name="subtitlesLanguage" maxlength="12" value=${code}
          disabled=${!settings.subtitlesEnabled} onInput=${(e) => setCode(e.target.value)} /></label>
      <button type="submit" class="btn pri" disabled=${!settings.subtitlesEnabled}>${t('settings.save')}</button>
    </form>`;
}

/** Unix ms as a short local date and time. */
function when(ms) {
  if (!Number.isFinite(ms)) return '—';
  return new Intl.DateTimeFormat(lang.value, { dateStyle: 'short', timeStyle: 'medium' }).format(new Date(ms));
}

/** Seconds as `m:ss` or `h:mm:ss`. */
function length(s) {
  if (!Number.isFinite(s) || s < 0) return '—';
  const total = Math.round(s);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const sec = String(total % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
}

function Journal() {
  const [entries, setEntries] = useState(null);
  const [note, say] = useNote();
  const load = async () => {
    try {
      setEntries((await api('GET', `${JOURNAL}?limit=20`)).entries);
    } catch (e) {
      say('err', errorText(e));
    }
  };
  useEffect(() => {
    load();
  }, []);
  const download = async (format) => {
    say(null);
    try {
      const res = await fetch(`${JOURNAL}/export?format=${format}`, { credentials: 'same-origin', cache: 'no-store' });
      if (!res.ok) throw new Error(t('ui.error_status', { status: res.status }));
      const name = /filename="([^"]+)"/.exec(res.headers.get('Content-Disposition') ?? '')?.[1] ?? `journal.${format}`;
      const url = URL.createObjectURL(await res.blob());
      const a = document.createElement('a');
      a.href = url;
      a.download = name;
      document.body.append(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 10000);
      say('ok', t('settings.advanced.journal_exported', { file: name }));
    } catch (e) {
      say('err', errorText(e));
    }
  };
  const clear = async () => {
    if (!window.confirm(t('settings.advanced.journal_clear_confirm'))) return;
    try {
      await api('DELETE', JOURNAL);
      say('ok', t('settings.advanced.journal_cleared'));
      await load();
    } catch (e) {
      say('err', errorText(e));
    }
  };
  return html`
    <${Block} title=${t('settings.advanced.journal_block')} hint=${t('settings.advanced.journal_hint')}>
      <div class="pl-actions" data-journal-actions>
        <button type="button" class="btn" data-export="csv" onClick=${() => download('csv')}>${t('settings.advanced.journal_export_csv')}</button>
        <button type="button" class="btn" data-export="json" onClick=${() => download('json')}>${t('settings.advanced.journal_export_json')}</button>
        <button type="button" class="btn" data-journal-clear disabled=${!entries?.length} onClick=${clear}>${t('settings.advanced.journal_clear')}</button>
      </div>
      ${entries === null
        ? html`<p class="set-sub">${t('settings.loading')}</p>`
        : entries.length === 0
          ? html`<p class="set-sub" data-journal-empty>${t('settings.advanced.journal_empty')}</p>`
          : html`<table class="journal" data-journal>
              <thead><tr>
                <th scope="col">${t('settings.advanced.journal_col_start')}</th>
                <th scope="col">${t('settings.advanced.journal_col_title')}</th>
                <th scope="col">${t('settings.advanced.journal_col_length')}</th>
              </tr></thead>
              <tbody>
                ${entries.map(
                  (e) => html`<tr key=${e.id}>
                    <td>${when(e.startedAt)}</td>
                    <td class="journal-title" title=${e.sourceUrl}>${e.title}</td>
                    <td>${e.endedAt ? length((e.endedAt - e.startedAt) / 1000) : t('settings.advanced.journal_airing')}</td>
                  </tr>`,
                )}
              </tbody>
            </table>`}
      <${Note} note=${note} />
    <//>`;
}

export function BroadcastOptions() {
  const settings = channelSettings.value;
  const [note, say] = useNote();
  useEffect(() => {
    loadChannelSettings();
  }, []);
  if (!settings) return html`<p class="set-sub">${t('settings.loading')}</p>`;
  const toggle = (key, label, hint) => html`
    <${Row} title=${label} hint=${hint}>
      <${Switch} label=${label} checked=${Boolean(settings[key])}
        onChange=${(v) => saveChannelWith(say, { [key]: v }, v ? t('settings.advanced.switched_on', { name: label }) : t('settings.advanced.switched_off', { name: label }))} />
    <//>`;
  const fadeLabel = t('settings.advanced.transition_length');
  return html`
    <${Block} title=${t('settings.advanced.broadcast_block')} hint=${t('settings.advanced.broadcast_hint')}>
      ${toggle('transitionsEnabled', t('settings.advanced.transitions'), t('settings.advanced.transitions_hint'))}
      <${Row} title=${fadeLabel}>
        <select class="sel" aria-label=${fadeLabel} name="transitionS" value=${String(settings.transitionS)} disabled=${!settings.transitionsEnabled}
          onChange=${(e) => saveChannelWith(say, { transitionS: Number(e.target.value) }, t('settings.advanced.saved'))}>
          ${FADES.map((s) => html`<option value=${String(s)}>${t('settings.advanced.seconds', { n: s.toLocaleString(lang.value) })}</option>`)}
        </select>
      <//>
      <${Row} title=${t('settings.advanced.peak')} hint=${t('settings.advanced.peak_hint')} block>
        <${Numbers} settings=${settings} say=${say} name="peak" okText=${t('settings.advanced.saved')}
          fields=${[
            { key: 'peakMbps', label: t('settings.advanced.peak_mbps'), min: 0, max: 12, step: 0.1 },
            { key: 'peakBufferS', label: t('settings.advanced.peak_buffer'), min: 0.5, max: 4, step: 0.5 },
          ]} />
      <//>
      ${toggle('subtitlesEnabled', t('settings.advanced.subtitles'), t('settings.advanced.subtitles_hint'))}
      <${Subtitles} settings=${settings} say=${say} />
      ${toggle('loudnessEnabled', t('settings.advanced.loudness'), t('settings.advanced.loudness_hint'))}
      <${Row} title=${t('settings.advanced.loudness_target')}>
        <${Segmented} label=${t('settings.advanced.loudness_target')} value=${settings.loudnessTarget} disabled=${!settings.loudnessEnabled}
          options=${TARGETS.map((v) => ({ value: v, label: t('settings.advanced.lufs', { n: v.toLocaleString(lang.value) }) }))}
          onChange=${(v) => saveChannelWith(say, { loudnessTarget: v }, t('settings.advanced.saved'))} />
      <//>
    <//>
    <${Block} title=${t('settings.advanced.alarms_block')}>
      ${toggle('alarmsEnabled', t('settings.advanced.alarms'), t('settings.advanced.alarms_hint'))}
      <${Numbers} settings=${settings} say=${say} name="alarms" okText=${t('settings.advanced.saved')} disabled=${!settings.alarmsEnabled}
        fields=${[
          { key: 'alarmBlackS', label: t('settings.advanced.alarm_black'), min: 2, max: 300, step: 1 },
          { key: 'alarmSilenceS', label: t('settings.advanced.alarm_silence'), min: 2, max: 300, step: 1 },
          { key: 'alarmSilenceDb', label: t('settings.advanced.alarm_level'), min: -80, max: -20, step: 1 },
        ]} />
    <//>
    <${Note} note=${note} />
    <${Journal} />`;
}
