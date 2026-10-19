// "How to watch in VRChat": the channel's address with a copy button and the three things to know.

import { useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { channel } from './store.js';

async function copy(input) {
  try {
    await navigator.clipboard.writeText(input.value);
  } catch {
    input.select();
    document.execCommand('copy');
  }
}

export function HowTo() {
  const [copied, setCopied] = useState(false);
  const address = channel.value?.channelUrl ?? '';
  if (!address) return null;
  const onCopy = async (e) => {
    await copy(e.currentTarget.parentElement.querySelector('input'));
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return html`
    <details class="v-howto card2">
      <summary>${t('viewer.howto_title')}</summary>
      <p>${t('viewer.howto_paste')}</p>
      <div class="v-linkbox">
        <input type="text" class="txt" readonly value=${address} aria-label=${t('viewer.channel_link')} />
        <button type="button" class="btn" onClick=${onCopy}>${copied ? t('viewer.copied') : t('viewer.copy')}</button>
      </div>
      <ol>
        <li>${t('viewer.howto_untrusted')}</li>
        <li>${t('viewer.howto_same_moment')}</li>
        <li>${t('viewer.howto_no_seek')}</li>
      </ol>
    </details>`;
}
