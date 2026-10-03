import fs from 'node:fs';
import path from 'node:path';
import { ClipSegmenter, type SegmentInfo } from './segmenter.ts';
import { buildClipArgs } from './ffmpegArgs.ts';

export type SlateKind = 'idle' | 'buffering';
export type Slates = Record<SlateKind, SegmentInfo[]>;

const SLATE_SECONDS = 8;

const TEXTS: Record<SlateKind, (requestUrl: string) => { title: string; subtitle: string }> = {
  idle: (url) => ({
    title: 'Brak filmów w kolejce',
    subtitle: url ? `Dodaj film: ${url}` : 'Dodaj film na stronie zgłoszeń',
  }),
  buffering: () => ({ title: 'Buforowanie…', subtitle: 'Film zaraz się zacznie' }),
};

/**
 * Filtr lavfi: ciemne tło 1080p30 i dwa wyśrodkowane teksty. Teksty i czcionka są czytane
 * z plików w katalogu roboczym ffmpeg, co omija kłopotliwe escapowanie ścieżek i znaków.
 */
export function slateFilter(withText: boolean, seconds: number): string {
  const base = `color=c=0x14161c:s=1920x1080:r=30:d=${seconds}`;
  if (!withText) return base;
  const common = 'fontfile=font.ttf:x=(w-text_w)/2';
  return [
    base,
    `drawtext=${common}:fontcolor=white:textfile=title.txt:fontsize=72:y=(h/2)-90`,
    `drawtext=${common}:fontcolor=0xb8bcc8:textfile=subtitle.txt:fontsize=40:y=(h/2)+30`,
  ].join(',');
}

export interface SlateOptions {
  ffmpegPath: string;
  fontFile: string;
  dir: string;
  /** Adres strony zgłoszeń pokazywany na planszy oczekiwania. */
  requestUrl: string;
  segmentSeconds: number;
}

async function generateOne(kind: SlateKind, o: SlateOptions): Promise<SegmentInfo[]> {
  const dir = path.join(o.dir, kind);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  let withText = false;
  if (fs.existsSync(o.fontFile)) {
    fs.copyFileSync(o.fontFile, path.join(dir, 'font.ttf'));
    const t = TEXTS[kind](o.requestUrl);
    fs.writeFileSync(path.join(dir, 'title.txt'), t.title, 'utf8');
    fs.writeFileSync(path.join(dir, 'subtitle.txt'), t.subtitle, 'utf8');
    withText = true;
  } else {
    console.warn(`Brak czcionki ${o.fontFile} — plansze bez tekstu. Ustaw FONT_FILE w .env.`);
  }
  const args = buildClipArgs({
    inputs: [{ url: slateFilter(withText, SLATE_SECONDS), headers: {} }],
    inputFormat: 'lavfi',
    mode: 'transcode',
    info: { codec: 'rawvideo', height: 1080, pixFmt: 'yuv420p', fps: 30, hasAudio: false, durationS: SLATE_SECONDS },
    encoder: 'libx264',
    segmentSeconds: o.segmentSeconds,
    outDir: dir,
    startAtS: 0,
    readrate: null,
  });
  const seg = new ClipSegmenter({ ffmpegPath: o.ffmpegPath, args, outDir: dir, cwd: dir });
  seg.start();
  const state = await new Promise<string>((resolve) => seg.once('exit', resolve));
  if (state !== 'done') throw new Error(`Nie udało się wygenerować planszy "${kind}": ${seg.error}`);
  const segments = seg.poll();
  if (segments.length === 0) throw new Error(`Plansza "${kind}" nie ma fragmentów.`);
  return segments;
}

export async function generateSlates(o: SlateOptions): Promise<Slates> {
  const [idle, buffering] = await Promise.all([generateOne('idle', o), generateOne('buffering', o)]);
  return { idle, buffering };
}
