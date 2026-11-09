// Settings → About: the program (icon, version, links), checking for a new version, the
// system info for a bug report, the data folder and the log, and the licences of what the
// program downloads and uses.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { lang, t } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { AppIcon } from '../../icons.js';
import { appRoute, info, refreshInfo } from '../../store.js';
import { copyText } from '../../components/playerbar.js';
import { openShortcuts } from '../../components/shortcuts.js';
import { Block, Note, WindowNote, WindowOnlyHint, saveConfig, useAccess, useNote } from './common.js';

/** The licence texts of the interface's libraries, shipped with the program. */
const VENDOR_LICENSES = '/static/vendor/LICENSES.txt';

/**
 * What the program downloads or uses: name, link (a name of the engine's list of links), what
 * for (key), licence (text, or a key).
 */
const CREDITS = [
  { name: 'FFmpeg (ffmpeg, ffprobe)', link: 'ffmpeg', use: 'settings.about.use_ffmpeg', license: 'GPL-3.0-or-later', note: 'settings.about.note_ffmpeg' },
  { name: 'yt-dlp', link: 'ytdlp', use: 'settings.about.use_ytdlp', license: 'Unlicense', note: 'settings.about.note_ytdlp' },
  { name: 'Deno', link: 'deno', use: 'settings.about.use_deno', license: 'MIT' },
  { name: 'cloudflared', link: 'cloudflared', use: 'settings.about.use_cloudflared', license: 'Apache-2.0' },
  { name: 'Preact, htm, Preact Signals', link: 'preact', use: 'settings.about.use_preact', license: 'MIT, Apache-2.0 (htm)' },
  { name: 'hls.js', link: 'hlsjs', use: 'settings.about.use_hlsjs', license: 'Apache-2.0' },
  { name: 'Tauri', link: 'tauri', use: 'settings.about.use_tauri', license: 'Apache-2.0 / MIT' },
  { name: 'Microsoft Edge WebView2', link: 'webview2', use: 'settings.about.use_webview2', license: 'settings.about.license_microsoft' },
  { name: 'Rust', link: 'thirdParty', use: 'settings.about.use_rust', license: 'settings.about.license_rust' },
];

/** Where the links lead (`GET …/about`: the interface itself names no web address); null until read. */
const links = signal(null);

async function loadLinks() {
  if (links.value) return;
  try {
    links.value = await api('GET', appRoute('/about'));
  } catch {
    // the names stay without links; the next visit asks again
  }
}

/** The offer found by a check made here (`undefined`: no check yet, the info's offer counts). */
const checked = signal(undefined);
const checking = signal(false);
const installing = signal(false);

const PROBLEMS = {
  offline: 'settings.about.problem_offline',
  not_found: 'settings.about.problem_not_found',
  limited: 'settings.about.problem_limited',
  failed: 'settings.about.problem_failed',
};

/**
 * A link that opens in the browser (the program's window sends it there): `to` names one of
 * the engine's links, `href` is an address of this program. Plain text until the link is known.
 */
function Out({ to, href, children, ...rest }) {
  const url = href ?? links.value?.[to];
  return url
    ? html`<a href=${url} target="_blank" rel="noopener" ...${rest}>${children}</a>`
    : html`<span ...${rest}>${children}</span>`;
}

function lastChecked(at) {
  if (!at) return null;
  const when = new Date(at);
  if (Number.isNaN(when.getTime())) return null;
  return new Intl.DateTimeFormat(lang.value, { dateStyle: 'medium', timeStyle: 'short' }).format(when);
}

function Header() {
  const i = info.value;
  return html`
    <div class="about-head">
      <${AppIcon} size=${56} />
      <div class="about-id">
        <h2 class="about-name">VRCinema</h2>
        <p class="about-version" data-version>${i?.version ? t('settings.about.version', { version: i.version }) : ''}</p>
        <p class="about-tagline">${t('settings.about.tagline')}</p>
        <nav class="about-links" aria-label=${t('settings.about.links')}>
          <${Out} to=${lang.value === 'pl' ? 'manualPl' : 'manual'} data-link="manual">${t('settings.about.manual')}<//>
          <${Out} to="project" data-link="project">${t('settings.about.project')}<//>
          <${Out} to="issue" data-link="issue">${t('settings.about.report_problem')}<//>
          <button type="button" class="linklike" data-link="shortcuts" onClick=${openShortcuts}>${t('settings.about.shortcuts')}</button>
        </nav>
      </div>
    </div>`;
}

