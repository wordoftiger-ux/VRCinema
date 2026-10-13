// Settings → Channel → Info ticker: the band burned into the stream (title, who added it, what
// is next, the owner's messages), its QR code and progress line. The numbers and limits are the
// engine's (spec section 7).

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api } from '../../api.js';
import { channel, environment, queue, requestLink } from '../../store.js';
import { Block, Note, Row, Segmented, Switch, channelSettings, saveChannelWith, useNote } from './common.js';

export const MAX_MESSAGES = 10;
export const MAX_MESSAGE_CHARS = 200;
const INTERVALS_S = [60, 120, 300, 600];
const SPEED_PX_PER_S = { slow: 120, normal: 180, fast: 270 };
const FRAME_PX = 1920;
const FONT_PX = 36;

/** Width of a character in ems (the engine's estimate for the usual fonts). */
function charEm(ch) {
  const c = ch.codePointAt(0);
  const wide =
    (c >= 0x1100 && c <= 0x11ff) || (c >= 0x2e80 && c <= 0xa4cf) || (c >= 0xac00 && c <= 0xd7a3) ||
    (c >= 0xf900 && c <= 0xfaff) || (c >= 0xfe30 && c <= 0xfe4f) || (c >= 0xff00 && c <= 0xff60) ||
    (c >= 0xffe0 && c <= 0xffe6) || (c >= 0x1f000 && c <= 0x1faff) || (c >= 0x20000 && c <= 0x3fffd);
  if (wide) return 1.1;
  if (ch === ' ') return 0.3;
  if (ch !== ch.toLowerCase() && ch === ch.toUpperCase()) return 0.75;
  if (/[0-9\p{L}]/u.test(ch)) return 0.6;
  if (/[!-/:-@[-`{-~·]/.test(ch)) return 0.45;
  return 0.8;
}

export const textWidthPx = (text) => [...text].reduce((sum, ch) => sum + charEm(ch), 0) * FONT_PX;

/** How long a text scrolls across a 1080p frame at `speed`, in seconds. */
export function visibleSeconds(text, speed) {
  return (FRAME_PX + textWidthPx(text)) / (SPEED_PX_PER_S[speed] ?? SPEED_PX_PER_S.normal);
}

/** The text of the band now (in the program's language: the estimate needs no more). */
function currentText(settings) {
  const now = channel.value?.current;
  const parts = [t('ticker.now', { title: now?.title ?? t('settings.ticker.sample_title') })];
  if (now?.requesterName) parts.push(t('ticker.added_by', { nick: now.requesterName }));
  else if (now?.requesterBadge === 'host') parts.push(t('ticker.added_by_host'));
  const next = queue.value[0];
  if (settings.tickerShowNext && next) parts.push(t('ticker.next', { title: next.title }));
  if (settings.tickerShowAddress && requestLink.value) parts.push(t('ticker.address', { address: requestLink.value }));
  const messages = settings.tickerMessages ?? [];
  if (settings.tickerMode === 'continuous') parts.push(...messages);
  else if (messages.length) parts.push(messages[0]);
  return parts.join(' · ');
}

/** The ticker messages: a list with a field to add; every change is saved at once. */
function Messages({ settings, say }) {
  const messages = settings.tickerMessages ?? [];
  const [text, setText] = useState('');
  const [editing, setEditing] = useState(null);
  const [draft, setDraft] = useState('');
  const [error, setError] = useState(null);

  const save = async (list, okText) => {
    for (const m of list) {
      if ([...m].length > MAX_MESSAGE_CHARS) return setError(t('settings.ticker.too_long', { max: MAX_MESSAGE_CHARS }));
    }
    if (list.length > MAX_MESSAGES) return setError(t('settings.ticker.too_many', { max: MAX_MESSAGES }));
    setError(null);
    return saveChannelWith(say, { tickerMessages: list }, okText);
  };
  const add = async (e) => {
    e.preventDefault();
    const value = text.trim();
    if (!value) return;
    if (await save([...messages, value], t('settings.ticker.messages_saved'))) setText('');
  };
  const finishEdit = async (index) => {
    const value = draft.trim();
    const list = value ? messages.map((m, i) => (i === index ? value : m)) : messages.filter((_, i) => i !== index);
    if (await save(list, t('settings.ticker.messages_saved'))) setEditing(null);
  };
  const length = [...text].length;
  const draftLength = [...draft].length;

  return html`
    <div class="messages" data-messages>
      <b id="ticker-messages">${t('settings.ticker.messages')}</b>
      <small>${t('settings.ticker.messages_hint', { max: MAX_MESSAGES })}</small>
      <ul class="msgs" aria-labelledby="ticker-messages">
        ${messages.map((m, i) => html`<li class="msg">
          ${editing === i
            ? html`<form class="inline wide" onSubmit=${(e) => { e.preventDefault(); finishEdit(i); }}>
                <input type="text" class="txt" aria-label=${t('settings.ticker.message_edit')} value=${draft}
                  data-over=${draftLength > MAX_MESSAGE_CHARS ? 'true' : undefined} onInput=${(e) => setDraft(e.target.value)} />
                <span class=${draftLength > MAX_MESSAGE_CHARS ? 'count over' : 'count'}>${draftLength}/${MAX_MESSAGE_CHARS}</span>
                <button type="submit" class="btn pri">${t('settings.save')}</button>
                <button type="button" class="btn" onClick=${() => { setEditing(null); setError(null); }}>${t('settings.cancel')}</button>
              </form>`
            : html`<code>${m}</code>
              <span class="msg-actions">
                <button type="button" class="btn" onClick=${() => { setEditing(i); setDraft(m); setError(null); }}>${t('settings.ticker.message_edit')}</button>
                <button type="button" class="btn" aria-label=${t('settings.ticker.message_remove_label', { text: m })}
                  onClick=${() => save(messages.filter((_, j) => j !== i), t('settings.ticker.messages_saved'))}>${t('settings.ticker.message_remove')}</button>
              </span>`}
        </li>`)}
      </ul>
      <form class="inline wide" onSubmit=${add}>
        <input type="text" class="txt" aria-label=${t('settings.ticker.message_new')} placeholder=${t('settings.ticker.message_new')}
          value=${text} onInput=${(e) => setText(e.target.value)} />
        <span class=${length > MAX_MESSAGE_CHARS ? 'count over' : 'count'}>${length}/${MAX_MESSAGE_CHARS}</span>
        <button type="submit" class="btn">${t('settings.ticker.message_add')}</button>
      </form>
      ${error ? html`<p class="set-note err" role="alert">${error}</p>` : null}
    </div>`;
}

const ENCODER_KEYS = {
  libx264: 'settings.ticker.encoder_cpu',
  h264_nvenc: 'settings.ticker.encoder_nvidia',
  h264_qsv: 'settings.ticker.encoder_intel',
  h264_amf: 'settings.ticker.encoder_amd',
};

/** Which converter the channel uses now and what that means for the ticker. */
function EncoderInfo() {
  const encoder = environment.value?.encoder;
  if (!encoder || !ENCODER_KEYS[encoder]) return null;
  const gpu = encoder !== 'libx264';
  return html`<p class=${gpu ? 'encoder-info gpu' : 'encoder-info cpu'} data-encoder=${encoder}>
    ${t(ENCODER_KEYS[encoder])} ${gpu ? t('settings.ticker.encoder_gpu_note') : t('settings.ticker.encoder_cpu_note')}</p>`;
}

export function Ticker() {
  const settings = channelSettings.value;
  const [note, say] = useNote();

  // The encoder and whether the computer keeps up: asked while this block is on the screen.
  useEffect(() => {
    let live = true;
    const read = async () => {
      try {
        const env = await api('GET', '/api/admin/environment');
        if (live) environment.value = env;
      } catch {
        // the next read tries again
      }
    };
    read();
    const id = setInterval(read, 3000);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, []);

  if (!settings) return null;
  const enabled = Boolean(settings.tickerEnabled);
  const save = (patch, text) => saveChannelWith(say, patch, text);
  const seconds = Math.round(visibleSeconds(currentText(settings), settings.tickerSpeed));

  return html`
    <${Block} title=${t('settings.ticker.title')} hint=${t('settings.ticker.hint')}>
      <div class="ticker-block" data-ticker=${enabled ? 'on' : 'off'}>
        <p class="set-sub" data-restart-note>${t('settings.ticker.restart_note')}</p>
        ${environment.value?.tickerSlow && enabled
          ? html`<p class="banner warn" role="alert" data-banner="ticker_slow">${t('settings.ticker.slow')}</p>`
          : null}
        <${Row} title=${t('settings.ticker.enabled')} hint=${t('settings.ticker.enabled_hint')}>
          <${Switch} label=${t('settings.ticker.enabled')} checked=${enabled}
            onChange=${(v) => save({ tickerEnabled: v }, v ? t('settings.ticker.enabled_on') : t('settings.ticker.enabled_off_restart'))} />
        <//>
        <${Row} title=${t('settings.ticker.mode')} hint=${t('settings.ticker.mode_hint')}>
          <${Segmented} label=${t('settings.ticker.mode')} value=${settings.tickerMode} disabled=${!enabled}
            options=${[
              { value: 'interval', label: t('settings.ticker.mode_interval') },
              { value: 'continuous', label: t('settings.ticker.mode_continuous') },
            ]}
            onChange=${(v) => save({ tickerMode: v }, t('settings.ticker.saved_restart'))} />
        <//>
        <${Row} title=${t('settings.ticker.interval')} hint=${t('settings.ticker.interval_hint')}>
          <${Segmented} label=${t('settings.ticker.interval')} value=${settings.tickerIntervalS}
            disabled=${!enabled || settings.tickerMode !== 'interval'}
            options=${INTERVALS_S.map((s) => ({ value: s, label: t('units.minutes', { m: s / 60 }) }))}
            onChange=${(v) => save({ tickerIntervalS: v }, t('settings.ticker.saved_restart'))} />
        <//>
        <${Row} title=${t('settings.ticker.speed')} hint=${t('settings.ticker.speed_hint', { seconds })}>
          <${Segmented} label=${t('settings.ticker.speed')} value=${settings.tickerSpeed} disabled=${!enabled}
            options=${[
              { value: 'slow', label: t('settings.ticker.speed_slow') },
              { value: 'normal', label: t('settings.ticker.speed_normal') },
              { value: 'fast', label: t('settings.ticker.speed_fast') },
            ]}
            onChange=${(v) => save({ tickerSpeed: v }, t('settings.ticker.saved_restart'))} />
        <//>
        <${Row} title=${t('settings.ticker.show_next')}>
          <${Switch} label=${t('settings.ticker.show_next')} checked=${Boolean(settings.tickerShowNext)} disabled=${!enabled}
            onChange=${(v) => save({ tickerShowNext: v }, t('settings.ticker.saved'))} />
        <//>
        <${Row} title=${t('settings.ticker.show_address')} hint=${t('settings.ticker.show_address_hint')}>
          <${Switch} label=${t('settings.ticker.show_address')} checked=${Boolean(settings.tickerShowAddress)} disabled=${!enabled}
            onChange=${(v) => save({ tickerShowAddress: v }, t('settings.ticker.saved'))} />
        <//>
        <${Row} title=${t('settings.ticker.qr')} hint=${t('settings.ticker.qr_hint')}>
          <${Segmented} label=${t('settings.ticker.qr')} value=${settings.tickerQr} disabled=${!enabled}
            options=${[
              { value: 'off', label: t('settings.ticker.qr_off') },
              { value: 'with_ticker', label: t('settings.ticker.qr_with_ticker') },
              { value: 'always', label: t('settings.ticker.qr_always') },
            ]}
            onChange=${(v) => save({ tickerQr: v }, t('settings.ticker.saved_restart'))} />
        <//>
        <${Row} title=${t('settings.ticker.progress')} hint=${t('settings.ticker.progress_hint')}>
          <${Switch} label=${t('settings.ticker.progress')} checked=${Boolean(settings.tickerProgress)} disabled=${!enabled}
            onChange=${(v) => save({ tickerProgress: v }, t('settings.ticker.saved_restart'))} />
        <//>
        <${Messages} settings=${settings} say=${say} />
        <${EncoderInfo} />
        <${Note} note=${note} />
      </div>
    <//>`;
}
