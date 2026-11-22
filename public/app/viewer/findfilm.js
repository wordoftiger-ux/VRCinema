// "Find a film" on the request page: search the owner's catalog by title, for a series pick a
// season and an episode. Tells the form which title is chosen (`{type, id}`, an episode by its
// own id like `tt2:1:2`) or `null`. Light on purpose: names only, no pictures.

import { useEffect, useRef, useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';

const BASE = '/api/channels/main/catalog';
const TIMEOUT = { timeoutMs: 20000 };

/** The seasons of a series (specials last) with their episodes in order. */
function seasonsOf(videos) {
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
const label = (r) => (r.year ? `${r.name} (${r.year})` : r.name);

export function FindFilm({ onPick, busy }) {
  const [q, setQ] = useState('');
  const [state, setState] = useState({ phase: 'idle' }); // idle | searching | results | error
  const [open, setOpen] = useState(null); // { title, meta | null, error }
  const [season, setSeason] = useState(null);
  const [episodeId, setEpisodeId] = useState(null);
  const [movie, setMovie] = useState(null);
  const run = useRef(0);

  useEffect(() => {
    const text = q.trim();
    const mine = ++run.current;
    if ([...text].length < 2) {
      setState({ phase: 'idle' });
      return undefined;
    }
    setState({ phase: 'searching' });
    const timer = setTimeout(() => {
      api('GET', `${BASE}/search?q=${encodeURIComponent(text)}`, undefined, TIMEOUT).then(
        (items) => mine === run.current && setState({ phase: 'results', items, text }),
        (err) => mine === run.current && setState({ phase: 'error', err }),
      );
    }, 350);
    return () => clearTimeout(timer);
  }, [q]);

  const choose = (r) => {
    setEpisodeId(null);
    setSeason(null);
    if (r.type === 'movie') {
      setOpen(null);
      setMovie(r);
      onPick({ type: 'movie', id: r.id });
      return;
    }
    setMovie(null);
    onPick(null);
    setOpen({ title: r, meta: null, error: null });
    api('GET', `${BASE}/meta/series/${encodeURIComponent(r.id)}`, undefined, TIMEOUT).then(
      (meta) => setOpen((o) => (o?.title === r ? { title: r, meta, error: null } : o)),
      (error) => setOpen((o) => (o?.title === r ? { title: r, meta: null, error } : o)),
    );
  };
  const back = () => {
    setOpen(null);
    setMovie(null);
    setEpisodeId(null);
    onPick(null);
  };

  if (open) {
    const { title, meta, error } = open;
    const { seasons, map } = meta ? seasonsOf(meta.videos) : { seasons: [], map: new Map() };
    const current = season ?? (seasons.find((s) => s > 0) ?? seasons[0] ?? null);
    const episodes = map.get(current) ?? [];
    return html`
      <div class="v-film" data-step="episodes">
        <p class="v-film-head"><b>${label(title)}</b>
          <button type="button" class="btn" data-action="film-back" disabled=${busy} onClick=${back}>${t('search.back')}</button></p>
        ${error ? html`<p class="v-film-note" role="alert">${errorText(error)}</p>` : null}
        ${!meta && !error ? html`<p class="v-film-note">${t('search.loading')}</p>` : null}
        ${meta && !seasons.length ? html`<p class="v-film-note">${t('search.no_episodes')}</p>` : null}
        ${seasons.length
          ? html`<label class="v-field">
              <span>${t('search.season_label')}</span>
              <select class="sel" value=${String(current)} onChange=${(e) => { setSeason(Number(e.currentTarget.value)); setEpisodeId(null); onPick(null); }}>
                ${seasons.map((n) => html`<option value=${String(n)}>${seasonName(n)}</option>`)}
              </select>
            </label>
            <ul class="v-film-list">
              ${episodes.map((v) => html`<li><button type="button" class=${episodeId === v.id ? 'v-film-row on' : 'v-film-row'}
                data-episode=${v.id} aria-pressed=${episodeId === v.id ? 'true' : 'false'} disabled=${busy}
                onClick=${() => { setEpisodeId(v.id); onPick({ type: 'series', id: v.id }); }}>
                <span class="v-film-no">${v.episode}</span>
                <span>${v.title || t('search.episode', { n: v.episode })}</span></button></li>`)}
            </ul>`
          : null}
      </div>`;
  }

  return html`
    <div class="v-film" data-step="search">
      <label class="v-field">
        <span>${t('search.label')}</span>
        <input type="search" class="txt" name="title" autocomplete="off" spellcheck="false"
          placeholder=${t('search.placeholder')} value=${q} readOnly=${busy}
          onInput=${(e) => { setQ(e.currentTarget.value); if (movie) back(); }} />
        <small>${t('search.hint')}</small>
      </label>
      ${movie ? html`<p class="v-film-chosen" data-chosen=${movie.id}><b>${label(movie)}</b></p>` : null}
      ${!movie && state.phase === 'searching' ? html`<p class="v-film-note">${t('search.searching')}</p>` : null}
      ${state.phase === 'error' ? html`<p class="v-film-note" role="alert">${errorText(state.err)}</p>` : null}
      ${!movie && state.phase === 'results'
        ? state.items.length
          ? html`<ul class="v-film-list">${state.items.map((r) => html`<li><button type="button" class="v-film-row"
              data-type=${r.type} data-id=${r.id} onClick=${() => choose(r)}>
              <span>${label(r)}</span><small>${t(r.type === 'series' ? 'search.type_series' : 'search.type_movie')}</small></button></li>`)}</ul>`
          : html`<p class="v-film-note">${t('search.empty', { query: state.text })}</p>`
        : null}
    </div>`;
}
