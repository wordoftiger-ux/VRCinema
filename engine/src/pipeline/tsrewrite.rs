//! MPEG-TS timestamp rewriting (port of `src/pipeline/tsrewrite.ts`).

use std::collections::HashMap;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TsError {
    #[error("Utracono synchronizację MPEG-TS na bajcie {0}")]
    SyncLost(usize),
    #[error("Niepełny pakiet MPEG-TS na bajcie {0}")]
    Truncated(usize),
}

const PACKET: usize = 188;
const WRAP: i128 = 1 << 33;

/// PES stream ids that have no optional PES header (no timestamps).
const NO_PES_HEADER_IDS: [u8; 8] = [0xbc, 0xbe, 0xbf, 0xf0, 0xf1, 0xf2, 0xf8, 0xff];

const AAC_SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];
/// Approximate video frame length (90 kHz) when the fps is unknown: ~33 ms.
const VIDEO_FRAME: u64 = 3_000;
/// One AAC frame at 48 kHz, used when the ADTS headers cannot be read.
const AAC_FRAME_48K: u64 = 1_920;

/// Reads a 33-bit timestamp from its 5-byte PES encoding.
fn read_ts(b: &[u8]) -> u64 {
    let hi = u64::from((b[0] >> 1) & 0x7);
    let lo = (u64::from(b[1]) << 22)
        | (u64::from(b[2] >> 1) << 15)
        | (u64::from(b[3]) << 7)
        | u64::from(b[4] >> 1);
    (hi << 30) | lo
}

/// Writes a 33-bit timestamp, keeping the 4-bit prefix and setting the marker bits.
fn write_ts(b: &mut [u8], ts: u64) {
    let prefix = b[0] & 0xf0;
    let hi = ((ts >> 30) & 0x7) as u8;
    let lo = ts & ((1 << 30) - 1);
    b[0] = prefix | (hi << 1) | 1;
    b[1] = ((lo >> 22) & 0xff) as u8;
    b[2] = ((((lo >> 15) & 0x7f) << 1) | 1) as u8;
    b[3] = ((lo >> 7) & 0xff) as u8;
    b[4] = (((lo & 0x7f) << 1) | 1) as u8;
}

/// Adds a (possibly negative) offset to a timestamp modulo 2^33.
fn wrap(ts: u64, offset: i64) -> u64 {
    (i128::from(ts) + i128::from(offset)).rem_euclid(WRAP) as u64
}

/// Reads the 33-bit PCR base and the 9-bit extension from 6 PCR bytes.
fn read_pcr_bytes(b: &[u8]) -> (u64, u16) {
    let base = (u64::from(b[0]) << 25)
        | (u64::from(b[1]) << 17)
        | (u64::from(b[2]) << 9)
        | (u64::from(b[3]) << 1)
        | u64::from(b[4] >> 7);
    let ext = (u16::from(b[4] & 1) << 8) | u16::from(b[5]);
    (base, ext)
}

/// Layout of one 188-byte packet; positions are relative to the packet start.
struct PacketLayout {
    pid: u16,
    pusi: bool,
    has_payload: bool,
    /// Position of the 6 PCR bytes.
    pcr_at: Option<usize>,
    /// Position of the PES header (`00 00 01`) with its optional header present.
    pes_at: Option<usize>,
}

fn layout(pkt: &[u8; PACKET]) -> PacketLayout {
    let pid = (u16::from(pkt[1] & 0x1f) << 8) | u16::from(pkt[2]);
    let pusi = pkt[1] & 0x40 != 0;
    let afc = (pkt[3] >> 4) & 0x3;
    let has_af = afc & 2 != 0;
    let has_payload = afc & 1 != 0;
    let mut p = 4usize;
    let mut pcr_at = None;
    if has_af {
        let af_len = usize::from(pkt[4]);
        if af_len >= 7 && pkt[5] & 0x10 != 0 {
            pcr_at = Some(6);
        }
        p += 1 + af_len;
    }
    let mut pes_at = None;
    if has_payload
        && pusi
        && p + 14 <= PACKET
        && pkt[p] == 0
        && pkt[p + 1] == 0
        && pkt[p + 2] == 1
        && !NO_PES_HEADER_IDS.contains(&pkt[p + 3])
        && pkt[p + 6] & 0xc0 == 0x80
    {
        pes_at = Some(p);
    }
    PacketLayout {
        pid,
        pusi,
        has_payload,
        pcr_at,
        pes_at,
    }
}

