// The link field above the queue (Enter adds), and Ctrl+V anywhere in the window (not in a
// text field) that adds the link on the clipboard.

import { signal } from '/static/vendor/signals.mjs';
import { useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t, tn } from '../i18n.js';
import { Icon } from '../icons.js';
import { errorText } from '../api.js';
import { addLink, showToast } from '../store.js';

/** How many links are being read now (a link may take a while: the program asks the site). */
const adding = signal(0);

/** Adds `url`; resolves to `{message}` on success or `{error}` (an `ApiError`). */
async function add(url) {
  adding.value += 1;
  try {
    const { added, title } = await addLink(url);
    return { message: added === 1 ? t('queue.added_one', { title }) : tn('queue.added_count', added) };
  } catch (error) {
    return { error };
  } finally {
    adding.value -= 1;
  }
}

/** Ctrl+V: the text on the clipboard is a link to add (anything else is said not to be). */
export async function pasteLink(text) {
  const url = String(text ?? '').trim().split(/\s+/)[0] ?? '';
  if (!/^https?:\/\/\S+$/i.test(url)) {
    showToast(t('queue.paste_not_link'));
    return;
  }
  const { message, error } = await add(url);
  if (error) showToast(errorText(error));
  else showToast(message, 'info');
}

export function AddLink() {
  const [value, setValue] = useState('');
  const [error, setError] = useState(null);
  const busy = adding.value > 0;

  const submit = async (e) => {
    e.preventDefault();
    const url = value.trim();
    if (!url || busy) return;
    setError(null);
    const result = await add(url);
    if (result.error) setError(result.error);
    else {
      setValue('');
      showToast(result.message, 'info');
    }
  };

  return html`
    <form class="addf" onSubmit=${submit} novalidate>
      <div class=${`addf-box ${error ? 'bad' : ''}`}>
        <${Icon} name="plus" size=${16} />
        <input type="text" class="addf-input" value=${value} inputmode="url" autocomplete="off" spellcheck="false"
          aria-label=${t('queue.add_label')} placeholder=${t('queue.add_placeholder')}
          aria-invalid=${error ? 'true' : 'false'} aria-describedby=${error ? 'addf-error' : undefined}
          onInput=${(e) => {
            setValue(e.currentTarget.value);
            setError(null);
          }} />
        <button type="submit" class="btn addf-button" disabled=${busy || !value.trim()}>${busy ? t('queue.adding') : t('queue.add_button')}</button>
      </div>
      ${error ? html`<p id="addf-error" class="addf-error" role="alert">${errorText(error)}</p>` : null}
    </form>`;
}
