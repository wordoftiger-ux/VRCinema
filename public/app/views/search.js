// The Search view: find a film or series in the catalogs of the addons (`#/search`), open its page
// (`#/search/<type>/<id>`, an episode of a series as `tt2:1:2`), pick an episode and a source from
// the addons and add it to the queue or put it next. With `?for=<item>` the page chooses the source
// of a viewer's title request instead (the Approvals list links there).

import { signal } from '/static/vendor/signals.mjs';
import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { Icon } from '../icons.js';
import { addonStreams, errorText, searchCatalog, streamAddons, titleMeta } from '../api.js';
import { addLink, addLinkNext, channel, chooseSource, showToast } from '../store.js';
import { route } from '../components/rail.js';
import { Poster } from '../components/poster.js';

/** Opens the page of a film or series (`type` is `movie` or `series`; an episode id has `:season:episode`). */
export function openTitle(type, id) {
  location.hash = `#/search/${type}/${encodeURIComponent(id)}`;
}

// ---------- The search page ----------

const DEBOUNCE_MS = 300;
const MIN_LETTERS = 2;

// Kept while the view is closed: coming back from a title page shows the same results.
const query = signal('');
const found = signal({ q: '', state: 'idle', items: [], error: null });

const typeLabel = (type) => (type === 'movie' || type === 'series' ? t(`search.type_${type}`) : type);

