// The poster of a film or series: the addon's picture, loaded without a referrer, or (without one, or
// when it does not load) a neutral stand-in.

import { useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';

export function Poster({ src = null, className = '' }) {
  const [failed, setFailed] = useState(null);
  if (src && failed !== src) {
    return html`<span class=${`poster ${className}`} aria-hidden="true">
      <img src=${src} alt="" loading="lazy" decoding="async" referrerpolicy="no-referrer" draggable="false"
        onError=${() => setFailed(src)} />
    </span>`;
  }
  return html`<span class=${`poster poster-none ${className}`} aria-hidden="true"></span>`;
}
