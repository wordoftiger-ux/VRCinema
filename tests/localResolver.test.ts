import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { MediaLibrary } from '../src/library/library.ts';
import { LocalResolver } from '../src/resolver/local.ts';
import { CompositeResolver } from '../src/resolver/index.ts';
import type { MediaResolver } from '../src/resolver/types.ts';

let dir: string;
let lib: MediaLibrary;

beforeAll(() => {
  dir = fs.mkdtempSync(path.join(os.tmpdir(), 'local-'));
  fs.mkdirSync(path.join(dir, 'Sezon 1'));
  fs.writeFileSync(path.join(dir, 'Sezon 1', 'Odcinek 1.mkv'), 'x');
  lib = new MediaLibrary([dir]);
});

afterAll(() => fs.rmSync(dir, { recursive: true, force: true }));

describe('LocalResolver', () => {
  const r = () => new LocalResolver(lib, async () => 1440);

  it('metadane: tytuł z nazwy pliku bez rozszerzenia i długość z ffprobe', async () => {
    expect(await r().fetchMeta(lib.toSourceUrl(0, 'Sezon 1/Odcinek 1.mkv'))).toEqual({
      title: 'Odcinek 1',
      durationS: 1440,
      isLive: false,
      isPlaylist: false,
    });
  });

  it('resolve podaje ścieżkę pliku bez nagłówków', async () => {
    const s = await r().resolve(lib.toSourceUrl(0, 'Sezon 1/Odcinek 1.mkv'));
    expect(s.inputs).toEqual([{ url: path.join(dir, 'Sezon 1', 'Odcinek 1.mkv'), headers: {} }]);
    expect(s.title).toBe('Odcinek 1');
  });

  it('usunięty plik daje błąd „Plik nie istnieje”', async () => {
    await expect(r().resolve(lib.toSourceUrl(0, 'Sezon 1/Usunięty.mkv'))).rejects.toThrow(/nie istnieje/);
  });

  it('folder zamiast pliku daje błąd', async () => {
    await expect(r().resolve(lib.toSourceUrl(0, 'Sezon 1'))).rejects.toThrow(/nie jest plikiem/);
  });
});

describe('CompositeResolver z plikami lokalnymi', () => {
  const tag = (name: string): MediaResolver => ({
    fetchMeta: async () => ({ title: name, durationS: 1, isLive: false, isPlaylist: false }),
    resolve: async () => ({ title: name, durationS: 1, isLive: false, inputs: [] }),
  });
  it('kieruje local: do modułu plików', async () => {
    const c = new CompositeResolver(tag('pd'), tag('yt'), tag('local'));
    expect((await c.fetchMeta('local:0/a.mp4')).title).toBe('local');
    expect((await c.fetchMeta('https://youtu.be/x')).title).toBe('yt');
  });
  it('bez skonfigurowanych folderów local: jest odrzucany', async () => {
    const c = new CompositeResolver(tag('pd'), tag('yt'));
    await expect(c.fetchMeta('local:0/a.mp4')).rejects.toThrow(/pliki z dysku/i);
  });
});
