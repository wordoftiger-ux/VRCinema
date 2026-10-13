// What the owner does with the built-in tunnel from Settings → Internet and the Share view —
// switching it on or off, reconnecting, going back to a quick link — in the window and in a
// remote panel alike (spec section 2). A session that came through the tunnel is asked first
// when the action ends its connection (spec section 10: "You will lose access to this panel"),
// and a connection that then drops is reported as such, not as an error.

import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { appRoute, refreshInfo, sessionViaTunnel } from '../store.js';
import { openDialog } from './dialog.js';
import { saveConfig } from '../views/settings/common.js';

const QUESTIONS = {
  off: {
    title: 'settings.internet.lose_access_title',
    text: 'settings.internet.lose_access',
    yes: 'settings.internet.lose_access_yes',
  },
  reconnect: {
    title: 'settings.internet.lose_access_reconnect_title',
    text: 'settings.internet.lose_access_reconnect',
    yes: 'settings.internet.lose_access_reconnect_yes',
  },
  mode: {
    title: 'settings.internet.lose_access_mode_title',
    text: 'settings.internet.lose_access_mode',
    yes: 'settings.internet.lose_access_mode_yes',
  },
};

/**
 * Asks before `kind` (`off`, `reconnect`, `mode`) ends the connection of a session that came
 * through the tunnel; resolves to whether to go on (always true for any other session).
 */
export function confirmLosingAccess(kind) {
  const q = QUESTIONS[kind];
  if (!q || !sessionViaTunnel.value) return Promise.resolve(true);
  return new Promise((resolve) => {
    openDialog({
      title: () => t(q.title),
      body: () => html`<p class="dialog-intro" data-lose-access=${kind}>${t(q.text)}</p>`,
      buttons: [
        { key: 'settings.internet.lose_access_cancel', action: () => resolve(false) },
        { key: q.yes, danger: true, action: () => resolve(true) },
      ],
      onClose: () => resolve(false),
    });
  });
}

/**
 * Runs `request` after the question of `kind`; `report(kind, text)` says how it went
 * (`ok`, `info` or `err`). A session through the tunnel whose connection drops on the way is
 * told so ("connection ended"). Resolves to the answer, or null.
 */
export async function tunnelAction(kind, request, okText, report) {
  if (!(await confirmLosingAccess(kind))) return null;
  const through = sessionViaTunnel.value;
  report(null);
  try {
    const answer = await request();
    report('ok', typeof okText === 'function' ? okText(answer) : okText);
    refreshInfo();
    return answer;
  } catch (e) {
    if (through && (e.code === 'network' || e.status === 502 || e.status === 503 || e.status === 530)) {
      report('info', t('settings.internet.connection_ended'));
    } else {
      report('err', errorText(e));
    }
    return null;
  }
}

/** Switches the tunnel on or off (the program's settings). */
export function switchTunnel(on, report) {
  const okText = on ? t('settings.internet.tunnel_on') : t('settings.internet.tunnel_off');
  return tunnelAction(
    on ? 'on' : 'off',
    async () => {
      const r = await saveConfig({ tunnelEnabled: on });
      if (!r.ok) throw r.cause ?? new Error(t('settings.restarting'));
      return r.data;
    },
    okText,
    report,
  );
}

/** A new cloudflared (a quick link gets a new address). */
export function reconnectTunnel(report) {
  return tunnelAction('reconnect', () => api('POST', appRoute('/tunnel/restart')), t('share.reconnected'), report);
}
