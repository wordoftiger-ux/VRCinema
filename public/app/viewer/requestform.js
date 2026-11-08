// "Add a video": the link, the nick (with this device's icon) and what the channel accepts.
// With the owner's "lists" setting on, the link field takes several links (one per line) and
// playlists; the answer lists what was added and why the rest was left out. With the owner's
// "video files" setting on, a small video or audio file can be sent instead of a link.

import { useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { lang, sentenceGap, sentences, t, tn } from '../i18n.js';
import { ApiError, api, errorText } from '../api.js';
import { RequesterBadge } from '../components/badge.js';
import { deviceKey, existingDeviceKey, saveBadge, saveNick, savedBadge, savedNick } from './device.js';
import { formatLimit } from './format.js';
import { NickPin, openAdopt } from './nickpin.js';
import { channel, refresh, showToast } from './store.js';

const NICK_MAX = 24;
export const UPLOAD_ROUTE = '/api/channels/main/upload';
export const UPLOAD_ACCEPT = 'video/*,audio/*,.mkv,.mov,.mp4,.webm,.avi,.flv,.ts,.mpg,.mpeg,.ogv,.wmv,'
  + '.mp3,.flac,.ogg,.oga,.opus,.m4a,.aac,.wav,.mka,.wma';

/**
 * Sends `file` as the raw body with the viewer's nick and key in headers; `onProgress` gets
 * the share sent (0–1). Resolves to the JSON answer; errors as `ApiError` like `api`.
 */
export function sendFile(file, { name, deviceKey }, onProgress) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', UPLOAD_ROUTE);
    xhr.setRequestHeader('X-VRCinema-Lang', lang.value);
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    xhr.setRequestHeader('X-Upload-Name', encodeURIComponent(file.name || ''));
    if (name) xhr.setRequestHeader('X-Viewer-Name', encodeURIComponent(name));
    if (deviceKey) xhr.setRequestHeader('X-Device-Key', encodeURIComponent(deviceKey));
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable && e.total > 0) onProgress?.(e.loaded / e.total);
    };
    xhr.onload = () => {
      let data = null;
      try {
        data = JSON.parse(xhr.responseText);
      } catch {
        data = null;
      }
      if (xhr.status >= 200 && xhr.status < 300) return resolve(data);
      reject(new ApiError(data?.error || t('ui.error_status', { status: xhr.status }), { status: xhr.status, code: data?.code ?? null, data }));
    };
    xhr.onerror = () => reject(new ApiError(t('ui.no_answer'), { code: 'network' }));
    xhr.ontimeout = xhr.onerror;
    xhr.send(file);
  });
}

/** The file box: a picker and a drop target, or the chosen file with a button to drop it. */
function FileBox({ uploads, file, onFile, busy }) {
  const [over, setOver] = useState(false);
  const drop = (e) => {
    e.preventDefault();
    setOver(false);
    if (!busy) onFile(e.dataTransfer?.files?.[0] ?? null);
  };
  const mb = (bytes) => (bytes / 1048576).toFixed(1);
  return html`
    <div class=${over ? 'v-file over' : 'v-file'} data-file=${file ? 'chosen' : 'none'}
      onDragOver=${(e) => { e.preventDefault(); setOver(true); }} onDragLeave=${() => setOver(false)} onDrop=${drop}>
      <span class="v-file-label">${t('viewer.upload_label')}</span>
      ${file
        ? html`<span class="v-file-chosen"><b title=${file.name}>${file.name}</b><small>${mb(file.size)} MB</small>
            <button type="button" class="btn" data-action="file-remove" disabled=${busy}
              aria-label=${t('viewer.upload_remove')} title=${t('viewer.upload_remove')} onClick=${() => onFile(null)}>×</button></span>`
        : html`<span class="v-file-pick">
            <label class=${busy ? 'btn disabled' : 'btn'}>${t('viewer.upload_pick')}
              <input type="file" class="sr-only" name="file" accept=${UPLOAD_ACCEPT} disabled=${busy}
                onChange=${(e) => { onFile(e.target.files?.[0] ?? null); e.target.value = ''; }} /></label>
            <span>${t('viewer.upload_drop')}</span></span>`}
      <small>${uploadLimits(uploads)}</small>
    </div>`;
}

/** The file limits: one length for both kinds, or the video's and the audio's. */
function uploadLimits({ maxMb, maxDurationS, maxAudioS }) {
  const limit = formatLimit(maxDurationS);
  const audio = formatLimit(maxAudioS ?? maxDurationS);
  const limits = audio === limit
    ? t('viewer.upload_limits', { mb: maxMb, limit })
    : t('viewer.upload_limits_audio', { mb: maxMb, limit, audio });
  return sentences(limits, t('viewer.upload_audio_note'));
}

