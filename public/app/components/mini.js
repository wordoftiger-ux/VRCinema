// The mini player: away from the Channel tab (History, Share, Settings) the Channel's screens
// stay mounted and keep playing in a small window docked in the bottom right corner (above the
// player bar; above the tab bar on a phone). The players are the same instances (the app shell
// keeps the Channel view mounted, CSS makes it small), so the Preview, On air, their volumes
// and the picture-in-picture layout carry on. A click on it goes back to Channel; its button
// mutes both screens in this window only (the channel's sound does not change).

import { computed, signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { Icon } from '../icons.js';
import { route } from './rail.js';

/** The Channel's screens are in the mini player (another tab is open). */
export const isMini = computed(() => route.value.view !== 'channel');
/** The mini player's own mute (only while it is mini). */
export const miniMuted = signal(false);

/** Whether a screen is silenced by the mini player now. */
export function mutedByMini() {
  return isMini.value && miniMuted.value;
}

const CONTROLS = 'button, input, select, textarea, a[href], summary, [role="switch"], [role="button"], [role="tab"], label';
const GAP = 8;

function overlapping(rect, view, mini) {
  let n = 0;
  for (const el of view.querySelectorAll(CONTROLS)) {
    if (mini.contains(el)) continue;
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) continue;
    if (r.left < rect.right && r.right > rect.left && r.top < rect.bottom && r.bottom > rect.top) n += 1000; // covered
    else if (r.left < rect.right + GAP && r.right > rect.left - GAP && r.top < rect.bottom + GAP && r.bottom > rect.top - GAP) n += 1; // too close
  }
  return n;
}

/**
 * The mini player sits in the bottom right corner; only where it would cover a control of the open
 * view (the end of a long list of settings) does it move to the top right corner, if that is
 * free. Looked at again whenever the view scrolls, resizes or changes; with both corners covered
 * it stays at the bottom.
 */
function placeMini(mini) {
  const view = document.getElementById('view');
  if (!view) return;
  const topEdge = Math.max(view.getBoundingClientRect().top, 0) + 16;
  const top = `${topEdge}px`;
  if (mini.style.getPropertyValue('--mini-top') !== top) mini.style.setProperty('--mini-top', top);
  // Both corners are measured in place (no paint in between), whichever one it is in now.
  delete mini.dataset.corner;
  const bottomRect = mini.getBoundingClientRect();
  mini.dataset.corner = 'top';
  const topRect = mini.getBoundingClientRect();
  const atBottom = overlapping(bottomRect, view, mini);
  const atTop = atBottom > 0 ? overlapping(topRect, view, mini) : atBottom;
  const corner = atBottom > 0 && atTop < atBottom ? 'top' : 'bottom';
  if (corner === 'top') mini.dataset.corner = 'top';
  else delete mini.dataset.corner;
}

function useMiniCorner(anchor) {
  useEffect(() => {
    const mini = anchor.current?.closest('.mini-player');
    if (!mini) return undefined;
    let frame = 0;
    const schedule = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        placeMini(mini);
      });
    };
    schedule();
    document.addEventListener('scroll', schedule, true);
    window.addEventListener('resize', schedule);
    const watcher = new MutationObserver(schedule);
    watcher.observe(document.getElementById('view') ?? document.body, { childList: true, subtree: true });
    return () => {
      if (frame) cancelAnimationFrame(frame);
      document.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
      watcher.disconnect();
      delete mini.dataset.corner;
      mini.style.removeProperty('--mini-top');
    };
  }, []);
}

/** Over the small screens: back to Channel (the whole picture) and mute. */
export function MiniControls() {
  const anchor = useRef(null);
  useMiniCorner(anchor);
  const muted = miniMuted.value;
  const muteLabel = muted ? t('mini.unmute') : t('mini.mute');
  return html`
    <button type="button" class="mini-back" ref=${anchor} aria-label=${t('mini.back')} title=${t('mini.back')}
      onClick=${() => (location.hash = '#/channel')}></button>
    <button type="button" class="mini-mute icon-button" aria-label=${muteLabel} title=${muteLabel}
      aria-pressed=${muted ? 'true' : 'false'} onClick=${() => (miniMuted.value = !muted)}>
      <${Icon} name=${muted ? 'muted' : 'volume'} size=${16} />
    </button>`;
}
