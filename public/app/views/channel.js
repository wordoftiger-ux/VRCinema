// The Channel view: the live preview of the channel's stream beside the queue panel. On a
// phone the three are one column: the preview, the player bar under it, the queue below.

import { html } from '../html.js';
import { Preview } from '../components/preview.js';
import { QueuePanel } from '../components/queue.js';
import { PlayerBar } from '../components/playerbar.js';
import { narrow } from '../components/rail.js';

export function ChannelView({ onShortcuts }) {
  return html`
    <main class="view channel" id="view" data-view="channel">
      <section class="stage">
        <${Preview} />
      </section>
      ${narrow.value ? html`<${PlayerBar} onShortcuts=${onShortcuts} />` : null}
      <${QueuePanel} />
    </main>`;
}
