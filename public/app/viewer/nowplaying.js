// The "now playing" card: the title, who added it, and how far it is (no bar for a live stream).

import { html } from '../html.js';
import { t, videoTitle } from '../i18n.js';
import { RequesterBadge } from '../components/badge.js';
import { formatTime } from './format.js';

/** `requested by Tomek`, `added by the host` for the owner, `requested anonymously`. */
export function requesterText(item) {
  if (item.requesterName) return t('queue.requested_by', { name: item.requesterName });
  return item.requesterBadge === 'host' ? t('viewer.added_by_host') : t('queue.requested_anonymously');
}

function Progress({ current }) {
  if (current.isLive) {
    return html`<p class="v-time"><span class="live">${t('player.live')}</span> ${t('viewer.live_stream')}</p>`;
  }
  const duration = Number.isFinite(current.durationS) && current.durationS > 0 ? current.durationS : null;
  const position = Math.max(0, Number(current.positionS) || 0);
  const shown = duration ? Math.min(position, duration) : position;
  const text = duration ? `${formatTime(shown)} / ${formatTime(duration)}` : formatTime(shown);
  if (!duration) return html`<p class="v-time">${text}</p>`;
  return html`
    <div class="v-progress" role="progressbar" aria-label=${t('viewer.progress')} aria-valuemin="0"
      aria-valuemax=${Math.round(duration)} aria-valuenow=${Math.round(shown)} aria-valuetext=${text}>
      <span style=${{ width: `${Math.min(100, (shown / duration) * 100)}%` }}></span>
    </div>
    <p class="v-time">${text}</p>`;
}

export function NowPlaying({ state }) {
  const current = state?.current;
  if (!current) {
    return html`
      <section class="v-now card2" aria-labelledby="v-now-h">
        <h2 id="v-now-h" class="v-eyebrow">${t('viewer.now_playing')}</h2>
        <p class="v-now-title v-idle">${t('player.nothing_playing')}</p>
        <p class="v-sub">${t('player.nothing_playing_hint')}</p>
      </section>`;
  }
  return html`
    <section class="v-now card2" aria-labelledby="v-now-h">
      <h2 id="v-now-h" class="v-eyebrow">
        ${t('viewer.now_playing')}${state.paused ? html` <span class="v-paused">${t('viewer.paused')}</span>` : null}
      </h2>
      <p class="v-now-title">${videoTitle(current.title)}</p>
      <p class="v-sub"><${RequesterBadge} badge=${current.requesterBadge} />${requesterText(current)}</p>
      <${Progress} current=${current} />
    </section>`;
}
