// A film's picture: the public thumbnail the channel gives, or (without one, or when it does
// not load) a neutral gradient. Loaded without a referrer; never blocks the layout.

import { useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';

/** `src` or null; `seed` (an id) picks one of a few navy-toned gradients for the stand-in. */
export function Pic({ src = null, seed = 0, className = '' }) {
  const [failed, setFailed] = useState(null);
  const shown = src && failed !== src;
  return html`
    <span class=${`v-pic v-pic-${Math.abs(Number(seed) || 0) % 4} ${className}`} aria-hidden="true">
      ${shown
        ? html`<img src=${src} alt="" loading="lazy" decoding="async" referrerpolicy="no-referrer" draggable="false"
            onError=${() => setFailed(src)} />`
        : null}
    </span>`;
}
