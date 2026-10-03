import type { MediaMeta, MediaResolver, ResolvedStream } from './types.ts';

const API = 'https://pixeldrain.com/api';
const HOSTS = ['pixeldrain.com', 'pixeldrain.net', 'pixeldra.in'];

export function isPixeldrainHost(url: URL): boolean {
  const host = url.hostname.toLowerCase();
  return HOSTS.some((h) => host === h || host.endsWith(`.${h}`));
}

export function parsePixeldrainUrl(url: URL): { kind: 'file' | 'list'; id: string } | null {
  const m = url.pathname.match(/^\/(?:(u)|(l)|api\/(file)|api\/(list))\/([A-Za-z0-9_-]+)/);
  if (!m) return null;
  return { kind: m[2] || m[4] ? 'list' : 'file', id: m[5] };
}

type FetchFn = (url: string, init?: RequestInit) => Promise<Response>;

export interface PixeldrainOptions {
  fetch?: FetchFn;
  /** Długość pliku w sekundach (ffprobe), bo API pixeldrain jej nie podaje. */
  probeDuration: (url: string, headers: Record<string, string>) => Promise<number | null>;
  apiKey?: string;
}

interface FileInfo {
  id: string;
  name: string;
  mime_type: string;
}

function isMedia(mime: string): boolean {
  return mime.startsWith('video/') || mime.startsWith('audio/');
}

export class PixeldrainResolver implements MediaResolver {
  private fetch: FetchFn;
  private opts: PixeldrainOptions;

  constructor(opts: PixeldrainOptions) {
    this.opts = opts;
    this.fetch = opts.fetch ?? ((u, i) => fetch(u, i));
  }

  private headers(): Record<string, string> {
    return this.opts.apiKey ? { Authorization: `Basic ${Buffer.from(`:${this.opts.apiKey}`).toString('base64')}` } : {};
  }

  private async getJson<T>(url: string): Promise<T> {
    const res = await this.fetch(url, { headers: this.headers(), signal: AbortSignal.timeout(20_000) });
    if (!res.ok) throw new Error(`pixeldrain: plik niedostępny (HTTP ${res.status}).`);
    return (await res.json()) as T;
  }

  private parse(url: string): { kind: 'file' | 'list'; id: string } {
    const parsed = parsePixeldrainUrl(new URL(url));
    if (!parsed) throw new Error('pixeldrain: nierozpoznany adres. Użyj linku /u/… albo /l/….');
    return parsed;
  }

  async fetchMeta(url: string): Promise<MediaMeta> {
    const { kind, id } = this.parse(url);
    if (kind === 'list') {
      const list = await this.getJson<{ title?: string; files?: FileInfo[] }>(`${API}/list/${id}`);
      const entries = (list.files ?? [])
        .filter((f) => isMedia(f.mime_type ?? ''))
        .map((f) => ({ url: `https://pixeldrain.com/u/${f.id}`, title: f.name, durationS: null }));
      return { title: list.title || 'Lista pixeldrain', durationS: null, isLive: false, isPlaylist: true, entries };
    }
    const info = await this.getJson<FileInfo>(`${API}/file/${id}/info`);
    if (!isMedia(info.mime_type ?? '')) throw new Error(`pixeldrain: "${info.name}" nie jest plikiem wideo.`);
    const durationS = await this.opts.probeDuration(`${API}/file/${id}`, this.headers()).catch(() => null);
    return { title: info.name, durationS, isLive: false, isPlaylist: false };
  }

  async resolve(url: string): Promise<ResolvedStream> {
    const { kind, id } = this.parse(url);
    if (kind === 'list') throw new Error('pixeldrain: listę trzeba najpierw rozwinąć na pojedyncze pliki.');
    const info = await this.getJson<FileInfo>(`${API}/file/${id}/info`);
    const streamUrl = `${API}/file/${id}`;
    return {
      inputs: [{ url: streamUrl, headers: this.headers() }],
      title: info.name,
      durationS: await this.opts.probeDuration(streamUrl, this.headers()).catch(() => null),
      isLive: false,
    };
  }
}
