// Przepisywanie znaczników czasu w strumieniu MPEG-TS.
//
// Każdy fragment (film, plansza, wznowienie) ffmpeg zapisuje z własną osią czasu. Nadajnik przesuwa
// PTS/DTS (90 kHz) i PCR (27 MHz) o stały offset, a liczniki ciągłości (CC) numeruje od nowa per PID.
// Dzięki temu cały kanał jest jedną ciągłą osią czasu, niezależnie od źródła fragmentu.

const PACKET = 188;
const WRAP = 2 ** 33;

const NO_PES_HEADER_IDS = new Set([0xbc, 0xbe, 0xbf, 0xf0, 0xf1, 0xf2, 0xf8, 0xff]);

function readTs(buf: Buffer, p: number): number {
  const hi = (buf[p] >> 1) & 0x7;
  const lo = (buf[p + 1] << 22) | ((buf[p + 2] >> 1) << 15) | (buf[p + 3] << 7) | (buf[p + 4] >> 1);
  return hi * 2 ** 30 + lo;
}

function writeTs(buf: Buffer, p: number, ts: number): void {
  const prefix = buf[p] & 0xf0;
  const hi = Math.floor(ts / 2 ** 30) & 0x7;
  const lo = ts % 2 ** 30;
  buf[p] = prefix | (hi << 1) | 1;
  buf[p + 1] = (lo >>> 22) & 0xff;
  buf[p + 2] = (((lo >>> 15) & 0x7f) << 1) | 1;
  buf[p + 3] = (lo >>> 7) & 0xff;
  buf[p + 4] = ((lo & 0x7f) << 1) | 1;
}

function wrap(ts: number): number {
  return ((ts % WRAP) + WRAP) % WRAP;
}

interface PacketLayout {
  pid: number;
  pusi: boolean;
  hasPayload: boolean;
  /** Pozycja PCR w buforze lub -1. */
  pcrAt: number;
  /** Pozycja nagłówka PES (00 00 01) lub -1. */
  pesAt: number;
}

function layout(buf: Buffer, off: number): PacketLayout {
  if (buf[off] !== 0x47) throw new Error(`Utracono synchronizację MPEG-TS na bajcie ${off}`);
  const pid = ((buf[off + 1] & 0x1f) << 8) | buf[off + 2];
  const pusi = (buf[off + 1] & 0x40) !== 0;
  const afc = (buf[off + 3] >> 4) & 0x3;
  const hasAf = (afc & 2) !== 0;
  const hasPayload = (afc & 1) !== 0;
  let p = off + 4;
  let pcrAt = -1;
  if (hasAf) {
    const afLen = buf[off + 4];
    if (afLen > 0 && (buf[off + 5] & 0x10) !== 0 && afLen >= 7) pcrAt = off + 6;
    p += 1 + afLen;
  }
  let pesAt = -1;
  const end = off + PACKET;
  if (hasPayload && pusi && p + 14 <= end && buf[p] === 0 && buf[p + 1] === 0 && buf[p + 2] === 1) {
    const sid = buf[p + 3];
    if (!NO_PES_HEADER_IDS.has(sid) && (buf[p + 6] & 0xc0) === 0x80) pesAt = p;
  }
  return { pid, pusi, hasPayload, pcrAt, pesAt };
}

/** Odczytuje PTS/DTS z pakietu zaczynającego się na off (do testów i diagnostyki). */
export function readPesTimestamps(buf: Buffer, off: number): { pts: number | null; dts: number | null } {
  const { pesAt } = layout(buf, off);
  if (pesAt < 0) return { pts: null, dts: null };
  const flags = buf[pesAt + 7] >> 6;
  const pts = flags & 2 ? readTs(buf, pesAt + 9) : null;
  const dts = flags === 3 ? readTs(buf, pesAt + 14) : null;
  return { pts, dts };
}

export function readPcr(buf: Buffer, off: number): { base: number; ext: number } | null {
  const { pcrAt: p } = layout(buf, off);
  if (p < 0) return null;
  const base = buf[p] * 2 ** 25 + buf[p + 1] * 2 ** 17 + buf[p + 2] * 2 ** 9 + buf[p + 3] * 2 + (buf[p + 4] >> 7);
  const ext = ((buf[p + 4] & 1) << 8) | buf[p + 5];
  return { base, ext };
}

/** Najmniejszy DTS (lub PTS, gdy brak DTS) we fragmencie, w jednostkach 90 kHz. */
export function minDts(buf: Buffer): number | null {
  let min: number | null = null;
  for (let off = 0; off + PACKET <= buf.length; off += PACKET) {
    const { pts, dts } = readPesTimestamps(buf, off);
    const t = dts ?? pts;
    if (t !== null && (min === null || t < min)) min = t;
  }
  return min;
}

