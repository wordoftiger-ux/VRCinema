import fs from 'node:fs';
import type { QueueItem, QueueRepo } from '../db/queue.ts';
import type { SettingsRepo } from '../db/settings.ts';
import type { SegmentInfo, SegmenterState } from './segmenter.ts';
import type { Slates, SlateKind } from './slates.ts';
import type { PublishedSegment, SegmentSource, SourceSegment } from './broadcaster.ts';

/** Uruchomiony proces cięcia jednego filmu (prawdziwy: ClipSegmenter; w testach: atrapa). */
export interface ClipHandle {
  state: SegmenterState;
  error: string | null;
  /** Długość źródła z ffprobe (może uzupełnić brak w kolejce). */
  durationS: number | null;
  poll(): SegmentInfo[];
  kill(): void;
  /** Zabija proces i usuwa pliki klipu. */
  dispose(): void;
}

export interface ClipFactory {
  /** Rozpoznaje źródło, analizuje je i uruchamia cięcie od startAtS. Odrzuca przy błędzie. */
  create(item: QueueItem, startAtS: number, attempt: number): Promise<ClipHandle>;
}

export type ChannelStatus = 'idle' | 'starting' | 'playing' | 'buffering';

export interface ChannelState {
  status: ChannelStatus;
  current: {
    itemId: number;
    title: string;
    durationS: number | null;
    positionS: number;
    requesterName: string | null;
    isLive: boolean;
  } | null;
}

export interface ChannelOptions {
  channelId: number;
  queue: QueueRepo;
  settings: SettingsRepo;
  factory: ClipFactory;
  slates: Slates;
  now?: () => number;
  log?: (msg: string) => void;
  /** Usuwanie wyemitowanych plików klipu (wstrzykiwane w testach). */
  removeFile?: (path: string) => void;
  stallTimeoutS?: number;
  startTimeoutS?: number;
  prefetchS?: number;
  /** O ile fragmentów widzowie są za najnowszym opublikowanym (do pokazywania "teraz leci"). */
  viewerLagSegments?: number;
}

interface ActiveClip {
  item: QueueItem;
  attempt: number;
  /** Sekunda filmu, od której wystartował bieżący proces. */
  baseS: number;
  handle: ClipHandle | null;
  startError: string | null;
  startedAt: number;
  cancelled: boolean;
  ready: SegmentInfo[];
  /** Sekundy wyemitowane z bieżącego procesu. */
  emittedS: number;
  emittedCount: number;
  stalledSince: number | null;
}

interface SegmentMeta {
  kind: 'clip' | SlateKind;
  item: QueueItem | null;
  positionS: number;
}

const STOP_STATES: SegmenterState[] = ['done', 'failed', 'killed'];

/**
 * Orkiestracja kanału: wybiera, co ma lecieć, i dostarcza Nadajnikowi kolejne fragmenty.
 * Zasada: next() zawsze coś zwraca — fragment filmu albo planszę — więc kanał nigdy nie milknie.
 */
export class Channel implements SegmentSource {
  private o: ChannelOptions;
  private now: () => number;
  private current: ActiveClip | null = null;
  private prefetched: ActiveClip | null = null;
  private slateIndex: Record<SlateKind, number> = { idle: 0, buffering: 0 };
  private lastSlateKind: SlateKind | null = null;
  private previousClipSegment: string | null = null;
  private pendingMeta = new Map<string, SegmentMeta>();
  private aired: SegmentMeta[] = [];

  constructor(o: ChannelOptions) {
    this.o = o;
    this.now = o.now ?? Date.now;
  }

  start(): void {
    this.o.queue.resetPlaying(this.o.channelId);
  }

  stop(): void {
    this.current?.handle?.dispose();
    this.prefetched?.handle?.dispose();
    if (this.current) this.current.cancelled = true;
    if (this.prefetched) this.prefetched.cancelled = true;
    this.current = null;
    this.prefetched = null;
  }

