// Test zgodności API: ten sam scenariusz żądań na serwerze TypeScript i Rust, porównanie odpowiedzi.
// Uruchomienie (z korzenia repo, po `cargo build` w engine/): node tools/api-parity/run.mjs
import { spawn, spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..', '..');
const PASSWORD = 'parity-haslo-123';
const exe = process.platform === 'win32' ? '.exe' : '';
const rustBin = path.join(root, 'engine', 'target', 'debug', `vrcinema${exe}`);
const ffmpeg = path.join(root, 'bin', `ffmpeg${exe}`);

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'vrcinema-parity-'));
const children = [];

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// ---------- Przygotowanie plików ----------

function prepareFixtures() {
  const media = path.join(tmp, 'media');
  for (const f of ['movie1.mp4', 'movie2.mkv', 'readme.txt', 'sub/clip3.mp4', 'sub/clip4.mkv']) {
    fs.mkdirSync(path.dirname(path.join(media, f)), { recursive: true });
    fs.writeFileSync(path.join(media, f), '');
  }
  fs.mkdirSync(path.join(media, 'empty'));

  // Prawdziwy, długi film, żeby pierwsza pozycja kolejki grała przez cały scenariusz (deterministyczny stan "playing").
  const clip = path.join(tmp, 'clip.mp4');
  const gen = spawnSync(ffmpeg, [
    '-v', 'error', '-f', 'lavfi', '-i', 'testsrc=size=160x90:rate=5', '-f', 'lavfi', '-i', 'sine=frequency=440',
    '-t', '300', '-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-b:a', '32k', '-y', clip,
  ]);
  if (gen.status !== 0) throw new Error(`Nie udało się wygenerować filmu testowego: ${gen.stderr}`);

  const fakeText = fs
    .readFileSync(path.join(import.meta.dirname, 'fake-meta.json'), 'utf8')
    .replaceAll('@MEDIA@', clip.replaceAll('\\', '/'));
  const fake = path.join(tmp, 'fake-meta.json');
  fs.writeFileSync(fake, fakeText);
  return { media, fake };
}

// ---------- Serwery ----------

function startServer(name, port, command, args, fx, extraEnv = {}) {
  const dataDir = path.join(tmp, `data-${name}`);
  fs.mkdirSync(dataDir, { recursive: true });
  const logFile = path.join(tmp, `${name}.log`);
  const out = fs.openSync(logFile, 'w');
  const child = spawn(command, args, {
    cwd: root,
    stdio: ['ignore', out, out],
    env: {
      ...process.env,
      PORT: String(port),
      DATA_DIR: dataDir,
      MEDIA_DIRS: fx.media,
      VRCINEMA_FAKE_RESOLVER: fx.fake,
      ADMIN_PASSWORD: PASSWORD,
      SESSION_SECRET: 'parity-secret-parity-secret-parity-secret',
      PUBLIC_URL: '',
      TRUST_CLOUDFLARE: 'false',
      TRUST_PROXY: 'false',
      ...extraEnv,
    },
  });
  children.push(child);
  child.on('error', (e) => console.error(`${name}: ${e.message}`));
  return { name, port, child, logFile, base: `http://127.0.0.1:${port}` };
}

function killAll() {
  for (const c of children) {
    if (c.pid === undefined || c.exitCode !== null) continue;
    if (process.platform === 'win32') spawnSync('taskkill', ['/PID', String(c.pid), '/T', '/F']);
    else c.kill('SIGKILL');
  }
}

async function waitReady(srv) {
  for (let i = 0; i < 240; i++) {
    if (srv.child.exitCode !== null) {
      throw new Error(`${srv.name} zakończył się (kod ${srv.child.exitCode}):\n${fs.readFileSync(srv.logFile, 'utf8')}`);
    }
    try {
      const res = await fetch(`${srv.base}/live/main.m3u8`);
      if (res.status === 200) return;
    } catch {}
    await sleep(500);
  }
  throw new Error(`${srv.name}: /live/main.m3u8 nie odpowiada 200`);
}

// ---------- Klient ----------

const TIMING_FIELDS = new Set(['createdAt', 'startedAt', 'finishedAt', 'positionS']);

