import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { FastifyInstance } from 'fastify';
import { buildApp, type AppDeps } from '../src/http/app.ts';
import { getClientIp } from '../src/http/clientIp.ts';
import { createSessionToken, verifySessionToken } from '../src/http/auth.ts';
import { openDatabase, MAIN_CHANNEL_ID } from '../src/db/database.ts';
import { QueueRepo } from '../src/db/queue.ts';
import { SettingsRepo } from '../src/db/settings.ts';
import type { MediaMeta, MediaResolver } from '../src/resolver/types.ts';

let app: FastifyInstance;
let queue: QueueRepo;
let settings: SettingsRepo;
let metas: Record<string, MediaMeta | Error>;
let liveDir: string;
let skipped = 0;
let queueChanged = 0;

const resolver: MediaResolver = {
  async fetchMeta(url) {
    const m = metas[url];
    if (!m) throw new Error('Unsupported URL');
    if (m instanceof Error) throw m;
    return m;
  },
  async resolve() {
    throw new Error('nie w testach');
  },
};

function video(title: string, durationS = 100): MediaMeta {
  return { title, durationS, isLive: false, isPlaylist: false };
}

async function login(): Promise<string> {
  const res = await app.inject({ method: 'POST', url: '/api/admin/login', payload: { password: 'tajne' } });
  expect(res.statusCode).toBe(200);
  const cookie = String(res.headers['set-cookie']);
  return cookie.split(';')[0];
}

function request(url: string, ip = '1.1.1.1', name?: string) {
  return app.inject({
    method: 'POST',
    url: '/api/channels/main/requests',
    payload: { url, name },
    remoteAddress: ip,
  });
}

beforeEach(async () => {
  const db = openDatabase(':memory:');
  queue = new QueueRepo(db);
  settings = new SettingsRepo(db);
  metas = {
    'https://youtu.be/a': video('A'),
    'https://youtu.be/b': video('B'),
    'https://youtu.be/c': video('C'),
    'https://youtu.be/d': video('D'),
    'https://youtu.be/long': video('Long', 99_999),
    'https://youtu.be/live': { title: 'Live', durationS: null, isLive: true, isPlaylist: false },
    'https://youtube.com/playlist?list=1': {
      title: 'PL',
      durationS: null,
      isLive: false,
      isPlaylist: true,
      entries: [
        { url: 'https://youtu.be/p1', title: 'P1', durationS: 10 },
        { url: 'https://youtu.be/p2', title: 'P2', durationS: 20 },
      ],
    },
  };
  liveDir = fs.mkdtempSync(path.join(os.tmpdir(), 'live-'));
  fs.writeFileSync(path.join(liveDir, '5.ts'), Buffer.alloc(188, 0x47));
  skipped = 0;
  queueChanged = 0;
  const deps: AppDeps = {
    config: { adminPassword: 'tajne', sessionSecret: 's'.repeat(64), publicUrl: '', trustCloudflare: false, trustProxy: false },
    channelId: MAIN_CHANNEL_ID,
    queue,
    settings,
    resolver,
    channel: {
      state: () => ({ status: 'idle', current: null }),
      skip: () => {
        skipped++;
      },
      onQueueChanged: () => {
        queueChanged++;
      },
      prefetchedItemId: null,
    },
    broadcaster: {
      playlist: () => '#EXTM3U\n',
      segmentPath: (name) => (name === '5.ts' ? path.join(liveDir, '5.ts') : null),
    },
    publicDir: path.resolve('public'),
  };
  app = await buildApp(deps);
});

afterEach(async () => {
  await app.close();
  fs.rmSync(liveDir, { recursive: true, force: true });
});

describe('strumień', () => {
  it('lista HLS bez cache i z CORS', async () => {
    const res = await app.inject('/live/main.m3u8');
    expect(res.statusCode).toBe(200);
    expect(res.headers['content-type']).toContain('mpegurl');
    expect(res.headers['cache-control']).toBe('no-cache');
    expect(res.headers['access-control-allow-origin']).toBe('*');
    expect(res.body).toBe('#EXTM3U\n');
  });
  it('fragment znany → 200, nieznany → 404', async () => {
    const ok = await app.inject('/live/main/5.ts');
    expect(ok.statusCode).toBe(200);
    expect(ok.headers['content-type']).toBe('video/mp2t');
    expect(ok.rawPayload.length).toBe(188);
    expect((await app.inject('/live/main/6.ts')).statusCode).toBe(404);
    expect((await app.inject('/live/main/..%2F..%2Fpackage.json')).statusCode).toBe(404);
  });
});

