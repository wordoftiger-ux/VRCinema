// Settings → Remote panel: logging in to the panel with a password from other devices. The
// program's own window never needs the password.

import { useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { Note, Row, Switch, WindowNote, saveWith, useConfig, useNote } from './common.js';

const MIN_PASSWORD = 8;

export function Remote() {
  const cfg = useConfig();
  const [note, say] = useNote();
  const [password, setPassword] = useState('');
  // Switching on without a password: setting the password finishes it.
  const [wantRemote, setWantRemote] = useState(false);
  const field = useRef(null);

  const toggle = (on) => {
    if (on && !cfg.hasPassword) {
      setWantRemote(true);
      say('info', t('settings.remote.password_first', { min: MIN_PASSWORD }));
      field.current?.focus();
      return;
    }
    setWantRemote(false);
    saveWith(say, { remoteAdmin: on }, on ? t('settings.remote.on') : t('settings.remote.off'));
  };

  const submit = async (e) => {
    e.preventDefault();
    if ([...password].length < MIN_PASSWORD) return say('err', t('errors.password_too_short', { min: MIN_PASSWORD }));
    const patch = { password };
    if (wantRemote) patch.remoteAdmin = true;
    const saved = await saveWith(say, patch, wantRemote ? t('settings.remote.set_and_on') : t('settings.remote.changed'));
    if (saved) {
      setPassword('');
      setWantRemote(false);
    }
  };

  return html`
    <div class="set-rows">
      <${WindowNote} />
      <${Row} title=${t('settings.remote.enable')} hint=${t('settings.remote.enable_hint')}>
        <${Switch} label=${t('settings.remote.enable')} checked=${Boolean(cfg?.remoteAdmin)} disabled=${!cfg} onChange=${toggle} />
      <//>
      <p class="set-sub" data-password-state=${cfg?.hasPassword ? 'set' : 'unset'}>${cfg?.hasPassword ? t('settings.remote.has_password') : t('settings.remote.no_password')}</p>
      <${Row} title=${cfg?.hasPassword ? t('settings.remote.change_password') : t('settings.remote.set_password')} hint=${t('settings.remote.password_hint', { min: MIN_PASSWORD })}>
        <form class="inline" onSubmit=${submit}>
          <input type="password" class="txt" autocomplete="new-password" ref=${field} minlength=${MIN_PASSWORD}
            aria-label=${t('settings.remote.password')} value=${password} disabled=${!cfg} onInput=${(e) => setPassword(e.target.value)} />
          <button type="submit" class="btn" disabled=${!cfg}>${cfg?.hasPassword ? t('settings.remote.change') : t('settings.remote.set')}</button>
        </form>
      <//>
      <${Note} note=${note} />
    </div>`;
}
