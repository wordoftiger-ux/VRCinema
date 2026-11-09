// The mini player: away from the Channel tab (History, Share, Settings) the Channel's screens
// stay mounted and keep playing in a small window docked in the bottom right corner (above the
// player bar; above the tab bar on a phone). The players are the same instances (the app shell
// keeps the Channel view mounted, CSS makes it small), so the Preview, On air, their volumes
// and the picture-in-picture layout carry on. A click on it goes back to Channel; its button
// mutes both screens in this window only (the channel's sound does not change).

import { computed, signal } from '/static/vendor/signals.mjs';
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

/** Over the small screens: back to Channel (the whole picture) and mute. */
export function MiniControls() {
  const muted = miniMuted.value;
  const muteLabel = muted ? t('mini.unmute') : t('mini.mute');
  return html`
    <button type="button" class="mini-back" aria-label=${t('mini.back')} title=${t('mini.back')}
      onClick=${() => (location.hash = '#/channel')}></button>
    <button type="button" class="mini-mute icon-button" aria-label=${muteLabel} title=${muteLabel}
      aria-pressed=${muted ? 'true' : 'false'} onClick=${() => (miniMuted.value = !muted)}>
      <${Icon} name=${muted ? 'muted' : 'volume'} size=${16} />
    </button>`;
}
