import { isPixeldrainHost } from './pixeldrain.ts';
import type { MediaMeta, MediaResolver, ResolvedStream } from './types.ts';

/** Kieruje pliki z dysku (local:) do modułu plików, pixeldrain do własnego modułu, resztę do yt-dlp. */
export class CompositeResolver implements MediaResolver {
  private pixeldrain: MediaResolver;
  private ytdlp: MediaResolver;
  private local: MediaResolver | null;

  constructor(pixeldrain: MediaResolver, ytdlp: MediaResolver, local: MediaResolver | null = null) {
    this.pixeldrain = pixeldrain;
    this.ytdlp = ytdlp;
    this.local = local;
  }

  private pick(url: string): MediaResolver {
    if (url.startsWith('local:')) {
      if (!this.local) throw new Error('Pliki z dysku są wyłączone (brak MEDIA_DIRS w .env).');
      return this.local;
    }
    return isPixeldrainHost(new URL(url)) ? this.pixeldrain : this.ytdlp;
  }

  async fetchMeta(url: string): Promise<MediaMeta> {
    return this.pick(url).fetchMeta(url);
  }

  async resolve(url: string): Promise<ResolvedStream> {
    return this.pick(url).resolve(url);
  }
}
