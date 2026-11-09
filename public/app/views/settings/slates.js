// Settings → Channel → Slates: the last frame of the video behind the waiting slates (dimmed,
// blurred or off) and the owner's own background (an image, GIF or short video) behind the idle
// slate and the countdown, with a preview of the slate over it.
//
// In the program's window the file is chosen with the program's own (native) picker, through
// the engine (`/api/desktop/slate-background/pick`), not with the web view's file chooser: the
// owner saw the whole window go blank behind that chooser once. Elsewhere (a remote panel, a
// browser) the page's file field uploads the file. Either way the block reads the background
// again afterwards, so the new preview shows.

import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, apiUpload, errorText } from '../../api.js';
import { access } from '../../store.js';
import { Block, Row, Switch, saveChannelWith } from './common.js';

export const SLATE_BG_ROUTE = '/api/admin/channels/main/slate-background';
export const SLATE_BG_PICK = '/api/desktop/slate-background/pick';

export const FRAME_STYLES = [
  { value: 'dim', label: 'settings.channel.slate_frame_dim' },
  { value: 'blur', label: 'settings.channel.slate_frame_blur' },
  { value: 'off', label: 'settings.channel.slate_frame_off' },
];

const ACCEPT = 'image/png,image/jpeg,image/webp,image/bmp,image/gif,video/mp4,video/webm,video/quicktime,video/x-matroska,.mkv,.mov';

export function SlateLook({ settings, say }) {
  const [info, setInfo] = useState(null);
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);
  const [shown, setShown] = useState(0);
  // Answers that arrive after the block is gone change nothing.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    api('GET', SLATE_BG_ROUTE).then((i) => alive.current && setInfo(i), () => alive.current && setInfo(null));
    return () => {
      alive.current = false;
    };
  }, []);
  const native = access.value === 'window';
  const bg = info?.background ?? null;
  const maxMb = Math.floor((info?.maxBytes ?? 31457280) / 1048576);
  const maxSeconds = info?.maxSeconds ?? 30;

  /** A new background is stored (`answer`): read again and shown. */
  const stored = async (answer) => {
    const fresh = await api('GET', SLATE_BG_ROUTE).catch(() => answer);
    if (!alive.current) return;
    setInfo(fresh ?? answer);
    setShown(Date.now());
    say('ok', t('settings.channel.slate_bg_saved'));
  };
  const upload = async (file) => {
    if (!file || busy) return;
    if (info && file.size > info.maxBytes) return say('err', t('errors.slate_background_too_big', { max: maxMb }));
    setBusy(true);
    say('info', t('settings.channel.slate_bg_uploading'));
    try {
      await stored(await apiUpload('PUT', SLATE_BG_ROUTE, file));
    } catch (e) {
      if (alive.current) say('err', errorText(e));
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  /** The window: the program's own picker; the engine stores the file chosen. */
  const pick = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const answer = await api('POST', SLATE_BG_PICK, undefined, { timeoutMs: 600000 });
      if (answer?.picked === false) return;
      if (alive.current) say('info', t('settings.channel.slate_bg_uploading'));
      await stored(answer);
    } catch (e) {
      if (alive.current) say('err', errorText(e));
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  const remove = async () => {
    setBusy(true);
    try {
      const answer = await api('DELETE', SLATE_BG_ROUTE);
      if (!alive.current) return;
      setInfo(answer);
      say('ok', t('settings.channel.slate_bg_removed'));
    } catch (e) {
      if (alive.current) say('err', errorText(e));
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  const drop = (e) => {
    e.preventDefault();
    setOver(false);
    upload(e.dataTransfer?.files?.[0]);
  };
  const preview = bg ? `${SLATE_BG_ROUTE}/preview?v=${bg.stamp}-${shown}-${settings.slateLanguage}` : null;
  const allLabel = t('settings.channel.slate_bg_all');
  return html`
    <${Block} title=${t('settings.channel.slates')} hint=${t('settings.channel.slates_hint')}>
      <${Row} title=${t('settings.channel.slate_frame')} hint=${t('settings.channel.slate_frame_hint')}>
        <select class="sel" aria-label=${t('settings.channel.slate_frame')} value=${settings.slateFrameStyle ?? 'dim'}
          onChange=${(e) => saveChannelWith(say, { slateFrameStyle: e.target.value }, t('settings.channel.slate_frame_saved'))}>
          ${FRAME_STYLES.map((s) => html`<option value=${s.value}>${t(s.label)}</option>`)}
        </select>
      <//>
      <${Row} title=${t('settings.channel.slate_bg')} hint=${t('settings.channel.slate_bg_hint')} block>
        <div class=${over ? 'slate-bg over' : 'slate-bg'} data-busy=${busy ? 'true' : 'false'}
          onDragOver=${(e) => { e.preventDefault(); setOver(true); }}
          onDragLeave=${() => setOver(false)} onDrop=${drop}>
          ${preview
            ? html`<img class="slate-bg-preview" src=${preview} alt=${t('settings.channel.slate_bg_preview')} />`
            : html`<div class="slate-bg-empty">${t('settings.channel.slate_bg_default')}</div>`}
          <div class="slate-bg-actions">
            ${native
              ? html`<button type="button" class="btn pri" data-action="slate-bg-pick" disabled=${busy} onClick=${pick}>
                  ${t('settings.channel.slate_bg_pick')}</button>`
              : html`<label class=${busy ? 'btn disabled' : 'btn pri'}>
                  ${t('settings.channel.slate_bg_pick')}
                  <input type="file" class="sr-only" accept=${ACCEPT} disabled=${busy}
                    onChange=${(e) => { upload(e.target.files?.[0]); e.target.value = ''; }} />
                </label>`}
            <span class="slate-bg-drop">${t('settings.channel.slate_bg_drop')}</span>
            ${bg ? html`<button type="button" class="btn" data-action="slate-bg-remove" disabled=${busy} onClick=${remove}>
              ${t('settings.channel.slate_bg_remove')}</button>` : null}
          </div>
          <small class="set-sub">${t('settings.channel.slate_bg_limits', { mb: maxMb, seconds: maxSeconds })}</small>
        </div>
      <//>
      <${Row} title=${allLabel} hint=${t('settings.channel.slate_bg_all_hint')}>
        <${Switch} label=${allLabel} checked=${Boolean(settings.slateBackgroundAll)}
          onChange=${(v) => saveChannelWith(say, { slateBackgroundAll: v }, t(v ? 'settings.channel.slate_bg_all_on' : 'settings.channel.slate_bg_all_off'))} />
      <//>
    <//>`;
}