function Updates() {
  const { managed } = useAccess();
  const [note, say] = useNote();
  const i = info.value;
  const offer = checked.value === undefined ? (i?.update ?? null) : checked.value;
  const when = lastChecked(i?.updateCheck?.at);

  const check = async () => {
    checking.value = true;
    say('info', t('settings.about.checking'));
    try {
      const r = await api('GET', appRoute('/update'), undefined, { timeoutMs: 30000 });
      checked.value = r.available;
      if (r.problem) say('err', t(PROBLEMS[r.problem] ?? PROBLEMS.failed));
      else say(r.available ? null : 'ok', r.available ? '' : t('settings.about.latest', { version: r.current }));
      refreshInfo();
    } catch (e) {
      say('err', errorText(e));
    } finally {
      checking.value = false;
    }
  };

  const install = async () => {
    installing.value = true;
    say('info', t('settings.about.installing'));
    try {
      const r = await api('POST', appRoute('/update/apply'), undefined, { timeoutMs: 600000 });
      say('ok', t('settings.about.installed', { version: r.version }));
    } catch (e) {
      installing.value = false;
      say('err', t('settings.about.install_failed', { error: errorText(e) }));
    }
  };

  const skip = async () => {
    const r = await saveConfig({ skippedUpdateVersion: offer.version });
    if (!r.ok) return say('err', r.error ?? t('settings.restarting'));
    checked.value = null;
    say('ok', t('settings.about.skipped', { version: offer.version }));
  };

  return html`
    <${Block} title=${t('settings.about.updates')}>
      ${offer
        ? html`<div class="banner" data-banner="update">
            <p>${t('settings.about.available', { version: offer.version })}</p>
            ${offer.notes ? html`<p class="set-sub">${offer.notes}</p>` : null}
            <div class="pl-actions">
              <button type="button" class="btn pri" disabled=${installing.value} onClick=${install}>${t('settings.about.install')}</button>
              <button type="button" class="btn" disabled=${installing.value} onClick=${skip}>${t('settings.about.skip')}</button>
            </div>
          </div>`
        : null}
      <div class="pl-actions">
        <button type="button" class="btn" data-check disabled=${!managed || checking.value} onClick=${check}>${t('settings.about.check')}</button>
        ${when ? html`<small class="about-checked" data-checked>${t('settings.about.last_checked', { time: when })}</small>` : null}
      </div>
      <${Note} note=${note} />
    <//>`;
}

function Diagnostics() {
  const { managed, windowed } = useAccess();
  const [note, say] = useNote();

  const copy = async () => {
    say(null);
    try {
      const r = await api('GET', appRoute('/diagnostics'), undefined, { timeoutMs: 20000 });
      await copyText(r.text);
      say('ok', t('settings.about.info_copied'));
    } catch (e) {
      say('err', errorText(e));
    }
  };

  const open = async (what) => {
    say(null);
    try {
      await api('POST', `/api/desktop/open/${what}`);
    } catch (e) {
      say('err', errorText(e));
    }
  };

  return html`
    <${Block} title=${t('settings.about.diagnostics')} hint=${t('settings.about.diagnostics_hint')}>
      <div class="pl-actions">
        <button type="button" class="btn" data-copy-info disabled=${!managed} onClick=${copy}>${t('settings.about.copy_info')}</button>
        <button type="button" class="btn" data-open="data-folder" disabled=${!windowed} onClick=${() => open('data-folder')}>${t('settings.about.open_data')}</button>
        <button type="button" class="btn" data-open="log" disabled=${!windowed} onClick=${() => open('log')}>${t('settings.about.open_log')}</button>
      </div>
      <${WindowOnlyHint} />
      <${Note} note=${note} />
    <//>`;
}

function Credits() {
  const text = (key) => (key.startsWith('settings.') ? t(key) : key);
  return html`
    <details class="about-credits" data-credits>
      <summary>${t('settings.about.licenses')}</summary>
      <p class="about-license" data-license>${t('settings.about.license_app')}</p>
      <p class="sblock-hint">${t('settings.about.licenses_intro')}</p>
      <table class="about-table">
        <thead><tr><th>${t('settings.about.col_name')}</th><th>${t('settings.about.col_use')}</th><th>${t('settings.about.col_license')}</th></tr></thead>
        <tbody>
          ${CREDITS.map(
            (c) => html`<tr data-credit=${c.name}>
              <td><${Out} to=${c.link}>${c.name}<//></td>
              <td>${t(c.use)}</td>
              <td>${text(c.license)}${c.note ? html`<small>${t(c.note)}</small>` : null}</td>
            </tr>`,
          )}
        </tbody>
      </table>
      <p class="about-more">
        <${Out} to="thirdParty" data-link="third-party">${t('settings.about.full_list')}<//>
        <${Out} href=${VENDOR_LICENSES} data-link="vendor-licenses">${t('settings.about.license_texts')}<//>
      </p>
    </details>`;
}

export function About() {
  const { managed, mode } = useAccess();
  useEffect(() => {
    if (managed) loadLinks();
  }, [managed, mode]);
  return html`
    <div class="set-rows about">
      <${WindowNote} />
      <${Header} />
      <${Updates} />
      <${Diagnostics} />
      <${Credits} />
    </div>`;
}