function Card({ item }) {
  return html`
    <li class="sr-item">
      <a class="sr-card" href=${`#/search/${item.type}/${encodeURIComponent(item.id)}`} data-type=${item.type} data-id=${item.id}>
        <${Poster} src=${item.poster} className="sr-poster" />
        <span class="sr-type">${typeLabel(item.type)}</span>
        <b class="sr-name">${item.name}</b>
        ${item.year ? html`<small class="sr-year">${item.year}</small>` : null}
      </a>
    </li>`;
}

function SearchPage() {
  const q = query.value;
  const run = useRef(0);
  const field = useRef(null);
  useEffect(() => field.current?.focus(), []);

  useEffect(() => {
    const text = q.trim();
    if (found.value.q === text && found.value.state !== 'error') return undefined;
    if ([...text].length < MIN_LETTERS) {
      found.value = { q: text, state: 'idle', items: [], error: null };
      return undefined;
    }
    const mine = (run.current += 1);
    const timer = setTimeout(async () => {
      found.value = { q: text, state: 'loading', items: found.value.items, error: null };
      try {
        const items = await searchCatalog(text);
        if (mine === run.current) found.value = { q: text, state: 'done', items, error: null };
      } catch (e) {
        if (mine === run.current) found.value = { q: text, state: 'error', items: [], error: errorText(e) };
      }
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [q]);

  const { state, items, error } = found.value;
  const short = [...q.trim()].length < MIN_LETTERS;
  return html`
    <div class="page search-page">
      <h1>${t('search.title')}</h1>
      <form class="sr-form" role="search" onSubmit=${(e) => e.preventDefault()}>
        <${Icon} name="search" size=${16} />
        <input type="search" class="sr-input" ref=${field} value=${q} autocomplete="off" spellcheck="false"
          aria-label=${t('search.label')} placeholder=${t('search.placeholder')}
          onInput=${(e) => (query.value = e.currentTarget.value)} />
      </form>
      ${short
        ? html`<p class="q-empty sr-hint">${t('search.hint')}</p>`
        : state === 'error'
          ? html`<p class="set-note err" role="alert">${error}</p>`
          : state === 'loading' && !items.length
            ? html`<p class="sr-busy" role="status"><span class="q-spinner inline" aria-hidden="true"></span>${t('search.searching')}</p>`
            : items.length
              ? html`<ul class="sr-grid">${items.map((item) => html`<${Card} key=${`${item.type}/${item.id}`} item=${item} />`)}</ul>`
              : state === 'done'
                ? html`<p class="q-empty sr-empty">${t('search.empty', { query: found.value.q })}</p>`
                : null}
    </div>`;
}

// ---------- The sources ----------

function Source({ stream, forItem }) {
  const [busy, setBusy] = useState(false);
  const run = async (job) => {
    if (busy) return;
    setBusy(true);
    try {
      await job();
    } catch (e) {
      showToast(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const add = () => run(async () => showToast(t('queue.added_one', { title: (await addLink(stream.url)).title }), 'info'));
  const next = () => run(async () => showToast(t('history.play_next_done', { title: (await addLinkNext(stream.url)).title }), 'info'));
  const use = () =>
    run(async () => {
      await chooseSource(forItem, stream.url);
      showToast(t('search.source_chosen'), 'info');
      location.hash = '#/channel';
    });
  return html`
    <li class="src-row">
      <div class="src-text">
        ${stream.name ? html`<b class="src-name">${stream.name}</b>` : null}
        ${stream.title ? html`<span class="src-title">${stream.title}</span>` : null}
      </div>
      <span class="src-actions">
        ${forItem
          ? html`<button type="button" class="btn pri src-use" disabled=${busy} onClick=${use}>${t('search.use_source')}</button>`
          : html`
              <button type="button" class="btn src-add" disabled=${busy} onClick=${add}>${t('search.add_to_queue')}</button>
              <button type="button" class="btn src-next" disabled=${busy} onClick=${next}>${t('search.play_next')}</button>`}
      </span>
    </li>`;
}

/** What every addon with sources offers for `type`/`id`: each addon is asked on its own and shows when it answers. */
function Sources({ type, id, forItem }) {
  const [addons, setAddons] = useState(null);
  const [error, setError] = useState(null);

  useEffect(() => {
    let dead = false;
    setAddons(null);
    setError(null);
    const update = (addonId, patch) =>
      !dead && setAddons((list) => list?.map((a) => (a.id === addonId ? { ...a, ...patch } : a)) ?? list);
    (async () => {
      try {
        const { addons: list } = await streamAddons(type, id);
        if (dead) return;
        setAddons(list.map((a) => ({ ...a, state: 'loading', streams: [] })));
        for (const a of list) {
          addonStreams(type, id, a.id).then(
            (r) => update(a.id, { state: 'ok', streams: r.streams ?? [] }),
            () => update(a.id, { state: 'error' }),
          );
        }
      } catch (e) {
        if (!dead) setError(errorText(e));
      }
    })();
    return () => {
      dead = true;
    };
  }, [type, id]);

  return html`
    <section class="sources" aria-labelledby="sources-title">
      <h3 class="sec" id="sources-title">${t('search.sources')}</h3>
      ${error ? html`<p class="set-note err" role="alert">${error}</p>` : null}
      ${!error && !addons ? html`<p class="sr-busy" role="status"><span class="q-spinner inline" aria-hidden="true"></span>${t('search.loading')}</p>` : null}
      ${addons && !addons.length ? html`<p class="q-empty">${t('search.no_stream_addons')}</p>` : null}
      ${(addons ?? []).map(
        (a) => html`
          <div class="src-addon" data-addon=${a.id} data-state=${a.state}>
            <h4>${a.name}</h4>
            ${a.state === 'loading'
              ? html`<p class="src-wait" role="status"><span class="q-spinner inline" aria-hidden="true"></span>${t('search.addon_loading', { name: a.name })}</p>`
              : a.state === 'error'
                ? html`<p class="src-fail">${t('addons.no_answer', { name: a.name })}</p>`
                : a.streams.length
                  ? html`<ul class="src-list">${a.streams.map((s, i) => html`<${Source} key=${`${i}-${s.url}`} stream=${s} forItem=${forItem} />`)}</ul>`
                  : html`<p class="src-fail">${t('search.addon_empty', { name: a.name })}</p>`}
          </div>`,
      )}
    </section>`;
}

// ---------- The title page ----------

/** The seasons of a series (`0` is specials, last) with their episodes in order. */
export function seasonsOf(videos) {
  const map = new Map();
  for (const v of videos ?? []) {
    if (!Number.isInteger(v.season) || !Number.isInteger(v.episode)) continue;
    if (!map.has(v.season)) map.set(v.season, []);
    map.get(v.season).push(v);
  }
  for (const list of map.values()) list.sort((a, b) => a.episode - b.episode);
  const seasons = [...map.keys()].sort((a, b) => (a === 0) - (b === 0) || a - b);
  return { seasons, map };
}

const seasonName = (n) => (n === 0 ? t('search.specials') : t('search.season', { n }));

function releasedText(released) {
  const d = new Date(released);
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

function TitlePage({ type, id, forItem }) {
  const baseId = id.split(':')[0];
  const [meta, setMeta] = useState(undefined);
  const [error, setError] = useState(null);
  const [episodeId, setEpisodeId] = useState(type === 'series' && id.includes(':') ? id : null);
  const [seasonPick, setSeasonPick] = useState(null);
  const request = channel.value?.pending?.find((i) => i.id === forItem);

  useEffect(() => {
    let dead = false;
    titleMeta(type, baseId).then(
      (m) => !dead && setMeta(m),
      (e) => {
        if (dead) return;
        setError(errorText(e));
        setMeta(null);
      },
    );
    return () => {
      dead = true;
    };
  }, [type, baseId]);

  const back = forItem ? '#/channel' : '#/search';
  const backLink = html`<a class="btn tp-back" href=${back}><${Icon} name="up" size=${14} />${t('search.back')}</a>`;
  const banner = forItem
    ? html`<p class="banner warn tp-for" role="status">${request ? t('search.choosing_for', { title: request.title }) : t('search.choosing_for_gone')}</p>`
    : null;
  if (meta === undefined) {
    return html`<div class="page title-page">${backLink}${banner}<p class="sr-busy" role="status"><span class="q-spinner inline" aria-hidden="true"></span>${t('search.loading')}</p></div>`;
  }
  if (!meta) {
    return html`<div class="page title-page">${backLink}${banner}<p class="set-note err" role="alert">${error}</p></div>`;
  }

  const { seasons, map } = type === 'series' ? seasonsOf(meta.videos) : { seasons: [], map: new Map() };
  const picked = episodeId ? Number(episodeId.split(':')[1]) : null;
  const season = seasonPick ?? (picked !== null && map.has(picked) ? picked : (seasons.find((s) => s > 0) ?? seasons[0] ?? null));
  const episodes = map.get(season) ?? [];
  const target = type === 'movie' ? id : episodeId;
  const facts = [meta.year, meta.runtime].filter(Boolean).join(' · ');
  return html`
    <div class="page title-page" data-type=${type}>
      ${backLink}${banner}
      <div class="tp-head">
        <${Poster} src=${meta.poster} className="tp-poster" />
        <div class="tp-info">
          <h1 class="tp-name">${meta.name}</h1>
          <p class="tp-facts"><span class="sr-type">${typeLabel(type)}</span>${facts ? ` ${facts}` : ''}</p>
          ${meta.description ? html`<p class="tp-desc">${meta.description}</p>` : null}
        </div>
      </div>
      ${type === 'series'
        ? html`
            <section class="episodes" aria-labelledby="episodes-title">
              <div class="ep-head">
                <h3 class="sec" id="episodes-title">${t('search.episodes')}</h3>
                ${seasons.length
                  ? html`<select class="sel ep-season" aria-label=${t('search.season_label')} value=${String(season)}
                      onChange=${(e) => setSeasonPick(Number(e.currentTarget.value))}>
                      ${seasons.map((n) => html`<option value=${String(n)}>${seasonName(n)}</option>`)}
                    </select>`
                  : null}
              </div>
              ${episodes.length
                ? html`<ul class="ep-list">
                    ${episodes.map((v) => {
                      const on = episodeId === v.id;
                      const when = v.released ? releasedText(v.released) : '';
                      return html`<li>
                        <button type="button" class=${on ? 'ep-row on' : 'ep-row'} data-episode=${v.id}
                          aria-pressed=${on ? 'true' : 'false'} onClick=${() => setEpisodeId(v.id)}>
                          <span class="ep-no">${v.episode}</span>
                          <span class="ep-title">${v.title || t('search.episode', { n: v.episode })}</span>
                          ${when ? html`<small class="ep-date">${when}</small>` : null}
                        </button>
                      </li>`;
                    })}
                  </ul>`
                : html`<p class="q-empty">${t('search.no_episodes')}</p>`}
            </section>`
        : null}
      ${target
        ? html`<${Sources} key=${target} type=${type} id=${target} forItem=${forItem} />`
        : html`<p class="q-empty tp-pick">${t('search.pick_episode')}</p>`}
    </div>`;
}

export function SearchView() {
  const { type, id, forItem } = route.value;
  return html`
    <main class="view search" id="view" data-view="search">
      ${id ? html`<${TitlePage} key=${`${type}/${id}/${forItem}`} type=${type} id=${id} forItem=${forItem} />` : html`<${SearchPage} />`}
    </main>`;
}
