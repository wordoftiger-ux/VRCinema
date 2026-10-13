// The small icon next to a requester's name: one of 12 shapes tinted by a hue (the viewer's
// device decides both, so the same viewer always has the same icon), or the owner's mark.
// Drawn on a 24×24 grid; the engine sends `{hue, shape}` (shape 0..11) or `"host"`.

import { html } from '../html.js';

export const SHAPES = [
  'M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18z', // circle
  'M5 5h14v14H5z', // square
  'M12 3L22 20H2z', // triangle
  'M12 2L22 12L12 22L2 12z', // diamond
  'M12 2.5L21.5 9.5L18 20.5H6L2.5 9.5z', // pentagon
  'M7 3.5h10l5 8.5l-5 8.5H7L2 12z', // hexagon
  'M12 2l2.9 6.9 7.4.6-5.6 4.8 1.7 7.2L12 17.5 5.7 21.5l1.7-7.2L1.7 9.5l7.4-.6z', // star
  'M9 3h6v6h6v6h-6v6H9v-6H3V9h6z', // plus
  'M12 21s-9-5.6-9-12a5 5 0 0 1 9-3 5 5 0 0 1 9 3c0 6.4-9 12-9 12z', // heart
  'M15 3a9 9 0 1 0 6 14A8 8 0 0 1 15 3z', // crescent
  'M8 2h8l6 6v8l-6 6H8l-6-6V8z', // octagon
  'M12 1.5l3 7.5 7.5 3-7.5 3-3 7.5-3-7.5-7.5-3 7.5-3z', // four-point star
];

// The owner: a house.
const HOST = 'M12 3L2 12h3v8h5v-5h4v5h5v-8h3z';

function validBadge(badge) {
  return (
    badge &&
    typeof badge === 'object' &&
    Number.isInteger(badge.shape) &&
    badge.shape >= 0 &&
    badge.shape < SHAPES.length &&
    Number.isFinite(badge.hue)
  );
}

/** The icon of `badge` (`"host"`, `{hue, shape}`); nothing for an anonymous requester. */
export function RequesterBadge({ badge }) {
  if (badge === 'host') {
    return html`<svg class="rbadge rbadge-host" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" focusable="false" data-badge="host"><path fill="currentColor" d=${HOST} /></svg>`;
  }
  if (!validBadge(badge)) return null;
  const hue = ((Math.round(badge.hue) % 360) + 360) % 360;
  return html`<svg class="rbadge" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" focusable="false"
    data-badge=${`shape-${badge.shape}`}><path fill=${`hsl(${hue} 70% 62%)`} d=${SHAPES[badge.shape]} /></svg>`;
}
