import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Broadcaster, START_PTS, type SourceSegment, type SegmentSource } from '../src/pipeline/broadcaster.ts';
import { minDts } from '../src/pipeline/tsrewrite.ts';

/** Fragment TS z jednym pakietem PES o podanym PTS. */
function tsWithPts(pts: number): Buffer {
  const buf = Buffer.alloc(188, 0xff);
  buf.set([0x47, 0x41, 0x00, 0x10], 0);
  const hi = Math.floor(pts / 2 ** 30) & 7;
  const lo = pts % 2 ** 30;
  buf.set(
    [0, 0, 1, 0xe0, 0, 0, 0x80, 0x80, 5,
      0x21 | (hi << 1), (lo >>> 22) & 0xff, (((lo >>> 15) & 0x7f) << 1) | 1, (lo >>> 7) & 0xff, ((lo & 0x7f) << 1) | 1],
    4,
  );
  return buf;
}

let dir: string;
let srcDir: string;
let now: number;

class FakeSource implements SegmentSource {
  queue: SourceSegment[] = [];
  counter = 0;
  /** Domyślny fragment, gdy kolejka pusta. */
  make(sourceKey: string, index: number, duration = 4, pts = 126_000): SourceSegment {
    const p = path.join(srcDir, `${sourceKey}-${index}-${this.counter++}.ts`);
    fs.writeFileSync(p, tsWithPts(pts));
    return { path: p, duration, sourceKey, index };
  }
  next(): SourceSegment {
    return this.queue.shift() ?? this.make('idle', 0);
  }
}

function make(source: SegmentSource, discontinuityTags = true) {
  return new Broadcaster({
    dir,
    windowSize: 10,
    segmentSeconds: 4,
    pathPrefix: 'main/',
    source,
    discontinuityTags: () => discontinuityTags,
    now: () => now,
  });
}

function lines(b: Broadcaster) {
  return b.playlist().split('\n');
}

beforeEach(() => {
  dir = fs.mkdtempSync(path.join(os.tmpdir(), 'live-'));
  srcDir = fs.mkdtempSync(path.join(os.tmpdir(), 'src-'));
  now = 1_000_000;
});

afterEach(() => {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.rmSync(srcDir, { recursive: true, force: true });
});

