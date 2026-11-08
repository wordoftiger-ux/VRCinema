// "Keep your nick with a PIN": under the request form. Setting a PIN protects the nick and
// shows a recovery code once; with the PIN (or that code) the nick works on another device or
// after the channel's address changed (a new address starts with no device key). The PIN and
// the code are never stored on the page.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { deviceKey } from './device.js';
import { showToast } from './store.js';

/** Whether the section is open and on which tab (`set` or `adopt`). */
export const pinPanel = signal({ open: false, tab: 'set' });

/** Opens the section on "I have a PIN" (after a nick was refused as taken). */
export function openAdopt() {
  pinPanel.value = { open: true, tab: 'adopt', focus: Date.now() };
}

const PIN_MAX = 64;

/** The recovery code, shown once, with Copy. */
function RecoveryCode({ code, onDone }) {
  const [copied, setCopied] = useState(false);
  const field = useRef(null);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
    } catch {
      // No clipboard (plain http): the code is selected for copying by hand.
      field.current?.select();
      return;
    }
    setCopied(true);
  };
  return html`
    <div class="v-pin-code" role="status" data-recovery>
      <b>${t('viewer.pin_code_title')}</b>
      <input class="txt" readOnly value=${code} ref=${field} aria-label=${t('viewer.pin_code_title')}
        onFocus=${(e) => e.currentTarget.select()} />
      <p>${t('viewer.pin_code_text')}</p>
      <div class="v-pin-actions">
        <button type="button" class="btn" data-action="copy-code" onClick=${copy}>
          ${copied ? t('viewer.pin_copied') : t('viewer.pin_copy')}</button>
        <button type="button" class="btn pri" data-action="code-done" onClick=${onDone}>${t('viewer.pin_code_done')}</button>
      </div>
    </div>`;
}

/** A labelled field of the section. */
function Field({ label, hint, inputRef, ...input }) {
  return html`
    <label class="v-field">
      <span>${label}</span>
      <input class="txt" spellcheck="false" ref=${inputRef} ...${input} />
      ${hint ? html`<small>${hint}</small>` : null}
    </label>`;
}

/**
 * `nick`: the nick field's text; `onNick(nick, badge)`: the nick and icon this device now
 * holds (the nick as the channel writes it).
 */
