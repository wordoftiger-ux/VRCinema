import fs from 'node:fs';
import Fastify, { type FastifyInstance, type FastifyReply, type FastifyRequest } from 'fastify';
import fastifyStatic from '@fastify/static';
import type { AppConfig } from '../config.ts';
import type { QueueItem, QueueRepo } from '../db/queue.ts';
import { sanitizeSettingsPatch, type SettingsRepo } from '../db/settings.ts';
import type { MediaResolver } from '../resolver/types.ts';
import type { ChannelState } from '../pipeline/channel.ts';
import { isAllowedDomain, parseSourceUrl, RateLimiter, validateMeta } from '../queue/rules.ts';
import {
  checkPassword,
  createSessionToken,
  parseCookies,
  SESSION_COOKIE,
  SESSION_TTL_MS,
  sessionCookie,
  verifySessionToken,
} from './auth.ts';
import { getClientIp, isHttps } from './clientIp.ts';
import { LibraryError, type MediaLibrary } from '../library/library.ts';

export interface AppDeps {
  config: Pick<AppConfig, 'adminPassword' | 'sessionSecret' | 'publicUrl' | 'trustCloudflare' | 'trustProxy'>;
  channelId: number;
  queue: QueueRepo;
  settings: SettingsRepo;
  resolver: MediaResolver;
  channel: {
    state(): ChannelState;
    skip(): void;
    onQueueChanged(): void;
    readonly prefetchedItemId: number | null;
  };
  broadcaster: {
    playlist(): string;
    segmentPath(name: string): string | null;
  };
  publicDir: string;
  /** Foldery z filmami na dysku (opcjonalnie). */
  library?: MediaLibrary;
  log?: (msg: string) => void;
}

const MAX_PLAYLIST_ITEMS = 200;
const LOGIN_FAILURES = 5;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;

class HttpError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

function publicItem(i: QueueItem) {
  return { id: i.id, title: i.title, durationS: i.durationS, requesterName: i.requesterName, isLive: i.isLive };
}

function adminItem(i: QueueItem) {
  return {
    ...publicItem(i),
    sourceUrl: i.sourceUrl,
    status: i.status,
    addedBy: i.addedBy,
    requesterIp: i.requesterIp,
    error: i.error,
    createdAt: i.createdAt,
    finishedAt: i.finishedAt,
  };
}

function cleanName(raw: unknown): string | null {
  if (typeof raw !== 'string') return null;
  const name = raw.replace(/[\u0000-\u001f\u007f]/g, '').trim().slice(0, 32);
  return name || null;
}