  // ---------- SegmentSource ----------

  next(): SourceSegment {
    this.releasePreviousSegment();
    for (let guard = 0; guard < 50; guard++) {
      const c = this.current ?? this.promote();
      if (!c) return this.slate('idle');

      if (c.startError !== null) {
        this.handleFailure(c, c.startError);
        continue;
      }
      if (c.handle) c.ready.push(...c.handle.poll());

      const seg = c.ready.shift();
      if (seg) return this.emitClip(c, seg);

      if (c.handle && c.handle.state === 'done') {
        this.finish(c, 'played', null);
        continue;
      }
      if (c.handle && (c.handle.state === 'failed' || c.handle.state === 'killed')) {
        this.handleFailure(c, c.handle.error ?? 'ffmpeg przerwał pracę');
        continue;
      }

      const now = this.now();
      if (!c.handle) {
        if (now - c.startedAt > (this.o.startTimeoutS ?? 120) * 1000) {
          this.handleFailure(c, 'Przekroczono czas uruchamiania źródła');
          continue;
        }
        return this.slate('buffering');
      }
      c.stalledSince ??= now;
      if (now - c.stalledSince > (this.o.stallTimeoutS ?? 30) * 1000) {
        c.handle.kill();
        this.finish(c, 'failed', 'Źródło nie dostarczało danych przez 30 s');
        continue;
      }
      return this.slate('buffering');
    }
    return this.slate('idle');
  }

  // ---------- Sterowanie ----------

  skip(): void {
    if (this.current) this.finish(this.current, 'played', 'Pominięty przez właściciela');
  }

  /** Wywoływane po zmianach w kolejce (usunięcie, przesunięcie, akceptacja). */
  onQueueChanged(): void {
    if (this.current) {
      const fresh = this.o.queue.get(this.current.item.id);
      if (!fresh || fresh.status !== 'playing') this.drop(this.current, true);
    }
    if (this.prefetched) {
      const head = this.o.queue.nextQueued(this.o.channelId);
      if (!head || head.id !== this.prefetched.item.id) this.drop(this.prefetched, false);
    }
  }

  onPublished(seg: PublishedSegment): void {
    const meta = this.pendingMeta.get(seg.path);
    if (!meta) return;
    if (meta.kind === 'clip') this.pendingMeta.delete(seg.path);
    this.aired.push(meta);
    if (this.aired.length > 20) this.aired.shift();
  }

  state(): ChannelState {
    const lag = this.o.viewerLagSegments ?? 3;
    const meta = this.aired[Math.max(0, this.aired.length - lag)];
    if (!meta) return { status: 'idle', current: null };
    if (meta.kind === 'idle') return { status: 'idle', current: null };
    if (!meta.item) return { status: 'buffering', current: null };
    return {
      status: meta.kind === 'clip' ? 'playing' : 'buffering',
      current: {
        itemId: meta.item.id,
        title: meta.item.title,
        durationS: meta.item.durationS,
        positionS: Math.round(meta.positionS),
        requesterName: meta.item.requesterName,
        isLive: meta.item.isLive,
      },
    };
  }

  /** Pozycja wyprzedzana w tle (do pokazania w panelu). */
  get prefetchedItemId(): number | null {
    return this.prefetched?.item.id ?? null;
  }

  // ---------- Wewnętrzne ----------

  private emitClip(c: ActiveClip, seg: SegmentInfo): SourceSegment {
    const positionS = c.baseS + c.emittedS;
    c.emittedS += seg.duration;
    c.emittedCount++;
    c.stalledSince = null;
    this.lastSlateKind = null;
    this.previousClipSegment = seg.path;
    this.pendingMeta.set(seg.path, { kind: 'clip', item: c.item, positionS });
    this.maybePrefetch(c);
    return { path: seg.path, duration: seg.duration, sourceKey: `clip-${c.item.id}-${c.attempt}`, index: seg.index };
  }

