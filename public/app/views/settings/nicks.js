// Settings → Channel → Viewer nicks: the nicks viewers have taken (with their icon and last use),
// releasing a nick, blocking the device that holds it, and the blocked devices. Never an address
// or a device key: the engine does not send them.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t, tn } from '../../i18n.js';
import { api, errorText } from '../../api.js';
import { RequesterBadge } from '../../components/badge.js';
import { openDialog } from '../../components/dialog.js';
import { finishedText } from '../history.js';
import { Block, Note, useNote } from './common.js';

const NICKS = '/api/admin/nicks';
const EVERY_MS = 5000;

/** `{nicks, blocked}` of the engine; null until read. */
const list = signal(null);

async function load() {
  try {
    list.value = await api('GET', NICKS);
    return true;
  } catch {
    return false;
  }
}

function Nick({ nick, run }) {
  return html`
    <li class="nick-row" data-skeleton=${nick.skeleton} data-blocked=${nick.blocked ? 'true' : 'false'}>
      <span class="nick-name"><${RequesterBadge} badge=${nick.badge} /><b>${nick.nick}</b>
        ${nick.blocked ? html`<span class="stt s-failed">${t('settings.nicks.blocked_tag')}</span>` : null}
        ${nick.protected ? html`<span class="stt nick-pin" data-pin>${tn('settings.nicks.pin_devices', nick.devices ?? 0)}</span>` : null}</span>
      <small class="nick-used">${t('settings.nicks.last_used', { when: finishedText(nick.lastUsedAt) })}</small>
      <span class="nick-actions">
        ${nick.protected
          ? html`<button type="button" class="btn" data-action="reset-pin" aria-label=${t('settings.nicks.reset_pin_label', { nick: nick.nick })}
              onClick=${() => confirmResetPin(nick, run)}>${t('settings.nicks.reset_pin')}</button>`
          : null}
        <button type="button" class="btn" data-action="release" aria-label=${t('settings.nicks.release_label', { nick: nick.nick })}
          onClick=${() => run(() => api('POST', `${NICKS}/${encodeURIComponent(nick.skeleton)}/release`), t('settings.nicks.released', { nick: nick.nick }))}>
          ${t('settings.nicks.release')}</button>
        ${nick.blocked
          ? null
          : html`<button type="button" class="btn danger" data-action="block" aria-label=${t('settings.nicks.block_label', { nick: nick.nick })}
              onClick=${() => confirmBlock(nick, run)}>${t('settings.nicks.block')}</button>`}
      </span>
    </li>`;
}

/** Resetting removes the viewer's PIN and signs their other devices out: asked about first. */
function confirmResetPin(nick, run) {
  openDialog({
    title: () => t('settings.nicks.reset_pin_title'),
    body: () => html`<p class="dialog-intro">${t('settings.nicks.reset_pin_body', { nick: nick.nick })}</p>`,
    buttons: [
      { key: 'settings.nicks.reset_pin', primary: true,
        action: () => run(() => api('POST', `${NICKS}/${encodeURIComponent(nick.skeleton)}/reset-pin`), t('settings.nicks.pin_reset_done', { nick: nick.nick })) },
      { key: 'settings.cancel' },
    ],
  });
}

/** Blocking refuses every video from that device: it is asked about first. */
function confirmBlock(nick, run) {
  openDialog({
    title: () => t('settings.nicks.block_title'),
    body: () => html`<p class="dialog-intro">${t('settings.nicks.block_body', { nick: nick.nick })}</p>`,
    buttons: [
      { key: 'settings.nicks.block', primary: true,
        action: () => run(() => api('POST', '/api/admin/devices/block', { skeleton: nick.skeleton }), t('settings.nicks.blocked_done', { nick: nick.nick })) },
      { key: 'settings.cancel' },
    ],
  });
}

export function Nicks() {
  const [note, say] = useNote();
  useEffect(() => {
    load();
    const id = setInterval(load, EVERY_MS);
    return () => clearInterval(id);
  }, []);

  const run = async (request, okText) => {
    say(null);
    try {
      await request();
      say('ok', okText);
    } catch (e) {
      say('err', errorText(e));
    }
    await load();
  };

  const data = list.value;
  const blocked = data?.blocked ?? [];

  return html`
    <${Block} title=${t('settings.nicks.title')} hint=${t('settings.nicks.hint')}>
      <div class="nicks" data-nicks>
        ${!data
          ? html`<p class="set-sub">${t('settings.loading')}</p>`
          : data.nicks.length
            ? html`<ul class="nick-list" aria-label=${t('settings.nicks.title')}>
                ${data.nicks.map((n) => html`<${Nick} key=${n.skeleton} nick=${n} run=${run} />`)}</ul>`
            : html`<p class="set-sub">${t('settings.nicks.empty')}</p>`}
        ${blocked.length
          ? html`<b class="nick-sub">${tn('settings.nicks.blocked_devices', blocked.length)}</b>
              <ul class="nick-list" data-blocked-list aria-label=${t('settings.nicks.blocked_title')}>
                ${blocked.map((b) => {
                  const name = b.nick || t('settings.nicks.no_nick');
                  return html`<li class="nick-row" data-block-id=${b.id}>
                  <span class="nick-name"><${RequesterBadge} badge=${b.badge} /><b>${name}</b></span>
                  <small class="nick-used">${t('settings.nicks.blocked_at', { when: finishedText(b.blockedAt) })}</small>
                  <span class="nick-actions">
                    <button type="button" class="btn" data-action="unblock" aria-label=${t('settings.nicks.unblock_label', { nick: name })}
                      onClick=${() => run(() => api('POST', '/api/admin/devices/unblock', { id: b.id }), t('settings.nicks.unblocked', { nick: name }))}>
                      ${t('settings.nicks.unblock')}</button>
                  </span></li>`;
                })}</ul>`
          : null}
        <${Note} note=${note} />
      </div>
    <//>`;
}
