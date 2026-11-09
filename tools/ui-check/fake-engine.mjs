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
// Files on disk (final wave I3): GET /api/admin/library?root=&path= and POST
// /api/admin/channels/main/local {root, path} like the engine's MEDIA_DIRS library; scenario
// `library`: null (switched off: {enabled: false}) or [{name, tree}] where a tree maps a folder
// name to its tree and a file name to its size in bytes; `libraryAddError`: the code POST …/local
// answers with.
// Request page (task 16): GET / serves public/index.html; GET /api/channels/main/state and POST /api/channels/main/requests (scenario viewerError, viewerNickTaken, viewerKnown; settings linkMode / allowedDomains / moderation).
// Nick PIN: POST /api/channels/main/nick/protect and …/nick/adopt (scenario viewerPins, viewerCodes), POST /api/admin/nicks/{skeleton}/reset-pin.
// Viewer lists: POST /api/channels/main/request-list (settings viewerLists / viewerListMax, scenario listDelayMs), the panel's pendingLists and POST /api/admin/channels/main/lists/{id}.
// GET /__test/stats: {maxPanelInFlight, maxViewerInFlight, maxMonitorInFlight}; GET /__test/hang never answers.
// The producer's monitor (window only, 403 for a remote session or without a window; requests
// record their query as `search`; maxMonitorInFlight counts the reads without `from`): GET
// /api/desktop/monitor.m3u8[?from=<row>] lists scenario `monitorFeed` ({clips: [{key, itemId,
// baseS, segs: [segment numbers of the HLS folder], src: 'a' (options.hlsDir) or 'b'
// (options.hlsDir2)}], firstSeq, antenna, aired}; null: an empty list, as without a film) like the
// engine (2 s rows, #VRCINEMA-AT/ANTENNA/AIRED, EXT-X-START for `from`); GET
// /api/desktop/monitor/<clip>/<n>.ts serves its segments; `monitorDelayMs` delays the list;
// `monitorPlayerFail`: the list loads of the player (with `from`) answer 500.
// The Preview's own film (window only): GET /api/desktop/preview?item=<id> answers scenario
// `previewSource[id]` ('ready' by default, 'preparing', 'unavailable') and
// /api/desktop/preview/<id>/media serves `options.previewFile` with Range. `itemId` is the film
// the controls act on (POST …/play-now switches it, position 0); `seekPending` the control flag;
// the panel carries `air` (`airLatencyS`). GET /__test/live lists the /live requests with
// their X-VRCinema-Air header. GET /app-icon.png serves desktop/icons/icon.png.
// Controls: `controlDelayMs` delays the answers of …/pause, …/resume and …/seek; with `armPause`
// a pause with `atS` only arms (`pausePending`, like the engine before the segment end).
// Scenario `viewerStateDelayMs`: GET /api/channels/main/state answers that late (setting it
// starts a new count of maxViewerInFlight).
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
// out. The program's routes for the remote panel (fix round 1): /api/admin/{info, config, tools…,
// encoder/detect, log, update…, restart, tunnel, tunnel/restart, tunnel/named/quick}
// answer like the window's (a remote session may PUT only the engine's allowlisted settings, else
// 403 `window_only`; a server without a window: Fastify's 404). Scenario `sessionViaTunnel`
// (the session came through the tunnel: switching it off, reconnecting or changing the link
// type through the panel routes applies the change and then drops the connection, as the real
// tunnel would), `channelDown` (channelRunning false with an error).
// Settings view: GET /api/admin/environment ({window, encoder, tickerSlow, language,
// sessionViaTunnel, publicUrl, tunnelEnabled, tunnel, channelRunning, channelError}; scenario `headless`: no window — the window routes answer 403 like for a remote session, window false; `tickerSlow`, `encoder`), GET/PUT /api/desktop/config (validated like the engine; a new port or lanAccess answers restarted: true), tools (GET /api/desktop/tools, POST install|update|repair then GET progress; scenario `toolsMissing`), POST encoder/detect, pick-cookies (scenario `pickCookies`), GET log, GET update (scenario `updateOffer` {version, notes}, `toolsFail`, `infoFail`: the info route answers 500), POST update/apply, POST restart; GET/PUT /api/admin/channels/main/settings (validated like the engine: ticker messages <= 10 x <= 200 chars, intervals 60/120/300/600; the Advanced keys too), GET/DELETE /api/admin/channels/main/journal and GET …/journal/export (scenario `journal`), `alarms` {black, silence} in the info and the environment, GET /api/admin/nicks and POST nicks/{skeleton}/release, devices/block, devices/unblock.

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

