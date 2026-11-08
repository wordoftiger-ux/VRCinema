// Settings → Channel → Slates: the last frame of the video behind the waiting slates (dimmed,
// blurred or off) and the owner's own background (an image, GIF or short video) behind the idle
// slate and the countdown, with a preview of the slate over it.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../../html.js';
import { t } from '../../i18n.js';
import { api, apiUpload, errorText } from '../../api.js';
import { Block, Row, Switch, saveChannelWith } from './common.js';

export const SLATE_BG_ROUTE = '/api/admin/channels/main/slate-background';

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
  useEffect(() => {
    api('GET', SLATE_BG_ROUTE).then(setInfo, () => setInfo(null));
  }, []);
  const bg = info?.background ?? null;
  const maxMb = Math.floor((info?.maxBytes ?? 31457280) / 1048576);
  const maxSeconds = info?.maxSeconds ?? 30;

  const upload = async (file) => {
    if (!file || busy) return;
    if (info && file.size > info.maxBytes) return say('err', t('errors.slate_background_too_big', { max: maxMb }));
    setBusy(true);
    say('info', t('settings.channel.slate_bg_uploading'));
    try {
      setInfo(await apiUpload('PUT', SLATE_BG_ROUTE, file));
      setShown(Date.now());
      say('ok', t('settings.channel.slate_bg_saved'));
    } catch (e) {
      say('err', errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    setBusy(true);
    try {
      setInfo(await api('DELETE', SLATE_BG_ROUTE));
      say('ok', t('settings.channel.slate_bg_removed'));
    } catch (e) {
      say('err', errorText(e));
    } finally {
      setBusy(false);
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
            <label class=${busy ? 'btn disabled' : 'btn pri'}>
              ${t('settings.channel.slate_bg_pick')}
              <input type="file" class="sr-only" accept=${ACCEPT} disabled=${busy}
                onChange=${(e) => { upload(e.target.files?.[0]); e.target.value = ''; }} />
            </label>
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