describe('zgłoszenia publiczne', () => {
  it('dodaje film na koniec kolejki z pseudonimem', async () => {
    const res = await request('https://youtu.be/a', '1.1.1.1', '  Ola  ');
    expect(res.statusCode).toBe(201);
    expect(res.json().status).toBe('queued');
    const [item] = queue.listByStatus(MAIN_CHANNEL_ID, ['queued']);
    expect(item).toMatchObject({ title: 'A', requesterName: 'Ola', requesterIp: '1.1.1.1', addedBy: 'public' });
  });

  it('niedozwolona domena → 400 bez pytania yt-dlp', async () => {
    const res = await request('https://vimeo.com/1');
    expect(res.statusCode).toBe(400);
    expect(res.json().error).toMatch(/serwis/i);
  });

  it('niepoprawny link → 400', async () => {
    expect((await request('to nie link')).statusCode).toBe(400);
  });

  it('błąd odczytu filmu → 400 z powodem', async () => {
    const res = await request('https://youtu.be/zzz');
    expect(res.statusCode).toBe(400);
    expect(res.json().error).toContain('Unsupported URL');
  });

  it('za długi film, transmisja i playlista → 400', async () => {
    expect((await request('https://youtu.be/long')).json().error).toMatch(/za długi/);
    expect((await request('https://youtu.be/live', '2.2.2.2')).json().error).toMatch(/na żywo/);
    expect((await request('https://youtube.com/playlist?list=1', '3.3.3.3')).json().error).toMatch(/Playlist/);
  });

  it('limit 3 zgłoszeń na 10 minut z jednego IP → 429', async () => {
    for (const x of ['a', 'b', 'c']) expect((await request(`https://youtu.be/${x}`)).statusCode).toBe(201);
    const res = await request('https://youtu.be/d');
    expect(res.statusCode).toBe(429);
    expect((await request('https://youtu.be/d', '9.9.9.9')).statusCode).toBe(201);
  });

  it('limit aktywnych filmów jednej osoby → 429', async () => {
    settings.update(MAIN_CHANNEL_ID, { maxActivePerRequester: 1, requestLimitCount: 100 });
    expect((await request('https://youtu.be/a')).statusCode).toBe(201);
    const res = await request('https://youtu.be/b');
    expect(res.statusCode).toBe(429);
    expect(res.json().error).toMatch(/w kolejce/);
  });

  it('tryb moderacji: zgłoszenie czeka na akceptację', async () => {
    settings.update(MAIN_CHANNEL_ID, { moderation: true });
    const res = await request('https://youtu.be/a');
    expect(res.json().status).toBe('pending_approval');
    expect(queue.listByStatus(MAIN_CHANNEL_ID, ['pending_approval'])).toHaveLength(1);
  });

  it('stan kanału pokazuje kolejkę bez adresów IP', async () => {
    await request('https://youtu.be/a', '1.1.1.1', 'Ola');
    const res = await app.inject({ url: '/api/channels/main/state', headers: { host: 'tv.local:8080' } });
    const body = res.json();
    expect(body.queue).toHaveLength(1);
    expect(body.queue[0]).toEqual({ id: expect.any(Number), title: 'A', durationS: 100, requesterName: 'Ola', isLive: false });
    expect(JSON.stringify(body)).not.toContain('1.1.1.1');
    expect(body.channelUrl).toBe('http://tv.local:8080/live/main.m3u8');
  });
});

