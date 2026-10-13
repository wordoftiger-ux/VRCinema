// The interface's icons: small inline SVGs drawn in the text colour (no icon font, no files).

import { html } from './html.js';

const PATHS = {
  channel: 'M8 5.5v13l10.5-6.5z',
  history: 'M12 4a8 8 0 1 1-7.4 5h2.2A6 6 0 1 0 12 6v3L7.5 5 12 1zM11 8h2v4.6l3.2 1.9-1 1.7L11 13.7z',
  share: 'M12 2.5l5 5-1.4 1.4L13 6.3V15h-2V6.3L8.4 8.9 7 7.5zM5 13h2v6h10v-6h2v8H5z',
  settings:
    'M10.3 2h3.4l.5 2.6c.6.2 1.1.5 1.6.9l2.5-.9 1.7 2.9-2 1.7a6 6 0 0 1 0 1.8l2 1.7-1.7 2.9-2.5-.9c-.5.4-1 .7-1.6.9l-.5 2.6h-3.4l-.5-2.6a6 6 0 0 1-1.6-.9l-2.5.9-1.7-2.9 2-1.7a6 6 0 0 1 0-1.8l-2-1.7 1.7-2.9 2.5.9c.5-.4 1-.7 1.6-.9zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z',
  play: 'M8 5v14l11-7z',
  pause: 'M7 5h3.5v14H7zM13.5 5H17v14h-3.5z',
  prev: 'M6 5h2v14H6zM9.5 12L19 5v14z',
  next: 'M16 5h2v14h-2zM14.5 12L5 5v14z',
  volume: 'M4 9h4l5-4v14l-5-4H4zM15.5 8.5a5 5 0 0 1 0 7l-1.4-1.4a3 3 0 0 0 0-4.2zM18.3 5.7a9 9 0 0 1 0 12.6l-1.4-1.4a7 7 0 0 0 0-9.8z',
  muted: 'M4 9h4l5-4v14l-5-4H4zM15.3 9.7l1.4-1.4 2.3 2.3 2.3-2.3 1.4 1.4-2.3 2.3 2.3 2.3-1.4 1.4-2.3-2.3-2.3 2.3-1.4-1.4 2.3-2.3z',
  copy: 'M8 3h11v13h-2V5H8zM4 7h11v14H4zm2 2v10h7V9z',
  minimize: 'M5 11.5h14v1H5z',
  maximize: 'M6 6h12v12H6zm1 1v10h10V7z',
  restore: 'M8 5h11v11h-2v-1h1V6H9v1H8zM5 8h11v11H5zm1 1v9h9V9z',
  close: 'M6.7 6l5.3 5.3L17.3 6l.7.7-5.3 5.3 5.3 5.3-.7.7-5.3-5.3L6.7 18l-.7-.7 5.3-5.3L6 6.7z',
  top: 'M5 4h14v2H5zM12 8l6.5 6.5-1.4 1.4L13 11.8V20h-2v-8.2l-4.1 4.1-1.4-1.4z',
  check: 'M9.5 16.2L5.3 12l-1.4 1.4 5.6 5.6L20.3 7.4 18.9 6z',
  plus: 'M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6z',
  keyboard: 'M3 6h18v12H3zm2 2v8h14V8zm1 1h2v2H6zm3 0h2v2H9zm3 0h2v2h-2zm3 0h3v2h-3zM7 13h10v2H7z',
};

/** `<svg>` of the icon `name`, hidden from screen readers (the button carries the label). */
export function Icon({ name, size = 20 }) {
  return html`<svg class="icon" width=${size} height=${size} viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path fill="currentColor" d=${PATHS[name]} /></svg>`;
}
