// The permanent link (a named Cloudflare tunnel): what mode the channel is in, the buttons
// that switch or delete it, and the two-step wizard that sets it up (log in to Cloudflare,
// choose the address). Window only: its routes are /api/desktop/*.
//
// The wizard's state lives in this module, not in the component: leaving the view (and so
// unmounting) stops the polling but not the work on the engine, and coming back picks up
// where it is (a login or a creation still running, or its outcome).

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { info, refreshInfo } from '../store.js';

const PROGRESS = '/api/desktop/tunnel/named/progress';
const CONFIG = '/api/desktop/config';

/** `{tunnelMode, namedTunnelHostname}` of the window's settings; null until read. */
const config = signal(null);
/** The open wizard: `{step, ...}` or null (closed). Steps: login, login_wait, host, working, dns_taken, done. */
const wizard = signal(null);
/** Message under the wizard / the buttons: `{text, kind: 'ok'|'err', help?: 'login'|'domain', helpUrl?}`. */
const notice = signal(null);
const confirmingDelete = signal(false);
const switching = signal(false);

// Every start of the wizard (and every leaving of the view) gets a new number: the polling
// loops of an older one end when they see theirs is not current.
let run = 0;
// The address of the creation started from this window (a return to the view finds its progress).
let lastCreateHost = '';
// The login page this window already opened in the browser (never opened twice).
let openedLoginUrl = '';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const say = (text, kind = 'ok', extra = {}) => (notice.value = text ? { text, kind, ...extra } : null);
const go = (step, extra = {}) => (wizard.value = { step, ...extra });

export async function loadConfig() {
  try {
    config.value = await api('GET', CONFIG);
  } catch {
    // the next change of the tunnel reads it again
  }
}

/** Reads the progress, retrying while the engine is restarting. */
async function readProgress() {
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await api('GET', PROGRESS);
    } catch (e) {
      if ((e.code !== 'network' && e.status !== 503) || attempt >= 12) throw e;
      await sleep(700);
    }
  }
}

function close() {
  run += 1;
  lastCreateHost = '';
  wizard.value = null;
  say('');
}

async function open() {
  const r = ++run;
  say('');
  go('login', { starting: true });
  let p;
  try {
    p = await readProgress();
  } catch (e) {
    return say(errorText(e), 'err');
  }
  if (r !== run) return;
  if (p.running && p.operation === 'login') return watchLogin(r);
  // Back in the view during (or after) a creation started from this window.
  if (p.operation === 'create' && lastCreateHost) {
    if (p.running) return watchCreate(r);
    return createOutcome(r, p);
  }
  if (p.loggedIn) return stepHost(r);
  stepLogin(r);
}

// ---------- Step 1: Cloudflare login ----------

function stepLogin(r, retry = false) {
  if (r !== run) return;
  go('login', { retry });
}

async function startLogin(r) {
  say('');
  try {
    await api('POST', '/api/desktop/tunnel/named/login');
  } catch (e) {
    // `busy`: a login is already running — it is only watched.
    if (e.code !== 'busy') return failedLogin(r, errorText(e));
  }
  watchLogin(r);
}

function failedLogin(r, text, helpUrl) {
  if (r !== run) return;
  stepLogin(r, true);
  say(text, 'err', helpUrl ? { help: 'login', helpUrl } : {});
}

async function watchLogin(r) {
  go('login_wait', { message: '', loginUrl: null });
  let bad = 0;
  for (;;) {
    await sleep(1000);
    if (r !== run) return;
    let p;
    try {
      p = await api('GET', PROGRESS);
      bad = 0;
    } catch (e) {
      bad += 1;
      if (bad > 20) return failedLogin(r, errorText(e));
      continue;
    }
    if (r !== run) return;
    go('login_wait', { message: p.message || t('permalink.waiting'), loginUrl: p.loginUrl ?? null });
    if (p.loginUrl && p.browserOpened === false && p.loginUrl !== openedLoginUrl) {
      // cloudflared did not open the browser itself: this window does, once per page (also
      // after a return to the view in the middle of the login).
      openedLoginUrl = p.loginUrl;
      window.open(p.loginUrl, '_blank', 'noopener');
    }
    if (p.running) continue;
    if (p.state === 'done' || p.loggedIn) return stepHost(r);
    const text = p.state === 'cancelled' ? t('permalink.login_cancelled') : p.message || t('permalink.login_failed');
    return failedLogin(r, text, p.helpUrl);
  }
}

async function cancelLogin() {
  try {
    await api('POST', '/api/desktop/tunnel/named/login', { cancel: true });
  } catch {
    // the progress shows what happened
  }
}

// ---------- Step 2: the address ----------

function stepHost(r, previous = '') {
  if (r !== run) return;
  go('host', { host: previous });
}

