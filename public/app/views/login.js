// Signing in to the panel. In a browser (remote panel, or a server without a window): the
// password of the remote panel, errors in the panel's language, and a choice of that language
// (kept in this browser). In the program's window there is no password: its session comes
// from the tray, so the window only says how to get it back.

import { useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { lang, t } from '../i18n.js';
import { Icon } from '../icons.js';
import { api, errorText } from '../api.js';
import { choosePanelLanguage, resumePolling } from '../store.js';
import { inWindow } from '../components/titlebar.js';
import { focusOnMount } from '../focus.js';
import { LanguageOptions } from './settings/common.js';

function Brand() {
  return html`<p class="login-brand"><span class="tb-logo" aria-hidden="true"><${Icon} name="play" size=${11} /></span>VRCinema</p>`;
}

function WindowSignedOut() {
  return html`
    <main class="view login" id="view" data-view="login">
      <div class="card2 login-card">
        <${Brand} />
        <h1 tabindex="-1" ref=${focusOnMount}>${t('login.window_title')}</h1>
        <p class="login-hint">${t('login.window_hint')}</p>
      </div>
    </main>`;
}

export function LoginView() {
  const [password, setPassword] = useState('');
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  if (inWindow()) return html`<${WindowSignedOut} />`;

  const submit = async (e) => {
    e.preventDefault();
    if (!password) return;
    setBusy(true);
    setError(null);
    try {
      await api('POST', '/api/admin/login', { password });
      setPassword('');
      resumePolling();
    } catch (err) {
      // Kept as the error itself: its text follows a language switch made after it.
      setError(err);
      setBusy(false);
    }
  };

  return html`
    <main class="view login" id="view" data-view="login">
      <form class="card2 login-card" onSubmit=${submit}>
        <${Brand} />
        <h1>${t('login.title')}</h1>
        <p class="login-hint">${t('login.hint')}</p>
        <label class="login-field">
          <span>${t('login.password')}</span>
          <input type="password" class="txt" name="password" autocomplete="current-password" required
            ref=${focusOnMount} value=${password} disabled=${busy}
            onInput=${(e) => setPassword(e.currentTarget.value)} />
        </label>
        ${error ? html`<p class="login-error" role="alert" data-code=${error.code ?? undefined}>${errorText(error)}</p>` : null}
        <button type="submit" class="btn pri login-submit" disabled=${busy || !password}>
          ${busy ? t('login.signing_in') : t('login.submit')}</button>
        <label class="login-lang">
          <span>${t('login.language')}</span>
          <select class="sel" value=${lang.value} onChange=${(e) => choosePanelLanguage(e.currentTarget.value)}>
            <${LanguageOptions} />
          </select>
        </label>
      </form>
    </main>`;
}
