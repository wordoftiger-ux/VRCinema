import type { ChannelSettings } from '../db/settings.ts';
import type { MediaMeta } from '../resolver/types.ts';

/** Zwraca URL tylko dla poprawnych adresów http(s). */
export function parseSourceUrl(raw: string): URL | null {
  const text = raw.trim();
  if (!text) return null;
  try {
    const url = new URL(text);
    return url.protocol === 'http:' || url.protocol === 'https:' ? url : null;
  } catch {
    return null;
  }
}

/** Domena dozwolona, gdy host jest równy domenie z listy albo jest jej poddomeną. */
export function isAllowedDomain(url: URL, domains: string[]): boolean {
  const host = url.hostname.toLowerCase().replace(/\.$/, '');
  return domains.some((d) => {
    const domain = d.toLowerCase();
    return host === domain || host.endsWith(`.${domain}`);
  });
}

function formatDuration(s: number): string {
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h} h ${m} min` : `${m} min`;
}

/** Zwraca powód odrzucenia albo null, gdy film może trafić do kolejki. */
export function validateMeta(
  meta: Pick<MediaMeta, 'durationS' | 'isLive' | 'isPlaylist'>,
  settings: ChannelSettings,
  isAdmin: boolean,
): string | null {
  if (isAdmin) return null;
  if (meta.isPlaylist) return 'Playlisty może dodawać tylko właściciel kanału.';
  if (meta.isLive) return 'Transmisje na żywo może dodawać tylko właściciel kanału.';
  if (meta.durationS !== null && meta.durationS > settings.maxDurationS) {
    return `Film jest za długi (limit: ${formatDuration(settings.maxDurationS)}).`;
  }
  return null;
}

/** Prosty limiter z przesuwanym oknem, trzymany w pamięci. */
export class RateLimiter {
  private hits = new Map<string, number[]>();
  private limit: number;
  private windowMs: number;
  private now: () => number;

  constructor(limit: number, windowMs: number, now: () => number = Date.now) {
    this.limit = limit;
    this.windowMs = windowMs;
    this.now = now;
  }

  setLimits(limit: number, windowMs: number): void {
    this.limit = limit;
    this.windowMs = windowMs;
  }

  /** Rejestruje próbę. Zwraca false, gdy limit w oknie został przekroczony. */
  hit(key: string): boolean {
    const now = this.now();
    const recent = (this.hits.get(key) ?? []).filter((t) => now - t < this.windowMs);
    if (recent.length >= this.limit) {
      this.hits.set(key, recent);
      return false;
    }
    recent.push(now);
    this.hits.set(key, recent);
    if (this.hits.size > 10_000) this.prune(now);
    return true;
  }

  /** Liczba prób w bieżącym oknie, bez rejestrowania nowej. */
  peek(key: string): number {
    const now = this.now();
    return (this.hits.get(key) ?? []).filter((t) => now - t < this.windowMs).length;
  }

  private prune(now: number): void {
    for (const [key, times] of this.hits) {
      if (times.every((t) => now - t >= this.windowMs)) this.hits.delete(key);
    }
  }
}