  private slate(kind: SlateKind): SourceSegment {
    const list = this.o.slates[kind];
    if (this.lastSlateKind !== kind) this.slateIndex[kind] = 0;
    this.lastSlateKind = kind;
    const index = this.slateIndex[kind] % list.length;
    this.slateIndex[kind] = index + 1;
    const seg = list[index];
    const item = kind === 'buffering' ? (this.current?.item ?? null) : null;
    const positionS = this.current ? this.current.baseS + this.current.emittedS : 0;
    this.pendingMeta.set(seg.path, { kind, item, positionS });
    return { path: seg.path, duration: seg.duration, sourceKey: kind, index };
  }

  /** Bieżący film się skończył albo nie ma żadnego: bierze wyprzedzony lub następny z kolejki. */
  private promote(): ActiveClip | null {
    const head = this.o.queue.nextQueued(this.o.channelId);
    if (this.prefetched && (!head || head.id !== this.prefetched.item.id)) this.drop(this.prefetched, false);
    if (!head) return null;
    const clip = this.prefetched ?? this.launch(head, 0, 0);
    this.prefetched = null;
    this.o.queue.setStatus(head.id, 'playing');
    clip.item = this.o.queue.get(head.id) ?? head;
    this.current = clip;
    return clip;
  }

  private maybePrefetch(c: ActiveClip): void {
    if (this.prefetched || c.item.isLive) return;
    const duration = c.item.durationS ?? c.handle?.durationS ?? null;
    if (duration === null) return;
    const remaining = duration - (c.baseS + c.emittedS);
    if (remaining > (this.o.prefetchS ?? 60)) return;
    const head = this.o.queue.nextQueued(this.o.channelId);
    if (head) this.prefetched = this.launch(head, 0, 0);
  }

  private launch(item: QueueItem, startAtS: number, attempt: number): ActiveClip {
    const clip: ActiveClip = {
      item,
      attempt,
      baseS: startAtS,
      handle: null,
      startError: null,
      startedAt: this.now(),
      cancelled: false,
      ready: [],
      emittedS: 0,
      emittedCount: 0,
      stalledSince: null,
    };
    this.o.factory.create(item, startAtS, attempt).then(
      (handle) => {
        if (clip.cancelled) handle.dispose();
        else clip.handle = handle;
      },
      (e: Error) => {
        clip.startError = e.message || 'Nieznany błąd';
      },
    );
    return clip;
  }

  private handleFailure(c: ActiveClip, error: string): void {
    this.o.log?.(`Film #${c.item.id} "${c.item.title}": ${error}`);
    c.handle?.dispose();
    if (c.attempt === 0) {
      // Jedna ponowna próba od miejsca przerwania (z nowym rozpoznaniem adresu, bo stary mógł wygasnąć).
      const resumeAt = c.item.isLive ? 0 : c.baseS + c.emittedS;
      this.current = this.launch(c.item, resumeAt, 1);
      return;
    }
    this.finish(c, 'failed', error);
  }

  private finish(c: ActiveClip, status: 'played' | 'failed', error: string | null): void {
    this.o.queue.setStatus(c.item.id, status, error);
    this.drop(c, true);
  }

  private drop(c: ActiveClip, isCurrent: boolean): void {
    c.cancelled = true;
    c.handle?.dispose();
    if (isCurrent && this.current === c) this.current = null;
    if (!isCurrent && this.prefetched === c) this.prefetched = null;
  }

  /** Nadajnik czyta fragment od razu po next(), więc przy kolejnym wywołaniu można go skasować. */
  private releasePreviousSegment(): void {
    if (!this.previousClipSegment) return;
    const p = this.previousClipSegment;
    this.previousClipSegment = null;
    (this.o.removeFile ?? ((f: string) => fs.rmSync(f, { force: true })))(p);
  }
}
