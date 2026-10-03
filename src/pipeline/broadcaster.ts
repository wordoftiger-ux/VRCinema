import fs from 'node:fs';
import path from 'node:path';
import { TsRewriter, minDts } from './tsrewrite.ts';
import { buildLivePlaylist, type PlaylistSegment } from './playlist.ts';

/** Początek osi czasu kanału (10 s), żeby przesunięte DTS nigdy nie były ujemne. */
export const START_PTS = 10 * 90_000;

/** Fragment dostarczany przez źródło (Kanał). */
export interface SourceSegment {
  path: string;
  duration: number;
  /** Identyfikator źródła, np. "clip-12-0" albo "idle". Zmiana oznacza nieciągłość. */
  sourceKey: string;
  /** Numer fragmentu w obrębie źródła. Kolejny numer = ta sama oś czasu. */
  index: number;
}

export interface SegmentSource {
  /** Zwraca następny fragment do emisji. Musi zawsze coś zwrócić (np. planszę). */
  next(): SourceSegment;
}

export interface PublishedSegment extends SourceSegment {
  seq: number;
  /** Czas kanału (s) na początku fragmentu. */
  channelTime: number;
}

export interface BroadcasterOptions {
  dir: string;
  windowSize: number;
  segmentSeconds: number;
  pathPrefix: string;
  source: SegmentSource;
  discontinuityTags: () => boolean;
  now?: () => number;
  onPublished?: (seg: PublishedSegment) => void;
  /** Ile sekund trzymać pliki po wypadnięciu z okna (dla wolniejszych widzów). */
  retainS?: number;
  log?: (msg: string) => void;
}

const INITIAL_SEGMENTS = 3;
const MAX_SOURCE_ATTEMPTS = 5;

/**
 * Publikuje fragmenty w tempie czasu rzeczywistego, przepisując znaczniki czasu na wspólną oś kanału,
 * i prowadzi listę HLS na żywo z oknem ostatnich fragmentów.
 */
export class Broadcaster {
  private o: BroadcasterOptions;
  private now: () => number;
  private rewriter = new TsRewriter();
  private published: PlaylistSegment[] = [];
  private retired: { name: string; at: number }[] = [];
  private seq = 0;
  private clock = 0;
  private offset = 0;
  private last: SourceSegment | null = null;
  private targetDuration: number;
  private discontinuitySequence = 0;
  private nextPublishAt = 0;
  private timer: NodeJS.Timeout | null = null;

  constructor(o: BroadcasterOptions) {
    this.o = o;
    this.now = o.now ?? Date.now;
    this.targetDuration = o.segmentSeconds;
  }

  /** Czyści katalog, publikuje 3 fragmenty na zapas i (domyślnie) uruchamia zegar. */
  start(withTimer = true): void {
    fs.rmSync(this.o.dir, { recursive: true, force: true });
    fs.mkdirSync(this.o.dir, { recursive: true });
    let first = 0;
    for (let i = 0; i < INITIAL_SEGMENTS; i++) {
      const d = this.publishOne();
      if (i === 0) first = d;
    }
    this.nextPublishAt = this.now() + first * 1000;
    if (withTimer) this.timer = setInterval(() => this.tick(), 250);
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  tick(): void {
    let guard = 0;
    while (this.now() >= this.nextPublishAt && guard++ < 100) {
      this.nextPublishAt += this.publishOne() * 1000;
    }
    this.cleanup();
  }

  /** Czas kanału w sekundach (suma opublikowanych fragmentów). */
  get channelTime(): number {
    return this.clock;
  }

  playlist(): string {
    return buildLivePlaylist({
      segments: this.published,
      targetDuration: this.targetDuration,
      discontinuitySequence: this.discontinuitySequence,
      pathPrefix: this.o.pathPrefix,
    });
  }

  segmentPath(name: string): string | null {
    if (!/^\d+\.ts$/.test(name)) return null;
    const known = this.published.some((s) => s.name === name) || this.retired.some((s) => s.name === name);
    return known ? path.join(this.o.dir, name) : null;
  }

  /** Publikuje jeden fragment i zwraca jego długość w sekundach. */
  private publishOne(): number {
    for (let attempt = 0; attempt < MAX_SOURCE_ATTEMPTS; attempt++) {
      const seg = this.o.source.next();
      try {
        return this.publish(seg);
      } catch (e) {
        this.o.log?.(`Pominięto uszkodzony fragment ${seg.path}: ${(e as Error).message}`);
      }
    }
    // Źródło uparcie zwraca złe dane: krótka przerwa i spróbujemy w następnym takcie.
    return 1;
  }

  private publish(seg: SourceSegment): number {
    const input = fs.readFileSync(seg.path);
    const continuing = this.last !== null && seg.sourceKey === this.last.sourceKey && seg.index === this.last.index + 1;
    let offset = this.offset;
    if (!continuing) {
      const base = minDts(input) ?? 0;
      offset = Math.round(START_PTS + this.clock * 90_000) - base;
    }
    const output = this.rewriter.rewrite(input, offset);
    const sourceChanged = this.last !== null && seg.sourceKey !== this.last.sourceKey;

    const seq = this.seq++;
    const name = `${seq}.ts`;
    fs.writeFileSync(path.join(this.o.dir, name), output);

    const published: PublishedSegment = { ...seg, seq, channelTime: this.clock };
    this.offset = offset;
    this.last = seg;
    this.clock += seg.duration;
    this.targetDuration = Math.max(this.targetDuration, Math.ceil(seg.duration));
    this.published.push({
      seq,
      name,
      duration: seg.duration,
      discontinuity: sourceChanged && this.o.discontinuityTags(),
    });
    while (this.published.length > this.o.windowSize) {
      const removed = this.published.shift()!;
      if (removed.discontinuity) this.discontinuitySequence++;
      this.retired.push({ name: removed.name, at: this.now() });
    }
    this.o.onPublished?.(published);
    return seg.duration;
  }

  private cleanup(): void {
    const limit = this.now() - (this.o.retainS ?? 60) * 1000;
    while (this.retired.length > 0 && this.retired[0].at < limit) {
      const { name } = this.retired.shift()!;
      try {
        fs.rmSync(path.join(this.o.dir, name), { force: true });
      } catch {
        // Plik może być akurat wysyłany widzowi (Windows blokuje usuwanie); spróbujemy przy kolejnym fragmencie.
        this.retired.unshift({ name, at: this.now() - (this.o.retainS ?? 60) * 1000 + 4000 });
        break;
      }
    }
  }
}
