import { describe, it, expect } from 'vitest';
import { TsRewriter, minDts, readPesTimestamps, readPcr } from '../src/pipeline/tsrewrite.ts';

const WRAP = 2 ** 33;

function encTs(prefix: number, ts: number): number[] {
  const hi = Math.floor(ts / 2 ** 30) & 0x7;
  const lo = ts % 2 ** 30;
  return [
    (prefix << 4) | (hi << 1) | 1,
    (lo >>> 22) & 0xff,
    (((lo >>> 15) & 0x7f) << 1) | 1,
    (lo >>> 7) & 0xff,
    ((lo & 0x7f) << 1) | 1,
  ];
}

function encPcr(base: number, ext: number): number[] {
  const b = Math.floor(base / 2); // górne 32 bity
  return [(b >>> 24) & 0xff, (b >>> 16) & 0xff, (b >>> 8) & 0xff, b & 0xff, ((base & 1) << 7) | 0x7e | ((ext >> 8) & 1), ext & 0xff];
}

interface PacketOpts {
  pid: number;
  pusi?: boolean;
  cc?: number;
  pcr?: number;
  pts?: number;
  dts?: number;
  payload?: boolean;
  psi?: boolean;
}

function packet(o: PacketOpts): Buffer {
  const buf = Buffer.alloc(188, 0xff);
  const hasPayload = o.payload !== false;
  const hasAf = o.pcr !== undefined;
  const afc = (hasAf ? 2 : 0) | (hasPayload ? 1 : 0);
  buf[0] = 0x47;
  buf[1] = (o.pusi ? 0x40 : 0) | ((o.pid >> 8) & 0x1f);
  buf[2] = o.pid & 0xff;
  buf[3] = (afc << 4) | ((o.cc ?? 0) & 0xf);
  let p = 4;
  if (hasAf) {
    const af = [0x10, ...encPcr(o.pcr!, 123)];
    buf[p] = af.length;
    buf.set(af, p + 1);
    p += 1 + af.length;
  }
  if (hasPayload && o.pusi && o.psi) {
    buf.set([0x00, 0x00, 0xb0, 0x0d], p); // pointer_field + PAT
  } else if (hasPayload && o.pusi && o.pts !== undefined) {
    const ts = o.dts !== undefined ? [...encTs(3, o.pts), ...encTs(1, o.dts)] : encTs(2, o.pts);
    buf.set([0, 0, 1, 0xe0, 0, 0, 0x80, o.dts !== undefined ? 0xc0 : 0x80, ts.length, ...ts], p);
  }
  return buf;
}

describe('TsRewriter', () => {
  it('przesuwa PTS i DTS w nagłówkach PES', () => {
    const seg = packet({ pid: 0x100, pusi: true, pts: 133_200, dts: 126_000 });
    const out = new TsRewriter().rewrite(seg, 900_000);
    expect(readPesTimestamps(out, 0)).toEqual({ pts: 1_033_200, dts: 1_026_000 });
    // oryginał nietknięty
    expect(readPesTimestamps(seg, 0)).toEqual({ pts: 133_200, dts: 126_000 });
  });

  it('przesuwa PCR o offset × 300 i zachowuje rozszerzenie', () => {
    const seg = packet({ pid: 0x100, pcr: 126_000 });
    const out = new TsRewriter().rewrite(seg, 90_000);
    expect(readPcr(out, 0)).toEqual({ base: 216_000, ext: 123 });
  });

  it('obsługuje ujemny offset i zawijanie 2^33', () => {
    const seg = packet({ pid: 0x101, pusi: true, pts: WRAP - 10 });
    const out = new TsRewriter().rewrite(seg, 100);
    expect(readPesTimestamps(out, 0).pts).toBe(90);
    const back = new TsRewriter().rewrite(packet({ pid: 0x101, pusi: true, pts: 50 }), -100);
    expect(readPesTimestamps(back, 0).pts).toBe(WRAP - 50);
  });

  it('nadaje ciągłe liczniki CC per PID między kolejnymi fragmentami', () => {
    const rw = new TsRewriter();
    const a = Buffer.concat([packet({ pid: 0x100, cc: 7 }), packet({ pid: 0x100, cc: 8 }), packet({ pid: 0x101, cc: 3 })]);
    const b = Buffer.concat([packet({ pid: 0x100, cc: 0 }), packet({ pid: 0x101, cc: 0 }), packet({ pid: 0x100, cc: 15, payload: false, pcr: 1 })]);
    const outA = rw.rewrite(a, 0);
    const outB = rw.rewrite(b, 0);
    const cc = (buf: Buffer, i: number) => buf[i * 188 + 3] & 0xf;
    expect([cc(outA, 0), cc(outA, 1), cc(outA, 2)]).toEqual([0, 1, 0]);
    // pakiet bez danych nie zwiększa licznika
    expect([cc(outB, 0), cc(outB, 1), cc(outB, 2)]).toEqual([2, 1, 2]);
  });

  it('nie rusza pakietów PAT/PMT poza licznikiem', () => {
    const pat = packet({ pid: 0, pusi: true, psi: true });
    const out = new TsRewriter().rewrite(pat, 12345);
    expect(out.subarray(4)).toEqual(pat.subarray(4));
  });

  it('odrzuca dane bez bajtu synchronizacji', () => {
    const bad = Buffer.alloc(188, 0);
    expect(() => new TsRewriter().rewrite(bad, 0)).toThrow(/synchroniz/);
  });
});

describe('minDts', () => {
  it('zwraca najmniejszy DTS (lub PTS, gdy brak DTS)', () => {
    const seg = Buffer.concat([
      packet({ pid: 0x100, pusi: true, pts: 140_000, dts: 130_000 }),
      packet({ pid: 0x101, pusi: true, pts: 128_000 }),
      packet({ pid: 0x100, pusi: true, pts: 150_000, dts: 133_000 }),
    ]);
    expect(minDts(seg)).toBe(128_000);
  });
  it('null, gdy brak znaczników', () => {
    expect(minDts(packet({ pid: 0, pusi: true, psi: true }))).toBeNull();
  });
});
