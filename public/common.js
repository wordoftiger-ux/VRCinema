// Wspólne funkcje strony zgłoszeń (index.html).

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