const AAC_SAMPLE_RATES = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
/** Przybliżona długość klatki wideo (90 kHz), gdy nie znamy fps: ~33 ms. */
const VIDEO_FRAME = 3_000;
/** Jedna ramka AAC przy 48 kHz, gdy nie da się odczytać nagłówków. */
const AAC_FRAME_48K = 1_920;

function payloadOffset(buf: Buffer, off: number): number {
  const afc = (buf[off + 3] >> 4) & 0x3;
  if ((afc & 1) === 0) return -1;
  return (afc & 2) !== 0 ? off + 5 + buf[off + 4] : off + 4;
}

/** Długość (90 kHz) ramek ADTS AAC zapisanych jedna po drugiej, albo null. */
function adtsDuration(data: Buffer): number | null {
  let p = 0;
  let frames = 0;
  let rate = 0;
  while (p + 7 <= data.length && data[p] === 0xff && (data[p + 1] & 0xf0) === 0xf0) {
    rate = AAC_SAMPLE_RATES[(data[p + 2] >> 2) & 0xf] ?? 0;
    const len = ((data[p + 3] & 3) << 11) | (data[p + 4] << 3) | (data[p + 5] >> 5);
    if (len < 7 || rate === 0) break;
    frames++;
    p += len;
  }
  return frames > 0 && rate > 0 ? Math.round((frames * 1024 * 90_000) / rate) : null;
}

/**
 * Koniec zawartości fragmentu (90 kHz): dla dźwięku PTS ostatniego PES plus długość jego ramek AAC
 * (ffmpeg pakuje kilka małych ramek w jeden PES), dla wideo ostatni DTS plus jedna klatka.
 */
export function contentEnd(buf: Buffer): number | null {
  const last = new Map<number, { ts: number; sid: number; chunks: Buffer[] }>();
  for (let off = 0; off + PACKET <= buf.length; off += PACKET) {
    const l = layout(buf, off);
    if (l.pesAt >= 0) {
      const flags = buf[l.pesAt + 7] >> 6;
      const pts = flags & 2 ? readTs(buf, l.pesAt + 9) : null;
      const dts = flags === 3 ? readTs(buf, l.pesAt + 14) : null;
      const ts = dts ?? pts;
      if (ts === null) continue;
      const start = l.pesAt + 9 + buf[l.pesAt + 8];
      last.set(l.pid, { ts, sid: buf[l.pesAt + 3], chunks: [buf.subarray(start, off + PACKET)] });
    } else if (!l.pusi && l.hasPayload && last.has(l.pid)) {
      const p = payloadOffset(buf, off);
      if (p >= 0) last.get(l.pid)!.chunks.push(buf.subarray(p, off + PACKET));
    }
  }
  let end: number | null = null;
  for (const { ts, sid, chunks } of last.values()) {
    const isAudio = sid >= 0xc0 && sid <= 0xdf;
    const e = isAudio ? ts + (adtsDuration(Buffer.concat(chunks)) ?? AAC_FRAME_48K) : ts + VIDEO_FRAME;
    if (end === null || e > end) end = e;
  }
  return end;
}

export class TsRewriter {
  private counters = new Map<number, number>();

  /** Zwraca kopię fragmentu z przesuniętymi znacznikami czasu i ciągłymi licznikami CC. */
  rewrite(input: Buffer, offset90k: number): Buffer {
    const buf = Buffer.from(input);
    const usable = buf.length - (buf.length % PACKET);
    for (let off = 0; off < usable; off += PACKET) {
      const l = layout(buf, off);

      // Licznik ciągłości: rośnie tylko dla pakietów z danymi.
      const last = this.counters.get(l.pid);
      let cc: number;
      if (last === undefined) cc = 0;
      else cc = l.hasPayload ? (last + 1) & 0xf : last;
      this.counters.set(l.pid, cc);
      buf[off + 3] = (buf[off + 3] & 0xf0) | cc;

      if (offset90k === 0) continue;

      if (l.pcrAt >= 0) {
        const p = l.pcrAt;
        const base = buf[p] * 2 ** 25 + buf[p + 1] * 2 ** 17 + buf[p + 2] * 2 ** 9 + buf[p + 3] * 2 + (buf[p + 4] >> 7);
        const nb = wrap(base + offset90k);
        const top = Math.floor(nb / 2);
        buf[p] = (top >>> 24) & 0xff;
        buf[p + 1] = (top >>> 16) & 0xff;
        buf[p + 2] = (top >>> 8) & 0xff;
        buf[p + 3] = top & 0xff;
        buf[p + 4] = ((nb & 1) << 7) | (buf[p + 4] & 0x7f);
      }

      if (l.pesAt >= 0) {
        const flags = buf[l.pesAt + 7] >> 6;
        if (flags & 2) writeTs(buf, l.pesAt + 9, wrap(readTs(buf, l.pesAt + 9) + offset90k));
        if (flags === 3) writeTs(buf, l.pesAt + 14, wrap(readTs(buf, l.pesAt + 14) + offset90k));
      }
    }
    return usable === buf.length ? buf : buf.subarray(0, usable);
  }
}
