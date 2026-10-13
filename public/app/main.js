// The new player interface: title bar (in the program's window), left rail with the views,
// the player bar at the bottom, keyboard shortcuts, menus and dialogs.

import { render } from '/static/vendor/preact.mjs';
import { html } from './html.js';
import { browserLang, chosenLang, loadLang, t } from './i18n.js';
import { Icon } from './icons.js';
import {
  abilities,
  environment,
  firstInfo,
  netStatus,
  restart,
  seekBy,
  session,
  skip,
  startPolling,
  toast,
  togglePause,
} from './store.js';
import { Titlebar, inWindow } from './components/titlebar.js';
import { Rail, narrow, route } from './components/rail.js';
import { PlayerBar } from './components/playerbar.js';
import { MenuLayer, menu } from './components/menu.js';
import { DialogLayer, dialog, openDialog } from './components/dialog.js';
import { DropZone } from './components/dropzone.js';
import { pasteLink } from './components/addlink.js';
import { ChannelView } from './views/channel.js';
import { HistoryView } from './views/history.js';
import { ShareView } from './views/share.js';
import { SettingsView } from './views/settings.js';
import { toggleStats } from './components/stats.js';
import { StatusStrip } from './components/status.js';
import { LoginView } from './views/login.js';

// ---------- Keyboard shortcuts ----------

const kbd = (k) => html`<kbd>${k}</kbd>`;
const plus = (...keys) => keys.map((k, i) => html`${i ? ' + ' : ''}${kbd(k)}`);
const either = (...keys) => keys.map((k, i) => html`${i ? ' / ' : ''}${kbd(k)}`);

function openShortcuts() {
  const rows = () => [
    [plus(t('shortcuts.key_space')), 'shortcuts.play_pause'],
    [either('←', '→'), 'shortcuts.seek_10'],
    [[...plus(t('shortcuts.key_shift'), '←'), ' / ', kbd('→')], 'shortcuts.seek_60'],
    [plus(t('shortcuts.key_home')), 'shortcuts.restart'],
    [plus('N'), 'shortcuts.next'],
    [plus('I'), 'shortcuts.stats'],
    [plus(t('shortcuts.key_ctrl'), 'V'), 'shortcuts.paste'],
    [plus('?'), 'shortcuts.help'],
  ];
  openDialog({
    title: () => t('shortcuts.title'),
    body: () => html`
      <p class="dialog-intro">${t('shortcuts.intro')}</p>
      <dl class="shortcuts">
        ${rows().map(([keys, key]) => html`<dt>${keys}</dt><dd>${t(key)}</dd>`)}
      </dl>`,
    buttons: [{ key: 'ui.close', primary: true }],
  });
}

/** Where typing goes: no shortcut acts there. */
function typingIn(el) {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  if (el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') return true;
  if (el.tagName === 'INPUT') {
    return !['button', 'checkbox', 'radio', 'range', 'submit', 'reset'].includes(el.type);
  }
  return false;
}

/** Elements that use Space and Enter themselves (buttons, links, menu items). */
function pressable(el) {
  return el instanceof HTMLElement && Boolean(el.closest('button, a[href], [role=button], [role=menuitem], [role=tab], summary, input'));
}

/** Elements that use the arrow keys and Home themselves (sliders). */
function adjustable(el) {
  return el instanceof HTMLElement && Boolean(el.closest('input[type=range], [role=slider]'));
}

function onKeyDown(e) {
  if (e.defaultPrevented || e.isComposing || menu.value || dialog.value) return;
  if (typingIn(e.target)) return;
  const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
  // The keys do what the buttons do, and only when the buttons are on (a live stream cannot
  // be paused or seeked; nothing playing has nothing to skip).
  const can = abilities.value;
  let action = null;
  if (e.key === ' ' && plain && !e.shiftKey && !pressable(e.target)) {
    if (can.canToggle) action = togglePause;
  } else if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && plain && !adjustable(e.target)) {
    const step = (e.shiftKey ? 60 : 10) * (e.key === 'ArrowLeft' ? -1 : 1);
    if (can.canSeek) action = () => seekBy(step);
  } else if (e.key === 'Home' && plain && !e.shiftKey && !adjustable(e.target)) {
    if (can.canSeek) action = restart;
  } else if ((e.key === 'n' || e.key === 'N') && plain) {
    if (can.current) action = skip;
  } else if ((e.key === 'i' || e.key === 'I') && plain) action = toggleStats;
  else if (e.key === '?' && plain) action = openShortcuts;
  if (!action) return;
  e.preventDefault();
  action();
}

