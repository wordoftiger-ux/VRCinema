import { isPixeldrainHost } from './pixeldrain.ts';
import type { MediaMeta, MediaResolver, ResolvedStream } from './types.ts';

/** Kieruje linki pixeldrain do własnego modułu, a wszystko inne do yt-dlp. */
export class CompositeResolver implements MediaResolver {
  private pixeldrain: MediaResolver;
  private ytdlp: MediaResolver;

  constructor(pixeldrain: MediaResolver, ytdlp: MediaResolver) {
    this.pixeldrain = pixeldrain;
    this.ytdlp = ytdlp;
  }

  private pick(url: string): MediaResolver {
    return isPixeldrainHost(new URL(url)) ? this.pixeldrain : this.ytdlp;
  }

  fetchMeta(url: string): Promise<MediaMeta> {
    return this.pick(url).fetchMeta(url);
  }

  resolve(url: string): Promise<ResolvedStream> {
    return this.pick(url).resolve(url);
  }
}
