// The window's own title bar (the window has no frame): drag to move, double click to
// maximize (Tauri's drag region), minimize / maximize / close buttons. The window commands
// are allowed only for the engine's own address (the window shell grants them to it); in a
// browser (remote panel) there is no title bar. A refused or failed command is never silent:
// it goes to the console, and the owner's own actions also show a message.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { AppIcon, Icon } from '../icons.js';
import { showToast } from '../store.js';

/** The page runs in the program's window (Tauri), not in a browser. */
export function inWindow() {
  return typeof window.__TAURI_INTERNALS__?.invoke === 'function';
}

function reportFailure(name, error, tell) {
  const text = String(error?.message ?? error);
  console.error(`VRCinema: window command ${name} failed:`, text);
  if (tell) showToast(t('titlebar.failed', { error: text }));
}

/**
 * Runs a command of the window (`minimize`, `toggle_maximize`, `close`, `is_maximized`):
 * its answer, or null when it failed (logged; `tell`: also shown to the owner).
 */
export function windowCommand(name, { tell = false } = {}) {
  if (!inWindow()) return Promise.resolve(null);
  return window.__TAURI_INTERNALS__.invoke(`plugin:window|${name}`).catch((e) => {
    reportFailure(name, e, tell);
    return null;
  });
}

/** Dragging and double-click maximising are Tauri's own calls: their refusals end up here. */
function onRejection(event) {
  const text = String(event.reason?.message ?? event.reason ?? '');
  if (text.includes('plugin:window|')) reportFailure('drag', event.reason, true);
}

export function Titlebar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let timer = 0;
    const check = async () => setMaximized(Boolean(await windowCommand('is_maximized')));
    const onResize = () => {
      clearTimeout(timer);
      timer = setTimeout(check, 100);
    };
    check();
    window.addEventListener('resize', onResize);
    window.addEventListener('unhandledrejection', onRejection);
    return () => {
      clearTimeout(timer);
      window.removeEventListener('resize', onResize);
      window.removeEventListener('unhandledrejection', onRejection);
    };
  }, []);

  const button = (name, icon, labelKey, cls = '') => html`
    <button type="button" class=${`wc-button ${cls}`} aria-label=${t(labelKey)} title=${t(labelKey)}
      onClick=${() => windowCommand(name, { tell: true })}><${Icon} name=${icon} size=${16} /></button>`;

  return html`
    <header class="titlebar" data-tauri-drag-region="deep">
      <${AppIcon} />
      <span class="tb-title">VRCinema</span>
      <div class="wc">
        ${button('minimize', 'minimize', 'titlebar.minimize')}
        ${maximized
          ? button('toggle_maximize', 'restore', 'titlebar.restore')
          : button('toggle_maximize', 'maximize', 'titlebar.maximize')}
        ${button('close', 'close', 'titlebar.close', 'wc-close')}
      </div>
    </header>`;
}
