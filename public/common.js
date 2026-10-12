// Wspólne funkcje stron kanału.

export function fmtTime(s) {
  if (s === null || s === undefined || !Number.isFinite(s)) return '';
  s = Math.max(0, Math.round(s));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
}

export function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') node.className = v;
    else if (k.startsWith('on')) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

// Język tych stron: komunikaty błędów serwera mają być w tym samym języku.
const PAGE_LANG = 'pl';

export async function api(method, url, body) {
  const headers = { 'x-vrcinema-lang': PAGE_LANG };
  if (body) headers['content-type'] = 'application/json';
  const res = await fetch(url, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
    credentials: 'same-origin',
  });
  let data = null;
  try {
    data = await res.json();
  } catch {
    data = null;
  }
  if (!res.ok) {
    const err = new Error(data?.error || `Błąd ${res.status}`);
    err.status = res.status;
    err.data = data; // reszta odpowiedzi błędu (np. `code`, `helpUrl` kreatora stałego linku)
    throw err;
  }
  return data;
}

const STATUS_TEXT = {
  playing: 'Na antenie',
  buffering: 'Ładowanie filmu',
  starting: 'Ładowanie filmu',
  idle: 'Przerwa',
  paused: 'Wstrzymane',
};

/** Wypełnia monitor programu: lampka, tytuł, wskaźnik postępu. */
export function renderMonitor(root, state) {
  const tally = root.querySelector('[data-tally]');
  tally.dataset.status = state.status;
  tally.textContent = STATUS_TEXT[state.status] ?? state.status;

  const title = root.querySelector('[data-title]');
  const by = root.querySelector('[data-by]');
  const meter = root.querySelector('[data-meter]');
  const pos = root.querySelector('[data-pos]');
  const dur = root.querySelector('[data-dur]');
  const c = state.current;

  if (!meter.children.length) for (let i = 0; i < 40; i++) meter.append(document.createElement('span'));

  if (!c) {
    title.textContent = 'Teraz nic nie leci';
    title.classList.add('idle');
    by.textContent = 'Kanał pokazuje planszę oczekiwania, dopóki ktoś nie doda filmu.';
    [...meter.children].forEach((s) => s.classList.remove('on'));
    pos.textContent = '';
    dur.textContent = '';
    return;
  }
  title.textContent = c.title;
  title.classList.remove('idle');
  by.textContent = c.requesterName ? `Dodane przez ${c.requesterName}` : '';
  if (c.isLive) {
    [...meter.children].forEach((s) => s.classList.add('on'));
    pos.textContent = 'Transmisja na żywo';
    dur.textContent = '';
    return;
  }
  const ratio = c.durationS ? Math.min(1, c.positionS / c.durationS) : 0;
  const lit = Math.round(ratio * 40);
  [...meter.children].forEach((s, i) => s.classList.toggle('on', i < lit));
  pos.textContent = fmtTime(c.positionS);
  dur.textContent = fmtTime(c.durationS);
}

export function setupCopy(input, button) {
  button.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(input.value);
    } catch {
      input.select();
      document.execCommand('copy');
    }
    const old = button.textContent;
    button.textContent = 'Skopiowano';
    setTimeout(() => (button.textContent = old), 1500);
  });
}

export function notice(node, text, kind) {
  node.textContent = text;
  node.className = `notice ${kind ?? ''}`;
}

// ---------- Okno aplikacji (VRCinema.exe) ----------

/** Okno aplikacji dostaje przy logowaniu znacznik; zwykła przeglądarka nie, więc nie pyta o /api/desktop. */
export function isAppWindow() {
  return document.cookie.split('; ').includes('vrcinema_app=1');
}

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Jak api(), ale przy krótkiej przerwie połączenia (serwer uruchamia się ponownie) próbuje jeszcze raz. */
export async function apiRetry(method, url, body, { tries = 12, delay = 700 } = {}) {
  for (let attempt = 1; ; attempt++) {
    try {
      return await api(method, url, body);
    } catch (err) {
      const down = err.status === undefined || err.status === 503;
      if (!down || attempt >= tries) throw err;
      await sleep(delay);
    }
  }
}

const ENCODER_TEXT = {
  h264_nvenc: 'Konwersja na karcie NVIDIA',
  h264_qsv: 'Konwersja na karcie Intel (Quick Sync)',
  h264_amf: 'Konwersja na karcie AMD',
  libx264: 'Konwersja na procesorze',
};

