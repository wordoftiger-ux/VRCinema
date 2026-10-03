import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { generateSlates, slateFilter } from '../src/pipeline/slates.ts';
import { loadConfig } from '../src/config.ts';
import { minDts } from '../src/pipeline/tsrewrite.ts';

const cfg = loadConfig({ ADMIN_PASSWORD: 'haslo123' });
const hasFfmpeg = fs.existsSync(cfg.ffmpegPath);

describe('slateFilter', () => {
  it('z czcionką rysuje dwa teksty z plików', () => {
    const f = slateFilter(true, 8);
    expect(f).toContain('textfile=title.txt');
    expect(f).toContain('textfile=subtitle.txt');
    expect(f).toContain('fontfile=font.ttf');
    expect(f.startsWith('color=c=0x14161c:s=1920x1080:r=30:d=8')).toBe(true);
  });
  it('bez czcionki tylko tło', () => {
    expect(slateFilter(false, 8)).toBe('color=c=0x14161c:s=1920x1080:r=30:d=8');
  });
});

describe.skipIf(!hasFfmpeg)('generateSlates z prawdziwym ffmpeg', () => {
  it('tworzy plansze idle i buffering z polskim tekstem', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'slates-'));
    const slates = await generateSlates({
      ffmpegPath: cfg.ffmpegPath,
      fontFile: cfg.fontFile,
      dir,
      requestUrl: 'https://tv.example.com/',
      segmentSeconds: 4,
    });
    for (const kind of ['idle', 'buffering'] as const) {
      expect(slates[kind].length).toBeGreaterThanOrEqual(1);
      const total = slates[kind].reduce((s, x) => s + x.duration, 0);
      expect(total).toBeGreaterThan(7.5);
      const buf = fs.readFileSync(slates[kind][0].path);
      expect(minDts(buf)).not.toBeNull();
    }
    expect(fs.readFileSync(path.join(dir, 'idle', 'title.txt'), 'utf8')).toBe('Brak filmów w kolejce');
    fs.rmSync(dir, { recursive: true, force: true });
  });
});
