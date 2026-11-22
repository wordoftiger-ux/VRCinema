// The link field above the queue (Enter adds), and Ctrl+V anywhere in the window (not in a
// text field) that adds the link on the clipboard.

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t, tn } from '../i18n.js';
import { Icon } from '../icons.js';
import { errorText, searchCatalog } from '../api.js';
import { addLink, showToast } from '../store.js';
import { openTitle } from '../views/search.js';
import { Poster } from './poster.js';

const SUGGEST_DEBOUNCE_MS = 300;
const SUGGEST_MAX = 6;

/** Text that may be a title: at least two letters and not a link. */
const isTitleText = (text) => [...text.trim()].length >= 2 && !/^(http|magnet:)/i.test(text.trim());

const typeLabel = (type) => (type === 'movie' || type === 'series' ? t(`search.type_${type}`) : type);

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
  if (!/^(https?:\/\/|magnet:)\S+$/i.test(url)) {
    showToast(t('queue.paste_not_link'));
    return;
  }
  await queueLink(url);
}

/** Adds `url` and says so in a message over the player bar (also when it is refused). */
export async function queueLink(url) {
  const { message, error } = await add(url);
  if (error) showToast(errorText(error));
  else showToast(message, 'info');
}

export function AddLink() {
  const [value, setValue] = useState('');
  const [error, setError] = useState(null);
  // Title suggestions under the field: `found` is null until an answer for the current text arrives.
  const [found, setFound] = useState(null);
  const [active, setActive] = useState(-1);
  const [open, setOpen] = useState(false);
  const seq = useRef(0);
  const busy = adding.value > 0;

  useEffect(() => {
    const mine = ++seq.current; // an answer for older text is ignored
    setFound(null);
    setActive(-1);
    if (!isTitleText(value)) return undefined;
    const timer = setTimeout(() => {
      searchCatalog(value.trim()).then(
        (items) => mine === seq.current && setFound(items.slice(0, SUGGEST_MAX)),
        () => mine === seq.current && setFound(null),
      );
    }, SUGGEST_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [value]);

  const shown = open && found !== null && isTitleText(value);

  const pick = (item) => {
    seq.current += 1;
    setValue('');
    setFound(null);
    setOpen(false);
    openTitle(item.type, item.id);
  };

  const onKeyDown = (e) => {
    if (!shown) return;
    const n = found.length;
    if (e.key === 'Escape') {
      e.preventDefault();
      setOpen(false);
    } else if (n && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      e.preventDefault();
      setActive((a) => (e.key === 'ArrowDown' ? (a + 1) % n : a <= 0 ? n - 1 : a - 1));
    } else if (e.key === 'Enter' && active >= 0 && found[active]) {
      e.preventDefault();
      pick(found[active]);
    }
  };

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
    <form class="addf" onSubmit=${submit} novalidate
      onFocusOut=${(e) => !e.currentTarget.contains(e.relatedTarget) && setOpen(false)}>
      <div class=${`addf-box ${error ? 'bad' : ''}`}>
        <${Icon} name="plus" size=${16} />
        <input type="text" class="addf-input" value=${value} inputmode="url" autocomplete="off" spellcheck="false"
          role="combobox" aria-autocomplete="list" aria-expanded=${shown ? 'true' : 'false'} aria-controls="addf-list"
          aria-activedescendant=${shown && active >= 0 ? `addf-opt-${active}` : undefined}
          aria-label=${t('queue.add_label')} placeholder=${t('queue.add_placeholder')}
          aria-invalid=${error ? 'true' : 'false'} aria-describedby=${error ? 'addf-error' : undefined}
          onKeyDown=${onKeyDown} onFocus=${() => setOpen(true)}
          onInput=${(e) => {
            setValue(e.currentTarget.value);
            setError(null);
            setOpen(true);
          }} />
        <button type="submit" class="btn addf-button" disabled=${busy || !value.trim()}>${busy ? t('queue.adding') : t('queue.add_button')}</button>
      </div>
      ${shown
        ? html`<ul id="addf-list" class="addf-list" role="listbox" aria-label=${t('queue.suggest_label')}>
            ${found.length
              ? found.map((item, i) => html`
                  <li id=${`addf-opt-${i}`} role="option" key=${item.type + item.id} class=${`addf-opt ${i === active ? 'on' : ''}`}
                    aria-selected=${i === active ? 'true' : 'false'} data-type=${item.type} data-id=${item.id}
                    onMouseDown=${(e) => e.preventDefault()} onClick=${() => pick(item)} onMouseMove=${() => i !== active && setActive(i)}>
                    <${Poster} src=${item.poster} className="addf-poster" />
                    <b class="sr-name">${item.name}</b>
                    ${item.year ? html`<small class="sr-year">${item.year}</small>` : null}
                    <span class="sr-type">${typeLabel(item.type)}</span>
                  </li>`)
              : html`<li class="addf-none" role="presentation">${t('queue.suggest_none')}</li>`}
          </ul>`
        : null}
      ${error ? html`<p id="addf-error" class="addf-error" role="alert">${errorText(error)}</p>` : null}
    </form>`;
}
