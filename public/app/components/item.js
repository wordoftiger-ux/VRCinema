// What every list of videos shows for one: a picture, the title (with the LIVE mark), and a
// line of details with the requester's icon.

import { useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t, videoTitle } from '../i18n.js';
import { RequesterBadge } from './badge.js';
import { formatTime } from './playerbar.js';

export const isLocal = (item) => String(item.sourceUrl ?? '').startsWith('local:');
/** A video file a viewer sent from the request page. */
export const isUpload = (item) => String(item.sourceUrl ?? '').startsWith('upload:');
export const isWeb = (item) => /^https?:\/\//i.test(String(item.sourceUrl ?? ''));

/**
 * The picture of a video: `src` (the item's `thumb`: the site's picture, loaded without a
 * referrer, or the engine's frame of a disk file), else (or when it does not load) a coloured
 * stand-in.
 */
export function Thumb({ id, src = null }) {
  const [failed, setFailed] = useState(null);
  const cls = `th th-${Math.abs(Number(id) || 0) % 4}`;
  if (src && failed !== src) {
    return html`<span class=${`${cls} th-pic`} aria-hidden="true">
      <img src=${src} alt="" loading="lazy" decoding="async" referrerpolicy="no-referrer" draggable="false"
        onError=${() => setFailed(src)} />
    </span>`;
  }
  return html`<span class=${cls} aria-hidden="true"></span>`;
}

/** `3:52` — empty for a live stream or a video of unknown length. */
export function lengthText(item) {
  return !item.isLive && Number.isFinite(item.durationS) ? formatTime(item.durationS) : '';
}

/** `3:52 · Tomek`, `1:42:10 · from disk`: what follows a title in the queue. */
export function itemDetails(item) {
  const parts = [lengthText(item)];
  if (isLocal(item)) parts.push(t('queue.from_disk'));
  if (isUpload(item)) parts.push(t('queue.uploaded'));
  if (item.requesterName) parts.push(item.requesterName);
  else if (item.requesterBadge === 'host') {
    if (!isLocal(item)) parts.push(t('queue.by_host'));
  } else parts.push(t('queue.anonymous'));
  return parts.filter(Boolean).join(' · ');
}

/** Title and details; `sub` replaces the details. */
export function ItemText({ item, sub }) {
  const title = videoTitle(item.title);
  return html`
    <div class="qi-text">
      <b class="qi-title" title=${title}>
        <span class="qi-name">${title}</span>
        ${item.isLive ? html`<span class="live">${t('player.live')}</span>` : null}
      </b>
      <small class="qi-sub"><${RequesterBadge} badge=${item.requesterBadge} />${sub ?? itemDetails(item)}</small>
    </div>`;
}
