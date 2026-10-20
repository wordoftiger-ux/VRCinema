// Translations of the interface: the dictionaries are /static/locales/<lang>.json (the same
// files the engine uses). A missing key falls back to English, then to the key itself.

import { signal, batch } from '/static/vendor/signals.mjs';

export const LANGS = ['en', 'pl', 'es', 'de', 'fr', 'ja'];

/** The language of the interface now; components that call `t` follow it. */
export const lang = signal('en');

// Loaded dictionaries by language; the signal changes with every new one (re-render).
const dicts = signal({});
const loading = new Map();

function fetchDict(code) {
  if (!loading.has(code)) {
    // A failed load is not kept: the next switch tries again.
    const promise = fetch(`/static/locales/${code}.json`, { cache: 'no-cache' })
      .then((res) => {
        if (!res.ok) throw new Error(String(res.status));
        return res.json();
      })
      .catch(() => {
        loading.delete(code);
        return {};
      });
    loading.set(code, promise);
  }
  return loading.get(code);
}

/** A supported language for a tag like `de-AT`; otherwise null. */
export function supported(tag) {
  const code = String(tag ?? '').trim().toLowerCase().slice(0, 2);
  return LANGS.includes(code) ? code : null;
}

/** The first supported language of the browser, else English. */
export function browserLang() {
  for (const tag of navigator.languages ?? [navigator.language]) {
    const code = supported(tag);
    if (code) return code;
  }
  return 'en';
}

// The language picked in this browser for the panel (the sign-in screen, Settings of a remote
// panel). It never changes the program's own language.
const CHOICE_KEY = 'vrcinema.lang';

/** The panel language picked in this browser; null when none was picked (or no storage). */
export function chosenLang() {
  try {
    return supported(localStorage.getItem(CHOICE_KEY));
  } catch {
    return null;
  }
}

/** Remembers `code` as this browser's panel language. */
export function rememberLang(code) {
  try {
    localStorage.setItem(CHOICE_KEY, code);
  } catch {
    // No storage (private mode): the choice lasts until the page is closed.
  }
}

/** Loads the dictionary of `code` (and English for the fallback) and switches to it. */
export async function loadLang(code) {
  const wanted = supported(code) ?? 'en';
  const [en, own] = await Promise.all([fetchDict('en'), fetchDict(wanted)]);
  batch(() => {
    dicts.value = { ...dicts.value, en, [wanted]: own };
    lang.value = wanted;
  });
  document.documentElement.lang = wanted;
  return wanted;
}

function lookup(key) {
  const all = dicts.value;
  return all[lang.value]?.[key] ?? all.en?.[key];
}

function fill(text, args) {
  if (!args) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name) => (name in args ? String(args[name]) : whole));
}

/** The text of `key` in the current language, with `{name}` fields filled from `args`. */
export function t(key, args) {
  const entry = lookup(key);
  if (entry === undefined) return key;
  return fill(typeof entry === 'string' ? entry : (entry.other ?? key), args);
}

/** The plural text of `key` for the number `n` (CLDR category of the language, `{count}` = n). */
export function tn(key, n, args) {
  const entry = lookup(key);
  if (entry === undefined) return key;
  const all = { ...args, count: n };
  if (typeof entry === 'string') return fill(entry, all);
  const category = new Intl.PluralRules(lang.value).select(n);
  return fill(entry[category] ?? entry.other ?? key, all);
}

/** What goes between two sentences (or a label and its text): a space, none in Japanese. */
export function sentenceGap() {
  return lang.value === 'ja' ? '' : ' ';
}

/** Sentences of the current language one after another (empty parts left out). */
export function sentences(...parts) {
  return parts.filter(Boolean).join(sentenceGap());
}

/** Source titles the engine stores in Polish when a source has none (kept for compatibility). */
const STORED_UNTITLED = 'Bez tytułu';

/** The title of a video as shown: the stored default becomes the localized "Untitled". */
export function videoTitle(title) {
  return !title || title === STORED_UNTITLED ? t('player.untitled') : title;
}
