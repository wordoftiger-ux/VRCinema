import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import { parseYtDlpMeta, parseYtDlpResolve, YtDlpResolver, ytdlpErrorMessage } from '../src/resolver/ytdlp.ts';
import { PixeldrainResolver, parsePixeldrainUrl } from '../src/resolver/pixeldrain.ts';
import { CompositeResolver } from '../src/resolver/index.ts';
import { runProcess } from '../src/util/process.ts';
import type { MediaResolver } from '../src/resolver/types.ts';

const fixture = (name: string) => JSON.parse(fs.readFileSync(`tests/fixtures/${name}`, 'utf8'));

describe('parseYtDlpMeta', () => {
  it('pojedynczy film', () => {
    expect(parseYtDlpMeta(fixture('ytdlp-meta-video.json'))).toEqual({
      title: 'Me at the zoo',
      durationS: 19,
      isLive: false,
      isPlaylist: false,
    });
  });
  it('playlista z wpisami', () => {
    const m = parseYtDlpMeta(fixture('ytdlp-meta-playlist.json'));
    expect(m.isPlaylist).toBe(true);
    expect(m.title).toBe('Popular Music Videos');
    expect(m.entries).toHaveLength(3);
    expect(m.entries![0]).toEqual({
      url: 'https://www.youtube.com/watch?v=fOT0BUpITw8',
      title: 'BELLAKEO (Video Oficial) - Peso Pluma, Anitta',
      durationS: 235,
    });
  });
  it('transmisja na żywo', () => {
    const m = parseYtDlpMeta({ _type: 'video', title: 'Live', live_status: 'is_live', is_live: true });
    expect(m).toEqual({ title: 'Live', durationS: null, isLive: true, isPlaylist: false });
  });
  it('brak tytułu', () => {
    expect(parseYtDlpMeta({ _type: 'video', duration: 5 }).title).toBe('Bez tytułu');
  });
});

describe('parseYtDlpResolve', () => {
  it('osobne wideo i dźwięk z nagłówkami', () => {
    const r = parseYtDlpResolve(fixture('ytdlp-resolve-split.json'));
    expect(r.inputs).toHaveLength(2);
    expect(r.inputs[0].url).toContain('itag=133');
    expect(r.inputs[1].url).toContain('itag=251');
    expect(r.inputs[0].headers['User-Agent']).toBeTruthy();
    expect(r.title).toBe('Me at the zoo');
    expect(r.durationS).toBe(19);
    expect(r.isLive).toBe(false);
  });
  it('jeden plik z wideo i dźwiękiem', () => {
    const r = parseYtDlpResolve({
      title: 'CDA',
      duration: 100,
      url: 'https://cdn/x.mp4',
      http_headers: { Referer: 'https://cda.pl' },
    });
    expect(r.inputs).toEqual([{ url: 'https://cdn/x.mp4', headers: { Referer: 'https://cda.pl' } }]);
  });
  it('błąd, gdy brak adresu', () => {
    expect(() => parseYtDlpResolve({ title: 'x' })).toThrow();
  });
});

describe('ytdlpErrorMessage', () => {
  it('wyciąga linię ERROR', () => {
    expect(ytdlpErrorMessage('WARNING: a\nERROR: [youtube] abc: Video unavailable\n')).toBe(
      '[youtube] abc: Video unavailable',
    );
    expect(ytdlpErrorMessage('cos innego')).toBe('cos innego');
  });
});

describe('YtDlpResolver', () => {
  it('przekazuje runtime JS, cookies i selektor formatu', async () => {
    const calls: string[][] = [];
    const r = new YtDlpResolver({
      ytdlpPath: 'yt-dlp',
      jsRuntime: '/node',
      cookiesFile: 'c.txt',
      run: async (_cmd, args) => {
        calls.push(args);
        return { code: 0, stdout: JSON.stringify(fixture('ytdlp-resolve-split.json')), stderr: '' };
      },
    });
    await r.resolve('https://youtu.be/x');
    expect(calls[0]).toContain('--js-runtimes');
    expect(calls[0]).toContain('node:/node');
    expect(calls[0]).toContain('--cookies');
    expect(calls[0]).toContain('-f');
    expect(calls[0].at(-1)).toBe('https://youtu.be/x');
  });
  it('zamienia błąd yt-dlp na czytelny wyjątek', async () => {
    const r = new YtDlpResolver({
      ytdlpPath: 'yt-dlp',
      run: async () => ({ code: 1, stdout: '', stderr: 'ERROR: Unsupported URL: https://x' }),
    });
    await expect(r.fetchMeta('https://x')).rejects.toThrow('Unsupported URL: https://x');
  });
});