/// Returns the packet at `off`, or an error when it is cut off or out of sync.
fn packet_at(buf: &[u8], off: usize) -> Result<&[u8; PACKET], TsError> {
    let end = off.checked_add(PACKET).ok_or(TsError::Truncated(off))?;
    let pkt: &[u8; PACKET] = buf
        .get(off..end)
        .and_then(|s| s.try_into().ok())
        .ok_or(TsError::Truncated(off))?;
    if pkt[0] != 0x47 {
        return Err(TsError::SyncLost(off));
    }
    Ok(pkt)
}

/// PTS and DTS of the PES header at `pes`. The DTS is `None` when it would not fit in the packet.
fn pes_times(pkt: &[u8; PACKET], pes: usize) -> (Option<u64>, Option<u64>) {
    let flags = pkt[pes + 7] >> 6;
    let pts = (flags & 2 != 0).then(|| read_ts(&pkt[pes + 9..pes + 14]));
    let dts = (flags == 3 && pes + 19 <= PACKET).then(|| read_ts(&pkt[pes + 14..pes + 19]));
    (pts, dts)
}

/// Reads PTS and DTS from the packet starting at `off` (for tests and diagnostics).
pub fn read_pes_timestamps(buf: &[u8], off: usize) -> (Option<u64>, Option<u64>) {
    let Ok(pkt) = packet_at(buf, off) else {
        return (None, None);
    };
    match layout(pkt).pes_at {
        Some(pes) => pes_times(pkt, pes),
        None => (None, None),
    }
}

/// Reads the PCR (33-bit base, 9-bit extension) of the packet starting at `off`.
pub fn read_pcr(buf: &[u8], off: usize) -> Option<(u64, u16)> {
    let pkt = packet_at(buf, off).ok()?;
    let p = layout(pkt).pcr_at?;
    Some(read_pcr_bytes(&pkt[p..p + 6]))
}

/// Smallest DTS (or PTS when there is no DTS) in the segment, in 90 kHz units.
/// Packets that are cut off or out of sync are skipped.
pub fn min_dts(buf: &[u8]) -> Option<u64> {
    buf.chunks_exact(PACKET)
        .enumerate()
        .filter_map(|(i, _)| {
            let (pts, dts) = read_pes_timestamps(buf, i * PACKET);
            dts.or(pts)
        })
        .min()
}

/// Timestamps wrap at 2^33 (see `shift`).
pub const TS_WRAP: u64 = 1 << 33;

/// How far `to` is ahead of `from` on the wrapped timeline, negative when it is behind
/// (a difference over half the range counts as the shorter way round).
pub fn ts_ahead(from: u64, to: u64) -> i64 {
    let forward = (to % TS_WRAP + TS_WRAP - from % TS_WRAP) % TS_WRAP;
    if forward > TS_WRAP / 2 {
        forward as i64 - TS_WRAP as i64
    } else {
        forward as i64
    }
}

/// First and last moment (DTS, or PTS when there is no DTS; 90 kHz) the segment covers. The
/// values are wrapped like the stream's, so a segment straddling the wrap runs from a large
/// value to a small one: the span is worked out on the wrapped timeline, not by comparing
/// numbers. Packets that are cut off or out of sync are skipped.
pub fn dts_span(buf: &[u8]) -> Option<(u64, u64)> {
    let mut stamps = buf.chunks_exact(PACKET).enumerate().filter_map(|(i, _)| {
        let (pts, dts) = read_pes_timestamps(buf, i * PACKET);
        dts.or(pts)
    });
    let first = stamps.next()?;
    let (mut behind, mut ahead) = (0i64, 0i64);
    for stamp in stamps {
        let d = ts_ahead(first, stamp);
        behind = behind.min(d);
        ahead = ahead.max(d);
    }
    let at = |d: i64| (i128::from(first) + i128::from(d)).rem_euclid(i128::from(TS_WRAP)) as u64;
    Some((at(behind), at(ahead)))
}