describe('Broadcaster', () => {
  it('na starcie publikuje 3 fragmenty, potem w tempie czasu rzeczywistego', () => {
    const b = make(new FakeSource());
    b.start(false);
    expect(lines(b).filter((l) => l.endsWith('.ts'))).toEqual(['main/0.ts', 'main/1.ts', 'main/2.ts']);
    now += 3_999;
    b.tick();
    expect(lines(b).filter((l) => l.endsWith('.ts'))).toHaveLength(3);
    now += 1;
    b.tick();
    expect(lines(b).filter((l) => l.endsWith('.ts'))).toHaveLength(4);
    now += 8_000;
    b.tick();
    expect(lines(b).filter((l) => l.endsWith('.ts'))).toHaveLength(6);
    expect(fs.existsSync(path.join(dir, '5.ts'))).toBe(true);
  });

  it('utrzymuje okno 10 fragmentów i przesuwa MEDIA-SEQUENCE', () => {
    const b = make(new FakeSource());
    b.start(false);
    now += 12 * 4_000;
    b.tick();
    const l = lines(b);
    expect(l.filter((x) => x.endsWith('.ts'))).toHaveLength(10);
    expect(l).toContain('#EXT-X-MEDIA-SEQUENCE:5');
  });

  it('oznacza nieciągłość przy zmianie źródła, ale nie przy pętli tej samej planszy', () => {
    const src = new FakeSource();
    src.queue.push(src.make('idle', 0), src.make('idle', 1), src.make('idle', 0), src.make('clip-1', 0), src.make('clip-1', 1));
    const b = make(src);
    b.start(false);
    now += 8_000;
    b.tick();
    const l = lines(b);
    const disc = l.indexOf('#EXT-X-DISCONTINUITY');
    expect(disc).toBeGreaterThan(-1);
    expect(l[disc + 2]).toBe('main/3.ts');
    expect(l.filter((x) => x === '#EXT-X-DISCONTINUITY')).toHaveLength(1);
  });

  it('bez znaczników nieciągłości, gdy wyłączone', () => {
    const src = new FakeSource();
    src.queue.push(src.make('a', 0), src.make('b', 0), src.make('c', 0));
    const b = make(src, false);
    b.start(false);
    expect(b.playlist()).not.toContain('#EXT-X-DISCONTINUITY\n');
  });

  it('liczy DISCONTINUITY-SEQUENCE dla znaczników, które wypadły z okna', () => {
    const src = new FakeSource();
    src.queue.push(src.make('a', 0), src.make('b', 0));
    const b = make(src);
    b.start(false);
    now += 11 * 4_000;
    b.tick();
    // znaczniki przy seq 1 (b) i seq 2 (powrót do planszy) wypadły z okna
    expect(b.playlist()).toContain('#EXT-X-DISCONTINUITY-SEQUENCE:2');
  });

  it('przesuwa znaczniki czasu tak, by oś czasu kanału była ciągła', () => {
    const src = new FakeSource();
    src.queue.push(src.make('a', 0, 4, 126_000), src.make('a', 1, 4, 486_000), src.make('b', 0, 4, 9_000_000));
    const b = make(src);
    b.start(false);
    const read = (n: number) => minDts(fs.readFileSync(path.join(dir, `${n}.ts`)));
    expect(read(0)).toBe(START_PTS);
    expect(read(1)).toBe(START_PTS + 360_000);
    // nowe źródło z zupełnie inną osią czasu trafia dokładnie na zegar kanału
    expect(read(2)).toBe(START_PTS + 720_000);
  });

  it('nowe źródło nie zaczyna się przed ostatnim pakietem poprzedniego (bez cofania DTS)', () => {
    const src = new FakeSource();
    // Fragment deklaruje 4 s, ale jego dźwięk sięga 4,05 s.
    const a = src.make('a', 0, 4, 126_000);
    fs.writeFileSync(a.path, Buffer.concat([tsWithPts(126_000), tsWithPts(126_000 + 364_500)]));
    src.queue.push(a, src.make('b', 0, 4, 5_000_000));
    const b = make(src);
    b.start(false);
    const read = (n: number) => minDts(fs.readFileSync(path.join(dir, `${n}.ts`)))!;
    const lastOfA = START_PTS + 364_500;
    expect(read(1)).toBeGreaterThan(lastOfA);
    expect(read(1) - lastOfA).toBeLessThanOrEqual(4_500);
  });

  it('gdy fragment deklaruje więcej niż zawiera, nowe źródło dokleja się tuż za ostatnim pakietem', () => {
    const src = new FakeSource();
    const a = src.make('a', 0, 4.067, 126_000);
    fs.writeFileSync(a.path, Buffer.concat([tsWithPts(126_000), tsWithPts(126_000 + 357_000)]));
    src.queue.push(a, src.make('b', 0, 4, 5_000_000));
    const b = make(src);
    b.start(false);
    const read = (n: number) => minDts(fs.readFileSync(path.join(dir, `${n}.ts`)))!;
    expect(read(1)).toBe(START_PTS + 357_000 + 3_000);
  });

  it('target duration: EXTINF zaokrąglony do całości nie przekracza wartości', () => {
    const src = new FakeSource();
    src.queue.push(src.make('a', 0, 4.066667));
    const b = make(src);
    b.start(false);
    expect(b.playlist()).toContain('#EXT-X-TARGETDURATION:4');
  });

  it('target duration rośnie przy długim fragmencie i już nie maleje', () => {
    const src = new FakeSource();
    src.queue.push(src.make('a', 0, 6.5));
    const b = make(src);
    b.start(false);
    now += 60_000;
    b.tick();
    expect(b.playlist()).toContain('#EXT-X-TARGETDURATION:7');
  });

  it('usuwa stare pliki po wypadnięciu z okna i zapasie czasu', () => {
    const b = make(new FakeSource());
    b.start(false);
    now += 11 * 4_000;
    b.tick();
    expect(fs.existsSync(path.join(dir, '0.ts'))).toBe(true);
    now += 61_000;
    b.tick();
    expect(fs.existsSync(path.join(dir, '0.ts'))).toBe(false);
  });

  it('segmentPath przepuszcza tylko znane fragmenty', () => {
    const b = make(new FakeSource());
    b.start(false);
    expect(b.segmentPath('1.ts')).toBe(path.join(dir, '1.ts'));
    expect(b.segmentPath('99.ts')).toBeNull();
    expect(b.segmentPath('../secret.ts')).toBeNull();
  });

  it('wywołuje onPublished z numerem fragmentu', () => {
    const seen: number[] = [];
    const b = new Broadcaster({
      dir, windowSize: 10, segmentSeconds: 4, pathPrefix: '', source: new FakeSource(),
      discontinuityTags: () => true, now: () => now, onPublished: (s) => seen.push(s.seq),
    });
    b.start(false);
    expect(seen).toEqual([0, 1, 2]);
  });

  it('wyjątek w źródle nie przerywa nadawania ani nie wyrzuca błędu z tick()', () => {
    const src = new FakeSource();
    let fail = false;
    const flaky: SegmentSource = {
      next: () => {
        if (fail) throw new Error('baza niedostępna');
        return src.next();
      },
    };
    const logs: string[] = [];
    const b = new Broadcaster({
      dir, windowSize: 10, segmentSeconds: 4, pathPrefix: '', source: flaky,
      discontinuityTags: () => true, now: () => now, log: (m) => logs.push(m),
    });
    b.start(false);
    fail = true;
    now += 4_000;
    expect(() => b.tick()).not.toThrow();
    expect(logs.some((l) => l.includes('baza niedostępna'))).toBe(true);
    fail = false;
    now += 5_000;
    b.tick();
    expect(b.playlist().split('\n').filter((l) => l.endsWith('.ts')).length).toBeGreaterThan(3);
  });

  it('pomija uszkodzony fragment źródła i bierze następny', () => {
    const src = new FakeSource();
    const broken = path.join(srcDir, 'broken.ts');
    fs.writeFileSync(broken, Buffer.alloc(188, 0));
    src.queue.push({ path: broken, duration: 4, sourceKey: 'x', index: 0 });
    const b = make(src);
    b.start(false);
    expect(lines(b).filter((l) => l.endsWith('.ts'))).toHaveLength(3);
  });
});