/** A pasted web address with a scheme and a path becomes just the host name. */
export function cleanHostname(text) {
  return String(text ?? '')
    .trim()
    .replace(/^https?:\/\//i, '')
    .replace(/\/.*$/, '');
}

async function createNamed(r, hostname, overwrite) {
  lastCreateHost = hostname;
  go('working', { host: hostname });
  say(t('permalink.creating'), 'ok');
  let working = true;
  (async () => {
    while (working && r === run) {
      await sleep(1000);
      try {
        const p = await api('GET', PROGRESS);
        if (working && r === run && p.running && p.message) say(p.message, 'ok');
      } catch {
        // no answer for a moment
      }
    }
  })();
  try {
    const result = await api('POST', '/api/desktop/tunnel/named/create', { hostname, overwrite });
    working = false;
    if (r !== run) return;
    go('done', { host: result.tunnel?.hostname ?? hostname });
    say(result.message, 'ok');
    refreshAll();
  } catch (e) {
    working = false;
    if (r !== run) return;
    createFailed(r, hostname, e.message, e.data?.code, e.data?.helpUrl, e);
  }
}

/** A creation still running on the engine (the view was left and entered again): its progress. */
async function watchCreate(r) {
  go('working', { host: lastCreateHost });
  say(t('permalink.creating'), 'ok');
  for (;;) {
    await sleep(1000);
    if (r !== run) return;
    let p;
    try {
      p = await api('GET', PROGRESS);
    } catch {
      continue;
    }
    if (r !== run) return;
    if (p.running) {
      say(p.message || t('permalink.creating'), 'ok');
      continue;
    }
    return createOutcome(r, p);
  }
}

/** The outcome of a finished creation, read from the progress (`done` / `failed`). */
async function createOutcome(r, p) {
  if (p.state === 'done') {
    await refreshAll();
    if (r !== run) return;
    go('done', { host: config.value?.namedTunnelHostname ?? lastCreateHost });
    return say(p.message, 'ok');
  }
  createFailed(r, lastCreateHost, p.message || t('permalink.create_failed'), p.code, p.helpUrl, null);
}

function createFailed(r, hostname, message, code, helpUrl, error) {
  if (r !== run) return;
  const text = error ? errorText(error) : message;
  if (code === 'not_logged_in') {
    stepLogin(r, true);
    return say(text, 'err');
  }
  if (code === 'dns_taken') {
    go('dns_taken', { host: hostname });
    return say(text, 'err');
  }
  stepHost(r, hostname);
  say(text, 'err', code === 'no_domain' && helpUrl ? { help: 'domain', helpUrl } : {});
}

// ---------- Switching and deleting ----------

async function refreshAll() {
  await Promise.all([loadConfig(), refreshInfo()]);
}

/** "Back to quick tunnel" (`quick`) and "Delete permanent link" (`delete`). */
async function namedAction(kind) {
  switching.value = true;
  close(); // an open wizard has nothing left to do
  say(kind === 'delete' ? t('permalink.deleting') : t('permalink.switching'), 'ok');
  try {
    const result = await api('POST', `/api/desktop/tunnel/named/${kind}`);
    say(result.message, 'ok');
  } catch (e) {
    say(errorText(e), 'err');
  }
  confirmingDelete.value = false;
  await refreshAll();
  switching.value = false;
}

async function backToNamed() {
  switching.value = true;
  try {
    await api('PUT', CONFIG, { tunnelMode: 'named' });
    say(t('permalink.named_running'), 'ok');
  } catch (e) {
    say(errorText(e), 'err');
  }
  await refreshAll();
  switching.value = false;
}

// ---------- The view ----------

function Notice() {
  const n = notice.value;
  if (!n) return null;
  const help = n.helpUrl
    ? html` <a href=${n.helpUrl} target="_blank" rel="noopener">${t(n.help === 'domain' ? 'permalink.help_domain' : 'permalink.help_login')}</a>`
    : null;
  return html`<p class=${`pl-notice ${n.kind}`} role=${n.kind === 'err' ? 'alert' : 'status'}>${n.text}${help}</p>`;
}

function HostForm({ initial, disabled }) {
  const [value, setValue] = useState(initial ?? '');
  const field = useRef(null);
  useEffect(() => {
    if (!disabled) field.current?.focus({ preventScroll: true });
  }, []);
  const submit = (e) => {
    e.preventDefault();
    const hostname = cleanHostname(value);
    if (hostname) createNamed(run, hostname, false);
  };
  return html`
    <form class="pl-form" onSubmit=${submit}>
      <p>${t('permalink.step_host')}</p>
      <label class="pl-label">${t('permalink.host_label')}
        <input type="text" class="pl-input" value=${value} disabled=${disabled} placeholder=${t('permalink.host_placeholder')}
          autocomplete="off" autocapitalize="off" spellcheck="false" required ref=${field}
          onInput=${(e) => setValue(e.currentTarget.value)} />
      </label>
      <div class="pl-row">
        <button type="submit" class="btn pri" disabled=${disabled}>${t('permalink.create')}</button>
        <button type="button" class="btn" disabled=${disabled} onClick=${close}>${t('permalink.cancel')}</button>
      </div>
    </form>`;
}

function Wizard() {
  const w = wizard.value;
  if (!w) return null;
  let body = null;
  if (w.step === 'login') {
    body = html`
      <p>${t('permalink.intro')}</p>
      <p>${t('permalink.step_login')}</p>
      <div class="pl-row">
        <button type="button" class="btn pri" disabled=${Boolean(w.starting)} onClick=${() => startLogin(run)}>
          ${w.retry ? t('permalink.retry') : t('permalink.connect')}</button>
        <button type="button" class="btn" onClick=${close}>${t('permalink.cancel')}</button>
      </div>`;
  } else if (w.step === 'login_wait') {
    body = html`
      <p>${t('permalink.step_login_wait')}</p>
      <p class="pl-notice ok" role="status">${w.message || t('permalink.starting')}</p>
      ${w.loginUrl
        ? html`<p class="pl-small">${t('permalink.login_link_lead')} <a href=${w.loginUrl} target="_blank" rel="noopener">${t('permalink.login_link')}</a></p>`
        : null}
      <div class="pl-row"><button type="button" class="btn" onClick=${cancelLogin}>${t('permalink.cancel')}</button></div>`;
  } else if (w.step === 'host' || w.step === 'working') {
    body = html`<${HostForm} key=${w.step === 'host' ? `host-${w.host}` : 'working'} initial=${w.host} disabled=${w.step === 'working'} />`;
  } else if (w.step === 'dns_taken') {
    body = html`
      <p>${t('permalink.dns_taken', { host: w.host })}</p>
      <div class="pl-row">
        <button type="button" class="btn pri" onClick=${() => createNamed(run, w.host, true)}>${t('permalink.overwrite')}</button>
        <button type="button" class="btn" onClick=${() => { say(''); stepHost(run, w.host); }}>${t('permalink.other_name')}</button>
      </div>`;
  } else if (w.step === 'done') {
    body = html`
      <p>${t('permalink.done', { host: w.host })}</p>
      <div class="pl-row"><button type="button" class="btn pri" onClick=${close}>${t('permalink.close')}</button></div>`;
  }
  return html`<div class="pl-wizard" role="group" aria-label=${t('permalink.wizard_label')}>${body}</div>`;
}

/** The permanent link block: mode text, buttons and the wizard. */
export function NamedLink() {
  const tunnel = info.value?.tunnel ?? null;
  const cfg = config.value;
  const mode = tunnel?.mode ?? cfg?.tunnelMode ?? 'quick';
  const saved = cfg?.namedTunnelHostname ?? null;
  const named = mode === 'named';
  const host = named ? (tunnel?.hostname ?? saved) : saved;
  const busy = switching.value;
  const open_ = wizard.value !== null;

  useEffect(() => {
    loadConfig();
    // Back in the view while the wizard was open: take up its progress.
    if (wizard.value) open();
    return () => {
      run += 1; // the polling of this view ends; the work on the engine goes on
    };
  }, []);

  // The settings change under the wizard (a creation finished, the mode switched).
  const key = `${tunnel?.mode}|${tunnel?.hostname}`;
  useEffect(() => {
    loadConfig();
  }, [key]);

  const actions = [];
  if (confirmingDelete.value) {
    actions.push(html`
      <p class="pl-small pl-confirm">${t('permalink.delete_confirm')}</p>
      <button type="button" class="btn danger" disabled=${busy} onClick=${() => namedAction('delete')}>${t('permalink.delete_yes')}</button>
      <button type="button" class="btn" onClick=${() => (confirmingDelete.value = false)}>${t('permalink.cancel')}</button>`);
  } else {
    if (named) {
      actions.push(html`<button type="button" class="btn" disabled=${busy} onClick=${() => namedAction('quick')}>${t('permalink.back_quick')}</button>`);
    } else if (saved) {
      actions.push(html`<button type="button" class="btn pri" disabled=${busy} onClick=${backToNamed}>${t('permalink.back_named', { host: saved })}</button>`);
    } else if (!open_) {
      actions.push(html`<button type="button" class="btn pri" onClick=${open}>${t('permalink.setup')}</button>`);
    }
    if (saved) {
      actions.push(html`<button type="button" class="btn danger" disabled=${busy} onClick=${() => (confirmingDelete.value = true)}>${t('permalink.delete')}</button>`);
    }
  }

  return html`
    <div class="named-link" data-mode=${mode}>
      <p class="pl-mode">${named && host ? t('permalink.mode_named', { host }) : t('permalink.mode_quick')}</p>
      <div class="pl-actions">${actions}</div>
      <${Notice} />
      <${Wizard} />
    </div>`;
}