/** The text of a failed request, in the page's language. */
export function requestErrorText(e) {
  // Not sent (no connection, no answer in time): the page does not try again by itself.
  if (e.code === 'network') return t('viewer.send_failed');
  if (e.code === 'too_many_active' && Number.isFinite(e.data?.count)) {
    return tn('errors.too_many_active', e.data.count);
  }
  return errorText(e);
}

/** The non-empty lines of the link field. */
export function linkLines(text) {
  return String(text).split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
}

/** What a list request did: the counts, then each left-out link or entry with its reason. */
function ListResult({ result }) {
  const left = result.results.filter((r) => r.status === 'skipped');
  const key = result.status === 'pending_approval' ? 'viewer.list_pending' : 'viewer.list_added';
  return html`
    <div class="v-list-result" role="status" data-added=${result.added} data-skipped=${result.skipped}>
      <p>
        ${result.added ? html`<span class="ok">${tn(key, result.added)}</span>` : null}
        ${left.length ? html`<span>${tn('viewer.list_skipped', left.length)}</span>` : null}
      </p>
      ${left.length
        ? html`<ul>${left.map((r, i) => html`<li key=${i} data-code=${r.code ?? undefined}>
            <b>${r.title || t('viewer.list_line', { line: r.line })}</b>${sentenceGap()}<span>${r.reason}</span></li>`)}</ul>`
        : null}
    </div>`;
}

/** The nick with two digits at the end (kept within the length limit): a free one is likely. */
function alternative(nick) {
  const digits = String(10 + Math.floor(Math.random() * 90));
  return [...nick].slice(0, NICK_MAX - digits.length).join('') + digits;
}

function Rules({ rules }) {
  if (!rules) return null;
  const allowlist = rules.linkMode === 'allowlist';
  const sites = rules.allowedDomains ?? [];
  return html`
    <div class="v-rules">
      ${allowlist
        ? html`<div><b>${t('viewer.sites_title')}</b>${sites.length
            ? html`<ul class="v-sites">${sites.map((s) => html`<li key=${s}>${s}</li>`)}</ul>`
            : html`${sentenceGap()}${t('viewer.sites_none')}`}</div>`
        : html`<p>${t('viewer.hint_any')}</p>`}
      <p>${sentences(t('viewer.limit', { limit: formatLimit(rules.maxDurationS) }), rules.moderation && t('viewer.moderation'))}</p>
    </div>`;
}

