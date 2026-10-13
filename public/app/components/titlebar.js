// The window's own title bar (the window has no frame): drag to move, double click to
// maximize (Tauri's drag region), minimize / maximize / close buttons. The window commands
// are allowed only for the engine's own address (the window shell grants them to it); in a
// browser (remote panel) there is no title bar.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { Icon } from '../icons.js';

/** The page runs in the program's window (Tauri), not in a browser. */
export function inWindow() {
  return typeof window.__TAURI_INTERNALS__?.invoke === 'function';
}

/** Runs a command of the window (`minimize`, `toggle_maximize`, `close`, `is_maximized`). */
export function windowCommand(name) {
  if (!inWindow()) return Promise.resolve(null);
  return window.__TAURI_INTERNALS__.invoke(`plugin:window|${name}`).catch(() => null);
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
    return () => {
      clearTimeout(timer);
      window.removeEventListener('resize', onResize);
    };
  }, []);

  const button = (name, icon, labelKey, cls = '') => html`
    <button type="button" class=${`wc-button ${cls}`} aria-label=${t(labelKey)} title=${t(labelKey)}
      onClick=${() => windowCommand(name)}><${Icon} name=${icon} size=${16} /></button>`;

  return html`
    <header class="titlebar" data-tauri-drag-region="deep">
      <span class="tb-logo" aria-hidden="true"><${Icon} name="play" size=${11} /></span>
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
