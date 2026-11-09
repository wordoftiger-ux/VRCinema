// The Channel view: the screens beside the queue panel, like a broadcasting program. The
// Preview (the film itself, the owner's controls act on it at once) is the main screen; On air
// (the channel as the viewers get it, a few seconds behind) is a picture-in-picture in its
// corner, swapped by a click, beside it, or in a window of its own (components/screens.js).
// On a phone, and in a remote panel, there is one screen: On air.
//
// The app shell keeps this view mounted on the other tabs too (`mini`): only its screens show
// then, small, in the mini player (components/mini.js), so the players keep playing.

import { html } from '../html.js';
import { t } from '../i18n.js';
import { MiniControls } from '../components/mini.js';
import { Preview } from '../components/preview.js';
import { MonitorScreen } from '../components/monitor.js';
import { QueuePanel } from '../components/queue.js';
import { PlayerBar } from '../components/playerbar.js';
import { narrow } from '../components/rail.js';
import { access } from '../store.js';
import { AirAway, ScreenTools, airLayout, setLayout } from '../components/screens.js';

function Screens() {
  // Without the Preview (a remote panel) the two screens stand side by side as before.
  const layout = access.value === 'window' ? airLayout.value : 'side';
  const previewRole = layout === 'swapped' ? 'pip' : layout === 'side' ? 'side' : 'main';
  const airRole = layout === 'pip' ? 'pip' : layout === 'swapped' ? 'main' : 'side';
  const swap = () => setLayout(layout === 'swapped' ? 'pip' : 'swapped');
  // The screens keep their order (and their players) whatever the layout: only classes change.
  return html`
    <section class=${`stage dual screens layout-${layout}`}>
      <${MonitorScreen} role=${previewRole} onActivate=${swap}
        tools=${access.value === 'window' && previewRole === 'pip' ? html`<${ScreenTools} which="preview" role="pip" />` : null} />
      ${layout === 'window'
        ? html`<${AirAway} />`
        : html`<${Preview} dual role=${airRole} onActivate=${swap}
            tools=${access.value === 'window' ? html`<${ScreenTools} which="air" role=${airRole} />` : null} />`}
    </section>`;
}

/**
 * `mini`: another tab is open; the screens stay (the same players) in the mini player. The root
 * element and the screens keep their place in the tree either way, so nothing is remounted.
 */
export function ChannelView({ onShortcuts, mini = false }) {
  const phone = narrow.value;
  return html`
    <div class=${mini ? 'mini-player' : 'view channel'} id=${mini ? undefined : 'view'}
      data-view=${mini ? undefined : 'channel'} role=${mini ? 'region' : 'main'}
      aria-label=${mini ? t('mini.region') : undefined}>
      ${phone ? html`<section class="stage"><${Preview} /></section>` : html`<${Screens} />`}
      ${mini ? html`<${MiniControls} />` : phone ? html`<${PlayerBar} onShortcuts=${onShortcuts} />` : null}
      ${mini ? null : html`<${QueuePanel} />`}
    </div>`;
}