describe('runProcess', () => {
  it('zwraca stdout i kod wyjścia', async () => {
    const r = await runProcess(process.execPath, ['-e', 'process.stdout.write("hej")'], { timeoutMs: 10_000 });
    expect(r).toMatchObject({ code: 0, stdout: 'hej' });
  });
  it('zabija proces po limicie czasu', async () => {
    const start = Date.now();
    await expect(
      runProcess(process.execPath, ['-e', 'setTimeout(()=>{}, 60000)'], { timeoutMs: 300 }),
    ).rejects.toThrow(/limit czasu/);
    expect(Date.now() - start).toBeLessThan(5000);
  });
});

describe('pixeldrain', () => {
  it('rozpoznaje adresy plików i list', () => {
    expect(parsePixeldrainUrl(new URL('https://pixeldrain.com/u/abc123'))).toEqual({ kind: 'file', id: 'abc123' });
    expect(parsePixeldrainUrl(new URL('https://pixeldrain.com/l/lst9#item=2'))).toEqual({ kind: 'list', id: 'lst9' });
    expect(parsePixeldrainUrl(new URL('https://pixeldrain.com/api/file/abc123?download'))).toEqual({
      kind: 'file',
      id: 'abc123',
    });
    expect(parsePixeldrainUrl(new URL('https://pixeldrain.com/'))).toBeNull();
  });

  const fakeFetch = async (url: string) => {
    if (url.endsWith('/api/file/abc/info')) {
      return new Response(JSON.stringify({ id: 'abc', name: 'Film ąę.mp4', mime_type: 'video/mp4' }));
    }
    if (url.endsWith('/api/file/doc/info')) {
      return new Response(JSON.stringify({ id: 'doc', name: 'a.pdf', mime_type: 'application/pdf' }));
    }
    if (url.endsWith('/api/list/L1')) {
      return new Response(
        JSON.stringify({
          id: 'L1',
          title: 'Lista',
          files: [
            { id: 'f1', name: 'one.mkv', mime_type: 'video/x-matroska' },
            { id: 'f2', name: 'readme.txt', mime_type: 'text/plain' },
          ],
        }),
      );
    }
    return new Response('not found', { status: 404 });
  };

  it('metadane pliku z długością z ffprobe', async () => {
    const r = new PixeldrainResolver({ fetch: fakeFetch, probeDuration: async () => 125.5 });
    expect(await r.fetchMeta('https://pixeldrain.com/u/abc')).toEqual({
      title: 'Film ąę.mp4',
      durationS: 125.5,
      isLive: false,
      isPlaylist: false,
    });
  });

  it('odrzuca pliki, które nie są wideo', async () => {
    const r = new PixeldrainResolver({ fetch: fakeFetch, probeDuration: async () => null });
    await expect(r.fetchMeta('https://pixeldrain.com/u/doc')).rejects.toThrow(/nie jest plikiem wideo/);
  });

  it('nieistniejący plik daje czytelny błąd', async () => {
    const r = new PixeldrainResolver({ fetch: fakeFetch, probeDuration: async () => null });
    await expect(r.fetchMeta('https://pixeldrain.com/u/zzz')).rejects.toThrow(/pixeldrain/);
  });

  it('lista staje się playlistą tylko z plikami wideo', async () => {
    const r = new PixeldrainResolver({ fetch: fakeFetch, probeDuration: async () => null });
    const m = await r.fetchMeta('https://pixeldrain.com/l/L1');
    expect(m.isPlaylist).toBe(true);
    expect(m.entries).toEqual([{ url: 'https://pixeldrain.com/u/f1', title: 'one.mkv', durationS: null }]);
  });

  it('resolve podaje adres API i klucz, gdy ustawiony', async () => {
    const r = new PixeldrainResolver({ fetch: fakeFetch, probeDuration: async () => 10, apiKey: 'KEY' });
    const s = await r.resolve('https://pixeldrain.com/u/abc');
    expect(s.inputs[0].url).toBe('https://pixeldrain.com/api/file/abc');
    expect(s.inputs[0].headers.Authorization).toBe(`Basic ${Buffer.from(':KEY').toString('base64')}`);
  });
});

describe('CompositeResolver', () => {
  const tag = (name: string): MediaResolver => ({
    fetchMeta: async () => ({ title: name, durationS: 1, isLive: false, isPlaylist: false }),
    resolve: async () => ({ title: name, durationS: 1, isLive: false, inputs: [] }),
  });
  it('kieruje pixeldrain do własnego modułu, resztę do yt-dlp', async () => {
    const c = new CompositeResolver(tag('pd'), tag('yt'));
    expect((await c.fetchMeta('https://pixeldrain.com/u/a')).title).toBe('pd');
    expect((await c.fetchMeta('https://www.cda.pl/video/1')).title).toBe('yt');
  });
});
