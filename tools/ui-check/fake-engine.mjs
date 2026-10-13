// A stand-in for the engine for browser checks of the interface (no ffmpeg, no channel): it
// serves public/ like the engine (/static/…, /admin) and answers the panel's API from a scenario
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
// First-run wizard: GET /wizard serves public/wizard.html; scenario `firstRun` (info.firstRunCompleted is its opposite; POST /api/desktop/wizard/complete clears it), `tunnelOnEnable` (state the tunnel takes when switched on: online (default), connecting, reconnecting, error), GET /api/desktop/tunnel; `configFail`: PUT /api/desktop/config answers 500.
// Stats route (GET /api/admin/channels/main/stats): scenario `stats` is 'full' (default) or
// 'nulls' (every group null, as when nothing plays) or 'partial' (the full one with a source analysis lacking height, frame rate and length). The preview's stream (/live/main.m3u8
// and its segments) comes from `options.hlsDir` (a folder with main.m3u8 and .ts files; the
// playlist is served without EXT-X-ENDLIST so the player treats it as live); the scenario
// `hls: false` makes it 404.
// Sign-in (task 15): POST /api/admin/login with the password `tajne-haslo` signs in (clears
// `signedOut`; a wrong one 401 `wrong_password`, scenario `remotePanelOff` 403
// `remote_panel_off`, messages in the request's language for en/pl); POST /api/admin/logout signs
// out. PUT /api/admin/tunnel {enabled} switches the tunnel (also remote; 404 `no_tunnel` headless).
// Scenario `sessionViaTunnel` (environment flag), `channelDown` (channelRunning false with an
// error), `restarting`.
// Settings view: GET /api/admin/environment ({window, encoder, tickerSlow, language,
// sessionViaTunnel, publicUrl, tunnelEnabled, tunnel, channelRunning, channelError}; scenario `headless`: no window — the window routes answer 403 like for a remote session, window false; `tickerSlow`, `encoder`), GET/PUT /api/desktop/config (validated like the engine; a new port or lanAccess answers restarted: true), tools (GET /api/desktop/tools, POST install|update|repair then GET progress; scenario `toolsMissing`), POST encoder/detect, pick-cookies (scenario `pickCookies`), GET log, GET update (scenario `updateOffer` {version, notes}, `toolsFail`, `infoFail`: the info route answers 500), POST update/apply, POST restart; GET/PUT /api/admin/channels/main/settings (validated like the engine: ticker messages <= 10 x <= 200 chars, intervals 60/120/300/600), GET /api/admin/nicks and POST nicks/{skeleton}/release, devices/block, devices/unblock.

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

  const initialConfig = () => ({
    port,
    lanAccess: false,
    publicUrl: '',
    remoteAdmin: false,
    hasPassword: false,
    videoEncoder: 'auto',
    detectedEncoder: null,
    ytdlpCookies: null,
    pixeldrainApiKey: null,
    autostart: false,
    trustCloudflare: false,
    firstRunCompleted: true,
    toolsInstalled: true,
    skippedUpdateVersion: null,
    pauseOnTunnelLoss: true,
    pauseWhenNoViewers: false,
  });
  const initialSettings = () => ({
    linkMode: 'all',
    allowedDomains: ['youtube.com', 'youtu.be'],
    blockedDomains: [],
    maxDurationS: 7200,
    requestLimitCount: 3,
    requestLimitWindowS: 600,
    maxActivePerRequester: 5,
    moderation: false,
    forceTranscode: false,
    discontinuityTags: true,
    slateLanguage: 'en',
    tickerEnabled: true,
    tickerMode: 'interval',
    tickerIntervalS: 60,
    tickerSpeed: 'normal',
    tickerShowNext: true,
    tickerShowAddress: false,
    tickerQr: 'with_ticker',
    tickerProgress: true,
    tickerMessages: ['Welcome to the Friday show!'],
  });
  const POPULAR = ['youtube.com', 'youtu.be', 'twitch.tv', 'kick.com', 'vimeo.com', 'pixeldrain.com', 'cda.pl'];
  const initialNicks = () => [
    { skeleton: 'kasia', nick: 'Kasia', badge: { hue: 40, shape: 3 }, lastUsedAt: Date.now() - 60000, blocked: false, blockId: null },
    { skeleton: 'a/b?c#d%e', nick: 'A/B?C#D%E', badge: { hue: 200, shape: 8 }, lastUsedAt: Date.now() - 7200000, blocked: false, blockId: null },
    { skeleton: 'troll', nick: 'Troll', badge: { hue: 10, shape: 5 }, lastUsedAt: Date.now() - 86400000, blocked: true, blockId: 1 },
  ];
  const initialTools = () => [
    { name: 'yt-dlp', installed: true, version: '2025.10.1', installedAt: 0 },
    { name: 'ffmpeg', installed: true, version: '7.1', installedAt: 0 },
    { name: 'deno', installed: true, version: '2.0.0', installedAt: 0 },
    { name: 'cloudflared', installed: true, version: '2025.9.0', installedAt: 0 },
  ];

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
      firstRun: false,
      configFail: false,
      tunnelOnEnable: 'online',
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
      headless: false,
      infoFail: false,
      toolsFail: false,
      tickerSlow: false,
      encoder: 'h264_nvenc',
      toolsMissing: [],
      updateOffer: null,
      pickCookies: 'C:/cookies.txt',
      remotePanelOff: false,
      sessionViaTunnel: false,
      channelDown: false,
    };
    s.queue = initialQueue();
    s.named = { mode: 'quick', host: null, loggedIn: false, progress: idleProgress() };
    s.rotations = 0;
    s.history = initialHistory();
    s.config = initialConfig();
    s.settings = initialSettings();
    s.nicks = initialNicks();
    s.blocks = [{ id: 1, nick: 'Troll', badge: { hue: 10, shape: 5 }, blockedAt: Date.now() - 3600000 }];
    s.tools = initialTools();
    s.toolsJob = null;
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
      firstRunCompleted: !s.firstRun,
      tunnelEnabled: s.tunnel !== 'off',
      tunnel: tunnelView(),
      paused: s.paused,
      pauseReasons: s.pauseReasons,
      toolsOk: s.toolsMissing.length === 0,
      missingTools: s.toolsMissing,
      toolsRunning: Boolean(s.toolsJob?.running),
      channelRunning: !s.channelDown,
      channelError: s.channelDown ? 'ffmpeg exited (code 1)' : null,
      restarting: false,
      sessionViaTunnel: false,
      encoder: s.encoder,
      update: s.updateOffer,
      tickerSlow: s.tickerSlow,
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

  const configView = () => ({
    ...s.config,
    language: s.language,
    tunnelEnabled: s.tunnel !== 'off',
    tunnelMode: s.named.mode,
    namedTunnelHostname: s.named.host,
  });

  function putConfig(res, patch) {
    const bad = (code, message) => coded(res, 400, code, message);
    if (s.configFail) return coded(res, 500, 'internal', 'Could not save the settings.');
    if ('port' in patch && !(Number.isInteger(patch.port) && patch.port >= 1024 && patch.port <= 65535)) return bad('invalid_port', 'The port must be a number from 1024 to 65535.');
    if (typeof patch.password === 'string' && [...patch.password].length < 8) return bad('password_too_short', 'The password must have at least 8 characters.');
    if ('ytdlpCookies' in patch && patch.ytdlpCookies !== null) return bad('cookies_by_picker', 'Choose the cookies file in the file picker.');
    if (typeof patch.publicUrl === 'string' && patch.publicUrl.trim() && !/^https?:\/\//.test(patch.publicUrl.trim())) return bad('invalid_public_url', 'The public address must start with http:// or https://.');
    const restarted = ('port' in patch && patch.port !== s.config.port) || ('lanAccess' in patch && patch.lanAccess !== s.config.lanAccess);
    for (const key of Object.keys(s.config)) {
      if (!(key in patch)) continue;
      s.config[key] = key === 'publicUrl' ? String(patch[key]).trim() : key === 'pixeldrainApiKey' ? (String(patch[key] ?? '').trim() || null) : patch[key];
    }
    if (typeof patch.password === 'string') s.config.hasPassword = true;
    if (patch.language) s.language = patch.language;
    if (patch.tunnelMode) s.named.mode = patch.tunnelMode;
    if ('tunnelEnabled' in patch) s.tunnel = patch.tunnelEnabled ? (s.tunnelOnEnable ?? 'online') : 'off';
    return json(res, 200, { ...configView(), restarted });
  }

  const INTERVALS = [60, 120, 300, 600];
  function settingsRoute(res, req, patch) {
    if (req.method === 'GET') return json(res, 200, { ...s.settings, popularDomains: POPULAR });
    if (req.method !== 'PUT') return json(res, 404, { error: 'Not Found' });
    const fail = (code, message) => coded(res, 400, code, message);
    const next = { ...s.settings };
    for (const [key, value] of Object.entries(patch ?? {})) {
      if (!(key in s.settings)) return fail('unknown_setting', `Unknown setting: ${key}`);
      if (key === 'tickerMessages') {
        if (!Array.isArray(value)) return fail('setting_not_list', `${key}: expected a list`);
        const list = value.map((m) => String(m).trim()).filter(Boolean);
        if (list.some((m) => [...m].length > 200)) return fail('ticker_message_too_long', `${key}: a message can have at most 200 characters`);
        if (list.length > 10) return fail('ticker_too_many_messages', `${key}: at most 10 messages`);
        next[key] = list;
      } else if (key === 'tickerIntervalS') {
        if (!INTERVALS.includes(value)) return fail('setting_not_one_of', `${key}: expected one of: 60, 120, 300, 600`);
        next[key] = value;
      } else if (key === 'allowedDomains' || key === 'blockedDomains') {
        next[key] = value.map((d) => String(d).trim().toLowerCase().replace(/^https?:\/\//, '').replace(/^www\./, '').split('/')[0]).filter(Boolean);
      } else next[key] = value;
    }
    s.settings = next;
    return json(res, 200, next);
  }

  const ENCODER_RESULT = 'h264_nvenc';
  // Window-only routes of the Settings view; answers true when it handled the request.
  async function desktopRoute(res, req, path, payload) {
    const lang = req.headers['x-vrcinema-lang'];
    const done = (status, body) => (json(res, status, body), true);
    if (path === '/api/desktop/tools' && req.method === 'GET') {
      return done(200, { tools: s.tools, missing: s.toolsMissing, ok: s.toolsMissing.length === 0, running: Boolean(s.toolsJob?.running) });
    }
    const job = path.match(/^\/api\/desktop\/tools\/(install|update|repair)$/);
    if (job && req.method === 'POST') {
      if (s.toolsJob?.running) return (coded(res, 409, 'tools_running', 'The tools are already being downloaded.'), true);
      const names = job[1] === 'update' ? [payload?.tool] : ['yt-dlp', 'ffmpeg', 'deno', 'cloudflared'];
      s.toolsJob = { running: true, done: false, names, at: 0, error: null };
      const tick = setInterval(() => {
        const j = s.toolsJob;
        if (!j) return clearInterval(tick);
        j.at += 1;
        if (j.at >= j.names.length * 3) {
          j.running = false;
          j.done = true;
          if (s.toolsFail) j.error = 'Download failed: https://example.invalid/tool.zip';
          else s.toolsMissing = [];
          clearInterval(tick);
        }
      }, 600);
      return done(202, { started: true });
    }
    if (path === '/api/desktop/tools/progress' && req.method === 'GET') {
      const j = s.toolsJob;
      if (!j) return done(200, { phase: null, tool: null, downloaded: 0, total: null, error: null, done: false, running: false });
      const step = Math.min(j.at, j.names.length * 3 - 1);
      const phase = ['download', 'verify', 'install'][step % 3];
      return done(200, { phase, tool: j.names[Math.floor(step / 3)], downloaded: phase === 'download' ? 20000000 : 40000000, total: 40000000, error: j.done ? j.error : null, done: j.done, running: j.running });
    }
    if (path === '/api/desktop/encoder/detect' && req.method === 'POST') {
      s.config.detectedEncoder = ENCODER_RESULT;
      return done(200, { detectedEncoder: ENCODER_RESULT, encoder: ENCODER_RESULT });
    }
    if (path === '/api/desktop/pick-cookies' && req.method === 'POST') {
      if (s.pickCookies) s.config.ytdlpCookies = s.pickCookies;
      return done(200, { picked: Boolean(s.pickCookies), ytdlpCookies: s.config.ytdlpCookies });
    }
    if (path === '/api/desktop/log' && req.method === 'GET') {
      const lines = lang === 'pl' ? ['VRCinema 0.2.0: start', 'Tunel: w sieci — https://proba-kanalu.trycloudflare.com'] : ['VRCinema 0.2.0: start', 'Tunnel: online — https://proba-kanalu.trycloudflare.com'];
      return done(200, { lines });
    }
    if (path === '/api/desktop/update' && req.method === 'GET') return done(200, { current: '0.2.0', available: s.updateOffer });
    if (path === '/api/desktop/update/apply' && req.method === 'POST') return done(200, { ok: true, version: s.updateOffer?.version ?? '0.2.0' });
    if (path === '/api/desktop/restart' && req.method === 'POST') return done(200, { ok: true, port });
    return false;
  }

  const server = createServer(async (req, res) => {
    const url = new URL(req.url, `http://127.0.0.1:${port}`);
    const path = url.pathname;
    if (path === '/admin') return serveFile(res, 'app.html');
    if (path === '/wizard') return serveFile(res, 'wizard.html');
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
    const noWindow = s.remote || s.headless;
    if (path === '/api/admin/login' && req.method === 'POST') {
      const pl = req.headers['x-vrcinema-lang'] === 'pl';
      if (s.remotePanelOff) return coded(res, 403, 'remote_panel_off', pl ? 'Panel zdalny jest wyłączony.' : 'The remote panel is switched off.');
      if (payload?.password !== 'tajne-haslo') return coded(res, 401, 'wrong_password', pl ? 'Złe hasło.' : 'Wrong password.');
      s.signedOut = false;
      return json(res, 200, { ok: true });
    }
    if (path === '/api/admin/logout' && req.method === 'POST') {
      s.signedOut = true;
      return json(res, 200, { ok: true });
    }
    if (path === '/api/desktop/info' || path === '/api/desktop/config') {
      if (s.signedOut) return coded(res, 401, 'unauthorized', 'Sign in');
      if (noWindow) return coded(res, 403, 'window_only', 'Window only');
      if (s.infoFail && path === '/api/desktop/info') return coded(res, 500, 'internal', 'boom');
      if (req.method === 'PUT') return putConfig(res, payload);
      if (path === '/api/desktop/config') return json(res, 200, configView());
      return json(res, 200, info());
    }
    if (s.signedOut) return coded(res, 401, 'unauthorized', 'Sign in');
    if (path === '/api/admin/environment') {
      const app = !s.headless;
      return json(res, 200, {
        window: app,
        encoder: s.encoder,
        tickerSlow: s.tickerSlow,
        language: s.language,
        sessionViaTunnel: app && s.sessionViaTunnel && s.tunnel !== 'off',
        publicUrl: '',
        tunnelEnabled: app ? s.tunnel !== 'off' : null,
        tunnel: app ? tunnelView() : null,
        channelRunning: app ? !s.channelDown : null,
        channelError: app && s.channelDown ? 'ffmpeg exited (code 1)' : null,
      });
    }
    if (path === '/api/admin/tunnel' && req.method === 'PUT') {
      if (typeof payload?.enabled !== 'boolean') return coded(res, 400, 'field_not_boolean', 'The field enabled must be true or false.');
      if (s.headless) return coded(res, 404, 'no_tunnel', 'This server has no built-in tunnel.');
      s.tunnel = payload.enabled ? (s.tunnelOnEnable ?? 'online') : 'off';
      return json(res, 200, { tunnelEnabled: s.tunnel !== 'off', tunnel: tunnelView() });
    }
    if (path.startsWith('/api/desktop/')) {
      if (noWindow) return coded(res, 403, 'window_only', 'Window only');
      if (await desktopRoute(res, req, path, payload)) return undefined;
      if (path === '/api/desktop/tunnel/qr') return sendQr(res);
      if (path === '/api/desktop/tunnel' && req.method === 'GET') return json(res, 200, tunnelView());
      if (path === '/api/desktop/wizard/complete' && req.method === 'POST') {
        s.firstRun = false;
        return json(res, 200, { ok: true });
      }
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
    if (path === '/api/admin/channels/main/settings') return settingsRoute(res, req, payload);
    if (path === '/api/admin/nicks' && req.method === 'GET') return json(res, 200, { nicks: s.nicks, blocked: s.blocks });
    const nickRoute = path.match(/^\/api\/admin\/nicks\/(.+)\/release$/);
    if (nickRoute && req.method === 'POST') {
      const skeleton = decodeURIComponent(nickRoute[1]);
      if (!s.nicks.some((n) => n.skeleton === skeleton)) return coded(res, 404, 'nick_not_found', 'No such nick.');
      s.nicks = s.nicks.filter((n) => n.skeleton !== skeleton);
      return json(res, 200, { ok: true });
    }
    if (path === '/api/admin/devices/block' && req.method === 'POST') {
      const nick = s.nicks.find((n) => n.skeleton === payload?.skeleton);
      if (!nick) return coded(res, 404, 'nick_not_found', 'No such nick.');
      const id = Math.max(0, ...s.blocks.map((x) => x.id)) + 1;
      s.blocks.unshift({ id, nick: nick.nick, badge: nick.badge, blockedAt: Date.now() });
      nick.blocked = true;
      nick.blockId = id;
      return json(res, 200, { ok: true });
    }
    if (path === '/api/admin/devices/unblock' && req.method === 'POST') {
      if (!s.blocks.some((x) => x.id === payload?.id)) return coded(res, 404, 'block_not_found', 'This device is not blocked.');
      s.blocks = s.blocks.filter((x) => x.id !== payload.id);
      for (const n of s.nicks) if (n.blockId === payload.id) Object.assign(n, { blocked: false, blockId: null });
      return json(res, 200, { ok: true });
    }
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
  console.log(`fake engine on http://127.0.0.1:${bound}/admin`);
}
