// "Add a video": the link, the nick (with this device's icon) and what the channel accepts.

import { useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { sentenceGap, sentences, t, tn } from '../i18n.js';
import { api, errorText } from '../api.js';
import { RequesterBadge } from '../components/badge.js';
import { deviceKey, existingDeviceKey, saveBadge, saveNick, savedBadge, savedNick } from './device.js';
import { formatLimit } from './format.js';
import { channel, refresh, showToast } from './store.js';

const NICK_MAX = 24;

/** The text of a failed request, in the page's language. */
export function requestErrorText(e) {
  // Not sent (no connection, no answer in time): the page does not try again by itself.
  if (e.code === 'network') return t('viewer.send_failed');
  if (e.code === 'too_many_active' && Number.isFinite(e.data?.count)) {
    return tn('errors.too_many_active', e.data.count);
  }
  return errorText(e);
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
  const nickField = useRef(null);

  /** Takes the suggested nick; the button goes away with the error, so the focus goes to the nick. */
  const takeSuggestion = () => {
    setNick(suggestion);
    setError(null);
    setSuggestion(null);
    nickField.current?.focus();
  };

  const submit = async (e) => {
    e.preventDefault();
    const name = nick.trim();
    if (!url.trim() || busy) return;
    setBusy(true);
    setError(null);
    setSuggestion(null);
    saveNick(name);
    // The key is made when a nick is first used; once it exists it goes with every request.
    const key = name ? deviceKey() : existingDeviceKey();
    const body = { url: url.trim(), name };
    if (key) body.deviceKey = key;
    try {
      const r = await api('POST', '/api/channels/main/requests', body, { timeoutMs: 60000 });
      const title = r.item?.title;
      showToast(r.status === 'pending_approval' ? t('viewer.pending', { title }) : t('viewer.added', { title }));
      setUrl('');
      if (name && r.item?.requesterBadge && typeof r.item.requesterBadge === 'object') {
        saveBadge(r.item.requesterBadge);
        setBadge(r.item.requesterBadge);
      }
      refresh();
    } catch (err) {
      // Kept as the error itself: its text follows a language switch made after it.
      setError(err);
      if (err.code === 'nick_taken') setSuggestion(alternative(name));
    } finally {
      setBusy(false);
    }
  };

  return html`
    <section class="v-add card2" aria-labelledby="v-add-h">
      <h2 id="v-add-h" class="v-eyebrow">${t('viewer.add_heading')}</h2>
      <form onSubmit=${submit} novalidate>
        <label class="v-field">
          <span>${t('viewer.link_label')}</span>
          <input type="url" class="txt" name="url" required autocomplete="off" inputmode="url"
            placeholder=${t('viewer.link_placeholder')} value=${url} readOnly=${busy}
            aria-describedby=${error ? 'v-error' : undefined} aria-invalid=${error ? 'true' : undefined}
            onInput=${(e) => setUrl(e.currentTarget.value)} />
        </label>
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
                    ${t('viewer.use_nick', { nick: suggestion })}</button>`
                : null}
            </div>`
          : null}
        <button type="submit" class="btn pri v-submit" disabled=${busy || !url.trim()}>
          ${busy ? t('viewer.adding') : t('viewer.add_button')}</button>
      </form>
      <${Rules} rules=${channel.value?.rules} />
    </section>`;
}