function stripTiming(v, topLevelChannel) {
  if (Array.isArray(v)) return v.map((x) => stripTiming(x, false));
  if (v && typeof v === 'object') {
    const out = {};
    for (const [k, val] of Object.entries(v)) {
      if (TIMING_FIELDS.has(k)) continue;
      if (topLevelChannel && (k === 'status' || k === 'current')) continue;
      // Etap 3: pola dodane przez pauzę (wartości sprawdzane osobno po stronie Rust w call()).
      if (topLevelChannel && (k === 'paused' || k === 'pauseReasons')) continue;
      // Etap 4: stan sterowania w panelu (serwer TS go nie ma).
      if (topLevelChannel && k === 'control') continue;
      out[k] = stripTiming(val, false);
    }
    return out;
  }
  return v;
}

/** Pełna lista atrybutów z wartościami; normalizowana tylko wartość tokenu i data Expires. */
function cookieShape(setCookies) {
  return setCookies.map((c) =>
    c
      .split(';')
      .map((part, i) => {
        const t = part.trim();
        const eq = t.indexOf('=');
        const k = eq < 0 ? t : t.slice(0, eq);
        const v = eq < 0 ? null : t.slice(eq + 1);
        if (i === 0) return `${k}=${v === '' ? '(pusty)' : '<token>'}`;
        if (k.toLowerCase() === 'expires') return 'Expires=<data>';
        return v === null ? k : `${k}=${v}`;
      })
      .join('; '),
  );
}

// Zamierzony status każdego kroku (sprawdzany na obu serwerach niezależnie od porównania serwerów).
const EXPECTED_STATUS = {
  "GET /": 200,
  "GET /admin": 200,
  "GET /static/common.js": 200,
  "GET /static/style.css": 200,
  "GET /static/brak.js (404)": 404,
  "GET /favicon.ico": 204,
  "GET /nie-ma (404)": 404,
  "GET state": 200,
  "GET /live/main.m3u8": 200,
  "request: nie link": 400,
  "request: ftp": 400,
  "request: niedozwolona domena": 400,
  "request: url nie string": 400,
  "request: pusty obiekt": 400,
  "request: zepsuty JSON": 400,
  "request: zły content-type": 415,
  "admin panel bez sesji (401)": 401,
  "admin skip bez sesji (401)": 401,
  "login: złe hasło": 401,
  "login: brak hasła": 401,
  "login: dobre": 200,
  "admin panel (pusty)": 200,
  "request: poprawne (ok1)": 201,
  "request: błąd resolvera": 400,
  "request: za długi": 400,
  "request: limit zgłoszeń (429)": 429,
  "GET admin settings (domyślne)": 200,
  "PUT settings: zły klucz": 400,
  "PUT settings: zły typ": 400,
  "PUT settings: ujemna liczba": 400,
  "PUT settings: dobre": 200,
  "request: transmisja na żywo": 400,
  "request: playlista": 400,
  "request: local: (niepoprawny link)": 400,
  "request: nieznany URL": 400,
  "request: ok2": 201,
  "request: ok4 (limit aktywnych?)": 201,
  "request: ok3 (limit aktywnych?)": 429,
  "request: limit aktywnych (429)": 429,
  "GET state z kolejką": 200,
  "PUT settings: moderacja": 200,
  "request: moderacja (pending)": 201,
  "GET state z regułami": 200,
  "admin panel z pending": 200,
  "PATCH: zły body": 400,
  "PATCH: brak body": 400,
  "PATCH: position ujemna": 400,
  "PATCH: position dla pending (409)": 409,
  "PATCH: approve": 200,
  "PATCH: approve ponownie (409)": 409,
  "PATCH: reject dla queued (409)": 409,
  "PATCH: position 0": 200,
  "PATCH: position ułamek": 400,
  "PATCH: nieistniejące id (404)": 404,
  "PATCH: id nie-liczba (404)": 404,
  "PATCH: position dla playing (409)": 409,
  "request: moderacja 2 (pending)": 201,
  "PATCH: reject": 200,
  "request: moderacja 3 (pending)": 201,
  "admin add: poprawny": 201,
  "admin add: playlista": 201,
  "admin add: pusta playlista": 400,
  "admin add: błąd resolvera": 400,
  "admin add: nieznany URL": 400,
  "admin add: nie link": 400,
  "admin add: bez url": 400,
  "library: root": 200,
  "library: sub": 200,
  "library: pusty folder": 200,
  "library: ..": 400,
  "library: ../..": 400,
  "library: sub/../..": 400,
  "library: absolutna": 400,
  "library: zły root": 400,
  "library: root nie-liczba": 400,
  "library: nieistniejący": 400,
  "library: plik zamiast folderu": 400,
  "local: plik": 201,
  "local: folder": 201,
  "local: pusty folder": 400,
  "local: ..": 400,
  "local: brak pliku": 400,
  "local: zły root": 400,
  "local: brak body": 400,
  "local: plik bez fałszywych metadanych": 400,
  "admin panel po dodaniu": 200,
  "DELETE: nieistniejące (404)": 404,
  "DELETE: queued": 200,
  "DELETE: ponownie (409)": 409,
  "DELETE: pending": 200,
  "segment: nieznany (404)": 404,
  "segment: ..%2F..%2Fpackage.json": 404,
  "segment: ..%5C..%5Cpackage.json": 404,
  "segment: ../../package.json (surowe)": 404,
  "segment: %2e%2e": 404,
  "logout": 200,
  "admin panel po wylogowaniu (401)": 401,
  "admin panel z podrobioną sesją (401)": 401,
  "login: dobre ponownie": 200,
  "skip": 200,
  "skip bez filmu": 200,
  "admin panel końcowy": 200,
  "login: zablokowany (zły)": 429,
  "login: zablokowany (dobry)": 429,
  "login: złe hasło #1": 401,
  "login: złe hasło #2": 401,
  "login: złe hasło #3": 401,
  "login: zablokowany #4": 429,
  "login: zablokowany #5": 429,
};

