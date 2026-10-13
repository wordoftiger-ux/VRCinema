// A stand-in for the engine for browser checks of the interface (no ffmpeg, no channel): it
// serves public/ like the engine (/static/…, /app) and answers the panel's API from a scenario
// in memory, recording every API request. Never port 8080.
//
//   [FAKE_HLS_DIR=<folder>] node tools/ui-check/fake-engine.mjs [port]      (default 8193)
//
// Test hooks (not engine routes): GET /__test/requests, POST /__test/reset,
// POST /__test/scenario {language, playing: 'film'|'live'|'none', canSeek, positionApprox,
// pending (number of videos waiting for approval), addError (code of the error POST …/items
// answers with), panelFail (the panel answers 500), panelDelayMs, patchDelayMs, signedOut, remote, tunnel}. The queue lives in memory: POST …/items adds,
// PATCH /api/admin/items/{id} moves ({position}) or decides ({action}), DELETE removes.
// GET /__test/stats: {maxPanelInFlight}; GET /__test/hang never answers.
// History (panel field `history`): played, skipped (played with a reason), failed (localized
// reason), a file from disk and an old row with a plain-text reason. Share view: GET
// /api/desktop/tunnel/qr (window; 403 for a remote session, 404 without a public address) and
// GET /api/admin/qr (panel; 404 without an address) answer a PNG; scenario `tunnelUrl` changes
// the address (the QR must follow), `tunnel` is 'online'|'off'|'connecting'|'error'.
// POST /api/desktop/tunnel/restart rotates the quick address. Permanent link wizard
// (/api/desktop/tunnel/named/…): scenario `namedLogin` 'done' (default, after `loginMs`),
// 'wait' (stays running until cancelled) or 'fail'; `browserOpened` (default false); host
// names: taken.example.com → dns_taken (unless overwrite), nodomain.example.com → no_domain
// (+ helpUrl), slow.example.com → takes 2.5 s; any other works. GET /api/desktop/config
// answers tunnelMode / namedTunnelHostname.
// Stats route (GET /api/admin/channels/main/stats): scenario `stats` is 'full' (default) or
// 'nulls' (every group null, as when nothing plays) or 'partial' (the full one with a source analysis lacking height, frame rate and length). The preview's stream (/live/main.m3u8
// and its segments) comes from `options.hlsDir` (a folder with main.m3u8 and .ts files; the
// playlist is served without EXT-X-ENDLIST so the player treats it as live); the scenario
// `hls: false` makes it 404.

import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const PUBLIC = join(fileURLToPath(new URL('.', import.meta.url)), '..', '..', 'public');
const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.mjs': 'application/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
};

