// Live capture of this computer (spec 2026-11-08-capture-design), only in the program's own
// window: the "Capture" button under the link field opens a dialog (a window, a screen or part
// of the desktop, a camera, or sound only; the sound device, frame rate, resolution, the title
// viewers see, a preview frame) that adds the capture to the queue as a live item. While a
// capture runs, the title bar shows a red "Capturing" pill with Stop.

import { signal } from '/static/vendor/signals.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { Icon } from '../icons.js';
import { errorText } from '../api.js';
import { openDialog } from './dialog.js';
import { addCapture, capturePreview, captureSources, capturing, showToast, stopCapture } from '../store.js';

const FPS = [30, 25, 15];
const HEIGHTS = [1080, 720, 480];
const KINDS = ['screen', 'window', 'camera', 'none'];

/** The dialog's state (one dialog at a time). */
const form = signal(null);
/** The listing: null until it arrives; `{error}` when it failed. */
const sources = signal(null);
const loading = signal(false);
/** The preview frame: `{url}` (an object URL), `{error}`, `{busy: true}` or null. */
const preview = signal(null);

function freshForm() {
  return {
    kind: 'screen',
    source: 'desktop',
    audio: '',
    fps: 30,
    height: 1080,
    fast: false,
    title: '',
    region: { x: 0, y: 0, width: 1280, height: 720 },
  };
}

function setPreview(next) {
  if (preview.value?.url) URL.revokeObjectURL(preview.value.url);
  preview.value = next;
}

function update(patch) {
  form.value = { ...form.value, ...patch };
  setPreview(null);
}

/** The choices of the "What to capture" list for the kind `kind`: `[{value, label}]`. */
function choices(kind, s) {
  if (!s || s.error) return [];
  if (kind === 'screen') {
    const out = [];
    if (s.desktop) out.push({ value: 'desktop', label: t('capture.all_screens') });
    for (const sc of s.screens ?? []) {
      const args = { n: sc.index + 1, width: sc.width, height: sc.height };
      out.push({ value: `screen:${sc.index}`, label: t(sc.primary ? 'capture.screen_main' : 'capture.screen', args) });
    }
    if (s.region) out.push({ value: 'region', label: t('capture.region') });
    return out;
  }
  if (kind === 'window') return (s.windows ?? []).map((title) => ({ value: title, label: title }));
  if (kind === 'camera') return (s.cameras ?? []).map((d) => ({ value: d.id, label: d.name }));
  return [];
}

/** The kinds this computer offers. */
function kinds(s) {
  if (!s || s.error) return KINDS;
  return KINDS.filter((k) => k === 'none' || k === 'camera' || choices(k, s).length > 0 || (k === 'window' && s.platform === 'windows'));
}

/** Picks the first available choice when the current one is not in the list. */
function settle() {
  const f = form.value;
  const s = sources.value;
  const list = choices(f.kind, s);
  const source = list.some((c) => c.value === f.source) ? f.source : (list[0]?.value ?? '');
  const audio = (s?.audio ?? []).some((d) => d.id === f.audio) ? f.audio : '';
  if (source !== f.source || audio !== f.audio) form.value = { ...f, source, audio };
}

async function refresh() {
  loading.value = true;
  setPreview(null);
  try {
    sources.value = await captureSources();
  } catch (e) {
    sources.value = { error: errorText(e) };
  } finally {
    loading.value = false;
  }
  settle();
}

/** The request for the engine, or null when nothing is chosen. */
function request() {
  const f = form.value;
  const base = {
    audio: f.audio || null,
    fps: f.fps,
    height: f.height,
    title: f.title.trim() || null,
  };
  if (f.kind === 'none') return f.audio ? { kind: 'none', ...base } : null;
  if (!f.source) return null;
  if (f.kind === 'screen') {
    if (f.source === 'desktop') return { kind: 'desktop', ...base };
    if (f.source === 'region') {
      const r = Object.fromEntries(Object.entries(f.region).map(([k, v]) => [k, Math.round(Number(v) || 0)]));
      return { kind: 'region', region: r, ...base };
    }
    return { kind: 'screen', source: f.source.slice('screen:'.length), fast: f.fast, ...base };
  }
  return { kind: f.kind, source: f.source, ...base };
}

async function takePreview() {
  const body = request();
  if (!body || body.kind === 'none') return;
  setPreview({ busy: true });
  try {
    const blob = await capturePreview(body);
    setPreview({ url: URL.createObjectURL(blob) });
  } catch (e) {
    setPreview({ error: errorText(e) });
  }
}

async function submit() {
  const body = request();
  setPreview(null);
  if (!body) {
    showToast(t('capture.nothing'));
    return;
  }
  try {
    const { title } = await addCapture(body);
    showToast(t('capture.added', { title }), 'info');
  } catch (e) {
    showToast(errorText(e));
  }
}

function Select({ label, value, options, onChange, data }) {
  return html`
    <label class="dialog-field">
      <span>${label}</span>
      <select class="sel" data-cap=${data} value=${value} onChange=${(e) => onChange(e.currentTarget.value)}>
        ${options.map((o) => html`<option value=${o.value} selected=${String(o.value) === String(value)}>${o.label}</option>`)}
      </select>
    </label>`;
}

function RegionFields() {
  const r = form.value.region;
  const field = (key, labelKey) => html`
    <label class="dialog-field cap-num"><span>${t(labelKey)}</span>
      <input type="number" class="num" data-cap-region=${key} value=${r[key]} step="1"
        onInput=${(e) => update({ region: { ...form.value.region, [key]: e.currentTarget.value } })} /></label>`;
  return html`<div class="dialog-field-row cap-region">
    ${field('x', 'capture.region_x')}${field('y', 'capture.region_y')}
    ${field('width', 'capture.region_width')}${field('height', 'capture.region_height')}
  </div>`;
}

