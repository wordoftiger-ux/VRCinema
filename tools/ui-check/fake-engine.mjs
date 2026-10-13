// A stand-in for the engine for browser checks of the interface (no ffmpeg, no channel): it
// serves public/ like the engine (/static/…, /app) and answers the panel's API from a scenario
// in memory, recording every API request. Never port 8080.
//
//   node tools/ui-check/fake-engine.mjs [port]      (default 8193)
//
// Test hooks (not engine routes): GET /__test/requests, POST /__test/reset,
// POST /__test/scenario {language, playing: 'film'|'live'|'none', canSeek, positionApprox,
// pending, signedOut, remote, tunnel}.

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

export function startFakeEngine(port = 8193) {
  if (port === 8080) throw new Error('never port 8080');
  let requests = [];
  let s;

  function reset() {
    requests = [];
    s = {
      language: 'en',
      playing: 'film',
      canSeek: true,
      positionApprox: false,
      pending: 0,
      signedOut: false,
      remote: false,
      tunnel: 'online',
      paused: false,
      pauseReasons: [],
      positionS: 92,
      durationS: 213,
    };
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
    const item = (id, title) => ({ id, title, durationS: 232, requesterName: 'Tomek', requesterBadge: null, isLive: false, sourceUrl: `https://www.youtube.com/watch?v=${id}`, status: 'queued', addedBy: 'viewer', error: null, createdAt: 0, finishedAt: null });
    return {
      status: s.playing === 'none' ? 'idle' : s.paused ? 'paused' : 'playing',
      current,
      paused: s.paused,
      pauseReasons: s.pauseReasons,
      prefetchedItemId: null,
      playing: current ? [{ ...item(7, current.title), status: 'playing', sourceUrl: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ' }] : [],
      queue: [item(8, 'Darude — Sandstorm')],
      pending: Array.from({ length: s.pending }, (_, i) => ({ ...item(20 + i, 'Nyan Cat'), status: 'pending_approval' })),
      history: [],
      settings: {},
      control: control(),
      channelUrl: `http://127.0.0.1:${port}/live/main.m3u8`,
      requestUrl: `http://127.0.0.1:${port}/`,
    };
  }

  function info() {
    return {
      version: '0.2.0',
      port,
      publicUrl: '',
      language: s.language,
      tunnelEnabled: s.tunnel !== 'off',
      tunnel: { state: s.tunnel, url: s.tunnel === 'online' ? 'https://proba-kanalu.trycloudflare.com' : null, message: null, mode: 'quick', hostname: null, warning: null },
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
    if (path.startsWith('/static/')) return serveFile(res, decodeURIComponent(path.slice('/static/'.length)));
    if (path === '/__test/requests') return json(res, 200, requests);
    if (path === '/__test/reset') {
      reset();
      return json(res, 200, { ok: true });
    }
    if (path === '/__test/scenario') {
      Object.assign(s, await body(req));
      return json(res, 200, s);
    }
    if (!path.startsWith('/api/')) return json(res, 404, { error: 'Not Found' });

    const payload = req.method === 'GET' ? null : await body(req).catch(() => null);
    requests.push({ method: req.method, path, body: payload, lang: req.headers['x-vrcinema-lang'] ?? null, at: Date.now() });
    if (path === '/api/desktop/info' || path === '/api/desktop/config') {
      if (s.signedOut) return coded(res, 401, 'unauthorized', 'Sign in');
      if (s.remote) return coded(res, 403, 'window_only', 'Window only');
      if (req.method === 'PUT') {
        if (payload?.language) s.language = payload.language;
        return json(res, 200, { restarted: false });
      }
      return json(res, 200, info());
    }
    if (s.signedOut) return coded(res, 401, 'unauthorized', 'Sign in');
    const base = '/api/admin/channels/main';
    if (path === base && req.method === 'GET') return json(res, 200, panel());
    if (req.method !== 'POST') return json(res, 404, { error: 'Not Found' });
    const action = path.slice(base.length + 1);
    if (action === 'skip') return json(res, 200, { ok: true });
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
  const { port: bound } = await startFakeEngine(port);
  console.log(`fake engine on http://127.0.0.1:${bound}/app`);
}