export function startFakeEngine(port = 8193, { hlsDir = null } = {}) {
  if (port === 8080) throw new Error('never port 8080');
  let requests = [];
  let s;
  let nextId;
  let panelInFlight = 0;
  let maxPanelInFlight = 0;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  const queued = (id, title, extra = {}) => ({
    id,
    title,
    durationS: 232,
    requesterName: 'Tomek',
    requesterBadge: { hue: 200, shape: id % 12 },
    isLive: false,
    sourceUrl: `https://www.youtube.com/watch?v=${id}`,
    status: 'queued',
    addedBy: 'viewer',
    error: null,
    createdAt: 0,
    finishedAt: null,
    ...extra,
  });
  const initialQueue = () => [
    queued(8, 'Darude — Sandstorm'),
    queued(9, 'Shrek 2 (2004).mkv', { durationS: 6130, requesterName: null, requesterBadge: 'host', sourceUrl: 'local:0/Shrek%202.mkv', addedBy: 'admin' }),
    queued(10, 'Lofi hip hop radio', { durationS: null, isLive: true, requesterName: 'Kasia', requesterBadge: { hue: 40, shape: 3 } }),
    queued(11, 'Bez tytułu', { requesterName: null, requesterBadge: null }),
  ];
  const pendingList = (n) =>
    Array.from({ length: n }, (_, i) => queued(20 + i, i === 0 ? 'Nyan Cat 10 godzin' : `Film do zatwierdzenia ${i}`, { status: 'pending_approval', requesterName: i === 1 ? null : 'Bartek', requesterBadge: i === 1 ? null : { hue: 300, shape: 7 } }));

  const idleProgress = () => ({ operation: null, state: 'idle', running: false, step: null, message: null, code: null, helpUrl: null, loginUrl: null, browserOpened: null });
  const initialHistory = () => {
    const now = Date.now();
    return [
      queued(1, 'Daft Punk — One More Time', { status: 'played', durationS: 320, finishedAt: now - 60000 }),
      queued(2, 'Kot gra na pianinie', { status: 'played', durationS: 48, requesterName: 'Kasia', error: 'Skipped by the owner', finishedAt: now - 120000 }),
      queued(3, 'Bez tytułu', { status: 'failed', durationS: null, requesterName: 'Bartek', sourceUrl: 'https://pixeldrain.com/u/aB3x', error: 'The file does not exist', finishedAt: now - 180000 }),
      queued(4, 'Shrek (2001).mkv', { status: 'played', durationS: 5402, requesterName: null, requesterBadge: 'host', sourceUrl: 'local:0/Shrek.mkv', addedBy: 'admin', finishedAt: now - 86400000 * 3 }),
      queued(5, 'Stary film', { status: 'failed', requesterName: null, requesterBadge: null, error: 'Nie udało się pobrać filmu (stary wpis).', finishedAt: now - 86400000 * 4 }),
    ];
  };

  function reset() {
    requests = [];
    s = {
      language: 'en',
      playing: 'film',
      canSeek: true,
      positionApprox: false,
      pending: 0,
      addError: null,
      panelFail: false,
      panelDelayMs: 0,
      patchDelayMs: 0,
      signedOut: false,
      remote: false,
      tunnel: 'online',
      tunnelUrl: 'https://proba-kanalu.trycloudflare.com',
      namedLogin: 'done',
      loginMs: 2500,
      browserOpened: false,
      paused: false,
      pauseReasons: [],
      positionS: 92,
      durationS: 213,
      stats: 'full',
      hls: true,
    };
    s.queue = initialQueue();
    s.named = { mode: 'quick', host: null, loggedIn: false, progress: idleProgress() };
    s.rotations = 0;
    s.history = initialHistory();
    s.pendingItems = [];
    nextId = 100;
    maxPanelInFlight = 0;
  }
  reset();

  function control() {
    const film = s.playing === 'film';
    const live = s.playing === 'live';
    return {
      itemId: s.playing === 'none' ? null : 7,
      isLive: live,
      positionS: film ? s.positionS : live ? 40 : 0,
      durationS: film ? s.durationS : null,
      canPause: film,
      canSeek: film && s.canSeek,
      seekPending: false,
      positionApprox: s.positionApprox,
      paused: s.paused,
      pauseReasons: s.pauseReasons,
    };
  }

  function panel() {
    const current =
      s.playing === 'none'
        ? null
        : {
            itemId: 7,
            title: s.playing === 'live' ? 'Lofi hip hop radio' : 'Never Gonna Give You Up',
            durationS: s.playing === 'film' ? s.durationS : null,
            positionS: s.positionS - 12,
            requesterName: 'Kasia',
            requesterBadge: { hue: 40, shape: 3 },
            isLive: s.playing === 'live',
          };
    const item = (id, title) => queued(id, title, { requesterBadge: null });
    return {
      status: s.playing === 'none' ? 'idle' : s.paused ? 'paused' : 'playing',
      current,
      paused: s.paused,
      pauseReasons: s.pauseReasons,
      prefetchedItemId: null,
      playing: current ? [{ ...item(7, current.title), status: 'playing', sourceUrl: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ' }] : [],
      queue: s.queue,
      pending: s.pendingItems,
      history: s.history,
      settings: {},
      control: control(),
      channelUrl: `http://127.0.0.1:${port}/live/main.m3u8`,
      requestUrl: `http://127.0.0.1:${port}/`,
    };
  }

  const statsAnswer = () => {
    const full = statsFull();
    if (s.stats === 'partial') full.onAir.probe = { codec: 'h264', height: null, fps: null, pixFmt: 'yuv420p', hasAudio: true, durationS: null };
    return full;
  };
  const statsFull = () =>
    s.stats === 'nulls'
      ? { onAir: null, hls: null, next: null, ffmpeg: null, viewers: { watching: 0, fetchesPerMin: 0, bytesPerS: 0, history: new Array(60).fill(0) }, tunnel: null }
      : {
          onAir: {
            title: 'Never Gonna Give You Up',
            source: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ',
            positionS: s.positionS,
            attempt: 1,
            probe: { codec: 'h264', height: 1080, fps: 25, pixFmt: 'yuv420p', hasAudio: true, durationS: 213 },
            mode: 'copy',
            encoder: null,
            readySegments: 5,
          },
          hls: { sequence: 18342, window: 10, segmentS: 4, discontinuities: 37, lastPts: 1482900, dtsContinuous: true },
          next: { title: 'Darude — Sandstorm', state: 'ready', readySegments: 3 },
          ffmpeg: { pid: 18872, speed: 1.9, cpuPercent: null, rssBytes: 88080384 },
          viewers: { watching: 4, fetchesPerMin: 58, bytesPerS: 1400000, history: Array.from({ length: 60 }, (_, i) => (i % 7) * 200000) },
          tunnel: { state: 'online', onlineS: 4320, reconnects: 1, connections: 4, locations: ['waw01', 'fra02'] },
        };

  async function serveLive(res, name) {
    if (!hlsDir || !s.hls || !/^(main\.m3u8|\d+\.ts)$/.test(name)) return json(res, 404, { error: 'Not Found' });
    try {
      let bytes = await readFile(join(hlsDir, name));
      if (name.endsWith('.m3u8')) bytes = Buffer.from(bytes.toString('utf8').replace('#EXT-X-ENDLIST', ''));
      res.writeHead(200, { 'content-type': name.endsWith('.ts') ? 'video/mp2t' : 'application/vnd.apple.mpegurl', 'cache-control': 'no-store' });
      res.end(bytes);
    } catch {
      json(res, 404, { error: 'Not Found' });
    }
  }

  const tunnelView = () => ({
    state: s.tunnel,
    url: s.tunnel === 'online' ? (s.named.mode === 'named' ? `https://${s.named.host}` : s.tunnelUrl) : null,
    message: s.tunnel === 'error' ? 'No internet' : null,
    mode: s.named.mode,
    hostname: s.named.mode === 'named' ? s.named.host : null,
    warning: null,
  });
  const publicAddress = () => (s.tunnel === 'online' ? tunnelView().url : null);

  // The 1x1 PNG the QR routes answer with.
  const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');
  const sendQr = (res) => {
    if (!publicAddress()) return coded(res, 404, 'no_public_address', 'No public address');
    res.writeHead(200, { 'content-type': 'image/png', 'cache-control': 'no-store' });
    return res.end(PNG);
  };

  const progressOf = (patch) => (s.named.progress = { ...idleProgress(), ...patch, loggedIn: s.named.loggedIn });
  async function namedRoute(res, action, req, payload) {
    const n = s.named;
    if (action === 'progress' && req.method === 'GET') return json(res, 200, { ...n.progress, loggedIn: n.loggedIn });
    if (req.method !== 'POST') return json(res, 404, { error: 'Not Found' });
    const lang = req.headers['x-vrcinema-lang'] === 'pl';
    if (action === 'login') {
      if (payload?.cancel) {
        n.loginRun = (n.loginRun ?? 0) + 1;
        progressOf({ operation: 'login', state: 'cancelled' });
        return json(res, 200, { ...n.progress });
      }
      if (n.progress.running) return coded(res, 409, 'busy', 'A permanent link operation is running');
      const run = (n.loginRun = (n.loginRun ?? 0) + 1);
      progressOf({
        operation: 'login',
        state: 'running',
        running: true,
        step: 'waiting_for_browser',
        message: lang ? 'Zaloguj się w Cloudflare w przeglądarce.' : 'Log in to Cloudflare in the browser.',
        loginUrl: 'https://dash.cloudflare.com/argotunnel?fake=1',
        browserOpened: s.browserOpened,
      });
      if (s.namedLogin !== 'wait') {
        setTimeout(() => {
          if (n.loginRun !== run) return;
          if (s.namedLogin === 'fail') progressOf({ operation: 'login', state: 'failed', code: 'timeout', message: 'cloudflared did not answer within 300 s.' });
          else {
            n.loggedIn = true;
            progressOf({ operation: 'login', state: 'done', message: 'Connected to Cloudflare.' });
          }
        }, s.loginMs);
      }
      return json(res, 202, { ...n.progress });
    }
    if (action === 'create') {
      const host = String(payload?.hostname ?? '').trim().toLowerCase();
      if (!n.loggedIn) return coded(res, 409, 'not_logged_in', 'Connect to Cloudflare first.');
      const fail = (status, code, message, extra = {}) => {
        progressOf({ operation: 'create', state: 'failed', code, message, ...extra });
        return json(res, status, { error: message, code, ...extra });
      };
      progressOf({ operation: 'create', state: 'running', running: true, step: 'creating_tunnel', message: 'Creating the tunnel vrcinema in Cloudflare…' });
      await sleep(host === 'slow.example.com' ? 2500 : 300);
      if (host === 'taken.example.com' && !payload?.overwrite) return fail(409, 'dns_taken', `The address ${host} is already taken in Cloudflare DNS.`, { helpUrl: null });
      if (host === 'nodomain.example.com') return fail(422, 'no_domain', `Add a domain to Cloudflare first (${host}).`, { helpUrl: 'https://example.invalid/add-domain' });
      n.mode = 'named';
      n.host = host;
      progressOf({ operation: 'create', state: 'done', message: `Permanent link https://${host} is ready.` });
      return json(res, 200, { ok: true, message: n.progress.message, tunnel: tunnelView() });
    }
    if (action === 'quick' || action === 'delete') {
      n.mode = 'quick';
      if (action === 'delete') {
        n.host = null;
        n.loggedIn = false;
      }
      progressOf({});
      return json(res, 200, { ok: true, message: action === 'delete' ? 'Permanent link deleted — the quick tunnel runs.' : 'The quick tunnel runs.', tunnel: tunnelView() });
    }
    return json(res, 404, { error: 'Not Found' });
  }

  function info() {
    return {
      version: '0.2.0',
      port,
      publicUrl: '',
      language: s.language,
      tunnelEnabled: s.tunnel !== 'off',
      tunnel: tunnelView(),
      paused: s.paused,
      pauseReasons: s.pauseReasons,
    };
  }

  const json = (res, status, body) => {
    res.writeHead(status, { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' });
    res.end(JSON.stringify(body));
  };
  const coded = (res, status, code, error) => json(res, status, { error, code });

  async function body(req) {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const text = Buffer.concat(chunks).toString('utf8');
    return text ? JSON.parse(text) : null;
  }

  async function serveFile(res, rel) {
    const path = normalize(join(PUBLIC, rel));
    if (!path.startsWith(PUBLIC + sep)) return json(res, 404, { error: 'Not Found' });
    try {
      const bytes = await readFile(path);
      res.writeHead(200, { 'content-type': TYPES[extname(path)] ?? 'application/octet-stream', 'cache-control': 'public, max-age=0' });
      res.end(bytes);
    } catch {
      json(res, 404, { error: 'Not Found' });
    }
  }

  const server = createServer(async (req, res) => {
    const url = new URL(req.url, `http://127.0.0.1:${port}`);
    const path = url.pathname;
    if (path === '/app') return serveFile(res, 'app.html');
    if (path === '/favicon.ico') {
      res.writeHead(204);
      return res.end();
    }
    if (path.startsWith('/live/')) return serveLive(res, path.slice('/live/'.length));
    if (path.startsWith('/static/')) return serveFile(res, decodeURIComponent(path.slice('/static/'.length)));
    if (path === '/__test/requests') return json(res, 200, requests);
    if (path === '/__test/stats') return json(res, 200, { maxPanelInFlight });
    if (path === '/__test/hang') return undefined;
    if (path === '/__test/reset') {
      reset();
      return json(res, 200, { ok: true });
    }
    if (path === '/__test/scenario') {
      const change = await body(req);
      Object.assign(s, change);
      if ('pending' in change) s.pendingItems = pendingList(change.pending);
      return json(res, 200, { ...s, queue: undefined, pendingItems: undefined });
    }
    if (!path.startsWith('/api/')) return json(res, 404, { error: 'Not Found' });

    const payload = req.method === 'GET' ? null : await body(req).catch(() => null);
    requests.push({ method: req.method, path, body: payload, lang: req.headers['x-vrcinema-lang'] ?? null, at: Date.now() });
    if (path === '/api/desktop/info' || path === '/api/desktop/config') {
      if (s.signedOut) return coded(res, 401, 'unauthorized', 'Sign in');
      if (s.remote) return coded(res, 403, 'window_only', 'Window only');
      if (req.method === 'PUT') {
        if (payload?.language) s.language = payload.language;
        if (payload?.tunnelMode) s.named.mode = payload.tunnelMode;
        return json(res, 200, { restarted: false });
      }
      if (path === '/api/desktop/config') return json(res, 200, { language: s.language, tunnelEnabled: s.tunnel !== 'off', tunnelMode: s.named.mode, namedTunnelHostname: s.named.host });
      return json(res, 200, info());
    }
    if (s.signedOut) return coded(res, 401, 'unauthorized', 'Sign in');
    if (path.startsWith('/api/desktop/')) {
      if (s.remote) return coded(res, 403, 'window_only', 'Window only');
      if (path === '/api/desktop/tunnel/qr') return sendQr(res);
      if (path === '/api/desktop/tunnel/restart' && req.method === 'POST') {
        if (s.tunnel === 'off') return coded(res, 409, 'tunnel_off', 'Tunnel is off');
        s.rotations += 1;
        s.tunnelUrl = `https://proba-kanalu-${s.rotations}.trycloudflare.com`;
        return json(res, 200, tunnelView());
      }
      if (path.startsWith('/api/desktop/tunnel/named/')) return namedRoute(res, path.slice('/api/desktop/tunnel/named/'.length), req, payload);
      return json(res, 404, { error: 'Not Found' });
    }
    if (path === '/api/admin/qr') return sendQr(res);
    const base = '/api/admin/channels/main';
    if (path === base && req.method === 'GET') {
      if (s.panelFail) return coded(res, 500, 'internal', 'boom');
      panelInFlight += 1;
      maxPanelInFlight = Math.max(maxPanelInFlight, panelInFlight);
      if (s.panelDelayMs) await sleep(s.panelDelayMs);
      const answer = panel();
      panelInFlight -= 1;
      return json(res, 200, answer);
    }
    if (path === `${base}/stats` && req.method === 'GET') return json(res, 200, statsAnswer());
    const itemRoute = path.match(/^\/api\/admin\/items\/(\d+)$/);
    if (itemRoute) {
      const id = Number(itemRoute[1]);
      const inQueue = s.queue.findIndex((i) => i.id === id);
      const inPending = s.pendingItems.findIndex((i) => i.id === id);
      if (inQueue < 0 && inPending < 0) return coded(res, 404, 'no_such_item', 'No such item');
      if (req.method === 'DELETE') {
        s.queue = s.queue.filter((i) => i.id !== id);
        s.pendingItems = s.pendingItems.filter((i) => i.id !== id);
        return json(res, 200, { ok: true });
      }
      if (req.method === 'PATCH') {
        if (s.patchDelayMs) await sleep(s.patchDelayMs);
        if (payload?.action === 'approve' || payload?.action === 'reject') {
          if (inPending < 0) return coded(res, 409, 'not_pending', 'Not pending');
          const [moved] = s.pendingItems.splice(inPending, 1);
          if (payload.action === 'approve') s.queue.push({ ...moved, status: 'queued' });
          return json(res, 200, { ok: true });
        }
        if (Number.isInteger(payload?.position)) {
          if (inQueue < 0) return coded(res, 409, 'move_only_queued', 'Only queued');
          const [moved] = s.queue.splice(inQueue, 1);
          s.queue.splice(Math.min(payload.position, s.queue.length), 0, moved);
          return json(res, 200, { ok: true });
        }
        return coded(res, 400, 'action_or_position', 'Give action or position');
      }
    }
    if (req.method !== 'POST') return json(res, 404, { error: 'Not Found' });
    const action = path.slice(base.length + 1);
    if (action === 'skip') return json(res, 200, { ok: true });
    if (action === 'items') {
      if (s.addError) {
        const pl = req.headers['x-vrcinema-lang'] === 'pl';
        const message = s.addError === 'domain_not_allowed'
          ? (pl ? 'Ten serwis nie jest dozwolony. Dozwolone: youtube.com.' : 'This site is not allowed. Allowed: youtube.com.')
          : `Refused: ${s.addError}`;
        return coded(res, 400, s.addError, message);
      }
      if (!/^https?:\/\//.test(payload?.url ?? '')) return coded(res, 400, 'invalid_link', 'Invalid link');
      s.queue.push(queued(nextId++, 'Added from ' + new URL(payload.url).hostname));
      return json(res, 201, { added: 1, title: s.queue.at(-1).title });
    }
    if (s.playing === 'none' && ['pause', 'seek', 'restart'].includes(action)) {
      return coded(res, 409, 'nothing_playing', `Nothing is playing (${req.headers['x-vrcinema-lang']})`);
    }
    if (s.playing === 'live' && ['pause', 'seek'].includes(action)) {
      return coded(res, 409, 'live_item', `A live stream cannot be paused or seeked (${req.headers['x-vrcinema-lang']})`);
    }
    switch (action) {
      case 'pause':
        s.paused = true;
        if (!s.pauseReasons.includes('manual')) s.pauseReasons = [...s.pauseReasons, 'manual'];
        break;
      case 'resume':
        s.pauseReasons = s.pauseReasons.filter((r) => r !== 'manual');
        s.paused = s.pauseReasons.length > 0;
        break;
      case 'seek':
        if (Number.isFinite(payload?.toS)) s.positionS = payload.toS;
        else if (Number.isFinite(payload?.byS)) s.positionS = Math.max(0, s.positionS + payload.byS);
        else return coded(res, 400, 'bad_request', 'Give toS or byS');
        break;
      case 'restart':
        s.positionS = 0;
        break;
      case 'play-now':
        break;
      default:
        return json(res, 404, { error: 'Not Found' });
    }
    return json(res, 200, { control: control() });
  });

  return new Promise((resolve) => {
    server.listen(port, '127.0.0.1', () => resolve({ server, port, close: () => new Promise((r) => server.close(r)) }));
  });
}

if (process.argv[1] && fileURLToPath(import.meta.url) === normalize(process.argv[1])) {
  const port = Number(process.argv[2] ?? 8193);
  const { port: bound } = await startFakeEngine(port, { hlsDir: process.env.FAKE_HLS_DIR ?? null });
  console.log(`fake engine on http://127.0.0.1:${bound}/app`);
}
