import path from 'node:path';
import type { VideoEncoder } from '../config.ts';
import type { StreamInput } from '../resolver/types.ts';
import type { VideoInfo, VideoMode } from './probe.ts';

export const VIDEO_BITRATE = '6M';
export const MAX_HEIGHT = 1080;
export const MAX_FPS = 30;

function isNetwork(url: string): boolean {
  return /^https?:\/\//i.test(url);
}

/** Opcje wejściowe wspólne dla ffmpeg i ffprobe: ponowne łączenie i nagłówki HTTP. */
export function inputOptions(input: StreamInput): string[] {
  const args: string[] = [];
  if (isNetwork(input.url)) {
    args.push(
      '-reconnect', '1',
      '-reconnect_streamed', '1',
      '-reconnect_on_network_error', '1',
      '-reconnect_delay_max', '5',
      '-rw_timeout', '20000000',
    );
  }
  const headerLines = Object.entries(input.headers).map(([k, v]) => `${k}: ${v}\r\n`);
  if (headerLines.length > 0) args.push('-headers', headerLines.join(''));
  return args;
}

export function encoderArgs(encoder: VideoEncoder): string[] {
  const rate = ['-b:v', VIDEO_BITRATE, '-maxrate', VIDEO_BITRATE, '-bufsize', '12M'];
  switch (encoder) {
    case 'h264_nvenc':
      return ['-c:v', 'h264_nvenc', '-preset', 'p4', '-profile:v', 'high', '-rc', 'cbr', ...rate];
    case 'h264_qsv':
      return ['-c:v', 'h264_qsv', '-preset', 'faster', '-profile:v', 'high', ...rate];
    case 'h264_amf':
      return ['-c:v', 'h264_amf', '-quality', 'speed', '-rc', 'cbr', ...rate];
    default:
      return ['-c:v', 'libx264', '-preset', 'veryfast', '-profile:v', 'high', '-level:v', '4.1', '-sc_threshold', '0', ...rate];
  }
}

export const AUDIO_ARGS = ['-c:a', 'aac', '-b:a', '160k', '-ar', '48000', '-ac', '2'];

export function segmentOutputArgs(segmentSeconds: number, outDir: string): string[] {
  return [
    '-max_muxing_queue_size', '1024',
    '-f', 'segment',
    '-segment_time', String(segmentSeconds),
    '-segment_format', 'mpegts',
    '-segment_list', path.join(outDir, 'list.csv'),
    '-segment_list_type', 'csv',
    '-reset_timestamps', '0',
    path.join(outDir, 'seg_%05d.ts'),
  ];
}

export interface ClipArgsOptions {
  inputs: StreamInput[];
  /** Format wejścia, np. "lavfi" dla sztucznych źródeł. */
  inputFormat?: string;
  mode: VideoMode;
  info: VideoInfo;
  encoder: VideoEncoder;
  segmentSeconds: number;
  outDir: string;
  /** Wznowienie od tej sekundy (0 = od początku). */
  startAtS: number;
  /** Mnożnik tempa czytania źródła względem czasu rzeczywistego; null = bez ograniczenia. */
  readrate: number | null;
}

export function buildClipArgs(o: ClipArgsOptions): string[] {
  const args = ['-hide_banner', '-loglevel', 'error', '-nostdin', '-y'];
  for (const input of o.inputs) {
    args.push(...inputOptions(input));
    if (o.startAtS > 0) args.push('-ss', String(o.startAtS));
    if (o.readrate !== null) args.push('-readrate', String(o.readrate), '-readrate_initial_burst', '20');
    if (o.inputFormat) args.push('-f', o.inputFormat);
    args.push('-i', input.url);
  }

  let audioMap: string;
  const addSilence = o.inputs.length === 1 && !o.info.hasAudio;
  if (o.inputs.length > 1) audioMap = '1:a:0';
  else if (!addSilence) audioMap = '0:a:0';
  else {
    args.push('-f', 'lavfi', '-i', 'anullsrc=channel_layout=stereo:sample_rate=48000');
    audioMap = `${o.inputs.length}:a:0`;
  }
  args.push('-map', '0:v:0', '-map', audioMap);
  if (addSilence) args.push('-shortest');

  if (o.mode === 'copy') {
    args.push('-c:v', 'copy');
  } else {
    const filters = [`scale=-2:'min(${MAX_HEIGHT},ih)'`];
    if (o.info.fps > MAX_FPS + 0.5) filters.push(`fps=${MAX_FPS}`);
    filters.push('format=yuv420p');
    args.push(
      '-vf', filters.join(','),
      ...encoderArgs(o.encoder),
      '-force_key_frames', `expr:gte(t,n_forced*${o.segmentSeconds})`,
    );
  }
  args.push(...AUDIO_ARGS, ...segmentOutputArgs(o.segmentSeconds, o.outDir));
  return args;
}