export function NickPin({ nick, onNick }) {
  const panel = pinPanel.value;
  const [pin, setPin] = useState('');
  const [current, setCurrent] = useState('');
  const [needCurrent, setNeedCurrent] = useState(false);
  const [useCode, setUseCode] = useState(false);
  const [recovery, setRecovery] = useState('');
  const [newPin, setNewPin] = useState('');
  const [signOut, setSignOut] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  const [code, setCode] = useState(null);
  const first = useRef(null);
  const name = nick.trim();
  const tab = panel.tab;

  useEffect(() => {
    if (panel.focus) first.current?.focus();
  }, [panel.focus]);

  const clear = () => {
    setPin('');
    setCurrent('');
    setRecovery('');
    setNewPin('');
  };
  const setTab = (next) => {
    pinPanel.value = { open: true, tab: next };
    setError(null);
  };

  const send = async (path, body, done) => {
    if (busy) return;
    setError(null);
    if (!name) {
      setError({ code: 'nick_pin_no_nick' });
      return;
    }
    setBusy(true);
    try {
      const r = await api('POST', path, { deviceKey: deviceKey(), name, ...body }, { timeoutMs: 20000 });
      clear();
      onNick(r.nick, r.requesterBadge);
      if (r.recoveryCode) setCode(r.recoveryCode);
      done(r);
    } catch (err) {
      setError(err);
      if (err.code === 'nick_pin_required') setNeedCurrent(true);
    } finally {
      setBusy(false);
    }
  };

  const save = (e) => {
    e.preventDefault();
    const body = { pin };
    if (needCurrent) body.currentPin = current;
    send('/api/channels/main/nick/protect', body, (r) => {
      setNeedCurrent(false);
      showToast(t('viewer.pin_saved', { nick: r.nick }));
    });
  };

  const adopt = (e) => {
    e.preventDefault();
    const body = useCode ? { recoveryCode: recovery, newPin } : { pin, signOutOthers: signOut };
    send('/api/channels/main/nick/adopt', body, (r) => {
      setUseCode(false);
      showToast(t('viewer.pin_adopted', { nick: r.nick }));
    });
  };

  const errorBox = error
    ? html`<div class="v-error" role="alert" data-code=${error.code ?? undefined}><p>${errorText(error)}</p></div>`
    : null;
  const submitText = (key) => (busy ? t('viewer.pin_checking') : t(key));

  return html`
    <details class="v-pin" open=${panel.open}
      onToggle=${(e) => {
        const open = e.currentTarget.open;
        if (open !== pinPanel.value.open) pinPanel.value = { ...pinPanel.value, open };
      }}>
      <summary>${t('viewer.pin_toggle')}</summary>
      <p class="v-pin-intro">${t('viewer.pin_intro')}</p>
      ${code
        ? html`<${RecoveryCode} code=${code} onDone=${() => setCode(null)} />`
        : html`
          <div class="v-pin-tabs">
            <button type="button" class="btn" data-tab="set" aria-pressed=${tab === 'set' ? 'true' : 'false'}
              onClick=${() => setTab('set')}>${t('viewer.pin_tab_set')}</button>
            <button type="button" class="btn" data-tab="adopt" aria-pressed=${tab === 'adopt' ? 'true' : 'false'}
              onClick=${() => setTab('adopt')}>${t('viewer.pin_tab_adopt')}</button>
          </div>
          ${tab === 'set'
            ? html`<form class="v-pin-form" data-form="set" onSubmit=${save} novalidate>
                ${needCurrent
                  ? html`<${Field} label=${t('viewer.pin_current_label')} type="password" name="currentPin"
                      autocomplete="current-password" maxlength=${PIN_MAX} value=${current} readOnly=${busy}
                      onInput=${(e) => setCurrent(e.currentTarget.value)} />`
                  : null}
                <${Field} label=${needCurrent ? t('viewer.pin_new_label') : t('viewer.pin_label')} hint=${t('viewer.pin_hint')}
                  type="password" name="pin" autocomplete="new-password" maxlength=${PIN_MAX} value=${pin} readOnly=${busy}
                  inputRef=${first} onInput=${(e) => setPin(e.currentTarget.value)} />
                ${errorBox}
                <button type="submit" class="btn pri v-submit" disabled=${busy || !pin}>${submitText('viewer.pin_save')}</button>
              </form>`
            : html`<form class="v-pin-form" data-form="adopt" onSubmit=${adopt} novalidate>
                ${useCode
                  ? html`
                    <${Field} label=${t('viewer.pin_recovery_label')} name="recoveryCode" autocomplete="off"
                      autocapitalize="characters" maxlength="24" placeholder="XXXX-XXXX-XXXX-XXXX" value=${recovery}
                      readOnly=${busy} inputRef=${first} onInput=${(e) => setRecovery(e.currentTarget.value)} />
                    <${Field} label=${t('viewer.pin_new_label')} hint=${t('viewer.pin_hint')} type="password" name="newPin"
                      autocomplete="new-password" maxlength=${PIN_MAX} value=${newPin} readOnly=${busy}
                      onInput=${(e) => setNewPin(e.currentTarget.value)} />`
                  : html`
                    <${Field} label=${t('viewer.pin_label')} type="password" name="pin" autocomplete="current-password"
                      maxlength=${PIN_MAX} value=${pin} readOnly=${busy} inputRef=${first}
                      onInput=${(e) => setPin(e.currentTarget.value)} />
                    <label class="v-check">
                      <input type="checkbox" name="signOutOthers" checked=${signOut}
                        onChange=${(e) => setSignOut(e.currentTarget.checked)} />
                      <span>${t('viewer.pin_sign_out')}</span>
                    </label>`}
                <button type="button" class="v-linkbtn" data-action="toggle-code"
                  onClick=${() => { setUseCode(!useCode); setError(null); }}>
                  ${useCode ? t('viewer.pin_use_pin') : t('viewer.pin_use_recovery')}</button>
                ${errorBox}
                <button type="submit" class="btn pri v-submit"
                  disabled=${busy || (useCode ? !recovery.trim() || !newPin : !pin)}>${submitText('viewer.pin_adopt')}</button>
              </form>`}`}
    </details>`;
}