class Client {
  constructor(srv) {
    this.srv = srv;
    this.jar = new Map();
    this.records = [];
  }

  cookieHeader() {
    return [...this.jar].map(([k, v]) => `${k}=${v}`).join('; ');
  }

  /** opts: json | raw (+ contentType), channelBody (usuń status/current), skipBody */
  async call(name, method, url, opts = {}) {
    const headers = {};
    const cookie = this.cookieHeader();
    if (cookie) headers.cookie = cookie;
    let body;
    if (opts.json !== undefined) {
      headers['content-type'] = 'application/json';
      body = JSON.stringify(opts.json);
    } else if (opts.raw !== undefined) {
      headers['content-type'] = opts.contentType ?? 'application/json';
      body = opts.raw;
    }
    const res = await fetch(`${this.srv.base}${url}`, { method, headers, body, redirect: 'manual' });
    const text = await res.text();
    const setCookies = res.headers.getSetCookie();
    for (const c of setCookies) {
      const [pair, ...attrs] = c.split(';');
      const eq = pair.indexOf('=');
      const k = pair.slice(0, eq);
      const v = pair.slice(eq + 1);
      const maxAge = attrs.map((a) => a.trim().toLowerCase()).find((a) => a.startsWith('max-age='));
      if (v === '' || maxAge === 'max-age=0') this.jar.delete(k);
      else this.jar.set(k, v);
    }
    const ct = res.headers.get('content-type');
    let parsed;
    let bodyShape;
    if (opts.skipBody) bodyShape = '(pominięte)';
    else if (ct && ct.includes('json')) {
      try {
        parsed = JSON.parse(text);
        bodyShape = stripTiming(parsed, opts.channelBody === true);
      } catch {
        bodyShape = `(niepoprawny JSON) ${text}`;
      }
    } else bodyShape = text.length > 300 ? `sha1:${crypto.createHash('sha1').update(text).digest('hex')} len:${text.length}` : text;
    if (parsed && opts.channelBody === true && this.srv.name === 'rust') {
      // Etap 3: pola dodane przez pauzę; serwer TS ich nie ma, więc tylko sprawdzamy wartości po stronie Rust.
      if (parsed.paused !== false) throw new Error(`rust: "${name}": paused = ${JSON.stringify(parsed.paused)}, oczekiwano false`);
      if (!Array.isArray(parsed.pauseReasons) || parsed.pauseReasons.length !== 0) {
        throw new Error(`rust: "${name}": pauseReasons = ${JSON.stringify(parsed.pauseReasons)}, oczekiwano []`);
      }
    }
    const apiError = parsed && typeof parsed === 'object' && 'error' in parsed && !('statusCode' in parsed);
    if (apiError && this.srv.name === 'rust') {
      // Etap 4: każdy błąd API (nie 404 trasy w stylu Fastify) ma stały `code` (serwer TS go nie ma)
      // — sprawdzany tu, pomijany w porównaniu.
      if (typeof parsed.code !== 'string' || !/^[a-z_]+$/.test(parsed.code)) {
        throw new Error(`rust: "${name}": code = ${JSON.stringify(parsed.code)}, oczekiwano kodu błędu`);
      }
      if (bodyShape && typeof bodyShape === 'object') delete bodyShape.code;
    }
    const normalized = JSON.stringify(bodyShape).replaceAll(`:${this.srv.port}`, ':PORT');
    this.records.push({
      name,
      status: res.status,
      'content-type': ct,
      'cache-control': res.headers.get('cache-control'),
      'access-control-allow-origin': res.headers.get('access-control-allow-origin'),
      'set-cookie': cookieShape(setCookies),
      body: JSON.parse(normalized),
    });
    const expect = opts.expect ?? (name.startsWith('cleanup DELETE') ? 200 : EXPECTED_STATUS[name]);
    if (expect === undefined) throw new Error(`Brak oczekiwanego statusu dla kroku "${name}"`);
    if (res.status !== expect) {
      throw new Error(`${this.srv.name}: "${name}" -> ${res.status}, oczekiwano ${expect}
  odpowiedź: ${text.slice(0, 500)}`);
    }
    if (process.env.PARITY_VERBOSE) console.log(this.srv.name, res.status, name);
    return { status: res.status, json: parsed };
  }

