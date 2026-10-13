// The Channel view: the live preview of the channel's stream beside the queue panel.

import { html } from '../html.js';
import { Preview } from '../components/preview.js';
import { QueuePanel } from '../components/queue.js';

export function ChannelView() {
  return html`
    <main class="view channel" id="view" data-view="channel">
      <section class="stage">
        <${Preview} />
      </section>
      <${QueuePanel} />
    </main>`;
}
