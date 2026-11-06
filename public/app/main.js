// The new player interface: title bar (in the program's window), left rail with the views,
// the player bar at the bottom, keyboard shortcuts, menus and dialogs.

import { render } from '/static/vendor/preact.mjs';
import { signal } from '/static/vendor/signals.mjs';
import { html } from './html.js';
import { browserLang, chosenLang, loadLang, t } from './i18n.js';
import { AppIcon, Icon } from './icons.js';
import {
  abilities,
  access,
  environment,
  firstInfo,
  netStatus,
  restart,
  seekBy,
  session,
  signOut,
  skip,
  startPolling,
  toast,
  togglePause,
} from './store.js';
import { Titlebar, inWindow } from './components/titlebar.js';
import { Rail, narrow, route } from './components/rail.js';
import { AirVolume, PlayerBar } from './components/playerbar.js';
import { Preview } from './components/preview.js';
import { announceAirWindow } from './components/screens.js';
import { MenuLayer, menu } from './components/menu.js';
import { DialogLayer, dialog, openDialog } from './components/dialog.js';
import { DropZone } from './components/dropzone.js';
import { pasteLink } from './components/addlink.js';
import { ChannelView } from './views/channel.js';
import { HistoryView } from './views/history.js';
import { ShareView } from './views/share.js';
import { SettingsView } from './views/settings.js';
import { toggleStats } from './components/stats.js';
import { monitorRunning, syncMonitor } from './components/monitor.js';
import { StatusStrip } from './components/status.js';
import { LoginView } from './views/login.js';

// ---------- Keyboard shortcuts ----------

const kbd = (k) => html`<kbd>${k}</kbd>`;
const plus = (...keys) => keys.map((k, i) => html`${i ? ' + ' : ''}${kbd(k)}`);
const either = (...keys) => keys.map((k, i) => html`${i ? ' / ' : ''}${kbd(k)}`);

function openShortcuts() {
  const rows = () => [
    [either(t('shortcuts.key_space'), 'K'), 'shortcuts.play_pause'],
    [either('←', '→'), 'shortcuts.seek_5'],
    [either('J', 'L'), 'shortcuts.seek_10'],
    [[...plus(t('shortcuts.key_shift'), '←'), ' / ', kbd('→')], 'shortcuts.seek_60'],
    [plus(t('shortcuts.key_wheel')), 'shortcuts.wheel'],
    [plus(t('shortcuts.key_home')), 'shortcuts.restart'],
    [plus('N'), 'shortcuts.next'],
    [plus('I'), 'shortcuts.stats'],
    [plus('S'), 'shortcuts.sync'],
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
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  if ((e.key === ' ' && plain && !e.shiftKey && !pressable(e.target)) || (key === 'k' && plain && !e.shiftKey)) {
    if (can.canToggle) action = togglePause;
  } else if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && plain && !adjustable(e.target)) {
    const step = (e.shiftKey ? 60 : 5) * (e.key === 'ArrowLeft' ? -1 : 1);
    if (can.canSeek) action = () => seekBy(step);
  } else if ((key === 'j' || key === 'l') && plain && !e.shiftKey) {
    // J / L: 10 s back / forward (like the video sites).
    if (can.canSeek) action = () => seekBy(key === 'j' ? -10 : 10);
  } else if (e.key === 'Home' && plain && !e.shiftKey && !adjustable(e.target)) {
    if (can.canSeek) action = restart;
  } else if ((e.key === 'n' || e.key === 'N') && plain) {
    if (can.current) action = skip;
  } else if ((e.key === 'i' || e.key === 'I') && plain) action = toggleStats;
  else if ((e.key === 's' || e.key === 'S') && plain) {
    // Only where the monitor runs (the window's Channel view); its button does the same.
    if (monitorRunning()) action = syncMonitor;
  } else if (e.key === '?' && plain) action = openShortcuts;
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

/**
 * The top of the screen on a phone (no title bar there): the name, the internet status and, for
 * a password session, signing out.
 */
function PhoneTop() {
  const net = netStatus.value;
  const password = access.value === 'remote' || access.value === 'headless';
  return html`
    <header class="ptop">
      <${AppIcon} />
      <b class="ptop-title">VRCinema</b>
      ${net ? html`<span class=${`pill net-${net}`} role="status"><span class="dot" aria-hidden="true"></span>${t(NET_KEYS[net])}</span>` : null}
      ${password
        ? html`<button type="button" class="icon-button ptop-out" data-sign-out aria-label=${t('settings.general.sign_out')}
            title=${t('settings.general.sign_out')} onClick=${signOut}><${Icon} name="logout" size=${20} /></button>`
        : null}
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

// ---------- On air in a window of its own ----------

const ON_TOP_KEY = 'vrcinema.airOnTop';

function storedOnTop() {
  try {
    return localStorage.getItem(ON_TOP_KEY) === 'true';
  } catch {
    return false;
  }
}

/** Keeps the air's window above the others (only in the program's window shell). */
function applyOnTop(value) {
  try {
    localStorage.setItem(ON_TOP_KEY, String(value));
  } catch {
    // Not remembered.
  }
  if (!inWindow()) return;
  window.__TAURI_INTERNALS__
    .invoke('plugin:window|set_always_on_top', { label: 'air', value })
    .catch((e) => console.error('VRCinema: always on top failed:', String(e?.message ?? e)));
}

const onTop = signal(storedOnTop());

function AirWindowApp() {
  return html`
    <div class="app air-window">
      <header class="aw-bar">
        <${AppIcon} />
        <b class="aw-title">${t('monitor.on_air')}</b>
        <${AirVolume} />
        ${inWindow()
          ? html`<label class="aw-top">
              <input type="checkbox" checked=${onTop.value}
                onChange=${(e) => {
                  onTop.value = e.currentTarget.checked;
                  applyOnTop(onTop.value);
                }} />
              <span>${t('screens.on_top')}</span>
            </label>`
          : null}
      </header>
      <section class="stage"><${Preview} /></section>
      <${Toast} />
    </div>`;
}

async function startAirWindow() {
  const info = await firstInfo();
  const program = info?.language ?? environment.value?.language;
  await loadLang(access.value === 'window' ? program : (chosenLang() ?? program ?? browserLang()));
  document.title = `VRCinema · ${t('monitor.on_air')}`;
  render(html`<${AirWindowApp} />`, document.getElementById('root'));
  startPolling();
  announceAirWindow();
  if (onTop.peek()) applyOnTop(true);
}

async function start() {
  if (new URLSearchParams(location.search).get('window') === 'air') {
    await startAirWindow();
    return;
  }
  // The program's language in its window. Elsewhere (remote panel, a server without a window,
  // the sign-in screen): the language picked in this browser, else the program's, else the
  // browser's.
  const info = await firstInfo();
  // The window's first run goes through the wizard first.
  if (info && access.value === 'window' && info.firstRunCompleted === false) {
    location.replace('/wizard');
    return;
  }
  const program = info?.language ?? environment.value?.language;
  await loadLang(access.value === 'window' ? program : (chosenLang() ?? program ?? browserLang()));
  render(html`<${App} />`, document.getElementById('root'));
  document.addEventListener('keydown', onKeyDown);
  document.addEventListener('paste', onPaste);
  startPolling();
}

start();