/// Payload start (relative to the packet), or `None` when the packet has no payload.
fn payload_offset(pkt: &[u8; PACKET]) -> Option<usize> {
    let afc = (pkt[3] >> 4) & 0x3;
    if afc & 1 == 0 {
        return None;
    }
    Some(if afc & 2 != 0 {
        5 + usize::from(pkt[4])
    } else {
        4
    })
}

/// Duration (90 kHz) of ADTS AAC frames written back to back, or `None`.
fn adts_duration(data: &[u8]) -> Option<u64> {
    let mut p = 0usize;
    let mut frames = 0u64;
    let mut rate = 0u32;
    while p + 7 <= data.len() && data[p] == 0xff && data[p + 1] & 0xf0 == 0xf0 {
        rate = AAC_SAMPLE_RATES
            .get(usize::from((data[p + 2] >> 2) & 0xf))
            .copied()
            .unwrap_or(0);
        let len = (usize::from(data[p + 3] & 3) << 11)
            | (usize::from(data[p + 4]) << 3)
            | usize::from(data[p + 5] >> 5);
        if len < 7 || rate == 0 {
            break;
        }
        frames += 1;
        p += len;
    }
    (frames > 0 && rate > 0)
        .then(|| ((frames * 1024 * 90_000) as f64 / f64::from(rate)).round() as u64)
}

/// The most recent PES of one PID.
struct LastPes {
    ts: u64,
    stream_id: u8,
    payload: Vec<u8>,
}

/// End of the segment content (90 kHz). Audio: PTS of the last PES plus the length of its AAC
/// frames (ffmpeg packs several small frames in one PES); video: last DTS plus one frame.
/// Packets that are cut off or out of sync are skipped.
pub fn content_end(buf: &[u8]) -> Option<u64> {
    let mut last: HashMap<u16, LastPes> = HashMap::new();
    for chunk in buf.chunks_exact(PACKET) {
        let Ok(pkt) = <&[u8; PACKET]>::try_from(chunk) else {
            continue;
        };
        if pkt[0] != 0x47 {
            continue;
        }
        let l = layout(pkt);
        if let Some(pes) = l.pes_at {
            let (pts, dts) = pes_times(pkt, pes);
            let Some(ts) = dts.or(pts) else { continue };
            let start = pes + 9 + usize::from(pkt[pes + 8]);
            last.insert(
                l.pid,
                LastPes {
                    ts,
                    stream_id: pkt[pes + 3],
                    payload: pkt.get(start..).unwrap_or_default().to_vec(),
                },
            );
        } else if !l.pusi
            && l.has_payload
            && let Some(entry) = last.get_mut(&l.pid)
            && let Some(p) = payload_offset(pkt)
        {
            entry
                .payload
                .extend_from_slice(pkt.get(p..).unwrap_or_default());
        }
    }
    last.values()
        .map(|e| {
            if (0xc0..=0xdf).contains(&e.stream_id) {
                e.ts + adts_duration(&e.payload).unwrap_or(AAC_FRAME_48K)
            } else {
                e.ts + VIDEO_FRAME
            }
        })
        .max()
}

/// Rewrites PTS/DTS/PCR by a constant offset and renumbers continuity counters per PID.
/// One instance spans the whole channel so the counters stay continuous across segments.
#[derive(Debug, Default)]
pub struct TsRewriter {
    counters: HashMap<u16, u8>,
}

