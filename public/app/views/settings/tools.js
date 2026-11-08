// Settings → Tools: the programs the channel needs (download, update, repair with progress),
// the state of the channel (missing tools, a failed start, restart) and the graphics card.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { appRoute, info, refreshInfo } from '../../store.js';
import { Block, Note, Row, WindowNote, saveWith, useAccess, useConfig, useNote, loadConfig } from './common.js';

export const TOOL_ORDER = ['yt-dlp', 'ffmpeg', 'deno', 'cloudflared'];
export const TOOL_NAME_KEYS = {
  'yt-dlp': 'settings.tools.name_ytdlp',
  ffmpeg: 'settings.tools.name_ffmpeg',
  deno: 'settings.tools.name_deno',
  cloudflared: 'settings.tools.name_cloudflared',
};
const PHASE_KEYS = {
  verify: 'settings.tools.phase_verify',
  install: 'settings.tools.phase_install',
  version: 'settings.tools.phase_version',
};
const JOB_DONE_KEYS = {
  update: 'settings.tools.job_update',
  install: 'settings.tools.job_install',
  repair: 'settings.tools.job_repair',
};
export const ENCODER_OPTIONS = [
  { value: 'auto', key: 'settings.tools.enc_auto' },
  { value: 'h264_nvenc', key: 'settings.tools.enc_nvidia' },
  { value: 'h264_qsv', key: 'settings.tools.enc_intel' },
  { value: 'h264_amf', key: 'settings.tools.enc_amd' },
  { value: 'h264_vaapi', key: 'settings.tools.enc_vaapi' },
  { value: 'libx264', key: 'settings.tools.enc_cpu' },
];
export const ENCODER_RESULT_KEYS = {
  h264_nvenc: 'settings.tools.result_nvidia',
  h264_qsv: 'settings.tools.result_intel',
  h264_amf: 'settings.tools.result_amd',
  h264_vaapi: 'settings.tools.result_vaapi',
  libx264: 'settings.tools.result_cpu',
};
const DEFAULT_VAAPI_DEVICE = '/dev/dri/renderD128';

