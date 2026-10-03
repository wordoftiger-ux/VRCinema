import fs from 'node:fs';
import path from 'node:path';
import type { VideoEncoder } from '../config.ts';
import type { QueueItem } from '../db/queue.ts';
import type { SettingsRepo } from '../db/settings.ts';
import type { MediaResolver } from '../resolver/types.ts';
import type { ClipFactory, ClipHandle } from './channel.ts';
import { buildClipArgs } from './ffmpegArgs.ts';
import { decideVideoMode, probeVideo, type VideoInfo } from './probe.ts';
import { ClipSegmenter } from './segmenter.ts';

/** Tempo czytania źródła: 1,5× czasu rzeczywistego daje zapas bez ściągania całego filmu na dysk. */
export const READRATE = 1.5;

export interface ClipFactoryDeps {
  resolver: MediaResolver;
  ffmpegPath: string;
  ffprobePath: string;
  encoder: VideoEncoder;
  segmentSeconds: number;
  clipsDir: string;
  settings: SettingsRepo;
  channelId: number;
  log?: (msg: string) => void;
  /** Podmiana analizy w testach. */
  probe?: typeof probeVideo;
}

function removeDir(dir: string): void {
  try {
    fs.rmSync(dir, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 });
  } catch {
    // Windows potrafi chwilę trzymać blokadę pliku; katalog zostanie wyczyszczony przy następnym starcie.
  }
}

export function createClipFactory(d: ClipFactoryDeps): ClipFactory {
  let counter = 0;
  return {
    async create(item: QueueItem, startAtS: number, attempt: number): Promise<ClipHandle> {
      const stream = await d.resolver.resolve(item.sourceUrl);
      const info: VideoInfo = await (d.probe ?? probeVideo)(d.ffprobePath, stream.inputs);
      const mode = decideVideoMode(info, d.settings.get(d.channelId).forceTranscode);
      const live = item.isLive || stream.isLive;
      const outDir = path.join(d.clipsDir, `${item.id}-${attempt}-${++counter}`);
      d.log?.(
        `Film #${item.id} "${item.title}": ${mode === 'copy' ? 'przepakowanie' : 'konwersja'} ` +
          `(${info.codec} ${info.height}p ${info.fps} kl./s${info.hasAudio ? '' : ', bez dźwięku'})` +
          (startAtS > 0 ? `, wznowienie od ${Math.round(startAtS)} s` : ''),
      );
      const seg = new ClipSegmenter({
        ffmpegPath: d.ffmpegPath,
        outDir,
        args: buildClipArgs({
          inputs: stream.inputs,
          mode,
          info,
          encoder: d.encoder,
          segmentSeconds: d.segmentSeconds,
          outDir,
          startAtS: live ? 0 : startAtS,
          readrate: live ? null : READRATE,
        }),
      });
      seg.start();
      return {
        get state() {
          return seg.state;
        },
        get error() {
          return seg.error;
        },
        durationS: info.durationS ?? stream.durationS,
        poll: () => seg.poll(),
        kill: () => seg.kill(),
        dispose: () => {
          if (seg.state === 'running') {
            seg.once('exit', () => removeDir(outDir));
            seg.kill();
          } else {
            removeDir(outDir);
          }
        },
      };
    },
  };
}