  get = (name, url, o) => this.call(name, 'GET', url, o);
  post = (name, url, json, o) => this.call(name, 'POST', url, { json, ...o });
}

const yt = (id) => `https://www.youtube.com/watch?v=${id}`;

function need(value, what) {
  if (value === undefined || value === null) throw new Error(`Brak wartości: ${what}`);
  return value;
}

async function waitPlaying(c, count = 1) {
  for (let i = 0; i < 120; i++) {
    const res = await fetch(`${c.srv.base}/api/admin/channels/main`, { headers: { cookie: c.cookieHeader() } });
    const j = await res.json();
    if (j.playing?.length === count) return;
    await sleep(500);
  }
  throw new Error(`${c.srv.name}: liczba granych pozycji nie osiągnęła ${count}`);
}

async function scenario(srv) {
  const c = new Client(srv);
  const adminPanel = '/api/admin/channels/main';
  const login = (name, password) => c.post(name, '/api/admin/login', { password });
  const request = (name, url, extra = {}) => c.post(name, '/api/channels/main/requests', { url, ...extra });

  // Strony i statyczne pliki
  await c.get('GET /', '/');
  await c.get('GET /admin', '/admin');
  await c.get('GET /static/common.js', '/static/common.js');
  await c.get('GET /static/style.css', '/static/style.css');
  await c.get('GET /static/brak.js (404)', '/static/brak.js');
  await c.get('GET /favicon.ico', '/favicon.ico');
  await c.get('GET /nie-ma (404)', '/nie-ma');
  await c.get('GET state', '/api/channels/main/state', { channelBody: true });
  await c.get('GET /live/main.m3u8', '/live/main.m3u8', { skipBody: true });

  // Zgłoszenia publiczne: błędne dane wejściowe
  await request('request: nie link', 'to nie jest link');
  await request('request: ftp', 'ftp://youtube.com/x');
  await request('request: niedozwolona domena', 'https://vimeo.com/123');
  await c.post('request: url nie string', '/api/channels/main/requests', { url: 5 });
  await c.post('request: pusty obiekt', '/api/channels/main/requests', {});
  await c.call('request: zepsuty JSON', 'POST', '/api/channels/main/requests', { raw: '{nie json' });
  await c.call('request: zły content-type', 'POST', '/api/channels/main/requests', { raw: 'url=x', contentType: 'application/x-www-form-urlencoded' });

  // Panel bez sesji
  await c.get('admin panel bez sesji (401)', adminPanel);
  await c.post('admin skip bez sesji (401)', `${adminPanel}/skip`, {});
  await login('login: złe hasło', 'zle-haslo');
  await login('login: brak hasła', undefined);
  await login('login: dobre', PASSWORD);
  await c.get('admin panel (pusty)', adminPanel, { channelBody: true });

  // Zgłoszenia publiczne: poprawne, błędy resolvera, limity
  const ok1 = await request('request: poprawne (ok1)', yt('ok1'), { name: '  Ala\u0001 ma kota  ' });
  await waitPlaying(c);
  await request('request: błąd resolvera', yt('err'));
  await request('request: za długi', yt('long'));
  await request('request: limit zgłoszeń (429)', yt('live'));
  await c.get('GET admin settings (domyślne)', `${adminPanel}/settings`);
  await c.call('PUT settings: zły klucz', 'PUT', `${adminPanel}/settings`, { json: { foo: 1 } });
  await c.call('PUT settings: zły typ', 'PUT', `${adminPanel}/settings`, { json: { moderation: 'tak' } });
  await c.call('PUT settings: ujemna liczba', 'PUT', `${adminPanel}/settings`, { json: { maxDurationS: -1 } });
  await c.call('PUT settings: dobre', 'PUT', `${adminPanel}/settings`, {
    json: { requestLimitCount: 100, maxActivePerRequester: 3, allowedDomains: [' HTTPS://www.YouTube.com/x ', 'youtu.be'] },
  });
  await request('request: transmisja na żywo', yt('live'));
  await request('request: playlista', 'https://www.youtube.com/playlist?list=pl');
  await request('request: local: (niepoprawny link)', 'local:0/movie1.mp4');
  await request('request: nieznany URL', 'https://www.youtube.com/watch?v=nieznany');
  await request('request: ok2', yt('ok2'), { name: 'x'.repeat(50) });
  await request('request: ok4 (limit aktywnych?)', 'https://youtu.be/ok4');
  await request('request: ok3 (limit aktywnych?)', yt('ok3'));
  await request('request: limit aktywnych (429)', yt('ok1'));
  await c.get('GET state z kolejką', '/api/channels/main/state', { channelBody: true });

  // Moderacja
  await c.call('PUT settings: moderacja', 'PUT', `${adminPanel}/settings`, { json: { moderation: true, maxActivePerRequester: 10 } });
  const ok5 = await request('request: moderacja (pending)', yt('ok5'));
  const ok5Id = need(ok5.json?.item?.id, 'id zgłoszenia ok5');
  const ok1Id = need(ok1.json?.item?.id, 'id zgłoszenia ok1');
  await c.get('GET state z regułami', '/api/channels/main/state', { channelBody: true });
  await c.get('admin panel z pending', adminPanel, { channelBody: true });
  const patch = (name, id, json) => c.call(name, 'PATCH', `/api/admin/items/${id}`, { json });
  await patch('PATCH: zły body', ok5Id, { foo: 1 });
  await patch('PATCH: brak body', ok5Id, undefined);
  await patch('PATCH: position ujemna', ok5Id, { position: -1 });
  await patch('PATCH: position dla pending (409)', ok5Id, { position: 0 });
  await patch('PATCH: approve', ok5Id, { action: 'approve' });
  await patch('PATCH: approve ponownie (409)', ok5Id, { action: 'approve' });
  await patch('PATCH: reject dla queued (409)', ok5Id, { action: 'reject' });
  await patch('PATCH: position 0', ok5Id, { position: 0 });
  await patch('PATCH: position ułamek', ok5Id, { position: 1.5 });
  await patch('PATCH: nieistniejące id (404)', 9999, { action: 'approve' });
  await patch('PATCH: id nie-liczba (404)', 'abc', { action: 'approve' });
  await patch('PATCH: position dla playing (409)', ok1Id, { position: 0 });
  const p2 = await request('request: moderacja 2 (pending)', yt('ok2'));
  await patch('PATCH: reject', need(p2.json?.item?.id, 'id p2'), { action: 'reject' });
  const p3 = await request('request: moderacja 3 (pending)', yt('ok3'));

  // Dodawanie przez administratora
  const add = (name, url) => c.post(name, `${adminPanel}/items`, { url });
  await add('admin add: poprawny', 'https://example.org/video');
  await add('admin add: playlista', 'https://example.org/list');
  await add('admin add: pusta playlista', 'https://example.org/empty');
  await add('admin add: błąd resolvera', 'https://example.org/broken');
  await add('admin add: nieznany URL', 'https://example.org/nieznany');
  await add('admin add: nie link', 'cos');
  await c.post('admin add: bez url', `${adminPanel}/items`, {});

  // Biblioteka plików
  await c.get('library: root', '/api/admin/library');
  await c.get('library: sub', '/api/admin/library?root=0&path=sub');
  await c.get('library: pusty folder', '/api/admin/library?root=0&path=empty');
  await c.get('library: ..', '/api/admin/library?root=0&path=..');
  await c.get('library: ../..', '/api/admin/library?root=0&path=..%2F..');
  await c.get('library: sub/../..', '/api/admin/library?root=0&path=sub%2F..%2F..');
  await c.get('library: absolutna', '/api/admin/library?root=0&path=C%3A%2FWindows');
  await c.get('library: zły root', '/api/admin/library?root=7');
  await c.get('library: root nie-liczba', '/api/admin/library?root=abc');
  await c.get('library: nieistniejący', '/api/admin/library?root=0&path=brak');
  await c.get('library: plik zamiast folderu', '/api/admin/library?root=0&path=movie1.mp4');
  const local = (name, json) => c.post(name, `${adminPanel}/local`, json);
  await local('local: plik', { root: 0, path: 'movie1.mp4' });
  await local('local: folder', { root: 0, path: 'sub' });
  await local('local: pusty folder', { root: 0, path: 'empty' });
  await local('local: ..', { root: 0, path: '../x.mp4' });
  await local('local: brak pliku', { root: 0, path: 'brak.mp4' });
  await local('local: zły root', { root: 3, path: 'movie1.mp4' });
  await local('local: brak body', {});
  await local('local: plik bez fałszywych metadanych', { root: 0, path: 'readme.txt' });

  // Panel po dodaniu, usuwanie
  const panel = await c.get('admin panel po dodaniu', adminPanel, { channelBody: true });
  const queued = need(panel.json?.queue, 'kolejka w panelu').map((i) => i.id);
  if (queued.length === 0) throw new Error('Kolejka w panelu jest pusta');
  await c.call('DELETE: nieistniejące (404)', 'DELETE', '/api/admin/items/9999');
  await c.call('DELETE: queued', 'DELETE', `/api/admin/items/${queued[0]}`);
  await c.call('DELETE: ponownie (409)', 'DELETE', `/api/admin/items/${queued[0]}`);
  await c.call('DELETE: pending', 'DELETE', `/api/admin/items/${need(p3.json?.item?.id, 'id p3')}`);

  // Fragmenty strumienia
  await c.get('segment: nieznany (404)', '/live/main/brak.ts');
  await c.get('segment: ..%2F..%2Fpackage.json', '/live/main/..%2F..%2Fpackage.json');
  await c.get('segment: ..%5C..%5Cpackage.json', '/live/main/..%5C..%5Cpackage.json');
  await c.get('segment: ../../package.json (surowe)', '/live/main/../../package.json');
  await c.get('segment: %2e%2e', '/live/main/%2e%2e');

  // Wylogowanie i ponowne logowanie
  await c.post('logout', '/api/admin/logout', {});
  await c.get('admin panel po wylogowaniu (401)', adminPanel);
  c.jar.set('vrcinema_session', 'podrobione.ciasteczko');
  await c.get('admin panel z podrobioną sesją (401)', adminPanel);
  c.jar.delete('vrcinema_session');
  await login('login: dobre ponownie', PASSWORD);

  // Posprzątanie kolejki, żeby skip nie uruchomił następnego filmu w wyścigu
  const before = await fetch(`${srv.base}${adminPanel}`, { headers: { cookie: c.cookieHeader() } }).then((r) => r.json());
  for (const i of need(before.queue, 'kolejka przed sprzątaniem')) await c.call(`cleanup DELETE ${i.id}`, 'DELETE', `/api/admin/items/${i.id}`);
  await c.post('skip', `${adminPanel}/skip`, {});
  await waitPlaying(c, 0); // Rust wykonuje skip przy następnym cyklu silnika, TS od razu
  await c.post('skip bez filmu', `${adminPanel}/skip`, {});
  await c.get('admin panel końcowy', adminPanel, { channelBody: true });

  // Blokada logowania na końcu (oba serwery widzą 127.0.0.1)
  // Wcześniej były 2 nieudane próby (złe hasło, brak hasła); 3 kolejne dobijają do limitu 5.
  for (let i = 1; i <= 3; i++) await login(`login: złe hasło #${i}`, 'zle-haslo');
  for (let i = 4; i <= 5; i++) await login(`login: zablokowany #${i}`, 'zle-haslo');
  await login('login: zablokowany (zły)', 'zle-haslo');
  await login('login: zablokowany (dobry)', PASSWORD);
  return c.records;
}

