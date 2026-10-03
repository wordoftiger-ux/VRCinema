import { runProcess, type RunFn } from '../util/process.ts';
import type { StreamInput } from '../resolver/types.ts';
import { inputOptions } from './ffmpegArgs.ts';

export interface VideoInfo {
  codec: string;
  height: number;
  pixFmt: string;
  fps: number;
  hasAudio: boolean;
  durationS: number | null;
}

export type VideoMode = 'copy' | 'transcode';

const COPYABLE_PIX_FMTS = ['yuv420p', 'yuvj420p'];

export function decideVideoMode(info: VideoInfo, forceTranscode: boolean): VideoMode {
  if (forceTranscode) return 'transcode';
  return info.codec === 'h264' && info.height > 0 && info.height <= 1080 && COPYABLE_PIX_FMTS.includes(info.pixFmt)
    ? 'copy'
    : 'transcode';
}

function parseRate(rate: unknown): number {
  if (typeof rate !== 'string') return 0;
  const [n, d] = rate.split('/').map(Number);
  if (!n || !d) return 0;
  return Math.round((n / d) * 100) / 100;
}

type Json = Record<string, unknown>;

export function parseProbeOutput(j: Json): VideoInfo {
  const streams = (Array.isArray(j.streams) ? j.streams : []) as Json[];
  const video = streams.find((s) => s.codec_type === 'video' && !(s.disposition as Json | undefined)?.attached_pic);
  if (!video) throw new Error('Źródło nie zawiera ścieżki wideo.');
  const format = (j.format ?? {}) as Json;
  const duration = Number(format.duration);
  return {
    codec: String(video.codec_name ?? ''),
    height: Number(video.height ?? 0),
    pixFmt: String(video.pix_fmt ?? ''),
    fps: parseRate(video.avg_frame_rate) || parseRate(video.r_frame_rate),
    hasAudio: streams.some((s) => s.codec_type === 'audio'),
    durationS: Number.isFinite(duration) && duration > 0 ? duration : null,
  };
}

/** Analizuje źródło ffprobe. Gdy dźwięk przychodzi osobnym wejściem, hasAudio = true. */
export async function probeVideo(
  ffprobePath: string,
  inputs: StreamInput[],
  run: RunFn = runProcess,
  timeoutMs = 45_000,
): Promise<VideoInfo> {
  const [video] = inputs;
  const args = [
    '-v', 'error',
    '-print_format', 'json',
    '-show_streams', '-show_format',
    ...inputOptions(video),
    '-i', video.url,
  ];
  const res = await run(ffprobePath, args, { timeoutMs });
  if (res.code !== 0) throw new Error(`ffprobe: ${res.stderr.trim().split(/\r?\n/).at(-1) || 'nie udało się odczytać źródła'}`);
  const info = parseProbeOutput(JSON.parse(res.stdout) as Json);
  return inputs.length > 1 ? { ...info, hasAudio: true } : info;
}

/** Długość pliku w sekundach albo null. */
export async function probeDuration(
  ffprobePath: string,
  url: string,
  headers: Record<string, string>,
  run: RunFn = runProcess,
): Promise<number | null> {
  try {
    return (await probeVideo(ffprobePath, [{ url, headers }], run, 30_000)).durationS;
  } catch {
    return null;
  }
}
