// The Channel view: the preview of the channel's stream (comes with a later step) beside the
// queue panel.

import { html } from '../html.js';
import { QueuePanel } from '../components/queue.js';

export function ChannelView() {
  return html`
    <main class="view channel" id="view" data-view="channel">
      <section class="stage">
        <div class="video" data-slot="preview"></div>
      </section>
      <${QueuePanel} />
    </main>`;
}
