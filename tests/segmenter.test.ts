import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { ClipSegmenter, parseSegmentCsv } from '../src/pipeline/segmenter.ts';
import { buildClipArgs } from '../src/pipeline/ffmpegArgs.ts';
import { loadConfig } from '../src/config.ts';

const cfg = loadConfig({ ADMIN_PASSWORD: 'haslo123' });
const hasFfmpeg = fs.existsSync(cfg.ffmpegPath);

describe('parseSegmentCsv', () => {
  it('czyta wiersze nazwa,start,koniec', () => {
    expect(parseSegmentCsv('seg_00000.ts,0.000000,4.004000\nseg_00001.ts,4.004000,8.000000\n')).toEqual([
      { name: 'seg_00000.ts', duration: 4.004 },
      { name: 'seg_00001.ts', duration: 3.996 },
    ]);
  });
  it('pomija niepełną ostatnią linię', () => {
    expect(parseSegmentCsv('seg_00000.ts,0,4\nseg_0000')).toHaveLength(1);
  });
});

describe.skipIf(!hasFfmpeg)('ClipSegmenter z prawdziwym ffmpeg', () => {
  it('tnie 10 s testowego obrazu na fragmenty i kończy się stanem done', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clip-'));
    const args = buildClipArgs({
      inputs: [{ url: 'testsrc2=size=640x360:rate=25:duration=10', headers: {} }],
      inputFormat: 'lavfi',
      mode: 'transcode',
      info: { codec: 'rawvideo', height: 360, pixFmt: 'yuv420p', fps: 25, hasAudio: false, durationS: 10 },
      encoder: 'libx264',
      segmentSeconds: 4,
      outDir: dir,
      startAtS: 0,
      readrate: null,
    });
    const seg = new ClipSegmenter({ ffmpegPath: cfg.ffmpegPath, args, outDir: dir });
    seg.start();
    const done = await new Promise<string>((resolve) => seg.once('exit', resolve));
    expect(done).toBe('done');
    const segments = seg.poll();
    expect(segments.length).toBeGreaterThanOrEqual(2);
    expect(segments[0].index).toBe(0);
    expect(fs.existsSync(segments[0].path)).toBe(true);
    const total = segments.reduce((s, x) => s + x.duration, 0);
    expect(total).toBeGreaterThan(9.5);
    expect(seg.poll()).toEqual([]);
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('zły adres kończy się stanem failed z komunikatem', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clip-'));
    const args = buildClipArgs({
      inputs: [{ url: path.join(dir, 'nie-ma.mp4'), headers: {} }],
      mode: 'copy',
      info: { codec: 'h264', height: 360, pixFmt: 'yuv420p', fps: 25, hasAudio: true, durationS: 10 },
      encoder: 'libx264',
      segmentSeconds: 4,
      outDir: dir,
      startAtS: 0,
      readrate: null,
    });
    const seg = new ClipSegmenter({ ffmpegPath: cfg.ffmpegPath, args, outDir: dir });
    seg.start();
    expect(await new Promise((r) => seg.once('exit', r))).toBe('failed');
    expect(seg.error).toBeTruthy();
    fs.rmSync(dir, { recursive: true, force: true });
  });
});