export function RequestForm() {
  const [url, setUrl] = useState('');
  const [nick, setNick] = useState(savedNick());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  const [suggestion, setSuggestion] = useState(null);
  const [badge, setBadge] = useState(savedBadge());
  const [result, setResult] = useState(null);
  const [file, setFile] = useState(null);
  const [progress, setProgress] = useState(null);
  const nickField = useRef(null);
  const rules = channel.value?.rules;
  const lists = Boolean(rules?.viewerLists);
  const uploads = rules?.uploads ?? null;
  const lines = lists ? linkLines(url) : [];
  // A chosen file goes instead of the link (the box goes away when the owner turns it off).
  const sendingFile = Boolean(uploads && file);
  const chooseFile = (f) => {
    setError(null);
    setFile(f);
  };

  /** Takes the suggested nick; the button goes away with the error, so the focus goes to the nick. */
  const takeSuggestion = () => {
    setNick(suggestion);
    setError(null);
    setSuggestion(null);
    nickField.current?.focus();
  };

  /** The nick this device holds after a PIN was set or used: kept with its icon. */
  const holdNick = (name, b) => {
    setNick(name);
    saveNick(name);
    setError(null);
    setSuggestion(null);
    if (b && typeof b === 'object') {
      saveBadge(b);
      setBadge(b);
    }
  };

  const submit = async (e) => {
    e.preventDefault();
    const name = nick.trim();
    if ((!url.trim() && !sendingFile) || busy) return;
    if (sendingFile && file.size > uploads.maxMb * 1048576) {
      setError(new ApiError(t('errors.upload_too_big', { max: uploads.maxMb }), { code: 'upload_too_big', data: { max: uploads.maxMb } }));
      return;
    }
    setBusy(true);
    setError(null);
    setSuggestion(null);
    setResult(null);
    saveNick(name);
    // The key is made when a nick is first used; once it exists it goes with every request.
    const key = name ? deviceKey() : existingDeviceKey();
    const body = lists ? { urls: lines, name } : { url: url.trim(), name };
    if (key) body.deviceKey = key;
    const keepBadge = (b) => {
      if (name && b && typeof b === 'object') {
        saveBadge(b);
        setBadge(b);
      }
    };
    try {
      if (sendingFile) {
        setProgress(0);
        const r = await sendFile(file, { name, deviceKey: key }, setProgress);
        const title = r.item?.title;
        showToast(r.status === 'pending_approval' ? t('viewer.pending', { title }) : t('viewer.added', { title }));
        setFile(null);
        keepBadge(r.item?.requesterBadge);
        refresh();
        return;
      }
      if (lists) {
        // Every link is read by the channel; a long list takes a while.
        const r = await api('POST', '/api/channels/main/request-list', body, { timeoutMs: 180000 });
        setResult(r);
        if (r.added) {
          setUrl('');
          keepBadge(r.requesterBadge);
        }
        refresh();
        return;
      }
      const r = await api('POST', '/api/channels/main/requests', body, { timeoutMs: 60000 });
      const title = r.item?.title;
      showToast(r.status === 'pending_approval' ? t('viewer.pending', { title }) : t('viewer.added', { title }));
      setUrl('');
      keepBadge(r.item?.requesterBadge);
      refresh();
    } catch (err) {
      // Kept as the error itself: its text follows a language switch made after it.
      setError(err);
      if (err.code === 'nick_taken') setSuggestion(alternative(name));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };
  const busyText = () => {
    if (sendingFile) {
      return progress !== null && progress < 1
        ? t('viewer.uploading', { percent: Math.floor(progress * 100) })
        : t('viewer.upload_checking');
    }
    return lists ? tn('viewer.checking_links', lines.length) : t('viewer.adding');
  };

  return html`
    <section class="v-add card2" aria-labelledby="v-add-h">
      <h2 id="v-add-h" class="v-eyebrow">${t('viewer.add_heading')}</h2>
      <form onSubmit=${submit} novalidate>
        ${lists
          ? html`<label class="v-field">
              <span>${t('viewer.links_label')}</span>
              <textarea class="txt v-links" name="urls" rows="4" autocomplete="off" spellcheck="false"
                placeholder=${t('viewer.links_placeholder')} value=${url} readOnly=${busy}
                aria-describedby=${error ? 'v-links-hint v-error' : 'v-links-hint'} aria-invalid=${error ? 'true' : undefined}
                onInput=${(e) => setUrl(e.currentTarget.value)}></textarea>
              <small id="v-links-hint">${tn('viewer.links_hint', rules.viewerListMax ?? 10)}</small>
            </label>`
          : html`<label class="v-field">
          <span>${t('viewer.link_label')}</span>
          <input type="url" class="txt" name="url" required=${!sendingFile} autocomplete="off" inputmode="url"
            placeholder=${t('viewer.link_placeholder')} value=${url} readOnly=${busy}
            aria-describedby=${error ? 'v-error' : undefined} aria-invalid=${error ? 'true' : undefined}
            onInput=${(e) => setUrl(e.currentTarget.value)} />
        </label>`}
        ${uploads ? html`<${FileBox} uploads=${uploads} file=${file} onFile=${chooseFile} busy=${busy} />` : null}
        <label class="v-field">
          <span>${t('viewer.nick_label')}</span>
          <span class="v-nick">
            ${badge && nick.trim()
              ? html`<span class="v-own-badge" role="img" aria-label=${t('viewer.your_icon')} title=${t('viewer.your_icon')}><${RequesterBadge} badge=${badge} /></span>`
              : null}
            <input type="text" class="txt" name="nick" maxlength=${NICK_MAX} autocomplete="nickname" ref=${nickField}
              value=${nick} readOnly=${busy} aria-describedby="v-nick-hint"
              onInput=${(e) => setNick(e.currentTarget.value)} />
          </span>
          <small id="v-nick-hint">${t('viewer.nick_hint')}</small>
        </label>
        ${error
          ? html`<div id="v-error" class="v-error" role="alert" data-code=${error.code ?? undefined}>
              <p>${requestErrorText(error)}</p>
              ${suggestion
                ? html`<button type="button" class="btn" onClick=${takeSuggestion}>
                    ${t('viewer.use_nick', { nick: suggestion })}</button>
                  <button type="button" class="btn" data-action="its-mine" onClick=${openAdopt}>
                    ${t('viewer.pin_its_mine')}</button>`
                : null}
            </div>`
          : null}
        <button type="submit" class="btn pri v-submit" disabled=${busy || (!url.trim() && !sendingFile)}>
          ${busy ? busyText() : sendingFile ? t('viewer.upload_button') : t('viewer.add_button')}</button>
      </form>
      <${NickPin} nick=${nick} onNick=${holdNick} />
      ${result ? html`<${ListResult} result=${result} />` : null}
      <${Rules} rules=${rules} />
    </section>`;
}
