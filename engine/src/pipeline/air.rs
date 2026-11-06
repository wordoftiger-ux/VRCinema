//! The air clock (spec `2026-11-06-preview-player-design.md`, 4.2): where in the stream a
//! typical viewer is, so the window's On air screen can stand there and say how far behind the
//! Preview the air is.
//!
//! The broadcaster publishes in real time; the stream's *edge* is the channel time at the end of
//! the newest segment, moving on with the clock until the next one. A viewer's latency is how
//! far behind the edge it plays. It is measured once per viewer address, when its player joins
//! (its first segment request, or one after a gap or a jump): the edge then minus the start of
//! the requested segment; afterwards the player plays in real time at that distance. The typical
//! latency is the median of the viewers seen lately, else the nominal one
//! (`viewer_lag_segments × segment length`, where HLS players start). It is an estimate: a
//! cache in front of the server or a stalled player is not seen.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A viewer counts while its last segment request is this recent.
pub const SEEN_WINDOW: Duration = Duration::from_secs(30);
/// Seconds of aired segments the timeline keeps (the largest latency it can place).
pub const KEEP_S: f64 = 180.0;
/// Addresses remembered at once.
const MAX_VIEWERS: usize = 4096;
/// A request further ahead than this many segments is a new join (the player jumped).
const JUMP_SEGMENTS: u64 = 3;

/// One published segment of the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct AirSegment {
    pub seq: u64,
    /// Channel time at its start.
    pub channel_time: f64,
    pub duration: f64,
    /// The film it shows (`None`: a slate).
    pub item_id: Option<i64>,
    /// The film position at its start.
    pub position_s: Option<f64>,
}

/// The recent stream as the engine published it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AirTimeline {
    /// Oldest first.
    pub segments: Vec<AirSegment>,
    /// Channel time at the end of the newest segment.
    pub edge_s: f64,
    /// When the newest segment was published.
    pub edge_at: Option<Instant>,
    /// Where HLS players start behind the edge (seconds).
    pub nominal_latency_s: f64,
}

impl AirTimeline {
    /// The edge at `now`: it moves on with the clock between two segments (at most by one
    /// segment, should the engine stall).
    pub fn edge_now(&self, now: Instant) -> f64 {
        let step = self.segments.last().map_or(0.0, |s| s.duration);
        let since = self
            .edge_at
            .map_or(0.0, |at| now.saturating_duration_since(at).as_secs_f64());
        self.edge_s + since.min(step)
    }

    pub fn segment(&self, seq: u64) -> Option<&AirSegment> {
        self.segments.iter().find(|s| s.seq == seq)
    }

    /// The segment showing channel time `t`.
    pub fn at(&self, t: f64) -> Option<&AirSegment> {
        self.segments
            .iter()
            .rev()
            .find(|s| s.channel_time <= t + 1e-6)
            .filter(|s| t < s.channel_time + s.duration + 1e-6)
    }
}

/// What a typical viewer sees now.
#[derive(Debug, Clone, PartialEq)]
pub struct AirEstimate {
    /// Seconds behind the edge.
    pub latency_s: f64,
    /// `viewers` (measured) or `nominal`.
    pub source: &'static str,
    /// Viewers measured.
    pub viewers: usize,
    /// The film on the viewers' screen (`None`: a slate, or not known).
    pub item_id: Option<i64>,
    pub position_s: Option<f64>,
}

/// The typical viewer of `timeline` at `now`, from the measured `latencies`.
pub fn estimate(timeline: &AirTimeline, latencies: &[f64], now: Instant) -> AirEstimate {
    let (latency_s, source) = match median(latencies) {
        Some(m) => (m, "viewers"),
        None => (timeline.nominal_latency_s, "nominal"),
    };
    let t = timeline.edge_now(now) - latency_s;
    let seg = timeline.at(t);
    let item_id = seg.and_then(|s| s.item_id);
    let position_s = seg.and_then(|s| Some(s.position_s? + (t - s.channel_time).max(0.0)));
    AirEstimate {
        latency_s,
        source,
        viewers: latencies.len(),
        item_id,
        position_s,
    }
}

fn median(values: &[f64]) -> Option<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[mid]
    } else {
        f64::midpoint(v[mid - 1], v[mid])
    })
}

#[derive(Debug, Clone, Copy)]
struct Seen {
    latency_s: f64,
    last_seq: u64,
    last_at: Instant,
}

/// Each viewer address's latency, measured when its player joins.
#[derive(Debug, Default)]
pub struct AirViewers {
    seen: Mutex<HashMap<IpAddr, Seen>>,
}

impl AirViewers {
    /// `ip` asked for segment `seq` of `timeline` at `now`.
    pub fn record(&self, ip: IpAddr, seq: u64, timeline: &AirTimeline, now: Instant) {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.len() >= MAX_VIEWERS && !seen.contains_key(&ip) {
            seen.retain(|_, s| now.saturating_duration_since(s.last_at) < SEEN_WINDOW);
            if seen.len() >= MAX_VIEWERS {
                return;
            }
        }
        let joined = match seen.get(&ip) {
            None => true,
            Some(s) => {
                now.saturating_duration_since(s.last_at) > SEEN_WINDOW
                    || seq < s.last_seq
                    || seq > s.last_seq + JUMP_SEGMENTS
            }
        };
        if joined {
            let Some(seg) = timeline.segment(seq) else {
                return;
            };
            let latency_s = (timeline.edge_now(now) - seg.channel_time).clamp(0.0, KEEP_S);
            seen.insert(
                ip,
                Seen {
                    latency_s,
                    last_seq: seq,
                    last_at: now,
                },
            );
        } else if let Some(s) = seen.get_mut(&ip) {
            s.last_seq = s.last_seq.max(seq);
            s.last_at = now;
        }
    }

