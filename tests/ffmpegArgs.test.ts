import { describe, it, expect } from 'vitest';
import path from 'node:path';
import { buildClipArgs, type ClipArgsOptions } from '../src/pipeline/ffmpegArgs.ts';
import { decideVideoMode, parseProbeOutput, type VideoInfo } from '../src/pipeline/probe.ts';

const h264: VideoInfo = { codec: 'h264', height: 1080, pixFmt: 'yuv420p', fps: 30, hasAudio: true, durationS: 60 };

function opts(over: Partial<ClipArgsOptions> = {}): ClipArgsOptions {
  return {
    inputs: [{ url: 'https://cdn/v.mp4', headers: { 'User-Agent': 'UA', Referer: 'https://x' } }],
    mode: 'copy',
    info: h264,
    encoder: 'libx264',
    segmentSeconds: 4,
    outDir: '/tmp/clip',
    startAtS: 0,
    readrate: 1.5,
    ...over,
  };
}

function valueAfter(args: string[], flag: string, nth = 0): string | undefined {
  let seen = 0;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === flag) {
      if (seen === nth) return args[i + 1];
      seen++;
    }
  }
  return undefined;
}

describe('decideVideoMode', () => {
  it('przepakowuje H.264 do 1080p w yuv420p', () => {
    expect(decideVideoMode(h264, false)).toBe('copy');
  });
  it('konwertuje inne kodeki, wyższe rozdzielczości i formaty pikseli', () => {
    expect(decideVideoMode({ ...h264, codec: 'vp9' }, false)).toBe('transcode');
    expect(decideVideoMode({ ...h264, height: 1440 }, false)).toBe('transcode');
    expect(decideVideoMode({ ...h264, pixFmt: 'yuv420p10le' }, false)).toBe('transcode');
  });
  it('forceTranscode wymusza konwersję', () => {
    expect(decideVideoMode(h264, true)).toBe('transcode');
  });
});

describe('parseProbeOutput', () => {
  it('odczytuje wideo, dźwięk, fps i długość', () => {
    const info = parseProbeOutput({
      streams: [
        { codec_type: 'video', codec_name: 'h264', height: 720, pix_fmt: 'yuv420p', avg_frame_rate: '30000/1001' },
        { codec_type: 'audio', codec_name: 'aac' },
      ],
      format: { duration: '123.45' },
    });
    expect(info).toEqual({ codec: 'h264', height: 720, pixFmt: 'yuv420p', fps: 29.97, hasAudio: true, durationS: 123.45 });
  });
  it('rzuca błąd, gdy brak wideo', () => {
    expect(() => parseProbeOutput({ streams: [{ codec_type: 'audio' }], format: {} })).toThrow(/wideo/);
  });
});

describe('buildClipArgs', () => {
  it('przepakowanie: kopiuje wideo, konwertuje dźwięk, segmentuje do CSV', () => {
    const a = opts();
    const args = buildClipArgs(a);
    expect(valueAfter(args, '-c:v')).toBe('copy');
    expect(valueAfter(args, '-c:a')).toBe('aac');
    expect(valueAfter(args, '-ar')).toBe('48000');
    expect(valueAfter(args, '-ac')).toBe('2');
    expect(valueAfter(args, '-b:a')).toBe('160k');
    expect(valueAfter(args, '-f')).toBe('segment');
    expect(valueAfter(args, '-segment_time')).toBe('4');
    expect(valueAfter(args, '-segment_list_type')).toBe('csv');
    expect(valueAfter(args, '-segment_list')).toBe(path.join('/tmp/clip', 'list.csv'));
    expect(args.at(-1)).toBe(path.join('/tmp/clip', 'seg_%05d.ts'));
    expect(valueAfter(args, '-map', 0)).toBe('0:v:0');
    expect(valueAfter(args, '-map', 1)).toBe('0:a:0');
  });

  it('nagłówki i readrate stoją przed -i', () => {
    const args = buildClipArgs(opts());
    const i = args.indexOf('-i');
    const h = args.indexOf('-headers');
    expect(h).toBeGreaterThan(-1);
    expect(h).toBeLessThan(i);
    expect(args[h + 1]).toBe('User-Agent: UA\r\nReferer: https://x\r\n');
    expect(args.indexOf('-readrate')).toBeLessThan(i);
    expect(valueAfter(args, '-readrate')).toBe('1.5');
  });

  it('osobne wejścia wideo i dźwięku mapuje z dwóch plików', () => {
    const args = buildClipArgs(
      opts({ inputs: [{ url: 'https://v', headers: {} }, { url: 'https://a', headers: {} }] }),
    );
    expect(args.filter((x) => x === '-i')).toHaveLength(2);
    expect(valueAfter(args, '-map', 1)).toBe('1:a:0');
    expect(args).not.toContain('-headers');
  });

  it('brak dźwięku: dokłada ciszę z anullsrc', () => {
    const args = buildClipArgs(opts({ info: { ...h264, hasAudio: false } }));
    expect(args).toContain('anullsrc=channel_layout=stereo:sample_rate=48000');
    expect(valueAfter(args, '-map', 1)).toBe('1:a:0');
    expect(args).toContain('-shortest');
  });

  it('wznowienie: -ss przed każdym wejściem', () => {
    const args = buildClipArgs(
      opts({ startAtS: 75.5, inputs: [{ url: 'https://v', headers: {} }, { url: 'https://a', headers: {} }] }),
    );
    expect(args.filter((x) => x === '-ss')).toHaveLength(2);
    expect(valueAfter(args, '-ss')).toBe('75.5');
    expect(args.indexOf('-ss')).toBeLessThan(args.indexOf('-i'));
  });

  it('konwersja libx264: skalowanie do 1080p, klatki kluczowe co 4 s, bez fps przy 30 kl./s', () => {
    const args = buildClipArgs(opts({ mode: 'transcode' }));
    expect(valueAfter(args, '-c:v')).toBe('libx264');
    expect(valueAfter(args, '-vf')).toBe("scale=-2:'min(1080,ih)',format=yuv420p");
    expect(valueAfter(args, '-force_key_frames')).toBe('expr:gte(t,n_forced*4)');
    expect(valueAfter(args, '-b:v')).toBe('6M');
  });

  it('konwersja źródła 60 kl./s ogranicza do 30', () => {
    const args = buildClipArgs(opts({ mode: 'transcode', info: { ...h264, fps: 59.94 } }));
    expect(valueAfter(args, '-vf')).toBe("scale=-2:'min(1080,ih)',fps=30,format=yuv420p");
  });

  it('enkoder sprzętowy NVIDIA', () => {
    const args = buildClipArgs(opts({ mode: 'transcode', encoder: 'h264_nvenc' }));
    expect(valueAfter(args, '-c:v')).toBe('h264_nvenc');
  });

  it('bez readrate (transmisja na żywo)', () => {
    expect(buildClipArgs(opts({ readrate: null }))).not.toContain('-readrate');
  });

  it('lokalne pliki nie dostają opcji ponownego łączenia', () => {
    const args = buildClipArgs(opts({ inputs: [{ url: '/tmp/a.mp4', headers: {} }] }));
    expect(args).not.toContain('-reconnect');
  });
});