export async function buildApp(d: AppDeps): Promise<FastifyInstance> {
  const app = Fastify({ logger: false, bodyLimit: 16 * 1024 });
  const { config, channelId, queue, settings } = d;
  const trusted = config.trustCloudflare || config.trustProxy;

  const s0 = settings.get(channelId);
  const requestLimiter = new RateLimiter(s0.requestLimitCount, s0.requestLimitWindowS * 1000);
  const loginLimiter = new RateLimiter(LOGIN_FAILURES, LOGIN_WINDOW_MS);

  const ipOf = (req: FastifyRequest) => getClientIp(req.headers, req.socket.remoteAddress ?? '', config);
  const https = (req: FastifyRequest) => isHttps(req.headers, Boolean((req.socket as { encrypted?: boolean }).encrypted), trusted);
  const baseUrl = (req: FastifyRequest) => {
    if (config.publicUrl) return config.publicUrl;
    const host = (trusted && (req.headers['x-forwarded-host'] as string | undefined)) || req.headers.host || 'localhost';
    return `${https(req) ? 'https' : 'http'}://${host}`;
  };
  const isAdmin = (req: FastifyRequest) =>
    verifySessionToken(parseCookies(req.headers.cookie)[SESSION_COOKIE], config.sessionSecret);

  app.setErrorHandler((err: Error & { statusCode?: number; validation?: unknown }, _req, reply) => {
    if (err instanceof HttpError) return reply.code(err.status).send({ error: err.message });
    if (err.statusCode && err.statusCode < 500) return reply.code(err.statusCode).send({ error: err.message });
    d.log?.(`Błąd serwera: ${err.stack ?? err.message}`);
    return reply.code(500).send({ error: 'Błąd serwera' });
  });

  await app.register(fastifyStatic, { root: d.publicDir, prefix: '/static/', index: false });

  // ---------- Strony ----------
  app.get('/', (_req, reply) => reply.sendFile('index.html'));
  app.get('/admin', (_req, reply) => reply.sendFile('admin.html'));

  // ---------- Strumień ----------
  const streamHeaders = (reply: FastifyReply) => reply.header('Access-Control-Allow-Origin', '*');

  app.get('/live/main.m3u8', (_req, reply) => {
    streamHeaders(reply)
      .header('Cache-Control', 'no-cache')
      .type('application/vnd.apple.mpegurl')
      .send(d.broadcaster.playlist());
  });

  app.get<{ Params: { name: string } }>('/live/main/:name', (req, reply) => {
    const file = d.broadcaster.segmentPath(req.params.name);
    if (!file || !fs.existsSync(file)) return reply.code(404).send({ error: 'Brak fragmentu' });
    streamHeaders(reply)
      .header('Cache-Control', 'public, max-age=60')
      .type('video/mp2t')
      .send(fs.createReadStream(file));
  });

  // ---------- API publiczne ----------
  app.get('/api/channels/main/state', (req) => {
    const s = settings.get(channelId);
    const base = baseUrl(req);
    return {
      ...d.channel.state(),
      queue: queue.listByStatus(channelId, ['queued']).map(publicItem),
      channelUrl: `${base}/live/main.m3u8`,
      requestUrl: `${base}/`,
      rules: { allowedDomains: s.allowedDomains, maxDurationS: s.maxDurationS, moderation: s.moderation },
    };
  });

  app.post<{ Body: { url?: unknown; name?: unknown } }>('/api/channels/main/requests', async (req, reply) => {
    const s = settings.get(channelId);
    const url = typeof req.body?.url === 'string' ? parseSourceUrl(req.body.url) : null;
    if (!url) throw new HttpError(400, 'To nie wygląda na poprawny link (http/https).');
    if (!isAllowedDomain(url, s.allowedDomains)) {
      throw new HttpError(400, `Ten serwis nie jest dozwolony. Dozwolone: ${s.allowedDomains.join(', ')}.`);
    }
    const ip = ipOf(req);
    requestLimiter.setLimits(s.requestLimitCount, s.requestLimitWindowS * 1000);
    if (queue.countActiveByIp(channelId, ip) >= s.maxActivePerRequester) {
      throw new HttpError(429, `Masz już ${s.maxActivePerRequester} filmów w kolejce. Poczekaj, aż się odtworzą.`);
    }
    if (!requestLimiter.hit(ip)) {
      throw new HttpError(429, 'Za dużo zgłoszeń w krótkim czasie. Spróbuj za kilka minut.');
    }
    let meta;
    try {
      meta = await d.resolver.fetchMeta(url.href);
    } catch (e) {
      throw new HttpError(400, `Nie udało się odczytać filmu: ${(e as Error).message}`);
    }
    const reason = validateMeta(meta, s, false);
    if (reason) throw new HttpError(400, reason);
    const item = queue.add({
      channelId,
      sourceUrl: url.href,
      title: meta.title,
      durationS: meta.durationS,
      isLive: meta.isLive,
      status: s.moderation ? 'pending_approval' : 'queued',
      addedBy: 'public',
      requesterName: cleanName(req.body.name),
      requesterIp: ip,
    });
    d.log?.(`Zgłoszenie od ${ip}: "${item.title}" (${item.status})`);
    return reply.code(201).send({ status: item.status, item: publicItem(item) });
  });

  // ---------- Logowanie ----------
  app.post<{ Body: { password?: unknown } }>('/api/admin/login', (req, reply) => {
    const ip = ipOf(req);
    if (loginLimiter.peek(ip) >= LOGIN_FAILURES) {
      throw new HttpError(429, 'Za dużo nieudanych prób logowania. Spróbuj za 15 minut.');
    }
    if (!checkPassword(req.body?.password, config.adminPassword)) {
      loginLimiter.hit(ip);
      throw new HttpError(401, 'Złe hasło.');
    }
    const token = createSessionToken(config.sessionSecret);
    reply.header('Set-Cookie', sessionCookie(token, https(req), Math.floor(SESSION_TTL_MS / 1000)));
    return { ok: true };
  });

  app.post('/api/admin/logout', (req, reply) => {
    reply.header('Set-Cookie', sessionCookie('', https(req), 0));
    return { ok: true };
  });

  // ---------- API administratora ----------
  app.register(async (admin) => {
    admin.addHook('onRequest', async (req) => {
      if (!isAdmin(req)) throw new HttpError(401, 'Zaloguj się.');
    });

    admin.get('/api/admin/channels/main', (req) => {
      const base = baseUrl(req);
      return {
        ...d.channel.state(),
        prefetchedItemId: d.channel.prefetchedItemId,
        playing: queue.listByStatus(channelId, ['playing']).map(adminItem),
        queue: queue.listByStatus(channelId, ['queued']).map(adminItem),
        pending: queue.listByStatus(channelId, ['pending_approval']).map(adminItem),
        history: queue.history(channelId, 50).map(adminItem),
        settings: settings.get(channelId),
        channelUrl: `${base}/live/main.m3u8`,
        requestUrl: `${base}/`,
      };
    });

    admin.post<{ Body: { url?: unknown } }>('/api/admin/channels/main/items', async (req, reply) => {
      const url = typeof req.body?.url === 'string' ? parseSourceUrl(req.body.url) : null;
      if (!url) throw new HttpError(400, 'To nie wygląda na poprawny link (http/https).');
      let meta;
      try {
        meta = await d.resolver.fetchMeta(url.href);
      } catch (e) {
        throw new HttpError(400, `Nie udało się odczytać: ${(e as Error).message}`);
      }
      const base = { channelId, status: 'queued' as const, addedBy: 'admin' as const, requesterName: null, requesterIp: null };
      let added = 0;
      if (meta.isPlaylist) {
        for (const e of (meta.entries ?? []).slice(0, MAX_PLAYLIST_ITEMS)) {
          queue.add({ ...base, sourceUrl: e.url, title: e.title, durationS: e.durationS, isLive: false });
          added++;
        }
        if (added === 0) throw new HttpError(400, 'Playlista jest pusta.');
      } else {
        queue.add({ ...base, sourceUrl: url.href, title: meta.title, durationS: meta.durationS, isLive: meta.isLive });
        added = 1;
      }
      d.channel.onQueueChanged();
      return reply.code(201).send({ added, title: meta.title });
    });

    admin.patch<{ Params: { id: string }; Body: { action?: unknown; position?: unknown } }>(
      '/api/admin/items/:id',
      (req) => {
        const item = queue.get(Number(req.params.id));
        if (!item || item.channelId !== channelId) throw new HttpError(404, 'Nie ma takiej pozycji.');
        const { action, position } = req.body ?? {};
        if (action === 'approve' || action === 'reject') {
          if (item.status !== 'pending_approval') throw new HttpError(409, 'Ta pozycja nie czeka na akceptację.');
          queue.setStatus(item.id, action === 'approve' ? 'queued' : 'rejected');
        } else if (typeof position === 'number' && Number.isInteger(position) && position >= 0) {
          if (item.status !== 'queued') throw new HttpError(409, 'Przesuwać można tylko pozycje w kolejce.');
          queue.move(item.id, position);
        } else {
          throw new HttpError(400, 'Podaj action (approve/reject) albo position.');
        }
        d.channel.onQueueChanged();
        return { ok: true };
      },
    );

    admin.delete<{ Params: { id: string } }>('/api/admin/items/:id', (req) => {
      const item = queue.get(Number(req.params.id));
      if (!item || item.channelId !== channelId) throw new HttpError(404, 'Nie ma takiej pozycji.');
      if (!['queued', 'pending_approval', 'playing'].includes(item.status)) {
        throw new HttpError(409, 'Ta pozycja już się zakończyła.');
      }
      queue.setStatus(item.id, 'removed');
      d.channel.onQueueChanged();
      return { ok: true };
    });

    // ---------- Pliki z dysku ----------
    const libraryError = (e: unknown): never => {
      if (e instanceof LibraryError) throw new HttpError(400, e.message);
      throw e;
    };

    admin.get<{ Querystring: { root?: string; path?: string } }>('/api/admin/library', (req) => {
      const lib = d.library;
      if (!lib || !lib.enabled) return { enabled: false, roots: [] };
      const root = Number(req.query.root ?? 0);
      try {
        return { enabled: true, roots: lib.roots().map((r) => ({ index: r.index, name: r.name })), ...lib.list(root, req.query.path ?? '') };
      } catch (e) {
        return libraryError(e);
      }
    });

    admin.post<{ Body: { root?: unknown; path?: unknown } }>('/api/admin/channels/main/local', async (req, reply) => {
      const lib = d.library;
      if (!lib || !lib.enabled) throw new HttpError(400, 'Pliki z dysku są wyłączone. Ustaw MEDIA_DIRS w pliku .env.');
      const root = Number(req.body?.root);
      const rel = typeof req.body?.path === 'string' ? req.body.path : '';
      let files: string[];
      try {
        const full = lib.resolve(root, rel);
        files = fs.statSync(full).isDirectory() ? lib.videosIn(root, rel).map((e) => e.path) : [rel];
      } catch (e) {
        return libraryError(e);
      }
      if (files.length === 0) throw new HttpError(400, 'W tym folderze nie ma plików wideo.');
      const added = [];
      for (const rel2 of files.slice(0, MAX_PLAYLIST_ITEMS)) {
        const sourceUrl = lib.toSourceUrl(root, rel2);
        const meta = await d.resolver.fetchMeta(sourceUrl).catch((e: Error) => {
          throw new HttpError(400, e.message);
        });
        added.push(queue.add({
          channelId, sourceUrl, title: meta.title, durationS: meta.durationS, isLive: false,
          status: 'queued', addedBy: 'admin', requesterName: null, requesterIp: null,
        }));
      }
      d.channel.onQueueChanged();
      return reply.code(201).send({ added: added.length, title: added.length === 1 ? added[0].title : rel.split('/').at(-1) || 'folder' });
    });

    admin.post('/api/admin/channels/main/skip', () => {
      d.channel.skip();
      return { ok: true };
    });

    admin.get('/api/admin/channels/main/settings', () => settings.get(channelId));

    admin.put<{ Body: Record<string, unknown> }>('/api/admin/channels/main/settings', (req) => {
      let patch;
      try {
        patch = sanitizeSettingsPatch(req.body ?? {});
      } catch (e) {
        throw new HttpError(400, (e as Error).message);
      }
      const updated = settings.update(channelId, patch);
      requestLimiter.setLimits(updated.requestLimitCount, updated.requestLimitWindowS * 1000);
      return updated;
    });
  });

  // Ułatwienie: favicon bez błędów 404 w konsoli.
  app.get('/favicon.ico', (_req, reply) => reply.code(204).send());

  return app;
}