    /// The latencies of the viewers seen lately.
    pub fn latencies(&self, now: Instant) -> Vec<f64> {
        let seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        seen.values()
            .filter(|s| now.saturating_duration_since(s.last_at) <= SEEN_WINDOW)
            .map(|s| s.latency_s)
            .collect()
    }
}

/// The aired segments a channel keeps for the timeline (at least [`KEEP_S`] seconds).
#[derive(Debug, Default)]
pub struct AirLog {
    segments: VecDeque<AirSegment>,
    total_s: f64,
}

impl AirLog {
    pub fn push(&mut self, seg: AirSegment) {
        self.total_s += seg.duration;
        self.segments.push_back(seg);
        while let Some(front) = self.segments.front() {
            if self.total_s - front.duration < KEEP_S {
                break;
            }
            self.total_s -= front.duration;
            self.segments.pop_front();
        }
    }

    pub fn segments(&self) -> Vec<AirSegment> {
        self.segments.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Segments of `d` seconds from seq 10: film 7 from 100 s, then a slate, then film 8 from 0.
    fn timeline(d: f64, edge_at: Instant) -> AirTimeline {
        let mut segments = Vec::new();
        let mut t = 0.0;
        for n in 0..10u64 {
            let (item_id, position_s) = match n {
                0..=5 => (Some(7), Some(100.0 + n as f64 * d)),
                6 => (None, None),
                _ => (Some(8), Some((n - 7) as f64 * d)),
            };
            segments.push(AirSegment {
                seq: 10 + n,
                channel_time: t,
                duration: d,
                item_id,
                position_s,
            });
            t += d;
        }
        AirTimeline {
            segments,
            edge_s: t,
            edge_at: Some(edge_at),
            nominal_latency_s: 3.0 * d,
        }
    }

    #[test]
    fn without_viewers_the_nominal_latency_places_the_typical_viewer() {
        let now = Instant::now();
        let tl = timeline(4.0, now);
        // Edge 40 s, 12 s behind: channel time 28 s = seq 17 (film 8 at 0 s).
        let e = estimate(&tl, &[], now);
        assert_eq!((e.source, e.viewers, e.latency_s), ("nominal", 0, 12.0));
        assert_eq!(e.item_id, Some(8));
        assert!((e.position_s.unwrap() - 0.0).abs() < 1e-9);
        // 2 s later the edge moved on: film 8 at 2 s.
        let e = estimate(&tl, &[], now + Duration::from_secs(2));
        assert!((e.position_s.unwrap() - 2.0).abs() < 1e-9);
        // The edge never runs on by more than a segment.
        assert_eq!(tl.edge_now(now + Duration::from_secs(60)), 44.0);
        // A slate on the viewers' screen: no film.
        let e = estimate(&tl, &[13.0], now);
        assert_eq!((e.source, e.item_id, e.position_s), ("viewers", None, None));
    }

    #[test]
    fn the_median_of_the_viewers_counts() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[5.0]), Some(5.0));
        assert_eq!(median(&[9.0, 1.0, 5.0]), Some(5.0));
        assert_eq!(median(&[1.0, 2.0, 4.0, 9.0]), Some(3.0));
        let now = Instant::now();
        let tl = timeline(4.0, now);
        // 30 s behind: channel time 10 s = seq 12, film 7 at 108 + 2.
        let e = estimate(&tl, &[30.0, 30.0, 2.0], now);
        assert_eq!((e.item_id, e.position_s), (Some(7), Some(110.0)));
    }

    #[test]
    fn a_viewer_is_measured_when_it_joins_and_again_after_a_jump_or_a_gap() {
        let now = Instant::now();
        let tl = timeline(4.0, now);
        let v = AirViewers::default();
        let ip: IpAddr = "203.0.113.5".parse().unwrap();
        // Joins at seq 17 (starts at 28 s) one second after the edge (40 s) was published.
        v.record(ip, 17, &tl, now + Duration::from_secs(1));
        assert_eq!(v.latencies(now + Duration::from_secs(1)), vec![13.0]);
        // Following requests keep the measure.
        v.record(ip, 18, &tl, now + Duration::from_secs(3));
        v.record(ip, 19, &tl, now + Duration::from_secs(4));
        assert_eq!(v.latencies(now + Duration::from_secs(4)), vec![13.0]);
        // Back to an older segment: joined again.
        v.record(ip, 12, &tl, now + Duration::from_secs(4));
        assert_eq!(v.latencies(now + Duration::from_secs(4)), vec![36.0]);
        // Unknown segments are not measured; a quiet viewer stops counting.
        let other: IpAddr = "203.0.113.6".parse().unwrap();
        v.record(other, 999, &tl, now);
        assert_eq!(v.latencies(now + Duration::from_secs(4)).len(), 1);
        assert!(
            v.latencies(now + SEEN_WINDOW + Duration::from_secs(5))
                .is_empty()
        );
    }

    #[test]
    fn the_log_keeps_enough_seconds() {
        let mut log = AirLog::default();
        for seq in 0..100 {
            log.push(AirSegment {
                seq,
                channel_time: seq as f64 * 4.0,
                duration: 4.0,
                item_id: None,
                position_s: None,
            });
        }
        let kept = log.segments();
        let total: f64 = kept.iter().map(|s| s.duration).sum();
        assert!((KEEP_S..KEEP_S + 4.0).contains(&total), "{total}");
        assert_eq!(kept.last().unwrap().seq, 99);
    }
}
