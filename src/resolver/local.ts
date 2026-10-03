import fs from 'node:fs';
import path from 'node:path';
import { MediaLibrary } from '../library/library.ts';
import type { MediaMeta, MediaResolver, ResolvedStream } from './types.ts';

/** Pliki z dysku właściciela, zapisane w kolejce jako "local:<folder>/<ścieżka względna>". */
export class LocalResolver implements MediaResolver {
  private library: MediaLibrary;
  private probeDuration: (file: string) => Promise<number | null>;

  constructor(library: MediaLibrary, probeDuration: (file: string) => Promise<number | null>) {
    this.library = library;
    this.probeDuration = probeDuration;
  }

  private file(url: string): string {
    const parsed = MediaLibrary.parseSourceUrl(url);
    if (!parsed) throw new Error('Niepoprawny adres pliku z dysku.');
    const full = this.library.resolve(parsed.root, parsed.path);
    if (!fs.statSync(full).isFile()) throw new Error(`To nie jest plikiem wideo: ${path.basename(full)}`);
    return full;
  }

  async fetchMeta(url: string): Promise<MediaMeta> {
    const full = this.file(url);
    return {
      title: path.parse(full).name,
      durationS: await this.probeDuration(full).catch(() => null),
      isLive: false,
      isPlaylist: false,
    };
  }

  async resolve(url: string): Promise<ResolvedStream> {
    const full = this.file(url);
    return {
      inputs: [{ url: full, headers: {} }],
      title: path.parse(full).name,
      durationS: null,
      isLive: false,
    };
  }
}