impl TsRewriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a copy of the segment with shifted timestamps and continuous CC counters. A
    /// trailing partial packet is dropped. On error the continuity counters are left unchanged.
    pub fn rewrite(&mut self, input: &[u8], offset_90k: i64) -> Result<Vec<u8>, TsError> {
        let usable = input.len() - input.len() % PACKET;
        let mut out = input[..usable].to_vec();
        let mut counters = self.counters.clone();
        for (i, chunk) in out.chunks_exact_mut(PACKET).enumerate() {
            let Ok(pkt) = <&mut [u8; PACKET]>::try_from(&mut *chunk) else {
                continue;
            };
            if pkt[0] != 0x47 {
                return Err(TsError::SyncLost(i * PACKET));
            }
            let l = layout(pkt);

            // Continuity counter: only advances for packets that carry a payload.
            let cc = match counters.get(&l.pid) {
                None => 0,
                Some(&last) if l.has_payload => (last + 1) & 0xf,
                Some(&last) => last,
            };
            counters.insert(l.pid, cc);
            pkt[3] = (pkt[3] & 0xf0) | cc;

            if offset_90k == 0 {
                continue;
            }

            if let Some(p) = l.pcr_at {
                let (base, _) = read_pcr_bytes(&pkt[p..p + 6]);
                let nb = wrap(base, offset_90k);
                let top = nb >> 1;
                pkt[p] = ((top >> 24) & 0xff) as u8;
                pkt[p + 1] = ((top >> 16) & 0xff) as u8;
                pkt[p + 2] = ((top >> 8) & 0xff) as u8;
                pkt[p + 3] = (top & 0xff) as u8;
                pkt[p + 4] = (((nb & 1) << 7) as u8) | (pkt[p + 4] & 0x7f);
            }

            if let Some(pes) = l.pes_at {
                let (pts, dts) = pes_times(pkt, pes);
                if let Some(pts) = pts {
                    write_ts(&mut pkt[pes + 9..pes + 14], wrap(pts, offset_90k));
                }
                if let Some(dts) = dts {
                    write_ts(&mut pkt[pes + 14..pes + 19], wrap(dts, offset_90k));
                }
            }
        }
        self.counters = counters;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WRAP: u64 = 1 << 33;

    fn enc_ts(prefix: u8, ts: u64) -> [u8; 5] {
        let hi = ((ts >> 30) & 0x7) as u8;
        let lo = ts & ((1 << 30) - 1);
        [
            (prefix << 4) | (hi << 1) | 1,
            ((lo >> 22) & 0xff) as u8,
            ((((lo >> 15) & 0x7f) << 1) | 1) as u8,
            ((lo >> 7) & 0xff) as u8,
            (((lo & 0x7f) << 1) | 1) as u8,
        ]
    }

    fn enc_pcr(base: u64, ext: u16) -> [u8; 6] {
        let b = base / 2; // upper 32 bits
        [
            ((b >> 24) & 0xff) as u8,
            ((b >> 16) & 0xff) as u8,
            ((b >> 8) & 0xff) as u8,
            (b & 0xff) as u8,
            (((base & 1) << 7) as u8) | 0x7e | (((ext >> 8) & 1) as u8),
            (ext & 0xff) as u8,
        ]
    }

    #[derive(Default)]
    struct PacketOpts {
        pid: u16,
        pusi: bool,
        cc: u8,
        pcr: Option<u64>,
        pts: Option<u64>,
        dts: Option<u64>,
        no_payload: bool,
        psi: bool,
    }

    fn packet(o: PacketOpts) -> Vec<u8> {
        let mut buf = vec![0xffu8; 188];
        let has_payload = !o.no_payload;
        let has_af = o.pcr.is_some();
        let afc = (if has_af { 2 } else { 0 }) | (if has_payload { 1 } else { 0 });
        buf[0] = 0x47;
        buf[1] = (if o.pusi { 0x40 } else { 0 }) | ((o.pid >> 8) & 0x1f) as u8;
        buf[2] = (o.pid & 0xff) as u8;
        buf[3] = (afc << 4) | (o.cc & 0xf);
        let mut p = 4;
        if let Some(pcr) = o.pcr {
            let mut af = vec![0x10u8];
            af.extend_from_slice(&enc_pcr(pcr, 123));
            buf[p] = af.len() as u8;
            buf[p + 1..p + 1 + af.len()].copy_from_slice(&af);
            p += 1 + af.len();
        }
        if has_payload && o.pusi && o.psi {
            buf[p..p + 4].copy_from_slice(&[0x00, 0x00, 0xb0, 0x0d]); // pointer_field + PAT
        } else if let (true, true, Some(pts)) = (has_payload, o.pusi, o.pts) {
            let mut ts = Vec::new();
            if let Some(dts) = o.dts {
                ts.extend_from_slice(&enc_ts(3, pts));
                ts.extend_from_slice(&enc_ts(1, dts));
            } else {
                ts.extend_from_slice(&enc_ts(2, pts));
            }
            let mut hdr = vec![
                0,
                0,
                1,
                0xe0,
                0,
                0,
                0x80,
                if o.dts.is_some() { 0xc0 } else { 0x80 },
                ts.len() as u8,
            ];
            hdr.extend_from_slice(&ts);
            buf[p..p + hdr.len()].copy_from_slice(&hdr);
        }
        buf
    }

    fn pid(pid: u16) -> PacketOpts {
        PacketOpts {
            pid,
            ..Default::default()
        }
    }

    #[test]
    fn shifts_pts_and_dts_in_pes_headers() {
        let seg = packet(PacketOpts {
            pusi: true,
            pts: Some(133_200),
            dts: Some(126_000),
            ..pid(0x100)
        });
        let out = TsRewriter::new().rewrite(&seg, 900_000).unwrap();
        assert_eq!(
            read_pes_timestamps(&out, 0),
            (Some(1_033_200), Some(1_026_000))
        );
        // the original is untouched
        assert_eq!(read_pes_timestamps(&seg, 0), (Some(133_200), Some(126_000)));
    }

    #[test]
    fn shifts_pcr_by_offset_and_keeps_the_extension() {
        let seg = packet(PacketOpts {
            pcr: Some(126_000),
            ..pid(0x100)
        });
        let out = TsRewriter::new().rewrite(&seg, 90_000).unwrap();
        assert_eq!(read_pcr(&out, 0), Some((216_000, 123)));
    }

    #[test]
    fn handles_negative_offset_and_2_pow_33_wrap() {
        let seg = packet(PacketOpts {
            pusi: true,
            pts: Some(WRAP - 10),
            ..pid(0x101)
        });
        let out = TsRewriter::new().rewrite(&seg, 100).unwrap();
        assert_eq!(read_pes_timestamps(&out, 0).0, Some(90));
        let back = TsRewriter::new()
            .rewrite(
                &packet(PacketOpts {
                    pusi: true,
                    pts: Some(50),
                    ..pid(0x101)
                }),
                -100,
            )
            .unwrap();
        assert_eq!(read_pes_timestamps(&back, 0).0, Some(WRAP - 50));
    }

    #[test]
    fn pcr_wraps_modulo_2_pow_33() {
        let seg = packet(PacketOpts {
            pcr: Some(WRAP - 1),
            ..pid(0x100)
        });
        let out = TsRewriter::new().rewrite(&seg, 3).unwrap();
        assert_eq!(read_pcr(&out, 0), Some((2, 123)));
    }

    #[test]
    fn extreme_offsets_do_not_panic() {
        let seg = packet(PacketOpts {
            pusi: true,
            pts: Some(5),
            ..pid(0x101)
        });
        for off in [i64::MIN, i64::MAX] {
            let out = TsRewriter::new().rewrite(&seg, off).unwrap();
            let expected = (5i128 + off as i128).rem_euclid(WRAP as i128) as u64;
            assert_eq!(read_pes_timestamps(&out, 0).0, Some(expected));
        }
    }

    #[test]
    fn assigns_continuous_cc_per_pid_across_segments() {
        let mut rw = TsRewriter::new();
        let a: Vec<u8> = [
            packet(PacketOpts {
                cc: 7,
                ..pid(0x100)
            }),
            packet(PacketOpts {
                cc: 8,
                ..pid(0x100)
            }),
            packet(PacketOpts {
                cc: 3,
                ..pid(0x101)
            }),
        ]
        .concat();
        let b: Vec<u8> = [
            packet(PacketOpts {
                cc: 0,
                ..pid(0x100)
            }),
            packet(PacketOpts {
                cc: 0,
                ..pid(0x101)
            }),
            packet(PacketOpts {
                cc: 15,
                no_payload: true,
                pcr: Some(1),
                ..pid(0x100)
            }),
        ]
        .concat();
        let out_a = rw.rewrite(&a, 0).unwrap();
        let out_b = rw.rewrite(&b, 0).unwrap();
        let cc = |buf: &[u8], i: usize| buf[i * 188 + 3] & 0xf;
        assert_eq!([cc(&out_a, 0), cc(&out_a, 1), cc(&out_a, 2)], [0, 1, 0]);
        // a packet without payload does not increment the counter
        assert_eq!([cc(&out_b, 0), cc(&out_b, 1), cc(&out_b, 2)], [2, 1, 2]);
    }

    #[test]
    fn cc_wraps_after_fifteen() {
        let mut rw = TsRewriter::new();
        let seg: Vec<u8> = (0..18).flat_map(|_| packet(pid(0x100))).collect();
        let out = rw.rewrite(&seg, 0).unwrap();
        let ccs: Vec<u8> = (0..18).map(|i| out[i * 188 + 3] & 0xf).collect();
        let expected: Vec<u8> = (0..18).map(|i| (i % 16) as u8).collect();
        assert_eq!(ccs, expected);
    }

    #[test]
    fn leaves_pat_pmt_packets_alone_except_the_counter() {
        let pat = packet(PacketOpts {
            pusi: true,
            psi: true,
            ..pid(0)
        });
        let out = TsRewriter::new().rewrite(&pat, 12345).unwrap();
        assert_eq!(out[4..], pat[4..]);
    }

    #[test]
    fn rejects_data_without_sync_byte() {
        let bad = vec![0u8; 188];
        let err = TsRewriter::new().rewrite(&bad, 0).unwrap_err();
        assert_eq!(err, TsError::SyncLost(0));
        assert!(err.to_string().contains("synchroniz"));
    }

    #[test]
    fn sync_loss_reports_the_byte_offset_and_keeps_counters_untouched() {
        let mut rw = TsRewriter::new();
        let mut seg: Vec<u8> = [packet(pid(0x100)), packet(pid(0x100))].concat();
        seg[188] = 0;
        let err = rw.rewrite(&seg, 0).unwrap_err();
        assert_eq!(err, TsError::SyncLost(188));
        assert_eq!(
            err.to_string(),
            "Utracono synchronizację MPEG-TS na bajcie 188"
        );
        // the failed call did not consume continuity counters
        let ok = rw.rewrite(&packet(pid(0x100)), 0).unwrap();
        assert_eq!(ok[3] & 0xf, 0);
    }

    #[test]
    fn drops_a_trailing_partial_packet() {
        let mut seg = packet(pid(0x100));
        seg.extend_from_slice(&[0x47, 1, 2, 3, 4]);
        let out = TsRewriter::new().rewrite(&seg, 0).unwrap();
        assert_eq!(out.len(), 188);
        assert!(
            TsRewriter::new()
                .rewrite(&[0x47; 100], 0)
                .unwrap()
                .is_empty()
        );
        assert!(TsRewriter::new().rewrite(&[], 5).unwrap().is_empty());
    }

    #[test]
    fn malformed_adaptation_fields_do_not_panic() {
        let mut seg = packet(pid(0x100));
        seg[3] = 0x30 | 5; // adaptation + payload
        seg[4] = 255; // adaptation length runs past the packet
        seg[5] = 0x10;
        seg[1] |= 0x40;
        let out = TsRewriter::new().rewrite(&seg, 1000).unwrap();
        assert_eq!(out.len(), 188);
        assert_eq!(read_pes_timestamps(&seg, 0), (None, None));
        assert_eq!(content_end(&seg), None);
        assert_eq!(min_dts(&seg), None);
    }

    #[test]
    fn pes_header_cut_off_at_packet_end_does_not_panic() {
        // PES header starts so late that a DTS field would not fit in the packet.
        let mut seg = vec![0xffu8; 188];
        seg[0] = 0x47;
        seg[1] = 0x40 | 1;
        seg[2] = 0;
        seg[3] = 0x30;
        seg[4] = 183 - 14; // payload (14 bytes) at the very end
        seg[5] = 0;
        let start = 188 - 14;
        let mut hdr = vec![0, 0, 1, 0xe0, 0, 0, 0x80, 0xc0, 10];
        hdr.extend_from_slice(&enc_ts(3, 1000));
        seg[start..start + hdr.len()].copy_from_slice(&hdr);
        assert_eq!(read_pes_timestamps(&seg, 0), (Some(1000), None));
        let out = TsRewriter::new().rewrite(&seg, 500).unwrap();
        assert_eq!(read_pes_timestamps(&out, 0), (Some(1500), None));
        assert_eq!(content_end(&seg), Some(1000 + 3000));
    }

    #[test]
    fn read_functions_tolerate_bad_offsets() {
        let seg = packet(PacketOpts {
            pusi: true,
            pts: Some(1),
            ..pid(0x100)
        });
        assert_eq!(read_pes_timestamps(&seg, 1), (None, None));
        assert_eq!(read_pes_timestamps(&seg, 188), (None, None));
        assert_eq!(read_pes_timestamps(&seg, usize::MAX), (None, None));
        assert_eq!(read_pcr(&seg, usize::MAX), None);
        assert_eq!(read_pcr(&seg, 0), None);
        assert_eq!(read_pes_timestamps(&[], 0), (None, None));
    }

    fn adts(payload_bytes: usize, sf_index: u8) -> Vec<u8> {
        let len = 7 + payload_bytes;
        let mut v = vec![
            0xff,
            0xf1,
            (1 << 6) | (sf_index << 2),
            (2 << 6) | (((len >> 11) & 3) as u8),
            ((len >> 3) & 0xff) as u8,
            (((len & 7) << 5) as u8) | 0x1f,
            0xfc,
        ];
        v.extend(std::iter::repeat_n(0x21u8, payload_bytes));
        v
    }

    /// Audio PES (stream_id 0xC0) with ADTS frames, split into TS packets.
    fn audio_pes(pid: u16, pts: u64, frames: &[Vec<u8>]) -> Vec<u8> {
        let data: Vec<u8> = frames.concat();
        let mut bytes = vec![0, 0, 1, 0xc0, 0, 0, 0x80, 0x80, 5];
        bytes.extend_from_slice(&enc_ts(2, pts));
        bytes.extend_from_slice(&data);
        let mut out = Vec::new();
        for (n, chunk) in bytes.chunks(184).enumerate() {
            let mut pkt = vec![0xffu8; 188];
            pkt[0] = 0x47;
            pkt[1] = (if n == 0 { 0x40 } else { 0 }) | ((pid >> 8) & 0x1f) as u8;
            pkt[2] = (pid & 0xff) as u8;
            if chunk.len() == 184 {
                pkt[3] = 0x10;
                pkt[4..].copy_from_slice(chunk);
            } else {
                // pad with an adaptation field, data at the end of the packet
                let af = 184 - chunk.len() - 1;
                pkt[3] = 0x30;
                pkt[4] = af as u8;
                if af > 0 {
                    pkt[5] = 0x00;
                }
                pkt[188 - chunk.len()..].copy_from_slice(chunk);
            }
            out.extend_from_slice(&pkt);
        }
        out
    }

    #[test]
    fn content_end_audio_is_last_pts_plus_aac_frames() {
        let buf: Vec<u8> = [
            audio_pes(0x101, 90_000, &[adts(6, 3)]),
            audio_pes(0x101, 100_000, &[adts(6, 3), adts(6, 3), adts(6, 3)]),
        ]
        .concat();
        assert_eq!(content_end(&buf), Some(100_000 + 3 * 1920));
    }

    #[test]
    fn content_end_audio_spanning_several_ts_packets() {
        let buf = audio_pes(0x101, 50_000, &[adts(300, 3), adts(300, 3)]);
        assert!(buf.len() > 188 * 2);
        assert_eq!(content_end(&buf), Some(50_000 + 2 * 1920));
    }

    #[test]
    fn content_end_44_1_khz() {
        assert_eq!(
            content_end(&audio_pes(0x101, 0, &[adts(6, 4)])),
            Some((1024.0f64 * 90_000.0 / 44_100.0).round() as u64)
        );
    }

    #[test]
    fn content_end_audio_without_adts_falls_back_to_one_48k_frame() {
        let buf = audio_pes(0x101, 1000, &[vec![0x21; 20]]);
        assert_eq!(content_end(&buf), Some(1000 + 1920));
    }

    #[test]
    fn content_end_video_is_last_dts_plus_one_frame() {
        let buf = packet(PacketOpts {
            pusi: true,
            pts: Some(10_000),
            dts: Some(7_000),
            ..pid(0x100)
        });
        assert_eq!(content_end(&buf), Some(7_000 + 3_000));
    }

    #[test]
    fn content_end_takes_the_maximum_over_all_streams() {
        let buf: Vec<u8> = [
            packet(PacketOpts {
                pusi: true,
                pts: Some(100_000),
                ..pid(0x100)
            }),
            audio_pes(0x101, 99_000, &[adts(6, 3), adts(6, 3)]),
        ]
        .concat();
        assert_eq!(content_end(&buf), Some(103_000));
    }

    #[test]
    fn content_end_none_without_timestamps() {
        assert_eq!(content_end(&[]), None);
        assert_eq!(
            content_end(&packet(PacketOpts {
                pusi: true,
                psi: true,
                ..pid(0)
            })),
            None
        );
    }

    #[test]
    fn min_dts_returns_smallest_dts_or_pts() {
        let seg: Vec<u8> = [
            packet(PacketOpts {
                pusi: true,
                pts: Some(140_000),
                dts: Some(130_000),
                ..pid(0x100)
            }),
            packet(PacketOpts {
                pusi: true,
                pts: Some(128_000),
                ..pid(0x101)
            }),
            packet(PacketOpts {
                pusi: true,
                pts: Some(150_000),
                dts: Some(133_000),
                ..pid(0x100)
            }),
        ]
        .concat();
        assert_eq!(min_dts(&seg), Some(128_000));
    }

    #[test]
    fn min_dts_none_without_timestamps() {
        let pat = packet(PacketOpts {
            pusi: true,
            psi: true,
            ..pid(0)
        });
        assert_eq!(min_dts(&pat), None);
    }

    fn at(pts: u64) -> Vec<u8> {
        packet(PacketOpts {
            pusi: true,
            pts: Some(pts),
            ..pid(0x100)
        })
    }

    #[test]
    fn dts_span_is_the_first_and_last_moment_of_the_segment() {
        let seg = [at(128_000), at(300_000), at(200_000)].concat();
        assert_eq!(dts_span(&seg), Some((128_000, 300_000)));
        assert_eq!(dts_span(&[]), None);
    }

    #[test]
    fn dts_span_of_a_segment_straddling_the_wrap_runs_from_before_it_to_after_it() {
        // Timestamps are wrapped modulo 2^33: the smallest value is not the start.
        let seg = [
            at(WRAP - 100_000),
            at(WRAP - 10_000),
            at(20_000),
            at(250_000),
        ]
        .concat();
        assert_eq!(dts_span(&seg), Some((WRAP - 100_000, 250_000)));
        // The same with another order of packets (audio before video).
        let seg = [at(5_000), at(WRAP - 100_000), at(250_000)].concat();
        assert_eq!(dts_span(&seg), Some((WRAP - 100_000, 250_000)));
    }
}
