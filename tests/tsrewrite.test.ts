import { describe, it, expect } from 'vitest';
import { TsRewriter, contentEnd, minDts, readPesTimestamps, readPcr } from '../src/pipeline/tsrewrite.ts';

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

function adts(payloadBytes: number, sfIndex = 3): number[] {
  const len = 7 + payloadBytes;
  return [
    0xff, 0xf1, (1 << 6) | (sfIndex << 2), (2 << 6) | ((len >> 11) & 3), (len >> 3) & 0xff, ((len & 7) << 5) | 0x1f, 0xfc,
    ...new Array(payloadBytes).fill(0x21),
  ];
}

/** PES dźwięku (stream_id 0xC0) z ramkami ADTS, rozłożony na pakiety TS. */
function audioPes(pid: number, pts: number, frames: number[][]): Buffer {
  const data = frames.flat();
  const header = [0, 0, 1, 0xc0, 0, 0, 0x80, 0x80, 5, ...encTs(2, pts)];
  const bytes = [...header, ...data];
  const packets: Buffer[] = [];
  for (let i = 0; i < bytes.length; i += 184) {
    const chunk = bytes.slice(i, i + 184);
    const pkt = Buffer.alloc(188, 0xff);
    pkt[0] = 0x47;
    pkt[1] = (i === 0 ? 0x40 : 0) | ((pid >> 8) & 0x1f);
    pkt[2] = pid & 0xff;
    if (chunk.length === 184) {
      pkt[3] = 0x10;
      pkt.set(chunk, 4);
    } else {
      // dopełnienie polem adaptacyjnym, dane na końcu pakietu
      const af = 184 - chunk.length - 1;
      pkt[3] = 0x30;
      pkt[4] = af;
      if (af > 0) pkt[5] = 0x00;
      pkt.set(chunk, 188 - chunk.length);
    }
    packets.push(pkt);
  }
  return Buffer.concat(packets);
}

describe('contentEnd', () => {
  it('dźwięk: koniec = PTS ostatniego PES + liczba ramek AAC × 1024 próbek', () => {
    const buf = Buffer.concat([
      audioPes(0x101, 90_000, [adts(6)]),
      audioPes(0x101, 100_000, [adts(6), adts(6), adts(6)]),
    ]);
    expect(contentEnd(buf)).toBe(100_000 + 3 * 1920);
  });
  it('dźwięk rozłożony na kilka pakietów TS', () => {
    const buf = audioPes(0x101, 50_000, [adts(300), adts(300)]);
    expect(contentEnd(buf)).toBe(50_000 + 2 * 1920);
  });
  it('44,1 kHz', () => {
    expect(contentEnd(audioPes(0x101, 0, [adts(6, 4)]))).toBe(Math.round((1024 * 90_000) / 44_100));
  });
  it('wideo: ostatni DTS + jedna klatka (≈33 ms)', () => {
    const buf = Buffer.concat([packet({ pid: 0x100, pusi: true, pts: 10_000, dts: 7_000 })]);
    expect(contentEnd(buf)).toBe(7_000 + 3_000);
  });
  it('bierze maksimum ze wszystkich strumieni', () => {
    const buf = Buffer.concat([
      packet({ pid: 0x100, pusi: true, pts: 100_000 }),
      audioPes(0x101, 99_000, [adts(6), adts(6)]),
    ]);
    expect(contentEnd(buf)).toBe(103_000);
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