// ---------- Porównanie ----------

function diffValues(a, b, p = '') {
  if (JSON.stringify(a) === JSON.stringify(b)) return [];
  const isObj = (x) => x && typeof x === 'object';
  if (isObj(a) && isObj(b) && Array.isArray(a) === Array.isArray(b)) {
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    return [...keys].flatMap((k) => diffValues(a[k], b[k], p ? `${p}.${k}` : k));
  }
  return [{ field: p || '(root)', ts: a, rust: b }];
}

function compare(tsRecs, rustRecs) {
  const diffs = [];
  if (tsRecs.length !== rustRecs.length) diffs.push({ name: '(liczba żądań)', field: 'count', ts: tsRecs.length, rust: rustRecs.length });
  for (let i = 0; i < Math.min(tsRecs.length, rustRecs.length); i++) {
    if (tsRecs[i].name !== rustRecs[i].name) throw new Error(`Rozjechane kroki scenariusza: #${i} "${tsRecs[i].name}" vs "${rustRecs[i].name}"`);
    for (const d of diffValues(tsRecs[i], rustRecs[i])) {
      diffs.push({ name: tsRecs[i].name, ...d });
    }
  }
  return diffs;
}

// ---------- main ----------

async function main() {
  if (!fs.existsSync(rustBin)) throw new Error(`Brak ${rustBin} — uruchom najpierw: cargo build (w engine/)`);
  const fx = prepareFixtures();
  const ts = startServer('ts', 8191, process.execPath, ['--disable-warning=ExperimentalWarning', 'src/server.ts'], fx);
  // Etap 4: serwer Rust domyślnie mówi po angielsku; serwer TS zna tylko polski.
  const rust = startServer('rust', 8192, rustBin, [], fx, { LANGUAGE: 'pl' });
  await Promise.all([waitReady(ts), waitReady(rust)]);
  console.log('Oba serwery gotowe, odtwarzam scenariusz...');
  const [tsRes, rustRes] = await Promise.allSettled([scenario(ts), scenario(rust)]);
  if (tsRes.status === 'rejected' || rustRes.status === 'rejected') {
    for (const [n, r] of [['TS', tsRes], ['Rust', rustRes]]) {
      console.error(`
[${n}] ${r.status === 'rejected' ? r.reason.message : 'scenariusz przeszedł'}`);
    }
    return 1;
  }
  const tsRecs = tsRes.value;
  const rustRecs = rustRes.value;
  console.log(`Żądań na serwer: ${tsRecs.length} (TS), ${rustRecs.length} (Rust)`);
  const diffs = compare(tsRecs, rustRecs);
  for (const d of diffs) {
    console.log(`\nRÓŻNICA: ${d.name}\n  pole: ${d.field}\n  TS:   ${JSON.stringify(d.ts)}\n  Rust: ${JSON.stringify(d.rust)}`);
  }
  console.log(diffs.length === 0 ? '\nOK: 0 różnic.' : `\nRóżnic: ${diffs.length}`);
  return diffs.length === 0 ? 0 : 1;
}

let code = 1;
try {
  code = await main();
} catch (e) {
  console.error(e);
} finally {
  killAll();
  await sleep(1500);
  try {
    fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch (e) {
    console.error(`Nie udało się usunąć ${tmp}: ${e.message}`);
  }
}
process.exit(code);