export function startFakeEngine(port = 8193, { hlsDir = null, hlsDir2 = null, previewFile = null } = {}) {
  if (port === 8080) throw new Error('never port 8080');
  let requests = [];
  /** When the live list started to grow (its first request after a reset). */
  let liveStartedAt = null;
  let uploads = [];
  let liveRequests = [];
  let monitorInFlight = 0;
  let maxMonitorInFlight = 0;
  // Rows the engine still keeps after they left the list (it serves them for a while).
  let monitorKept = [];
  let s;
  let nextId;
  let panelInFlight = 0;
  let maxPanelInFlight = 0;
  let viewerInFlight = 0;
  let maxViewerInFlight = 0;
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
    obsLan: false,
    obsPriority: true,
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
    slateFrameStyle: 'dim',
    slateBackgroundAll: false,
    oscEnabled: false,
    oscHost: '127.0.0.1',
    oscPort: 9000,
    oscTemplate: '▶ {title} — {added}',
    oscRepeatS: 0,
    audioVisual: 'auto',
    tickerEnabled: true,
    tickerMode: 'interval',
    tickerIntervalS: 60,
    tickerSpeed: 'normal',
    tickerShowNext: true,
    tickerShowAddress: false,
    tickerQr: 'with_ticker',
    tickerProgress: true,
    tickerMessages: ['Welcome to the Friday show!'],
    viewerLists: false,
    viewerListMax: 10,
    viewerUploads: false,
    viewerUploadMaxMb: 50,
    viewerUploadMaxS: 600,
    viewerUploadAudioMaxS: 600,
    viewerUploadQuotaMb: 2048,
    uploadMbps: 0,
    transitionsEnabled: false,
    transitionS: 1,
    peakMbps: 0,
    peakBufferS: 2,
    subtitlesEnabled: false,
    subtitlesLanguage: 'en',
    loudnessEnabled: false,
    loudnessTarget: -16,
    alarmsEnabled: false,
    alarmBlackS: 10,
    alarmSilenceS: 15,
    alarmSilenceDb: -50,
    maxHeight: 0,
  });
  const POPULAR = ['youtube.com', 'youtu.be', 'twitch.tv', 'kick.com', 'vimeo.com', 'pixeldrain.com', 'cda.pl'];
  const initialNicks = () => [
    { skeleton: 'kasia', nick: 'Kasia', badge: { hue: 40, shape: 3 }, lastUsedAt: Date.now() - 60000, blocked: false, blockId: null, protected: true, devices: 2 },
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
    liveStartedAt = null;
    requests = [];
    uploads = [];
    liveRequests = [];
    s = {
      itemId: 7,
      previewSource: {},
      airLatencyS: 12,
      seekPending: false,
      language: 'en',
      playing: 'film',
      canSeek: true,
      positionApprox: false,
      pending: 0,
      addError: null,
      viewerError: null,
      viewerNickTaken: [],
      viewerKnown: [],
      viewerPins: {},
      viewerCodes: {},
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
      alarms: { black: false, silence: false },
      journal: [],
      encoder: 'h264_nvenc',
      toolsMissing: [],
      updateOffer: null,
      pickCookies: 'C:/cookies.txt',
      remotePanelOff: false,
      sessionViaTunnel: false,
      channelDown: false,
      library: null,
      libraryAddError: null,
      monitorFeed: null,
      monitorDelayMs: 0,
      monitorPlayerFail: false,
      controlDelayMs: 0,
      armPause: false,
      pausePending: false,
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
    s.pendingLists = [];
    nextId = 100;
    maxPanelInFlight = 0;
    maxViewerInFlight = 0;
    maxMonitorInFlight = 0;
  }
  reset();

  function control() {
    const film = s.playing === 'film';
    const live = s.playing === 'live';
    return {
      itemId: s.playing === 'none' ? null : s.itemId,
      isLive: live,
      positionS: film ? s.positionS : live ? 40 : 0,
      durationS: film ? s.durationS : null,
      canPause: film,
      canSeek: film && s.canSeek,
      seekPending: s.seekPending,
      positionApprox: s.positionApprox,
      pausePending: s.pausePending,
      paused: s.paused,
      pauseReasons: s.pauseReasons,
    };
  }

  // The queue tools (POST clear | remove-requester | undo | play-next), like the engine: a bulk
  // removal takes the queue and the approvals (the film on air with includePlaying) and can be
  // undone once. A viewer is the same nick and icon (the fake has no device keys); the film on
  // air is Kasia's. Answers undefined for any other action.
  function queueTools(res, action, payload) {
    const send = (status, body) => (json(res, status, body), true);
    const fail = (status, code, error) => (coded(res, status, code, error), true);
    const onAir = { requesterName: 'Kasia', requesterBadge: { hue: 40, shape: 3 } };
    const same = (a, b) =>
      a.requesterBadge !== 'host' && (a.requesterName || a.requesterBadge) &&
      a.requesterName === b.requesterName && JSON.stringify(a.requesterBadge) === JSON.stringify(b.requesterBadge);
    const take = (keep, includePlaying, block = false) => {
      const removed = { queue: s.queue.filter((i) => !keep(i)), pending: s.pendingItems.filter((i) => !keep(i)), playing: null };
      s.queue = s.queue.filter(keep);
      s.pendingItems = s.pendingItems.filter(keep);
      if (includePlaying && s.playing !== 'none') {
        removed.playing = s.playing;
        s.playing = 'none';
      }
      const count = removed.queue.length + removed.pending.length + (removed.playing ? 1 : 0);
      const undo = count || block ? (s.nextUndo = (s.nextUndo ?? 0) + 1) : null;
      if (undo) (s.undos ??= new Map()).set(undo, removed);
      return send(200, { removed: count, blocked: block, undo });
    };
    if (action === 'clear') return take(() => false, payload?.includePlaying === true);
    if (action === 'remove-requester') {
      const item = [...s.queue, ...s.pendingItems].find((i) => i.id === payload?.itemId);
      if (!item) return fail(404, 'no_such_item', 'No such item');
      if (item.requesterBadge === 'host' || !(item.requesterName || item.requesterBadge)) return fail(409, 'no_requester', 'No viewer');
      const canBlock = item.requesterBadge !== null;
      const mine = (i) => same(i, item);
      const playing = s.playing !== 'none' && same(onAir, item) ? { itemId: 7, title: panel().current.title } : null;
      if (payload?.dryRun) {
        return send(200, {
          name: item.requesterName, queued: s.queue.filter(mine).length, pending: s.pendingItems.filter(mine).length,
          playing, canBlock, blocked: false,
        });
      }
      if (payload?.block && !canBlock) return fail(409, 'cannot_block', 'Cannot block');
      return take((i) => !mine(i), Boolean(playing) && payload?.includePlaying === true, payload?.block === true);
    }
    if (action === 'undo') {
      const removed = s.undos?.get(payload?.undo);
      if (!removed) return fail(404, 'undo_expired', 'It is too late to undo this.');
      s.undos.delete(payload.undo);
      s.queue = [...removed.queue, ...s.queue];
      s.pendingItems = [...removed.pending, ...s.pendingItems];
      if (removed.playing) s.playing = removed.playing;
      return send(200, { restored: removed.queue.length + removed.pending.length + (removed.playing ? 1 : 0) });
    }
    // Broadcast: in/out points and the start time of a queued video (recorded for the checks).
    if (action === 'trim' || action === 'schedule') {
      const item = s.queue.find((i) => i.id === payload?.itemId);
      if (!item) return fail(404, 'no_such_item', 'No such item');
      (s.broadcastCalls ??= []).push({ action, ...payload });
      if (action === 'trim') {
        item.inS = payload.inS ?? null;
        item.outS = payload.outS ?? null;
        return send(200, { inS: item.inS, outS: item.outS });
      }
      item.startAt = payload.startAt ?? (Number.isFinite(payload.afterS) ? Date.now() + payload.afterS * 1000 : null);
      return send(200, { startAt: item.startAt });
    }
    if (action === 'play-next') {
      const inQueue = s.queue.findIndex((i) => i.id === payload?.itemId);
      if (inQueue >= 0) {
        s.queue.unshift(...s.queue.splice(inQueue, 1));
        return send(200, { itemId: payload.itemId });
      }
      const old = s.history.find((i) => i.id === payload?.itemId);
      if (!old) return fail(409, 'not_playable_next', 'Not playable next');
      const again = { ...old, id: nextId++, status: 'queued', error: null, finishedAt: null };
      s.queue.unshift(again);
      return send(200, { itemId: again.id });
    }
    return undefined;
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
      status: s.countdown && s.playing === 'none' ? 'countdown' : s.playing === 'none' ? 'idle' : s.paused ? 'paused' : 'playing',
      current,
      paused: s.paused,
      pauseReasons: s.pauseReasons,
      prefetchedItemId: null,
      // Broadcast: how ready the next video is, and the viewers' need against the upload.
      next: s.next ?? null,
      link: s.link ?? null,
      // Scenario `obs`: the OBS stream's state (off, waiting, live, brb).
      obs: { state: s.obs ?? 'off' },
      playing: current ? [{ ...item(7, current.title), status: 'playing', sourceUrl: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ' }] : [],
      queue: s.queue,
      pending: s.pendingItems,
      pendingLists: s.pendingLists
        .map((l) => ({ ...l, itemIds: l.itemIds.filter((id) => s.pendingItems.some((i) => i.id === id)) }))
        .filter((l) => l.itemIds.length >= 2),
      history: s.history,
      settings: {},
      control: control(),
      // The engine's air clock: a typical viewer `airLatencyS` behind the edge, 12 s of film
      // behind the server's position.
      air: { latencyS: s.airLatencyS, source: 'nominal', viewers: 0, itemId: current ? s.itemId : null, positionS: s.playing === 'film' ? s.positionS - 12 : null, segmentS: 4 },
      channelUrl: `http://127.0.0.1:${port}/live/main.m3u8`,
      requestUrl: `http://127.0.0.1:${port}/`,
    };
  }

  // The request page's routes. GET …/state is the public state (queue without links; `rules`
  // from the channel settings). POST …/requests answers like the engine: a nick needs a device
  // key (400 invalid_device_key), `viewerNickTaken` lists skeletons held by another device
  // (409 nick_taken), `viewerError` is a code to refuse every request with (400, 429 for
  // too_many_active / too_many_requests), `viewerKnown` the keys that already own a nick.
  const publicItem = (i) => ({ id: i.id, title: i.title, durationS: i.durationS, requesterName: i.requesterName, requesterBadge: i.requesterBadge, isLive: i.isLive });
  function viewerState() {
    const p = panel();
    return {
      status: p.status,
      current: p.current,
      paused: p.paused,
      pauseReasons: p.pauseReasons,
      queue: s.queue.map(publicItem),
      channelUrl: p.channelUrl,
      requestUrl: p.requestUrl,
      rules: { linkMode: s.settings.linkMode, allowedDomains: s.settings.allowedDomains, maxDurationS: s.settings.maxDurationS, moderation: s.settings.moderation, viewerLists: s.settings.viewerLists, viewerListMax: s.settings.viewerListMax,
        uploads: s.settings.viewerUploads ? { maxMb: s.settings.viewerUploadMaxMb, maxDurationS: Math.min(s.settings.maxDurationS, s.settings.viewerUploadMaxS), maxAudioS: Math.min(s.settings.maxDurationS, s.settings.viewerUploadAudioMaxS) } : null },
    };
  }

  // Viewer lists (POST …/request-list) like the engine: 403 viewer_lists_off with the setting
  // off; each line is a link (a line with "playlist" expands to three videos), a non-link is
  // invalid_link, a 192.168. host local_address, a repeated one duplicate_link, beyond
  // viewerListMax list_full. Scenario `listDelayMs` delays the answer.
  const LIST_REASONS = {
    invalid_link: ['That does not look like a valid link (http/https).', 'To nie wygląda na poprawny link (http/https).'],
    local_address: ['Links to a local network or to this computer are not allowed.', 'Linki do sieci lokalnej lub do tego komputera są niedozwolone.'],
    duplicate_link: ['This video is already in your list.', 'Ten film już jest na twojej liście.'],
    list_full: ['Over the limit of videos in one list.', 'Ponad limit filmów na jedną listę.'],
  };
  async function viewerList(res, req, payload) {
    const pl = req.headers['x-vrcinema-lang'] === 'pl';
    if (!s.settings.viewerLists) return coded(res, 403, 'viewer_lists_off', pl ? 'Ten kanał przyjmuje jeden link naraz. Wyślij pojedynczy link.' : 'This channel takes one link at a time. Send a single link.');
    const lines = Array.isArray(payload?.urls) ? payload.urls.map((u) => String(u).trim()).filter(Boolean) : [];
    if (!lines.length) return coded(res, 400, 'invalid_link', LIST_REASONS.invalid_link[pl ? 1 : 0]);
    if (s.listDelayMs) await sleep(s.listDelayMs);
    const key = payload.deviceKey ?? null;
    const nick = String(payload.name ?? '').trim();
    const badge = key ? { hue: parseInt(key.slice(0, 3), 16) % 360, shape: parseInt(key[3], 16) % 12 } : null;
    const pending = s.settings.moderation;
    const status = pending ? 'pending_approval' : 'queued';
    const results = [];
    const added = [];
    const seen = new Set();
    let title = null;
    const skip = (line, code, t = null) => results.push({ line, title: t, status: 'skipped', code, reason: LIST_REASONS[code][pl ? 1 : 0] });
    const add = (line, t) => {
      if (added.length >= s.settings.viewerListMax) return skip(line, 'list_full', t);
      const item = queued(nextId++, t, { requesterName: nick || null, requesterBadge: badge, status });
      added.push(item);
      results.push({ line, title: t, status });
    };
    lines.forEach((text, i) => {
      const line = i + 1;
      if (!/^https?:\/\//.test(text)) return skip(line, 'invalid_link');
      if (new URL(text).hostname.startsWith('192.168.')) return skip(line, 'local_address');
      if (seen.has(text)) return skip(line, 'duplicate_link');
      seen.add(text);
      if (text.includes('playlist')) {
        title = 'Fake playlist';
        for (let n = 1; n <= 3; n += 1) add(line, `Playlist video ${n}`);
      } else add(line, 'Added from ' + new URL(text).hostname);
    });
    if (pending) s.pendingItems.push(...added);
    else s.queue.push(...added);
    let listId = null;
    if (added.length) {
      listId = nextId++;
      s.pendingLists.push({ id: listId, title, itemIds: added.map((i) => i.id) });
    }
    const skipped = results.filter((r) => r.status === 'skipped').length;
    return json(res, 200, { status, added: added.length, skipped, listId, requesterBadge: badge, results });
  }
  const VIEWER_ERRORS = {
    domain_not_allowed: [400, 'This site is not allowed. Allowed: youtube.com, youtu.be.', 'Ten serwis nie jest dozwolony. Dozwolone: youtube.com, youtu.be.'],
    local_address: [400, 'Links to a local network or to this computer are not allowed.', 'Linki do sieci lokalnej lub do tego komputera są niedozwolone.'],
    too_many_active: [429, 'You already have 5 videos in the queue.', 'Masz już 5 filmów w kolejce.'],
    too_many_requests: [429, 'Too many requests in a short time. Try again in a few minutes.', 'Za dużo zgłoszeń w krótkim czasie. Spróbuj za kilka minut.'],
  };
  function viewerRequest(res, req, payload) {
    const pl = req.headers['x-vrcinema-lang'] === 'pl';
    const fail = (status, code, en, plText) => coded(res, status, code, pl ? plText : en);
    const refusal = VIEWER_ERRORS[s.viewerError];
    if (refusal) {
      const body = { error: pl ? refusal[2] : refusal[1], code: s.viewerError };
      if (s.viewerError === 'too_many_active') body.count = 5;
      return json(res, refusal[0], body);
    }
    if (!/^https?:\/\//.test(payload?.url ?? '')) return fail(400, 'invalid_link', 'That does not look like a valid link (http/https).', 'To nie wygląda na poprawny link (http/https).');
    const key = payload.deviceKey ?? null;
    const nick = String(payload.name ?? '').trim();
    if (key !== null && !/^[0-9a-f]{32}$/.test(key)) return fail(400, 'invalid_device_key', 'The device key must be 32 hexadecimal characters.', 'Klucz urządzenia musi mieć 32 znaki szesnastkowe.');
    if (nick && key === null) return fail(400, 'invalid_device_key', 'The device key must be 32 hexadecimal characters.', 'Klucz urządzenia musi mieć 32 znaki szesnastkowe.');
    if (nick && s.viewerNickTaken.includes(nick.toLowerCase()) && !s.viewerKnown.includes(key)) {
      return fail(409, 'nick_taken', 'This nick is already taken by someone else. Pick another one.', 'Ten nick jest już zajęty przez kogoś innego. Wybierz inny.');
    }
    if (nick && key && !s.viewerKnown.includes(key)) s.viewerKnown.push(key);
    const badge = key ? { hue: parseInt(key.slice(0, 3), 16) % 360, shape: parseInt(key[3], 16) % 12 } : null;
    const item = queued(nextId++, 'Added from ' + new URL(payload.url).hostname, { requesterName: nick || null, requesterBadge: badge });
    const pending = s.settings.moderation;
    if (pending) s.pendingItems.push({ ...item, status: 'pending_approval' });
    else s.queue.push(item);
    return json(res, 201, { status: pending ? 'pending_approval' : 'queued', item: publicItem(item) });
  }

  // A viewer's video or audio file (POST …/upload, raw body; nick, key and file name in
  // headers) like the engine: 403 uploads_off with the setting off, a body not starting like
  // an MP4, Matroska or ID3-tagged MP3 file 415 upload_not_video; else queued (or pending)
  // with the file name as the title. Scenario `uploadDelayMs` delays the answer. Each upload is kept in `uploads`.
  async function viewerUpload(req, res) {
    const pl = req.headers['x-vrcinema-lang'] === 'pl';
    const chunks = [];
    for await (const ch of req) chunks.push(ch);
    const bytes = Buffer.concat(chunks);
    const header = (name) => (req.headers[name] === undefined ? null : decodeURIComponent(req.headers[name]));
    const name = header('x-upload-name');
    const nick = (header('x-viewer-name') ?? '').trim();
    const key = header('x-device-key');
    uploads.push({ size: bytes.length, name, nick, key, lang: req.headers['x-vrcinema-lang'] ?? null });
    if (s.uploadDelayMs) await sleep(s.uploadDelayMs);
    if (!s.settings.viewerUploads) return coded(res, 403, 'uploads_off', pl ? 'Wysyłanie plików jest wyłączone na tym kanale.' : 'Sending files is turned off on this channel.');
    const head = bytes.subarray(0, 8).toString('latin1');
    if (head.slice(4, 8) !== 'ftyp' && !head.startsWith('\x1a\x45\xdf\xa3') && !head.startsWith('ID3')) {
      return coded(res, 415, 'upload_not_video', pl ? 'Ten plik nie jest filmem ani plikiem dźwiękowym, które kanał może odtworzyć (działają MP4, WebM, MKV, MOV, AVI, MP3, FLAC, Ogg, M4A, WAV i podobne).' : 'This file isn’t a video or audio file the channel can play (MP4, WebM, MKV, MOV, AVI, MP3, FLAC, Ogg, M4A, WAV and similar work).');
    }
    if (nick && !/^[0-9a-f]{32}$/.test(key ?? '')) return coded(res, 400, 'invalid_device_key', 'The device key must be 32 hexadecimal characters.');
    const badge = key ? { hue: parseInt(key.slice(0, 3), 16) % 360, shape: parseInt(key[3], 16) % 12 } : null;
    const title = String(name ?? 'Video').replace(/^.*[\\/]/, '').replace(/\.[^.]{1,5}$/, '') || 'Video';
    const item = queued(nextId++, title, { requesterName: nick || null, requesterBadge: badge, sourceUrl: `upload:${'0'.repeat(32)}.${head.startsWith('ID3') ? 'mp3' : 'mp4'}` });
    const pending = s.settings.moderation;
    if (pending) s.pendingItems.push({ ...item, status: 'pending_approval' });
    else s.queue.push(item);
    return json(res, 201, { status: pending ? 'pending_approval' : 'queued', item: publicItem(item) });
  }

  // Nick PIN (like the engine's routes, without hashing): `viewerPins` {nick: pin} and
  // `viewerCodes` {nick: recovery code} of the scenario; a PIN or code makes the key known.
  function viewerNickPin(res, kind, payload) {
    const key = payload?.deviceKey;
    const nick = String(payload?.name ?? '').trim();
    const name = nick.toLowerCase();
    if (!/^[0-9a-f]{32}$/.test(key ?? '')) return coded(res, 400, 'invalid_device_key', 'The device key must be 32 hexadecimal characters.');
    if (!nick) return coded(res, 400, 'nick_pin_no_nick', 'Type your nick first.');
    const badge = { hue: parseInt(key.slice(0, 3), 16) % 360, shape: parseInt(key[3], 16) % 12 };
    const valid = (p) => typeof p === 'string' && (/^\d{4,8}$/.test(p) || (!/^\d+$/.test(p) && [...p].length >= 8));
    const newCode = () => `CODE-${String(nextId++).padStart(4, '0')}-ABCD-EFGH`;
    const known = () => { if (!s.viewerKnown.includes(key)) s.viewerKnown.push(key); };
    if (kind === 'protect') {
      if (!valid(payload.pin)) return coded(res, 400, 'nick_pin_invalid', 'Use 4–8 digits, or a passphrase of at least 8 characters.');
      if (s.viewerNickTaken.includes(name) && !s.viewerKnown.includes(key)) return coded(res, 409, 'nick_taken', 'This nick is already taken by someone else. Pick another one.');
      if (s.viewerPins[name] !== undefined) {
        if (payload.currentPin === undefined) return coded(res, 409, 'nick_pin_required', 'This nick already has a PIN. Enter the current one to change it.');
        if (payload.currentPin !== s.viewerPins[name]) return coded(res, 403, 'nick_pin_wrong', 'This nick and PIN (or recovery code) don’t match.');
      }
      s.viewerPins[name] = payload.pin;
      s.viewerCodes[name] = newCode();
      if (!s.viewerNickTaken.includes(name)) s.viewerNickTaken.push(name);
      known();
      return json(res, 200, { nick, recoveryCode: s.viewerCodes[name], requesterBadge: badge });
    }
    const wrong = () => coded(res, 403, 'nick_pin_wrong', 'This nick and PIN (or recovery code) don’t match.');
    if (payload.recoveryCode !== undefined) {
      if (!valid(payload.newPin)) return coded(res, 400, 'nick_pin_invalid', 'Use 4–8 digits, or a passphrase of at least 8 characters.');
      if (!s.viewerCodes[name] || payload.recoveryCode.toUpperCase() !== s.viewerCodes[name]) return wrong();
      s.viewerPins[name] = payload.newPin;
      s.viewerCodes[name] = newCode();
      s.viewerKnown = [key];
      return json(res, 200, { nick: nick.charAt(0).toUpperCase() + nick.slice(1), recoveryCode: s.viewerCodes[name], requesterBadge: badge });
    }
    if (s.viewerPins[name] === undefined || payload.pin !== s.viewerPins[name]) return wrong();
    if (payload.signOutOthers) s.viewerKnown = [];
    known();
    return json(res, 200, { nick: nick.charAt(0).toUpperCase() + nick.slice(1), requesterBadge: badge });
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

  // The folder's list (40 s) would run out a few seconds after a player joins at its live edge:
  // it grows like a channel instead, one segment per segment length since the first request
  // after a reset, the folder's segments over and over (n.ts is segment n % count, with a
  // discontinuity at each new round).
  async function liveList() {
    const lines = (await readFile(join(hlsDir, 'main.m3u8'), 'utf8')).replace('#EXT-X-ENDLIST', '').split(/\r?\n/);
    const first = lines.findIndex((l) => l.startsWith('#EXTINF'));
    const head = lines.slice(0, first).filter(Boolean);
    const segs = [];
    for (let i = first; i < lines.length; i += 1) {
      if (lines[i].startsWith('#EXTINF')) segs.push(lines[i]);
    }
    return { head, segs };
  }

  async function serveLive(res, name) {
    if (!hlsDir || !s.hls || !/^(main\.m3u8|\d+\.ts)$/.test(name)) return json(res, 404, { error: 'Not Found' });
    try {
      let bytes;
      const { head, segs } = await liveList();
      if (name.endsWith('.m3u8')) {
        liveStartedAt ??= Date.now();
        const count = segs.length + Math.floor((Date.now() - liveStartedAt) / 2000);
        const out = [...head];
        for (let i = 0; i < count; i += 1) {
          if (i > 0 && i % segs.length === 0) out.push('#EXT-X-DISCONTINUITY');
          out.push(segs[i % segs.length], `${i}.ts`);
        }
        bytes = Buffer.from(`${out.join('\n')}\n`);
      } else {
        bytes = await readFile(join(hlsDir, `${Number(name.slice(0, -3)) % segs.length}.ts`));
      }
      res.writeHead(200, { 'content-type': name.endsWith('.ts') ? 'video/mp2t' : 'application/vnd.apple.mpegurl', 'cache-control': 'no-store' });
      res.end(bytes);
    } catch {
      json(res, 404, { error: 'Not Found' });
    }
  }

  // The Preview's own film (window only): scenario `previewSource` per item id ('ready',
  // 'preparing', 'unavailable'; default 'ready'); the media is `options.previewFile` with Range.
  function previewStatus(res, params) {
    const id = Number(params.get('item'));
    const known = id === s.itemId || s.queue.some((i) => i.id === id);
    if (!known) return coded(res, 404, 'no_such_item', 'No such item');
    const state = s.previewSource?.[id] ?? 'ready';
    if (state === 'ready' && previewFile) return json(res, 200, { itemId: id, state, url: `/api/desktop/preview/${id}/media` });
    if (state === 'preparing') return json(res, 200, { itemId: id, state });
    return json(res, 200, { itemId: id, state: 'unavailable', reason: 'failed' });
  }

  async function previewMedia(req, res, id) {
    if (!previewFile || (s.previewSource?.[id] ?? 'ready') !== 'ready') return coded(res, 404, 'no_such_item', 'No such item');
    const bytes = await readFile(previewFile);
    const len = bytes.length;
    const m = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range ?? '');
    const head = { 'content-type': 'video/mp4', 'accept-ranges': 'bytes', 'cache-control': 'no-store' };
    if (!m) {
      res.writeHead(200, { ...head, 'content-length': len });
      return res.end(bytes);
    }
    let first = m[1] === '' ? len - Number(m[2]) : Number(m[1]);
    let last = m[1] === '' || m[2] === '' ? len - 1 : Math.min(Number(m[2]), len - 1);
    if (first >= len || first < 0 || last < first) {
      res.writeHead(416, { 'content-range': `bytes */${len}` });
      return res.end();
    }
    res.writeHead(206, { ...head, 'content-range': `bytes ${first}-${last}/${len}`, 'content-length': last - first + 1 });
    return res.end(bytes.subarray(first, last + 1));
  }

  // The producer's monitor (window only): the list of `monitorFeed` and its segments.
  function monitorRows() {
    const f = s.monitorFeed;
    const rows = [];
    if (!f || s.playing === 'none') return rows;
    let seq = f.firstSeq ?? 0;
    for (const clip of f.clips) {
      clip.segs.forEach((n, i) => {
        rows.push({ seq, clip: clip.key, name: `${n}.ts`, src: clip.src ?? 'a', itemId: clip.itemId ?? 7, positionS: clip.baseS + 2 * i, disc: i === 0 && rows.length > 0 });
        seq += 1;
      });
    }
    return rows;
  }
  async function monitorList(res, params) {
    // The window's own reads (no `from`; hls.js reloads the list with one) are counted.
    const own = !params.has('from');
    if (own) monitorInFlight += 1;
    maxMonitorInFlight = Math.max(maxMonitorInFlight, monitorInFlight);
    try {
      if (s.monitorDelayMs) await sleep(s.monitorDelayMs);
      // The player's own loads fail (the window's reads still answer): the monitor gives up.
      if (s.monitorPlayerFail && !own) return coded(res, 500, 'internal', 'boom');
      const f = s.monitorFeed;
      const rows = monitorRows();
      if (rows.length) monitorKept = rows;
      const lines = ['#EXTM3U', '#EXT-X-VERSION:3', '#EXT-X-TARGETDURATION:2', `#EXT-X-MEDIA-SEQUENCE:${rows[0]?.seq ?? f?.firstSeq ?? 0}`, '#EXT-X-DISCONTINUITY-SEQUENCE:0'];
      const at = rows.findIndex((r) => String(r.seq) === params.get('from'));
      if (at >= 0) lines.push(`#EXT-X-START:TIME-OFFSET=${(2 * at).toFixed(3)},PRECISE=YES`);
      if (rows.some((r) => r.seq === f?.antenna)) lines.push(`#VRCINEMA-ANTENNA:${f.antenna}`);
      if (rows.some((r) => r.seq === f?.aired)) lines.push(`#VRCINEMA-AIRED:${f.aired}`);
      for (const r of rows) {
        if (r.disc) lines.push('#EXT-X-DISCONTINUITY');
        lines.push(`#VRCINEMA-AT:${r.itemId},${r.positionS.toFixed(3)}`, '#EXTINF:2.000000,', `monitor/${r.clip}/${r.name}`);
      }
      res.writeHead(200, { 'content-type': 'application/vnd.apple.mpegurl', 'cache-control': 'no-store' });
      res.end(`${lines.join('\n')}\n`);
    } finally {
      if (own) monitorInFlight -= 1;
    }
  }
  async function monitorSegment(res, clip, name) {
    const row = [...monitorRows(), ...monitorKept].find((r) => r.clip === clip && r.name === name);
    const dir = row?.src === 'b' ? hlsDir2 : hlsDir;
    if (!row || !dir) return coded(res, 404, 'no_segment', 'No such segment');
    try {
      const bytes = await readFile(join(dir, name));
      res.writeHead(200, { 'content-type': 'video/mp2t', 'cache-control': 'no-store' });
      return res.end(bytes);
    } catch {
      return coded(res, 404, 'no_segment', 'No such segment');
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
      alarms: s.alarms,
      // Live capture (window session only): scenario `captureRunning` (item ids).
      capture: s.remote ? null : { running: s.captureRunning ?? [], queued: s.queue.filter((i) => String(i.sourceUrl).startsWith('capture:')).length },
    };
  }

  // Live capture of this computer (window only; the remote panel has no such routes).
  const CAPTURE_SOURCES = {
    platform: 'windows',
    desktop: true,
    region: true,
    screens: [
      { index: 0, x: 0, y: 0, width: 2560, height: 1440, primary: true },
      { index: 1, x: 2560, y: 0, width: 1920, height: 1080, primary: false },
    ],
    windows: ['Notatnik — szkic.txt', 'VRChat'],
    cameras: [{ id: '@device_pnp_cam', name: 'Brio 100' }],
    audio: [{ id: '@device_cm_mic', name: 'Mikrofon (Scarlett Solo USB)' }],
    fast: true,
    notes: ['loopback'],
  };
  const TINY_JPEG = Buffer.from('/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=', 'base64');
  function captureRoute(res, req, route, payload) {
    if (route === '/api/desktop/capture/sources' && req.method === 'GET') return json(res, 200, CAPTURE_SOURCES);
    if (route === '/api/desktop/capture/preview' && req.method === 'POST') {
      if (s.capturePreviewFail) return coded(res, 422, 'capture_preview_failed', 'No preview: device busy');
      res.writeHead(200, { 'content-type': 'image/jpeg', 'cache-control': 'no-store' });
      return res.end(TINY_JPEG);
    }
    if (route === '/api/desktop/capture' && req.method === 'POST') {
      const title = String(payload?.title ?? '').trim() || 'Live';
      const item = queued(nextId++, title, { isLive: true, durationS: null, requesterName: null, requesterBadge: 'host', addedBy: 'admin', sourceUrl: `capture:${'ab'.repeat(16)}` });
      s.queue.push(item);
      return json(res, 201, { itemId: item.id, title });
    }
    if (route === '/api/desktop/capture/stop' && req.method === 'POST') {
      const before = s.queue.length;
      s.queue = s.queue.filter((i) => !String(i.sourceUrl).startsWith('capture:'));
      const stopped = (s.captureRunning ?? []).length;
      s.captureRunning = [];
      return json(res, 200, { stopped, removed: before - s.queue.length });
    }
    return json(res, 404, { error: 'Not Found' });
  }

  const json = (res, status, body) => {
    res.writeHead(status, { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' });
    res.end(JSON.stringify(body));
  };
  const coded = (res, status, code, error) => json(res, status, { error, code });

  // ---------- Files on disk (MEDIA_DIRS) ----------
  const libraryNode = (root, rel) => {
    let node = s.library?.[root]?.tree;
    for (const part of String(rel).split('/').filter(Boolean)) node = node && typeof node === 'object' ? node[part] : undefined;
    return node;
  };
  const libraryEntries = (node, rel) =>
    Object.entries(node)
      .map(([name, v]) => ({
        type: typeof v === 'object' ? 'dir' : 'file',
        name,
        path: rel ? `${rel}/${name}` : name,
        ...(typeof v === 'object' ? {} : { sizeBytes: v }),
      }))
      .sort((a, b) => (a.type === b.type ? a.name.localeCompare(b.name) : a.type === 'dir' ? -1 : 1));
  function libraryListing(res, query) {
    if (!s.library) return json(res, 200, { enabled: false, roots: [] });
    const root = Number(query.get('root') || 0);
    const rel = (query.get('path') ?? '').replace(/^\/+|\/+$/g, '');
    if (!s.library[root]) return coded(res, 400, 'no_such_root', 'There is no such root folder.');
    const node = libraryNode(root, rel);
    if (!node || typeof node !== 'object') return coded(res, 400, 'not_a_folder', 'This is not a folder.');
    return json(res, 200, {
      enabled: true,
      roots: s.library.map((r, index) => ({ index, name: r.name })),
      root,
      path: rel,
      parent: rel ? rel.split('/').slice(0, -1).join('/') : null,
      entries: libraryEntries(node, rel),
    });
  }
  function libraryAdd(res, payload) {
    if (!s.library) return coded(res, 400, 'library_disabled', 'Files from disk are switched off. Set MEDIA_DIRS in the .env file.');
    if (s.libraryAddError) return coded(res, 400, s.libraryAddError, `Refused: ${s.libraryAddError}`);
    const root = Number(payload?.root ?? 0);
    const rel = String(payload?.path ?? '');
    if (!s.library[root]) return coded(res, 400, 'no_such_root', 'There is no such root folder.');
    const node = libraryNode(root, rel);
    if (node === undefined) return coded(res, 400, 'not_a_folder', 'This is not a folder.');
    const files = typeof node === 'object' ? libraryEntries(node, rel).filter((e) => e.type === 'file') : [{ name: rel.split('/').at(-1) }];
    if (!files.length) return coded(res, 400, 'no_videos_in_folder', 'There are no video files in this folder.');
    for (const f of files) s.queue.push(queued(nextId++, f.name.replace(/\.[^.]+$/, ''), { sourceUrl: `local:${root}/${f.name}` }));
    const title = files.length === 1 ? s.queue.at(-1).title : rel.split('/').at(-1) || 'folder';
    return json(res, 201, { added: files.length, title });
  }

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

  // The pixeldrain key is write-only, as in the engine: only whether there is one.
  const configView = () => ({
    ...Object.fromEntries(Object.entries(s.config).filter(([k]) => k !== 'pixeldrainApiKey')),
    hasPixeldrainKey: Boolean(s.config.pixeldrainApiKey),
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

  async function slateBackground(req, res, path) {
    const view = () => ({ background: s.slateBg ?? null, maxBytes: 31457280, maxSeconds: 30, maxSide: 4096 });
    if (path.endsWith('/preview')) {
      if (!s.slateBg) return coded(res, 404, 'slate_background_none', 'No background of your own is set for the slates.');
      res.writeHead(200, { 'content-type': 'image/png', 'cache-control': 'no-store' });
      return res.end(s.slateBgBytes);
    }
    if (req.method === 'GET') return json(res, 200, view());
    if (req.method === 'DELETE') {
      s.slateBg = null;
      return json(res, 200, view());
    }
    if (req.method !== 'PUT') return json(res, 404, { error: 'Not Found' });
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const bytes = Buffer.concat(chunks);
    requests.push({ method: 'PUT', path, body: { bytes: bytes.length, type: req.headers['content-type'] } });
    if (bytes.subarray(0, 4).toString('latin1') !== '\x89PNG') return coded(res, 422, 'slate_background_unknown', 'This file cannot be used as the slates’ background.');
    s.slateBgBytes = bytes;
    s.slateBg = { motion: false, seconds: 0, stamp: Date.now(), source: 'image' };
    return json(res, 200, view());
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
      } else if (key === 'oscPort') {
        if (!Number.isInteger(value) || value < 1 || value > 65535) return fail('setting_out_of_range', `${key}: expected a number from 1 to 65535`);
        next[key] = value;
      } else if (key === 'oscRepeatS') {
        if (!Number.isInteger(value) || (value !== 0 && (value < 5 || value > 600))) return fail('osc_repeat_invalid', `${key}: expected 0 (off) or a number from 5 to 600`);
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
    if (path === '/') return serveFile(res, 'index.html');
    if (path === '/admin') return serveFile(res, 'app.html');
    if (path === '/wizard') return serveFile(res, 'wizard.html');
    if (path === '/favicon.ico') {
      res.writeHead(204);
      return res.end();
    }
    if (path.startsWith('/live/')) {
      // The window's own On air screen marks its requests (the engine's air clock skips them).
      liveRequests.push({ path, air: req.headers['x-vrcinema-air'] ?? null });
      return serveLive(res, path.slice('/live/'.length));
    }
    if (path === '/app-icon.png') {
      res.writeHead(200, { 'content-type': 'image/png' });
      return res.end(await readFile(join(PUBLIC, '..', 'desktop', 'icons', 'icon.png')));
    }
    if (path === '/__test/live') return json(res, 200, liveRequests);
    if (path.startsWith('/static/')) return serveFile(res, decodeURIComponent(path.slice('/static/'.length)));
    if (path === '/__test/requests') return json(res, 200, requests);
    if (path === '/__test/uploads') return json(res, 200, uploads);
    if (path === '/__test/stats') return json(res, 200, { maxPanelInFlight, maxViewerInFlight, maxMonitorInFlight });
    if (path === '/__test/hang') return undefined;
    if (path === '/__test/reset') {
      reset();
      return json(res, 200, { ok: true });
    }
    if (path === '/__test/scenario') {
      const change = await body(req);
      Object.assign(s, change);
      if ('pending' in change) s.pendingItems = pendingList(change.pending);
      if ('settingsPatch' in change) Object.assign(s.settings, change.settingsPatch);
      if ('viewerStateDelayMs' in change) maxViewerInFlight = viewerInFlight;
      return json(res, 200, { ...s, queue: undefined, pendingItems: undefined });
    }
    if (!path.startsWith('/api/')) return json(res, 404, { error: 'Not Found' });

    // The owner's slate background (raw file body): stored in memory, the preview is a PNG.
    if (path.startsWith('/api/admin/channels/main/slate-background')) return slateBackground(req, res, path);
    if (path === '/api/channels/main/upload' && req.method === 'POST') return viewerUpload(req, res);
    const payload = req.method === 'GET' ? null : await body(req).catch(() => null);
    requests.push({ method: req.method, path, search: url.search, body: payload, lang: req.headers['x-vrcinema-lang'] ?? null, at: Date.now() });
    const noWindow = s.remote || s.headless;
    if (path === '/api/channels/main/state' && req.method === 'GET') {
      viewerInFlight += 1;
      maxViewerInFlight = Math.max(maxViewerInFlight, viewerInFlight);
      try {
        if (s.viewerStateDelayMs) await sleep(s.viewerStateDelayMs);
        return json(res, 200, viewerState());
      } finally {
        viewerInFlight -= 1;
      }
    }
    if (path === '/api/channels/main/requests' && req.method === 'POST') return viewerRequest(res, req, payload);
    if (path === '/api/channels/main/request-list' && req.method === 'POST') return viewerList(res, req, payload);
    if (path === '/api/channels/main/nick/protect' && req.method === 'POST') return viewerNickPin(res, 'protect', payload);
    if (path === '/api/channels/main/nick/adopt' && req.method === 'POST') return viewerNickPin(res, 'adopt', payload);
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
    // The program's routes for the remote panel: the window's, under /api/admin.
    const MIRRORED = ['info', 'config', 'tools', 'tools/install', 'tools/update', 'tools/repair', 'tools/progress', 'encoder/detect', 'log', 'update', 'update/apply', 'restart', 'tunnel', 'tunnel/restart', 'tunnel/named/quick'];
    const mirror = path.startsWith('/api/admin/') && MIRRORED.includes(path.slice('/api/admin/'.length));
    if (mirror && s.signedOut) return coded(res, 401, 'login_required', 'Log in.');
    if (mirror && s.headless) return json(res, 404, { message: `Route ${req.method}:${path} not found`, error: 'Not Found', statusCode: 404 });
    const viaTunnel = mirror && s.remote && s.sessionViaTunnel && s.tunnel !== 'off';
    const route = mirror ? path.replace('/api/admin/', '/api/desktop/') : path;
    const endsConnection =
      viaTunnel &&
      ((route === '/api/desktop/config' && req.method === 'PUT' && (payload?.tunnelEnabled === false || 'tunnelMode' in (payload ?? {}))) ||
        route === '/api/desktop/tunnel/restart' ||
        route === '/api/desktop/tunnel/named/quick');
    // The browser sends an idempotent request (PUT) again on a dropped keep-alive connection:
    // that second try is dropped as well (the tunnel is gone for it too).
    const key = `${req.method} ${path} ${JSON.stringify(payload)}`;
    if (s.lastDrop?.key === key && Date.now() - s.lastDrop.at < 3000) return req.socket.destroy();
    if (endsConnection) {
      // The change is made; the answer never arrives (the tunnel carried this connection).
      s.lastDrop = { key, at: Date.now() };
      if (route === '/api/desktop/config' && payload.tunnelEnabled === false) s.tunnel = 'off';
      if (route === '/api/desktop/config' && payload.tunnelMode) s.named.mode = payload.tunnelMode;
      if (route === '/api/desktop/tunnel/restart') s.tunnelUrl = `https://proba-kanalu-${(s.rotations += 1)}.trycloudflare.com`;
      if (route === '/api/desktop/tunnel/named/quick') s.named.mode = 'quick';
      return req.socket.destroy();
    }
    if (route === '/api/desktop/info' || route === '/api/desktop/config') {
      if (s.signedOut) return coded(res, 401, 'unauthorized', 'Sign in');
      if (!mirror && noWindow) return coded(res, 403, 'window_only', 'Window only');
      if (s.infoFail && route === '/api/desktop/info') return coded(res, 500, 'internal', 'boom');
      if (req.method === 'PUT') {
        const REMOTE = ['language', 'autostart', 'publicUrl', 'videoEncoder', 'ytdlpCookies', 'pixeldrainApiKey', 'skippedUpdateVersion', 'tunnelEnabled', 'tunnelMode', 'pauseOnTunnelLoss', 'pauseWhenNoViewers'];
        const windowOnly = Object.keys(payload ?? {}).some((k) => !REMOTE.includes(k));
        if (mirror && s.remote && windowOnly) return coded(res, 403, 'window_only', 'This feature is only available in the app window.');
        return putConfig(res, payload);
      }
      if (route === '/api/desktop/config') return json(res, 200, configView());
      return json(res, 200, { ...info(), sessionViaTunnel: viaTunnel });
    }
    if (s.signedOut) return coded(res, 401, 'unauthorized', 'Sign in');
    if (path === '/api/admin/environment') {
      const app = !s.headless;
      return json(res, 200, {
        window: app,
        encoder: s.encoder,
        // Scenario `lightProfile`: a computer converting on its processor (Raspberry Pi 5).
        lightProfile: Boolean(s.lightProfile),
        tickerSlow: s.tickerSlow,
        alarms: s.alarms,
        language: s.language,
        sessionViaTunnel: app && s.sessionViaTunnel && s.tunnel !== 'off',
        publicUrl: '',
        tunnelEnabled: app ? s.tunnel !== 'off' : null,
        tunnel: app ? tunnelView() : null,
        channelRunning: app ? !s.channelDown : null,
        channelError: app && s.channelDown ? 'ffmpeg exited (code 1)' : null,
        restarting: app ? false : null,
      });
    }
    if (route.startsWith('/api/desktop/')) {
      if (!mirror && noWindow) return coded(res, 403, 'window_only', 'Window only');
      if (route === '/api/desktop/monitor.m3u8' && req.method === 'GET') return monitorList(res, url.searchParams);
      if (route === '/api/desktop/preview' && req.method === 'GET') return previewStatus(res, url.searchParams);
      const media = route.match(/^\/api\/desktop\/preview\/(\d+)\/media$/);
      if (media && req.method === 'GET') return previewMedia(req, res, Number(media[1]));
      const monitorSeg = route.match(/^\/api\/desktop\/monitor\/([a-z0-9-]+)\/(\d+\.ts)$/);
      if (monitorSeg && req.method === 'GET') return monitorSegment(res, monitorSeg[1], monitorSeg[2]);
      if (route.startsWith('/api/desktop/capture')) {
        if (mirror) return json(res, 404, { error: 'Not Found' });
        return captureRoute(res, req, route, payload);
      }
      if (await desktopRoute(res, req, route, payload)) return undefined;
      if (route === '/api/desktop/tunnel/qr') return sendQr(res);
      // OBS (window only): scenario `obsKey`, `obsStatus` (listening, streaming, failed).
      if (route === '/api/desktop/obs' || route === '/api/desktop/obs/key') {
        if (mirror && s.remote) return coded(res, 403, 'window_only', 'Window only');
        if (route.endsWith('/key') && req.method === 'POST') s.obsKey = `nowyklucz${Date.now() % 100000}`;
        const lanServers = s.config.obsLan ? ['rtmp://192.168.1.20:1935/live'] : [];
        return json(res, 200, {
          server: 'rtmp://127.0.0.1:1935/live',
          lanServers,
          key: s.obsKey ?? 'abcdefghijklmnopqrstuvwx',
          port: 1935,
          lan: s.config.obsLan,
          priority: s.config.obsPriority,
          status: s.obsStatus ?? 'listening',
          error: s.obsStatus === 'failed' ? 'address in use' : null,
          onAir: s.obs ?? 'off',
        });
      }
      if (route === '/api/desktop/tunnel' && req.method === 'GET') return json(res, 200, tunnelView());
      if (route === '/api/desktop/wizard/complete' && req.method === 'POST') {
        s.firstRun = false;
        return json(res, 200, { ok: true });
      }
      if (route === '/api/desktop/tunnel/restart' && req.method === 'POST') {
        if (s.tunnel === 'off') return coded(res, 409, 'tunnel_off', 'Tunnel is off');
        s.rotations += 1;
        s.tunnelUrl = `https://proba-kanalu-${s.rotations}.trycloudflare.com`;
        return json(res, 200, tunnelView());
      }
      if (route.startsWith('/api/desktop/tunnel/named/')) return namedRoute(res, route.slice('/api/desktop/tunnel/named/'.length), req, payload);
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
    const resetPin = path.match(/^\/api\/admin\/nicks\/(.+)\/reset-pin$/);
    if (resetPin && req.method === 'POST') {
      const nick = s.nicks.find((n) => n.skeleton === decodeURIComponent(resetPin[1]));
      if (!nick?.protected) return coded(res, 404, 'nick_no_pin', 'This nick has no PIN.');
      Object.assign(nick, { protected: false, devices: 0 });
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
    if (path === '/api/admin/library' && req.method === 'GET') return libraryListing(res, url.searchParams);
    const base = '/api/admin/channels/main';
    if (path === `${base}/local` && req.method === 'POST') return libraryAdd(res, payload);
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
    // The broadcast journal (Settings → Advanced): scenario `journal` (rows, newest first).
    if (path === `${base}/journal` && req.method === 'GET') {
      return json(res, 200, { entries: s.journal.slice(0, Number(url.searchParams.get('limit') ?? 20)) });
    }
    if (path === `${base}/journal` && req.method === 'DELETE') {
      const removed = s.journal.length;
      s.journal = [];
      return json(res, 200, { ok: true, removed });
    }
    if (path === `${base}/journal/export` && req.method === 'GET') {
      const csv = url.searchParams.get('format') !== 'json';
      res.writeHead(200, {
        'Content-Type': csv ? 'text/csv; charset=utf-8' : 'application/json',
        'Content-Disposition': `attachment; filename="vrcinema-journal-20261106.${csv ? 'csv' : 'json'}"`,
      });
      return res.end(csv ? 'started,ended,duration_s,title\r\n' : JSON.stringify(s.journal));
    }
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
    const listRoute = action.match(/^lists\/(\d+)$/);
    if (listRoute) {
      const list = s.pendingLists.find((l) => l.id === Number(listRoute[1]));
      if (!list) return coded(res, 404, 'no_such_list', 'That list does not exist.');
      const mine = s.pendingItems.filter((i) => list.itemIds.includes(i.id));
      if (!mine.length) return coded(res, 409, 'not_pending', 'Not pending');
      s.pendingItems = s.pendingItems.filter((i) => !list.itemIds.includes(i.id));
      if (payload?.action === 'approve') s.queue.push(...mine.map((i) => ({ ...i, status: 'queued' })));
      return json(res, 200, { changed: mine.length });
    }
    const queueTool = queueTools(res, action, payload);
    if (queueTool) return queueTool;
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
    if (s.controlDelayMs && ['pause', 'resume', 'seek'].includes(action)) await sleep(s.controlDelayMs);
    switch (action) {
      case 'pause':
        if (s.armPause && Number.isFinite(payload?.atS)) {
          s.pausePending = true;
          break;
        }
        s.paused = true;
        if (!s.pauseReasons.includes('manual')) s.pauseReasons = [...s.pauseReasons, 'manual'];
        break;
      case 'resume':
        s.pausePending = false;
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
        if (Number.isInteger(payload?.itemId)) {
          s.itemId = payload.itemId;
          s.positionS = 0;
          s.paused = false;
          s.pauseReasons = [];
        }
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
  const { port: bound } = await startFakeEngine(port, { hlsDir: process.env.FAKE_HLS_DIR ?? null, hlsDir2: process.env.FAKE_HLS_DIR2 ?? null });
  console.log(`fake engine on http://127.0.0.1:${bound}/admin`);
}
