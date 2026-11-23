// The audio track and the subtitles of a film (spec 2026-11-22-viewer-and-tracks §4): a dialog
// with two selects, "Audio" and "Subtitles", opened from a queue row's menu and from the button
// in the player bar for the film on air. "Auto" is first and says what it picks; the choice is
// sent as soon as a select changes. The film on air restarts from its position (a toast says so).

import { html } from '../html.js';
import { lang, t } from '../i18n.js';
import { Icon } from '../icons.js';
import { openDialog } from './dialog.js';
import { channel, queue, setTracks, showToast } from '../store.js';

/** 3-letter codes of the files (both the bibliographic and the terminology ones) to 2-letter ones. */
const TWO = {
  eng: 'en', pol: 'pl', ger: 'de', deu: 'de', fre: 'fr', fra: 'fr', spa: 'es', ita: 'it', rus: 'ru', jpn: 'ja',
  chi: 'zh', zho: 'zh', por: 'pt', cze: 'cs', ces: 'cs', ukr: 'uk', kor: 'ko', dut: 'nl', nld: 'nl', swe: 'sv',
  tur: 'tr', ara: 'ar', hin: 'hi', hun: 'hu', gre: 'el', ell: 'el', heb: 'he', nor: 'no', dan: 'da', fin: 'fi',
  rum: 'ro', ron: 'ro', bul: 'bg', slo: 'sk', slk: 'sk', hrv: 'hr', srp: 'sr', lit: 'lt', lav: 'lv', est: 'et',
  slv: 'sl', tha: 'th', vie: 'vi', ind: 'id',
};

const AUDIO_CODECS = { ac3: 'AC3', eac3: 'E-AC3', dts: 'DTS', aac: 'AAC', truehd: 'TrueHD', mp3: 'MP3', opus: 'Opus', flac: 'FLAC', vorbis: 'Vorbis' };
const SUB_CODECS = { hdmv_pgs_subtitle: 'PGS', dvd_subtitle: 'VobSub', dvb_subtitle: 'DVB', subrip: 'SRT', srt: 'SRT', ass: 'ASS', ssa: 'SSA', mov_text: 'Text', webvtt: 'WebVTT' };
const LAYOUTS = { 1: '1.0', 2: '2.0', 6: '5.1', 8: '7.1' };

/** `Polish` from `pol`, in the interface language; the code itself when it has no name. */
export function languageName(code) {
  const raw = String(code ?? '').trim().toLowerCase();
  if (!raw || raw === 'und') return '';
  const two = TWO[raw] ?? raw;
  try {
    const name = new Intl.DisplayNames([lang.value], { type: 'language' }).of(two);
    return name && name.toLowerCase() !== two ? name : raw;
  } catch {
    return raw;
  }
}

/** `Polish · AC3 5.1` / `English (forced) · PGS` / `Track 2 · AAC`: how a track is listed. */
export function trackLabel(track, kind) {
  const name = languageName(track.lang) || t('tracks.track_n', { n: track.index + 1 });
  const forced = kind === 'subtitles' && track.forced ? ` (${t('tracks.forced')})` : '';
  const codec = (kind === 'audio' ? AUDIO_CODECS : SUB_CODECS)[track.codec] ?? String(track.codec ?? '').toUpperCase();
  const layout = kind === 'audio' ? LAYOUTS[track.channels] ?? (track.channels ? `${track.channels} ch` : '') : '';
  return `${name}${forced} · ${[codec, layout].filter(Boolean).join(' ')}`;
}

/** The first option of a select: `Auto (Polish · AC3 5.1)`. */
function autoLabel(list, index, kind) {
  const track = index == null ? null : list.find((x) => x.index === index);
  return track ? t('tracks.auto', { what: trackLabel(track, kind) }) : t('tracks.auto_none');
}

/** The item with this id in the panel (on air, waiting, or awaiting approval). */
export function panelItem(id) {
  const ch = channel.value;
  return [...(ch?.playing ?? []), ...queue.value, ...(ch?.pending ?? [])].find((i) => i.id === id) ?? null;
}

/** The film on air with its tracks known, else null. */
export const onAirTracksItem = () => {
  const item = channel.value?.playing?.[0];
  return item?.tracks && !item.isLive ? item : null;
};

async function choose(item, patch) {
  const answer = await setTracks(item.id, patch);
  if (!answer) return;
  showToast(answer.restart ? t('tracks.restarts') : t('tracks.saved'), 'info');
}

function Select({ name, label, value, auto, options, onChange }) {
  return html`
    <label class="dialog-field">
      <span>${label}</span>
      <select class="sel" name=${name} value=${value} onChange=${(e) => onChange(e.currentTarget.value)}>
        <option value="">${auto}</option>
        ${options}
      </select>
    </label>`;
}

function TracksBody({ id }) {
  const item = panelItem(id);
  const tr = item?.tracks;
  if (!tr) return html`<p class="dialog-note" data-tracks-unread>${t('tracks.unread')}</p>`;
  const audioValue = tr.chosenAudio == null ? '' : String(tr.chosenAudio);
  const subValue = tr.chosenSubtitle == null ? '' : String(tr.chosenSubtitle);
  return html`
    <div class="dialog-fields" data-dialog="tracks">
      <${Select} name="audio" label=${t('tracks.audio')} value=${audioValue} auto=${autoLabel(tr.audio, tr.autoAudio, 'audio')}
        options=${tr.audio.map((a) => html`<option value=${String(a.index)}>${trackLabel(a, 'audio')}</option>`)}
        onChange=${(v) => choose(item, { audio: v === '' ? null : Number(v) })} />
      <${Select} name="subtitle" label=${t('tracks.subtitles')} value=${subValue} auto=${autoLabel(tr.subtitles, tr.autoSubtitle, 'subtitles')}
        options=${[
          html`<option value="-1">${t('tracks.off')}</option>`,
          ...tr.subtitles.map((s) => html`<option value=${String(s.index)}>${trackLabel(s, 'subtitles')}</option>`),
        ]}
        onChange=${(v) => choose(item, { subtitle: v === '' ? null : Number(v) })} />
    </div>
    <p class="dialog-note">${t('tracks.hint')}</p>`;
}

/** "Audio and subtitles…": the choice for one film. */
export function openTracks(item) {
  openDialog({
    title: () => t('tracks.title'),
    body: () => html`<${TracksBody} id=${item.id} />`,
    buttons: [{ key: 'ui.close' }],
  });
}

/** The player bar's button for the film on air; nothing until its tracks are known. */
export function TracksButton() {
  const item = onAirTracksItem();
  if (!item) return null;
  return html`
    <button type="button" class="ctl-button" data-tracks-button aria-label=${t('tracks.title')} title=${t('tracks.title')}
      onClick=${() => openTracks(item)}><${Icon} name="tracks" size=${20} /></button>`;
}