describe('panel administratora', () => {
  it('bez sesji → 401', async () => {
    expect((await app.inject('/api/admin/channels/main')).statusCode).toBe(401);
    expect((await app.inject({ method: 'POST', url: '/api/admin/channels/main/skip' })).statusCode).toBe(401);
  });

  it('złe hasło → 401, po 5 próbach blokada 429', async () => {
    for (let i = 0; i < 5; i++) {
      const r = await app.inject({ method: 'POST', url: '/api/admin/login', payload: { password: 'x' } });
      expect(r.statusCode).toBe(401);
    }
    const blocked = await app.inject({ method: 'POST', url: '/api/admin/login', payload: { password: 'tajne' } });
    expect(blocked.statusCode).toBe(429);
  });

  it('logowanie ustawia ciasteczko HttpOnly, które otwiera panel', async () => {
    const res = await app.inject({ method: 'POST', url: '/api/admin/login', payload: { password: 'tajne' } });
    expect(String(res.headers['set-cookie'])).toMatch(/HttpOnly/);
    const cookie = String(res.headers['set-cookie']).split(';')[0];
    const panel = await app.inject({ url: '/api/admin/channels/main', headers: { cookie } });
    expect(panel.statusCode).toBe(200);
    expect(panel.json()).toHaveProperty('settings');
  });

  it('dodanie playlisty przez admina rozwija ją na pozycje', async () => {
    const cookie = await login();
    const res = await app.inject({
      method: 'POST', url: '/api/admin/channels/main/items', headers: { cookie },
      payload: { url: 'https://youtube.com/playlist?list=1' },
    });
    expect(res.statusCode).toBe(201);
    expect(res.json().added).toBe(2);
    expect(queue.listByStatus(MAIN_CHANNEL_ID, ['queued']).map((i) => i.title)).toEqual(['P1', 'P2']);
  });

  it('admin może dodać transmisję i film z dowolnej domeny', async () => {
    const cookie = await login();
    metas['https://vimeo.com/1'] = video('Vimeo');
    for (const url of ['https://youtu.be/live', 'https://vimeo.com/1']) {
      const res = await app.inject({ method: 'POST', url: '/api/admin/channels/main/items', headers: { cookie }, payload: { url } });
      expect(res.statusCode).toBe(201);
    }
    expect(queue.listByStatus(MAIN_CHANNEL_ID, ['queued'])[0].isLive).toBe(true);
  });

  it('akceptacja, przesunięcie, usunięcie i pominięcie', async () => {
    const cookie = await login();
    settings.update(MAIN_CHANNEL_ID, { moderation: true });
    await request('https://youtu.be/a');
    const pending = queue.listByStatus(MAIN_CHANNEL_ID, ['pending_approval'])[0];
    let r = await app.inject({ method: 'PATCH', url: `/api/admin/items/${pending.id}`, headers: { cookie }, payload: { action: 'approve' } });
    expect(r.statusCode).toBe(200);
    expect(queue.get(pending.id)!.status).toBe('queued');

    const b = queue.add({ channelId: 1, sourceUrl: 'u', title: 'B', durationS: 1, isLive: false, status: 'queued', addedBy: 'admin', requesterName: null, requesterIp: null });
    r = await app.inject({ method: 'PATCH', url: `/api/admin/items/${b.id}`, headers: { cookie }, payload: { position: 0 } });
    expect(r.statusCode).toBe(200);
    expect(queue.nextQueued(MAIN_CHANNEL_ID)!.id).toBe(b.id);

    r = await app.inject({ method: 'DELETE', url: `/api/admin/items/${b.id}`, headers: { cookie } });
    expect(r.statusCode).toBe(200);
    expect(queue.get(b.id)!.status).toBe('removed');
    expect(queueChanged).toBeGreaterThanOrEqual(3);

    r = await app.inject({ method: 'POST', url: '/api/admin/channels/main/skip', headers: { cookie } });
    expect(r.statusCode).toBe(200);
    expect(skipped).toBe(1);
  });

  it('ustawienia: zapis poprawnych, odrzucenie złych', async () => {
    const cookie = await login();
    let r = await app.inject({
      method: 'PUT', url: '/api/admin/channels/main/settings', headers: { cookie },
      payload: { moderation: true, allowedDomains: ['https://www.Vimeo.com/', 'youtube.com'] },
    });
    expect(r.statusCode).toBe(200);
    expect(r.json().allowedDomains).toEqual(['vimeo.com', 'youtube.com']);
    r = await app.inject({ method: 'PUT', url: '/api/admin/channels/main/settings', headers: { cookie }, payload: { moderation: 'tak' } });
    expect(r.statusCode).toBe(400);
  });

  it('wylogowanie unieważnia ciasteczko w przeglądarce', async () => {
    const cookie = await login();
    const r = await app.inject({ method: 'POST', url: '/api/admin/logout', headers: { cookie } });
    expect(String(r.headers['set-cookie'])).toMatch(/Max-Age=0/);
  });
});

describe('strony', () => {
  it('serwuje stronę zgłoszeń i panel', async () => {
    expect((await app.inject('/')).headers['content-type']).toContain('text/html');
    expect((await app.inject('/admin')).headers['content-type']).toContain('text/html');
  });
});

describe('getClientIp', () => {
  const headers = { 'cf-connecting-ip': '5.5.5.5', 'x-forwarded-for': '6.6.6.6, 7.7.7.7' };
  it('domyślnie adres połączenia', () => {
    expect(getClientIp(headers, '127.0.0.1', { trustCloudflare: false, trustProxy: false })).toBe('127.0.0.1');
  });
  it('Cloudflare', () => {
    expect(getClientIp(headers, '127.0.0.1', { trustCloudflare: true, trustProxy: false })).toBe('5.5.5.5');
  });
  it('reverse proxy: pierwszy adres z X-Forwarded-For', () => {
    expect(getClientIp(headers, '127.0.0.1', { trustCloudflare: false, trustProxy: true })).toBe('6.6.6.6');
  });
});

describe('sesje', () => {
  it('token ważny do wygaśnięcia i odporny na podróbki', () => {
    const t = createSessionToken('sekret', 1000, 0);
    expect(verifySessionToken(t, 'sekret', 999)).toBe(true);
    expect(verifySessionToken(t, 'sekret', 1001)).toBe(false);
    expect(verifySessionToken(t, 'inny', 0)).toBe(false);
    expect(verifySessionToken(t.replace(/^\d+/, '99999'), 'sekret', 0)).toBe(false);
    expect(verifySessionToken('', 'sekret', 0)).toBe(false);
  });
});
