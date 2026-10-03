import { runProcess, type RunFn } from '../util/process.ts';
import type { MediaMeta, MediaResolver, ResolvedStream, StreamInput } from './types.ts';

/** Preferuje H.264 do 1080p (można przepakować bez konwersji), w razie braku bierze cokolwiek do 1080p. */
export const FORMAT_SELECTOR =
  'bv*[vcodec^=avc1][height<=1080]+ba/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]/bv*+ba/b';

type Json = Record<string, unknown>;

function num(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : null;
}

function headers(v: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  if (v && typeof v === 'object') {
    for (const [k, val] of Object.entries(v as Json)) if (typeof val === 'string') out[k] = val;
  }
  return out;
}

function isLive(j: Json): boolean {
  return j.live_status === 'is_live' || j.is_live === true;
}

export function parseYtDlpMeta(j: Json): MediaMeta {
  const title = typeof j.title === 'string' && j.title.trim() ? j.title.trim() : 'Bez tytułu';
  if (j._type === 'playlist') {
    const entries = (Array.isArray(j.entries) ? (j.entries as Json[]) : [])
      .filter((e) => e && (typeof e.url === 'string' || typeof e.webpage_url === 'string'))
      .map((e) => ({
        url: String(e.webpage_url ?? e.url),
        title: typeof e.title === 'string' && e.title.trim() ? e.title.trim() : 'Bez tytułu',
        durationS: num(e.duration),
      }));
    return { title, durationS: null, isLive: false, isPlaylist: true, entries };
  }
  return { title, durationS: num(j.duration), isLive: isLive(j), isPlaylist: false };
}

export function parseYtDlpResolve(j: Json): ResolvedStream {
  let inputs: StreamInput[];
  if (Array.isArray(j.requested_formats) && j.requested_formats.length > 0) {
    inputs = (j.requested_formats as Json[]).map((f) => ({ url: String(f.url), headers: headers(f.http_headers) }));
  } else if (typeof j.url === 'string') {
    inputs = [{ url: j.url, headers: headers(j.http_headers) }];
  } else {
    throw new Error('yt-dlp nie zwrócił adresu strumienia.');
  }
  return {
    inputs,
    title: typeof j.title === 'string' && j.title.trim() ? j.title.trim() : 'Bez tytułu',
    durationS: num(j.duration),
    isLive: isLive(j),
  };
}

/** Wyciąga ostatnią linię "ERROR:" z wyjścia yt-dlp. */
export function ytdlpErrorMessage(stderr: string): string {
  const lines = stderr.split(/\r?\n/).filter((l) => l.startsWith('ERROR:'));
  if (lines.length > 0) return lines.at(-1)!.replace(/^ERROR:\s*/, '').trim();
  return stderr.trim().split(/\r?\n/).at(-1) || 'Nieznany błąd yt-dlp';
}

export interface YtDlpOptions {
  ytdlpPath: string;
  /** Ścieżka do node.exe dla rozwiązywania zabezpieczeń YouTube. */
  jsRuntime?: string;
  cookiesFile?: string;
  timeoutMs?: number;
  run?: RunFn;
}

export class YtDlpResolver implements MediaResolver {
  private opts: YtDlpOptions;
  private run: RunFn;

  constructor(opts: YtDlpOptions) {
    this.opts = opts;
    this.run = opts.run ?? runProcess;
  }

  async fetchMeta(url: string): Promise<MediaMeta> {
    return parseYtDlpMeta(await this.exec(['-J', '--flat-playlist', url]));
  }

  async resolve(url: string): Promise<ResolvedStream> {
    return parseYtDlpResolve(await this.exec(['-j', '--no-playlist', '-f', FORMAT_SELECTOR, url]));
  }

  private async exec(args: string[]): Promise<Json> {
    const common = ['--no-progress', '--no-warnings', '--socket-timeout', '20'];
    if (this.opts.jsRuntime) common.push('--js-runtimes', `node:${this.opts.jsRuntime}`);
    if (this.opts.cookiesFile) common.push('--cookies', this.opts.cookiesFile);
    const url = args.at(-1)!;
    const res = await this.run(this.opts.ytdlpPath, [...common, ...args.slice(0, -1), '--', url], {
      timeoutMs: this.opts.timeoutMs ?? 60_000,
    });
    if (res.code !== 0) throw new Error(ytdlpErrorMessage(res.stderr));
    try {
      return JSON.parse(res.stdout) as Json;
    } catch {
      throw new Error('yt-dlp zwrócił niepoprawne dane.');
    }
  }
}
