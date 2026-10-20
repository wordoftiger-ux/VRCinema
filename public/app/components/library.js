// Files on disk (the folders of MEDIA_DIRS) under the link field, for a server without a window
// only (the window adds files by dropping them): a root folder from the list, the folders that
// open, one folder up, the videos with their size and "Add to queue", or the whole folder.

import { useEffect, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { lang, t, tn } from '../i18n.js';
import { Icon } from '../icons.js';
import { api, errorText } from '../api.js';
import { environment, refreshPanel, showToast } from '../store.js';

const LIBRARY = '/api/admin/library';
const ADD = '/api/admin/channels/main/local';

/** `bytes` in GB with two decimals, in the interface language's number format. */
function gigabytes(bytes) {
  const size = new Intl.NumberFormat(lang.value, { minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(
    (bytes ?? 0) / 1024 ** 3,
  );
  return t('library.size', { size });
}

export function Library() {
  const headless = environment.value?.window === false;
  const [listing, setListing] = useState(null);
  const [busy, setBusy] = useState(false);

  const load = async (root, path) => {
    try {
      const d = await api('GET', `${LIBRARY}?root=${root}&path=${encodeURIComponent(path)}`);
      setListing(d.enabled ? d : null);
    } catch (e) {
      showToast(errorText(e));
    }
  };

  useEffect(() => {
    if (headless) load(0, '');
    else setListing(null);
  }, [headless]);

  if (!headless || !listing) return null;

  const add = async (path) => {
    if (busy) return;
    setBusy(true);
    try {
      const { added, title } = await api('POST', ADD, { root: listing.root, path }, { timeoutMs: 180000 });
      showToast(added === 1 ? t('queue.added_one', { title }) : tn('queue.added_count', added), 'info');
    } catch (e) {
      showToast(errorText(e));
    } finally {
      setBusy(false);
      refreshPanel();
    }
  };

  const hasFiles = listing.entries.some((e) => e.type === 'file');
  return html`
    <section class="lib" aria-labelledby="lib-title">
      <h3 class="sec" id="lib-title">${t('library.title')}</h3>
      <div class="lib-bar">
        <select class="lib-root" aria-label=${t('library.folder')} value=${String(listing.root)}
          onChange=${(e) => load(Number(e.currentTarget.value), '')}>
          ${listing.roots.map((r) => html`<option value=${String(r.index)}>${r.name}</option>`)}
        </select>
        <button type="button" class="btn lib-add-folder" disabled=${busy || !hasFiles} onClick=${() => add(listing.path)}>
          ${t('library.add_folder')}</button>
      </div>
      <p class="lib-where">${listing.path ? t('library.you_are_in', { path: listing.path }) : t('library.root')}</p>
      ${listing.parent !== null && listing.parent !== undefined
        ? html`<button type="button" class="lib-up" onClick=${() => load(listing.root, listing.parent)}>
            <${Icon} name="up" size=${14} />${t('library.up')}</button>`
        : null}
      ${listing.entries.length
        ? html`<ul class="lib-list">
            ${listing.entries.map((e) =>
              e.type === 'dir'
                ? html`<li><button type="button" class="lib-dir" onClick=${() => load(listing.root, e.path)}>
                    <${Icon} name="folder" size=${16} />${e.name}</button></li>`
                : html`<li class="lib-file">
                    <span class="lib-name" title=${e.name}>${e.name}</span>
                    <span class="lib-size">${gigabytes(e.sizeBytes)}</span>
                    <button type="button" class="btn lib-add" disabled=${busy} onClick=${() => add(e.path)}>${t('library.add')}</button>
                  </li>`,
            )}
          </ul>`
        : html`<p class="q-empty lib-empty">${t('library.empty')}</p>`}
    </section>`;
}
