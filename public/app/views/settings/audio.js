// Settings → Channel → Audio files: the picture an audio-only file or link (MP3, FLAC,
// SoundCloud, ...) airs with — the cover art, a waveform, spectrum bars or the plain background
// with the title and artist (spec 2026-11-08-audio-design.md).

import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { Block, Row, saveChannelWith } from './common.js';

const LOOKS = ['auto', 'cover', 'waveform', 'spectrum', 'off'];

export function AudioLook({ settings, say }) {
  const label = t('settings.channel.audio_visual');
  return html`
    <${Block} title=${t('settings.channel.audio')} hint=${t('settings.channel.audio_hint')}>
      <${Row} title=${label} hint=${t('settings.channel.audio_visual_hint')}>
        <select class="sel" aria-label=${label} value=${settings.audioVisual ?? 'auto'}
          onChange=${(e) => saveChannelWith(say, { audioVisual: e.target.value }, t('settings.channel.audio_visual_saved'))}>
          ${LOOKS.map((v) => html`<option value=${v}>${t(`settings.channel.audio_visual_${v}`)}</option>`)}
        </select>
      <//>
    <//>`;
}