/** The VA-API render node (Linux), shown while VA-API is chosen or was found; empty: the default one. */
function VaapiDevice({ cfg, say }) {
  const [value, setValue] = useState(cfg.vaapiDevice ?? '');
  useEffect(() => setValue(cfg.vaapiDevice ?? ''), [cfg.vaapiDevice]);
  const submit = (e) => {
    e.preventDefault();
    const v = value.trim();
    if (v && !v.startsWith('/')) return say('err', t('settings.tools.vaapi_device_invalid'));
    saveWith(say, { vaapiDevice: v }, t('settings.tools.vaapi_device_saved'));
  };
  return html`
    <${Row} title=${t('settings.tools.vaapi_device')} hint=${t('settings.tools.vaapi_device_hint')} block>
      <form class="inline wide" data-form="vaapiDevice" onSubmit=${submit}>
        <input type="text" class="txt" name="vaapiDevice" maxlength="200" aria-label=${t('settings.tools.vaapi_device')}
          placeholder=${DEFAULT_VAAPI_DEVICE} value=${value} onInput=${(e) => setValue(e.target.value)} />
        <button type="submit" class="btn pri">${t('settings.save')}</button>
      </form>
    <//>`;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const megabytes = (bytes) => `${(bytes / 1024 ** 2).toFixed(bytes >= 100 * 1024 ** 2 ? 0 : 1)} MB`;

/** The tools as the engine lists them; null until read. */
const tools = signal(null);
/** A download, update or repair that is running (or just ended): `{names, progress, note}`. */
const job = signal(null);

async function loadTools() {
  for (let attempt = 1; attempt <= 6; attempt += 1) {
    try {
      tools.value = await api('GET', appRoute('/tools'));
      return;
    } catch (e) {
      if (e.code !== 'network' && e.status !== 503) return;
      await sleep(700);
    }
  }
}

/** What one tool row of a running job says: `{kind: waiting|active|ready|error, text, ratio}`. */
export function rowState(name, names, p) {
  if (!names.includes(name)) return { kind: 'ready', text: t('settings.tools.installed') };
  if (p.done && !p.running && !p.error) return { kind: 'ready', text: t('settings.tools.done') };
  if (p.tool === name && p.error && !p.running) return { kind: 'error', text: p.error };
  if (p.tool === name && p.phase === 'download') {
    const text = p.total
      ? t('settings.tools.downloading_of', { done: megabytes(p.downloaded), total: megabytes(p.total) })
      : t('settings.tools.downloading', { done: megabytes(p.downloaded) });
    return { kind: 'active', ratio: p.total ? p.downloaded / p.total : null, text };
  }
  if (p.tool === name) return { kind: 'active', ratio: 1, text: t(PHASE_KEYS[p.phase] ?? 'settings.tools.preparing') };
  const at = TOOL_ORDER.indexOf(p.tool);
  if (at >= 0 && TOOL_ORDER.indexOf(name) < at) return { kind: 'ready', text: t('settings.tools.done') };
  return { kind: 'waiting', text: t('settings.tools.waiting') };
}

/** Starts a job and follows its progress until it ends. */
async function runJob(kind, tool) {
  if (job.value && !job.value.finished) return;
  const missing = info.value?.missingTools?.length ? info.value.missingTools : TOOL_ORDER;
  const names = kind === 'update' ? [tool] : kind === 'install' ? missing : TOOL_ORDER;
  job.value = { names, progress: {}, note: null, finished: false };
  const set = (patch) => (job.value = { ...job.value, ...patch });
  const route = appRoute(`/tools/${kind}`);
  try {
    try {
      await api('POST', route, kind === 'update' ? { tool } : undefined);
    } catch (e) {
      if (e.status !== 409) throw e; // 409: the job already runs: follow it
    }
    let bad = 0;
    for (;;) {
      await sleep(500);
      let p;
      try {
        p = await api('GET', appRoute('/tools/progress'));
        bad = 0;
      } catch (e) {
        // After an installation the server restarts: short gaps do not end the watching.
        if (++bad > 40) throw e;
        continue;
      }
      set({ progress: p });
      if (!p.running && p.done) {
        set({ note: p.error ? { kind: 'err', text: t('settings.tools.failed', { error: p.error }) } : { kind: 'ok', text: t(JOB_DONE_KEYS[kind]) } });
        break;
      }
    }
  } catch (e) {
    set({ note: { kind: 'err', text: errorText(e) } });
  } finally {
    set({ finished: true });
    await Promise.all([loadTools(), refreshInfo()]);
  }
}

function ToolRow({ name, tool, busy }) {
  return html`
    <div class="kv tool" data-tool=${name}>
      <span class="name">${t(TOOL_NAME_KEYS[name] ?? 'settings.tools.name_other', { name })}</span>
      <span class="state">${tool?.installed
        ? t('settings.tools.version', { version: tool.version ?? t('settings.tools.version_unknown') })
        : t('settings.tools.not_installed')}</span>
      <button type="button" class="btn" disabled=${busy || !tool} onClick=${() => (tool?.installed ? runJob('update', name) : runJob('install'))}>
        ${tool?.installed ? t('settings.tools.update') : t('settings.tools.download')}</button>
    </div>`;
}

function Progress() {
  const j = job.value;
  if (!j) return null;
  return html`
    <div class="tool-list" data-job=${j.finished ? 'finished' : 'running'}>
      ${j.names.map((name) => {
        const s = rowState(name, j.names, j.progress);
        const pct = s.kind === 'ready' ? 100 : s.kind === 'active' ? (s.ratio === null ? null : Math.round(Math.min(1, Math.max(0, s.ratio)) * 100)) : 0;
        return html`<div class="tool-row" data-kind=${s.kind} data-tool=${name}>
          <div class="head"><span>${t(TOOL_NAME_KEYS[name] ?? 'settings.tools.name_other', { name })}</span><span class="state">${s.text}</span></div>
          <div class=${pct === null ? 'bar indeterminate' : 'bar'} role="progressbar" aria-label=${t(TOOL_NAME_KEYS[name] ?? 'settings.tools.name_other', { name })}
            aria-valuenow=${pct ?? undefined}><span style=${pct === null ? '' : `width:${pct}%`}></span></div>
        </div>`;
      })}
      <${Note} note=${j.note} />
    </div>`;
}

/** The state of the channel: missing tools or a failed start, with what to do. */
function ChannelState({ say }) {
  const i = info.value;
  if (!i || i.restarting) return null;
  const restart = async () => {
    say(null);
    try {
      await api('POST', appRoute('/restart'));
      say('ok', t('settings.tools.restarted'));
    } catch (e) {
      say('err', errorText(e));
    }
    await refreshInfo();
  };
  const running = Boolean(job.value && !job.value.finished) || Boolean(i.toolsRunning);
  if (!i.toolsOk) {
    const missing = (i.missingTools ?? []).join(', ');
    return html`<p class="banner warn" role="status" data-banner="tools_missing">
      ${i.channelRunning ? t('settings.tools.missing_running', { tools: missing }) : t('settings.tools.missing_stopped')}
      <button type="button" class="btn" disabled=${running} onClick=${() => runJob('repair')}>${t('settings.tools.repair')}</button></p>`;
  }
  if (!i.channelRunning) {
    return html`<p class="banner err" role="alert" data-banner="channel_down">
      ${t('settings.tools.channel_down')}${i.channelError ? ` ${t('settings.tools.channel_down_reason', { reason: i.channelError })}` : ''}
      <button type="button" class="btn" disabled=${running} onClick=${restart}>${t('settings.tools.restart')}</button></p>`;
  }
  return null;
}

export function Tools() {
  const { managed } = useAccess();
  const cfg = useConfig();
  const [note, say] = useNote();
  useEffect(() => {
    if (managed) loadTools();
  }, [managed]);

  const list = tools.value;
  const busy = Boolean(job.value && !job.value.finished) || Boolean(list?.running);

  const detect = async (e) => {
    const button = e.currentTarget;
    button.disabled = true;
    say('info', t('settings.tools.detecting'));
    try {
      const r = await api('POST', appRoute('/encoder/detect'), undefined, { timeoutMs: 60000 });
      say('ok', t('settings.tools.detected_now', { result: t(ENCODER_RESULT_KEYS[r.detectedEncoder] ?? 'settings.tools.result_cpu') }));
      await Promise.all([loadConfig(), refreshInfo()]);
    } catch (err) {
      say('err', errorText(err));
    } finally {
      button.disabled = false;
    }
  };

  return html`
    <div class="set-rows">
      <${WindowNote} />
      ${managed ? html`<${ChannelState} say=${say} />` : null}
      <${Block} title=${t('settings.tools.title')} hint=${t('settings.tools.hint')}>
        <div class="tools-list">
          ${TOOL_ORDER.map((name) => html`<${ToolRow} key=${name} name=${name} tool=${list?.tools?.find((x) => x.name === name)} busy=${busy || !managed} />`)}
        </div>
        <${Progress} />
        <div class="pl-actions">
          <button type="button" class="btn" disabled=${busy || !managed} onClick=${() => runJob('repair')}>${t('settings.tools.repair')}</button>
        </div>
      <//>
      <${Block} title=${t('settings.tools.gpu')}>
        <${Row} title=${t('settings.tools.encoder')} hint=${t('settings.tools.encoder_hint')}>
          <select class="sel" aria-label=${t('settings.tools.encoder')} value=${cfg?.videoEncoder ?? 'auto'} disabled=${!cfg}
            onChange=${(e) => saveWith(say, { videoEncoder: e.target.value }, e.target.value === 'auto' ? t('settings.tools.encoder_auto') : t('settings.tools.encoder_saved'))}>
            ${ENCODER_OPTIONS.map((o) => html`<option value=${o.value}>${t(o.key)}</option>`)}
          </select>
        <//>
        <div class="kv">
          <span class="state" data-encoder-state>${cfg?.detectedEncoder
            ? t('settings.tools.last_test', { result: t(ENCODER_RESULT_KEYS[cfg.detectedEncoder] ?? 'settings.tools.result_cpu') })
            : t('settings.tools.no_test')}</span>
          <button type="button" class="btn" disabled=${!cfg} onClick=${detect}>${t('settings.tools.test')}</button>
        </div>
        ${cfg && (cfg.videoEncoder === 'h264_vaapi' || (cfg.videoEncoder === 'auto' && cfg.detectedEncoder === 'h264_vaapi'))
          ? html`<${VaapiDevice} cfg=${cfg} say=${say} />`
          : null}
      <//>
      <${Note} note=${note} />
    </div>`;
}