function Body() {
  const f = form.value;
  const s = sources.value;
  if (!f) return null;
  const list = choices(f.kind, s);
  const audio = [{ value: '', label: t('capture.audio_none') }, ...(s?.audio ?? []).map((d) => ({ value: d.id, label: d.name }))];
  const screen = f.kind === 'screen' && f.source.startsWith('screen:');
  const p = preview.value;
  const pictured = f.kind !== 'none' && Boolean(f.source);
  return html`
    <div class="cap" data-capture>
      ${loading.value ? html`<p class="dialog-note" role="status" data-cap-loading>${t('capture.loading')}</p>` : null}
      ${s?.error ? html`<p class="cap-error" role="alert">${s.error}</p>` : null}
      <div class="dialog-fields">
        <${Select} label=${t('capture.kind_label')} value=${f.kind} data="kind"
          options=${kinds(s).map((k) => ({ value: k, label: t(`capture.kind_${k}`) }))}
          onChange=${(kind) => {
            update({ kind, source: '' });
            settle();
          }} />
        ${f.kind !== 'none'
          ? html`<div class="dialog-field">
              <span>${t('capture.source_label')}</span>
              <span class="dialog-field-row">
                ${list.length
                  ? html`<select class="sel cap-source" data-cap="source" aria-label=${t('capture.source_label')} value=${f.source}
                      onChange=${(e) => update({ source: e.currentTarget.value })}>
                      ${list.map((o) => html`<option value=${o.value} selected=${o.value === f.source}>${o.label}</option>`)}
                    </select>`
                  : html`<span class="cap-empty" data-cap-empty>${loading.value ? '' : t('capture.nothing')}</span>`}
                <button type="button" class="btn btn-small" data-cap-refresh disabled=${loading.value} onClick=${refresh}>
                  ${t('capture.refresh')}</button>
              </span>
            </div>`
          : null}
        ${f.kind === 'screen' && f.source === 'region' ? html`<${RegionFields} />` : null}
        ${screen && s?.fast
          ? html`<label class="dialog-check"><input type="checkbox" data-cap="fast" checked=${f.fast}
              onChange=${(e) => update({ fast: e.currentTarget.checked })} /><span>${t('capture.fast')}</span></label>`
          : null}
        ${f.kind === 'window' ? html`<p class="dialog-note">${t('capture.window_hint')}</p>` : null}
        <${Select} label=${t('capture.audio_label')} value=${f.audio} data="audio" options=${audio}
          onChange=${(v) => update({ audio: v })} />
        <div class="dialog-field-row cap-pair">
          <${Select} label=${t('capture.fps_label')} value=${f.fps} data="fps"
            options=${FPS.map((n) => ({ value: n, label: t('capture.fps', { fps: n }) }))}
            onChange=${(v) => update({ fps: Number(v) })} />
          <${Select} label=${t('capture.size_label')} value=${f.height} data="height"
            options=${HEIGHTS.map((h) => ({ value: h, label: `${h}p` }))}
            onChange=${(v) => update({ height: Number(v) })} />
        </div>
        <label class="dialog-field"><span>${t('capture.title_label')}</span>
          <input type="text" class="txt cap-title" data-cap="title" maxlength="120" value=${f.title}
            placeholder=${t('capture.default_title')} onInput=${(e) => (form.value = { ...form.value, title: e.currentTarget.value })} /></label>
      </div>
      ${pictured
        ? html`<div class="cap-preview">
            <button type="button" class="btn btn-small" data-cap-preview disabled=${Boolean(p?.busy)} onClick=${takePreview}>
              ${t('capture.preview')}</button>
            ${p?.url ? html`<img src=${p.url} alt=${t('capture.preview_alt')} data-cap-frame />` : null}
            ${p?.error ? html`<span class="cap-error" role="alert">${p.error}</span>` : null}
          </div>`
        : null}
      ${(s?.notes ?? []).map((n) => html`<p class="dialog-note" data-cap-note=${n}>${t(`capture.note_${n}`)}</p>`)}
      <p class="dialog-note cap-privacy">${t('capture.privacy')}</p>
    </div>`;
}

/** Opens the capture dialog and lists what can be captured. */
export function openCapture() {
  form.value = freshForm();
  sources.value = null;
  setPreview(null);
  openDialog({
    title: () => t('capture.dialog_title'),
    body: () => html`<${Body} />`,
    buttons: [{ key: 'capture.add', primary: true, action: submit }, { key: 'settings.cancel' }],
    onClose: () => setPreview(null),
  });
  refresh();
}

/** The button under the link field (the window only). */
export function CaptureButton() {
  return html`
    <div class="q-capture">
      <button type="button" class="btn btn-small" data-capture-open title=${t('capture.open_title')} onClick=${openCapture}>
        <${Icon} name="pip" size=${16} />${t('capture.open')}</button>
    </div>`;
}

/** The title bar's red pill while a capture runs, with Stop. */
export function CaptureIndicator() {
  if (!capturing.value) return null;
  return html`
    <div class="cap-on" role="status" data-capture-on data-tauri-drag-region="false" title=${t('capture.indicator_title')}>
      <span class="cap-dot" aria-hidden="true"></span>
      <span>${t('capture.indicator')}</span>
      <button type="button" class="cap-stop" data-capture-stop aria-label=${t('capture.stop_label')} title=${t('capture.stop_label')}
        onClick=${async () => {
          if (await stopCapture()) showToast(t('capture.stopped'), 'info');
        }}>${t('capture.stop')}</button>
    </div>`;
}
