// The Channel view: two screens beside the queue panel, like a broadcasting program — on the
// left "Preview" (the newest material the server has, a few seconds after a seek or a skip),
// on the right "On air" (the channel's stream as the viewers get it). On a phone there is one
// column: the On air screen, the player bar under it, the queue below.

import { html } from '../html.js';
import { Preview } from '../components/preview.js';
import { MonitorScreen } from '../components/monitor.js';
import { QueuePanel } from '../components/queue.js';
import { PlayerBar } from '../components/playerbar.js';
import { narrow } from '../components/rail.js';

export function ChannelView({ onShortcuts }) {
  const phone = narrow.value;
  return html`
    <main class="view channel" id="view" data-view="channel">
      ${phone
        ? html`<section class="stage"><${Preview} /></section>`
        : html`<section class="stage dual"><${MonitorScreen} /><${Preview} dual /></section>`}
      ${phone ? html`<${PlayerBar} onShortcuts=${onShortcuts} />` : null}
      <${QueuePanel} />
    </main>`;
}