// ---------- The app ----------

/** Ctrl+V outside text fields: the link on the clipboard goes to the queue. */
function onPaste(e) {
  if (e.defaultPrevented || menu.value || dialog.value || typingIn(e.target)) return;
  if (session.value !== 'ok') return;
  const text = e.clipboardData?.getData('text/plain');
  if (!text) return;
  e.preventDefault();
  pasteLink(text);
}

function View() {
  const { view, section } = route.value;
  if (view === 'channel') return html`<${ChannelView} onShortcuts=${openShortcuts} />`;
  if (view === 'history') return html`<${HistoryView} />`;
  if (view === 'share') return html`<${ShareView} />`;
  if (view === 'settings') return html`<${SettingsView} />`;
  const titleKey = {
    channel: 'nav.channel',
    history: 'nav.history',
    share: 'nav.share',
    settings: 'nav.settings',
  }[view];
  return html`
    <main class="view" id="view" data-view=${view} data-section=${section ?? undefined}>
      <div class="page"><h1>${t(titleKey)}</h1></div>
    </main>`;
}

const NET_KEYS = {
  online: 'net.online',
  connecting: 'net.connecting',
  offline: 'net.offline',
  manual: 'net.manual',
  local: 'net.local',
};

/** The top of the screen on a phone (no title bar there): the name and the internet status. */
function PhoneTop() {
  const net = netStatus.value;
  return html`
    <header class="ptop">
      <span class="tb-logo" aria-hidden="true"><${Icon} name="play" size=${11} /></span>
      <b class="ptop-title">VRCinema</b>
      ${net ? html`<span class=${`pill net-${net}`} role="status"><span class="dot" aria-hidden="true"></span>${t(NET_KEYS[net])}</span>` : null}
    </header>`;
}

function Toast() {
  const msg = toast.value;
  if (!msg) return null;
  return html`
    <div class=${`toast toast-${msg.kind}`} role=${msg.kind === 'error' ? 'alert' : 'status'}>
      <span>${msg.text}</span>
      <button type="button" class="icon-button" aria-label=${t('ui.close')} title=${t('ui.close')}
        onClick=${() => (toast.value = null)}><${Icon} name="close" size=${14} /></button>
    </div>`;
}

function App() {
  const signedOut = session.value === 'signed_out';
  const phone = narrow.value;
  // Signed out: the sign-in screen alone (nothing of the panel can be read yet).
  if (signedOut) {
    return html`
      <div class=${`app signed-out ${inWindow() ? 'in-window' : 'in-browser'}`}>
        ${inWindow() ? html`<${Titlebar} />` : null}
        <${LoginView} />
      </div>`;
  }
  // On a phone the rail is a tab bar at the bottom and the player bar sits under the preview
  // (in the Channel view).
  return html`
    <div class=${`app ${inWindow() ? 'in-window' : 'in-browser'}${phone ? ' phone' : ''}`}>
      ${inWindow() ? html`<${Titlebar} />` : phone ? html`<${PhoneTop} />` : null}
      <${StatusStrip} />
      <div class="body">
        <${Rail} />
        <${View} />
      </div>
      <${Toast} />
      <${DropZone} />
      ${phone ? null : html`<${PlayerBar} onShortcuts=${openShortcuts} />`}
      <${MenuLayer} />
      <${DialogLayer} />
    </div>`;
}

async function start() {
  // The program's language in its window. Elsewhere (remote panel, a server without a window,
  // the sign-in screen): the language picked in this browser, else the program's, else the
  // browser's.
  const info = await firstInfo();
  // The window's first run goes through the wizard first.
  if (info && info.firstRunCompleted === false) {
    location.replace('/wizard');
    return;
  }
  await loadLang(info?.language ?? chosenLang() ?? environment.value?.language ?? browserLang());
  render(html`<${App} />`, document.getElementById('root'));
  document.addEventListener('keydown', onKeyDown);
  document.addEventListener('paste', onPaste);
  startPolling();
}

start();
