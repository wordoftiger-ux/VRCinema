import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { MediaLibrary, LibraryError } from '../src/library/library.ts';

let base: string;
let films: string;
let series: string;
let outside: string;

function touch(p: string) {
  fs.mkdirSync(path.dirname(p), { recursive: true });
  fs.writeFileSync(p, 'x');
}

beforeAll(() => {
  base = fs.mkdtempSync(path.join(os.tmpdir(), 'lib-'));
  films = path.join(base, 'Filmy');
  series = path.join(base, 'Seriale');
  outside = path.join(base, 'Prywatne');
  touch(path.join(films, 'Film A.mp4'));
  touch(path.join(films, 'notatki.txt'));
  touch(path.join(films, 'Okładka.JPG'));
  touch(path.join(films, 'Stary.AVI'));
  for (const n of [10, 2, 1]) touch(path.join(series, 'Sezon 1', `Odcinek ${n}.mkv`));
  touch(path.join(series, 'Sezon 1', 'Podkatalog', 'x.mp4'));
  touch(path.join(outside, 'tajne.mp4'));
  // Dowiązanie (junction) wewnątrz biblioteki, które prowadzi poza nią.
  fs.symlinkSync(outside, path.join(films, 'skrot'), 'junction');
});

afterAll(() => {
  fs.rmSync(base, { recursive: true, force: true });
});

describe('MediaLibrary', () => {
  it('pokazuje foldery główne z nazwami', () => {
    const lib = new MediaLibrary([films, series]);
    expect(lib.roots()).toEqual([
      { index: 0, name: 'Filmy', path: films },
      { index: 1, name: 'Seriale', path: series },
    ]);
  });

  it('listuje foldery i tylko pliki wideo, ukrywa dowiązania wychodzące poza folder', () => {
    const lib = new MediaLibrary([films]);
    const l = lib.list(0, '');
    expect(l.entries.map((e) => [e.type, e.name])).toEqual([
      ['file', 'Film A.mp4'],
      ['file', 'Stary.AVI'],
    ]);
    expect(l.parent).toBeNull();
  });

  it('sortuje naturalnie (2 przed 10) i podaje ścieżki względne', () => {
    const lib = new MediaLibrary([series]);
    const l = lib.list(0, 'Sezon 1');
    expect(l.entries.map((e) => e.name)).toEqual(['Podkatalog', 'Odcinek 1.mkv', 'Odcinek 2.mkv', 'Odcinek 10.mkv']);
    expect(l.entries[1].path).toBe('Sezon 1/Odcinek 1.mkv');
    expect(l.parent).toBe('');
  });

  it('videosIn zwraca filmy z folderu bez podfolderów, w kolejności naturalnej', () => {
    const lib = new MediaLibrary([series]);
    expect(lib.videosIn(0, 'Sezon 1').map((v) => v.path)).toEqual([
      'Sezon 1/Odcinek 1.mkv',
      'Sezon 1/Odcinek 2.mkv',
      'Sezon 1/Odcinek 10.mkv',
    ]);
  });

  it('blokuje wyjście poza folder: .., ścieżki bezwzględne, zły numer folderu', () => {
    const lib = new MediaLibrary([films]);
    expect(() => lib.resolve(0, '../Prywatne/tajne.mp4')).toThrow(LibraryError);
    expect(() => lib.resolve(0, outside)).toThrow(LibraryError);
    expect(() => lib.resolve(0, 'C:/Windows/win.ini')).toThrow(LibraryError);
    expect(() => lib.resolve(5, 'Film A.mp4')).toThrow(LibraryError);
    expect(() => lib.resolve(0, 'Film A.mp4\0.txt')).toThrow(LibraryError);
  });

  it('blokuje dowiązania prowadzące poza bibliotekę', () => {
    const lib = new MediaLibrary([films]);
    expect(() => lib.resolve(0, 'skrot/tajne.mp4')).toThrow(LibraryError);
    expect(() => lib.list(0, 'skrot')).toThrow(LibraryError);
  });

  it('resolve zwraca ścieżkę bezwzględną istniejącego pliku', () => {
    const lib = new MediaLibrary([films]);
    expect(fs.realpathSync(lib.resolve(0, 'Film A.mp4'))).toBe(fs.realpathSync(path.join(films, 'Film A.mp4')));
  });

  it('brakujący plik daje czytelny błąd', () => {
    const lib = new MediaLibrary([films]);
    expect(() => lib.resolve(0, 'Nie ma.mp4')).toThrow(/nie istnieje/);
  });

  it('adres źródła local: działa w obie strony, także z polskimi znakami i spacjami', () => {
    const lib = new MediaLibrary([films]);
    const url = lib.toSourceUrl(0, 'Sezon 1/Żółć #1.mp4');
    expect(url.startsWith('local:0/')).toBe(true);
    expect(MediaLibrary.parseSourceUrl(url)).toEqual({ root: 0, path: 'Sezon 1/Żółć #1.mp4' });
    expect(MediaLibrary.parseSourceUrl('https://youtu.be/x')).toBeNull();
  });
});