/** Wynik wyboru enkodera zwykłym językiem. */
export function encoderLabel(encoder) {
  return ENCODER_TEXT[encoder] ?? encoder;
}

export const TOOL_TEXT = {
  'yt-dlp': 'yt-dlp (pobieranie z serwisów)',
  ffmpeg: 'ffmpeg (konwersja obrazu)',
  deno: 'Deno (wymagane przez yt-dlp)',
  cloudflared: 'cloudflared (publiczny adres kanału)',
};
export const TOOL_ORDER = ['yt-dlp', 'ffmpeg', 'deno', 'cloudflared'];

export function fmtMB(bytes) {
  return (bytes / 1024 ** 2).toFixed(bytes >= 100 * 1024 ** 2 ? 0 : 1) + ' MB';
}

/** Pasek postępu; `ratio` null oznacza nieznany rozmiar (pasek w ruchu). */
export function progressBar(label) {
  return el('div', { class: 'bar', role: 'progressbar', 'aria-label': label }, el('span'));
}

export function setBar(bar, ratio) {
  bar.classList.toggle('indeterminate', ratio === null);
  if (ratio !== null) {
    const pct = Math.round(Math.min(1, Math.max(0, ratio)) * 100);
    bar.firstElementChild.style.width = pct + '%';
    bar.setAttribute('aria-valuenow', String(pct));
  }
}

const PHASE_TEXT = { verify: 'Sprawdzanie sumy kontrolnej', install: 'Instalowanie', version: 'Sprawdzanie wersji' };

/** Stan jednego narzędzia w trakcie zadania, z odpowiedzi tools/progress (`p`) i listy brakujących. */
export function toolRowState(name, missing, p) {
  if (!missing.includes(name)) return { kind: 'ready', text: 'Zainstalowane' };
  if (p.done && !p.running && !p.error) return { kind: 'ready', text: 'Gotowe' };
  if (p.tool === name && p.error && !p.running) return { kind: 'error', text: p.error };
  if (p.tool === name && p.phase === 'download') {
    const text = p.total ? `Pobieranie ${fmtMB(p.downloaded)} z ${fmtMB(p.total)}` : `Pobieranie ${fmtMB(p.downloaded)}`;
    return { kind: 'active', ratio: p.total ? p.downloaded / p.total : null, text };
  }
  if (p.tool === name) return { kind: 'active', ratio: 1, text: PHASE_TEXT[p.phase] ?? 'Przygotowanie' };
  const at = TOOL_ORDER.indexOf(p.tool);
  if (at >= 0 && TOOL_ORDER.indexOf(name) < at) return { kind: 'ready', text: 'Gotowe' };
  return { kind: 'waiting', text: 'Czeka' };
}

/** Wiersze z paskami postępu narzędzi; `update(missing, progress)` odświeża je w miejscu. */
export function toolProgressList(names) {
  const rows = new Map();
  const root = el('div', { class: 'tool-list' });
  for (const name of names) {
    const state = el('span', { class: 'state' }, 'Czeka');
    const bar = progressBar(TOOL_TEXT[name] ?? name);
    setBar(bar, 0);
    const row = el('div', { class: 'tool-row', 'data-kind': 'waiting' }, el('div', { class: 'head' }, el('span', {}, TOOL_TEXT[name] ?? name), state), bar);
    rows.set(name, { row, state, bar });
    root.append(row);
  }
  const update = (missing, progress) => {
    for (const [name, r] of rows) {
      const s = toolRowState(name, missing, progress);
      r.row.dataset.kind = s.kind;
      r.state.textContent = s.text;
      setBar(r.bar, s.kind === 'ready' ? 1 : s.kind === 'active' ? s.ratio : 0);
    }
  };
  return { root, update };
}

/** Odpytuje tools/progress co 0,5 s aż zadanie się skończy; zwraca ostatni odczyt (z `error` przy porażce). */
export async function watchToolsJob(onProgress) {
  let bad = 0;
  for (;;) {
    await sleep(500);
    let p;
    try {
      p = await api('GET', '/api/desktop/tools/progress');
      bad = 0;
    } catch (err) {
      // Po instalacji serwer uruchamia się ponownie: krótkie przerwy nie przerywają obserwacji.
      if (++bad > 40) throw err;
      continue;
    }
    onProgress(p);
    if (!p.running && p.done) return p;
  }
}
