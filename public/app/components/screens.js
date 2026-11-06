// Where the two screens of the Channel view sit (spec 2026-11-06-preview-player-design, 4.2):
// the Preview is the main screen and On air a picture-in-picture in its corner (`pip`); a click
// on the small screen swaps them (`swapped`); they can stand side by side (`side`) or On air can
// go to a window of its own (`window`, with "always on top" in the program's window shell).
// Remembered in this browser. The air's window says it is alive over a BroadcastChannel; when
// it closes (or stops answering) On air comes back where it was.

import { signal, effect } from '/static/vendor/signals.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { Icon } from '../icons.js';

const KEY = 'vrcinema.screens';
const LAYOUTS = ['pip', 'swapped', 'side', 'window'];
const CHANNEL = 'vrcinema-air';
const ALIVE_EVERY_MS = 1000;
/** The air's window is gone when it has not answered for this long… */
const GONE_AFTER_MS = 4000;
/** …but a new one gets this long to open first. */
const OPEN_GRACE_MS = 10000;

function stored() {
  try {
    const value = JSON.parse(localStorage.getItem(KEY) ?? 'null');
    if (value && LAYOUTS.includes(value.layout)) return value;
  } catch {
    // Nothing stored.
  }
  return { layout: 'pip', docked: 'pip' };
}

const first = stored();
/** `pip`, `swapped`, `side` or `window`. */
export const airLayout = signal(first.layout === 'window' ? first.docked : first.layout);
/** Where On air goes back to from its own window. */
let docked = first.docked && first.docked !== 'window' ? first.docked : 'pip';

effect(() => {
  const layout = airLayout.value;
  if (layout !== 'window') docked = layout;
  try {
    localStorage.setItem(KEY, JSON.stringify({ layout, docked }));
  } catch {
    // No storage: not remembered.
  }
});

let bus = null;
let lastAlive = 0;
let openedAt = 0;
let watchdog = 0;

function channel() {
  if (!bus && typeof BroadcastChannel === 'function') {
    bus = new BroadcastChannel(CHANNEL);
    bus.onmessage = (e) => {
      if (e.data?.type === 'alive') lastAlive = performance.now();
      else if (e.data?.type === 'closed' && airLayout.peek() === 'window') airLayout.value = docked;
    };
  }
  return bus;
}

export function setLayout(layout) {
  if (!LAYOUTS.includes(layout)) return;
  if (layout === 'window') {
    openAirWindow();
    return;
  }
  airLayout.value = layout;
}

/** On air in a window of its own (the program's window shell opens it as a real window). */
export function openAirWindow() {
  channel();
  openedAt = performance.now();
  window.open('/admin?window=air', 'vrcinema-air', 'popup=yes,width=640,height=400');
  airLayout.value = 'window';
  clearInterval(watchdog);
  watchdog = setInterval(() => {
    if (airLayout.peek() !== 'window') {
      clearInterval(watchdog);
      return;
    }
    const now = performance.now();
    if (now - openedAt > OPEN_GRACE_MS && now - lastAlive > GONE_AFTER_MS) airLayout.value = docked;
  }, ALIVE_EVERY_MS);
}

/** On air back in the main window. */
export function bringAirBack() {
  channel()?.postMessage({ type: 'close' });
  airLayout.value = docked;
}

/** The air's own window: tells the main window it is alive; closes when asked to. */
export function announceAirWindow() {
  const b = channel();
  if (!b) return;
  const alive = () => b.postMessage({ type: 'alive' });
  alive();
  setInterval(alive, ALIVE_EVERY_MS);
  window.addEventListener('pagehide', () => b.postMessage({ type: 'closed' }));
  b.addEventListener('message', (e) => {
    if (e.data?.type === 'close') window.close();
  });
}

/** One small button of a screen's corner. */
function Tool({ icon, labelKey, onClick }) {
  return html`<button type="button" class="screen-tool" aria-label=${t(labelKey)} title=${t(labelKey)}
    onClick=${(e) => {
      e.stopPropagation();
      onClick();
    }}><${Icon} name=${icon} size=${14} /></button>`;
}

/**
 * The layout buttons of a screen: on the small screen "swap", on On air (small or beside)
 * "side by side" / "picture in picture" and "own window".
 */
export function ScreenTools({ which, role }) {
  const layout = airLayout.value;
  const tools = [];
  if (role === 'pip') tools.push(html`<${Tool} icon="swap" labelKey="screens.swap" onClick=${() => setLayout(layout === 'swapped' ? 'pip' : 'swapped')} />`);
  if (which === 'air' || role === 'pip') {
    if (layout === 'side') tools.push(html`<${Tool} icon="pip" labelKey="screens.pip" onClick=${() => setLayout('pip')} />`);
    else tools.push(html`<${Tool} icon="dock" labelKey="screens.side" onClick=${() => setLayout('side')} />`);
    tools.push(html`<${Tool} icon="popout" labelKey="screens.window" onClick=${() => setLayout('window')} />`);
  }
  if (!tools.length) return null;
  return html`<div class="screen-tools" role="toolbar" aria-label=${t('screens.tools')}>${tools}</div>`;
}

/** In the main window while On air has its own window. */
export function AirAway() {
  return html`
    <div class="air-away" role="status">
      <${Icon} name="popout" size=${14} />
      <span>${t('screens.away')}</span>
      <button type="button" class="btn btn-small" onClick=${bringAirBack}>${t('screens.bring_back')}</button>
    </div>`;
}
