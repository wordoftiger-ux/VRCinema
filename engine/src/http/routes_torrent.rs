//! The loopback stream route of the torrent client: `GET /internal/torrent/{token}/{hash}` serves
//! the file being downloaded (with `Range`) to ffmpeg on this computer, and the panel's
//! per-item `torrent` status.
//!
//! The route is for this computer only: the peer of the socket must be loopback, the request
//! must carry none of the headers a tunnel or a proxy adds (cloudflared connects from loopback
//! too), and the address has the random token of this run.

use std::io::SeekFrom;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{ApiError, AppState};
use crate::db::QueueItem;
use crate::torrent::hub::StreamSlot;
use crate::torrent::{FileStream, TorrentState, parse_magnet};

const CHUNK: usize = 64 * 1024;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/internal/torrent/{token}/{hash}", get(stream))
}

fn not_found() -> ApiError {
    ApiError::coded(StatusCode::NOT_FOUND, "not_found", &[])
}

/// Headers only a tunnel or a proxy puts on a request.
const FORWARDED: [&str; 6] = [
    "cf-connecting-ip",
    "x-forwarded-for",
    "cf-ray",
    "forwarded",
    "x-real-ip",
    "cf-visitor",
];

fn content_type(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "ts" | "m2ts" => "video/mp2t",
        "mpg" | "mpeg" => "video/mpeg",
        _ => "application/octet-stream",
    }
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` against a file of `len` bytes: `Ok(None)`: no range or
/// one that is not valid syntax (ignored, RFC 9110: everything is sent); `Err`: valid but not
/// satisfiable.
fn parse_range(value: &str, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((a, b)) = spec.split_once('-') else {
        return Ok(None);
    };
    let (a, b) = (a.trim(), b.trim());
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    let (start, end) = if a.is_empty() {
        let Some(n) = Some(b)
            .filter(|b| digits(b))
            .and_then(|b| b.parse::<u64>().ok())
        else {
            return Ok(None);
        };
        if n == 0 {
            return Err(());
        }
        (len.saturating_sub(n), len.saturating_sub(1))
    } else {
        let Some(start) = Some(a)
            .filter(|a| digits(a))
            .and_then(|a| a.parse::<u64>().ok())
        else {
            return Ok(None);
        };
        let end = if b.is_empty() {
            len.saturating_sub(1)
        } else {
            match Some(b)
                .filter(|b| digits(b))
                .and_then(|b| b.parse::<u64>().ok())
            {
                Some(end) if end >= start => end.min(len.saturating_sub(1)),
                // `bytes=5-3` or junk: not a valid range.
                _ => return Ok(None),
            }
        };
        (start, end)
    };
    if len == 0 || start >= len {
        return Err(());
    }
    Ok(Some((start, end)))
}

/// The bytes of `file` (positioned already), `remaining` of them. `slot` is held until the
/// body goes (sent, or the client left).
fn body_of(file: Box<dyn FileStream>, remaining: u64, slot: StreamSlot) -> Body {
    let stream = futures_util::stream::unfold(
        (file, remaining, slot),
        |(mut file, left, slot)| async move {
            if left == 0 {
                return None;
            }
            let mut buf = vec![0u8; CHUNK.min(usize::try_from(left).unwrap_or(CHUNK))];
            match file.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((
                        Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)),
                        (file, left - n as u64, slot),
                    ))
                }
                Err(e) => Some((Err(e), (file, 0, slot))),
            }
        },
    );
    Body::from_stream(stream)
}

async fn stream(
    State(state): State<AppState>,
    UrlPath((token, hash)): UrlPath<(String, String)>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let from_here = state.torrents.is_this_computer(peer.ip())
        && !FORWARDED.iter().any(|h| headers.contains_key(*h));
    if !from_here || !super::auth::safe_equal(&token, state.torrents.token()) {
        return Err(not_found());
    }
    let hash = hash.to_ascii_lowercase();
    let manager = state.torrents.manager_if_running().ok_or_else(not_found)?;
    // The length and the range are worked out before a stream is opened: each open stream
    // holds one of the library's few blocking slots.
    let status = manager
        .status(&hash)
        .filter(|s| matches!(s.state, TorrentState::Downloading | TorrentState::Complete))
        .ok_or_else(not_found)?;
    let len = status.total;
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, len));
    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, content_type(&status.file_name))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store");
    let (status, start, count) = match range {
        Some(Ok(Some((a, b)))) => {
            res = res.header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{len}"));
            (StatusCode::PARTIAL_CONTENT, a, b - a + 1)
        }
        Some(Err(())) => {
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .expect("static response"));
        }
        _ => (StatusCode::OK, 0, len),
    };
    let res = res
        .status(status)
        .header(header::CONTENT_LENGTH, HeaderValue::from(count));
    if method == Method::HEAD {
        return Ok(res.body(Body::empty()).expect("valid response"));
    }
    let busy = || ApiError::coded(StatusCode::SERVICE_UNAVAILABLE, "too_many_requests", &[]);
    let slot = state.torrents.stream_slot().ok_or_else(busy)?;
    let mut file = manager.open(&hash).await.map_err(|_| not_found())?;
    file.seek(SeekFrom::Start(start))
        .await
        .map_err(|_| not_found())?;
    Ok(res
        .body(body_of(file, count, slot))
        .expect("valid response"))
}

/// What the panel shows of a magnet item: `{state, progress, rateBps, etaS, ready}`; `idle`:
/// not started yet (or torrents are off).
pub(crate) fn torrent_json(state: &AppState, item: &QueueItem) -> Value {
    let status = parse_magnet(&item.source_url).and_then(|m| {
        let manager = state.torrents.manager_if_running()?;
        let s = manager.status(&m.info_hash)?;
        let ready = manager.will_make_it(&m.info_hash, item.duration_s);
        Some((s, ready))
    });
    let Some((s, ready)) = status else {
        return json!({
            "state": "idle", "progress": 0, "rateBps": 0, "etaS": null, "ready": false,
        });
    };
    let name = match s.state {
        TorrentState::Starting => "starting",
        TorrentState::Downloading => "downloading",
        TorrentState::Complete => "complete",
        TorrentState::WaitingForDisk => "waitingForDisk",
        TorrentState::Failed(_) => "failed",
    };
    let progress = if s.state == TorrentState::Complete {
        1.0
    } else if s.total > 0 {
        (s.downloaded as f64 / s.total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let eta = (s.state == TorrentState::Downloading && s.rate_bps > 0.0 && s.total > 0)
        .then(|| (s.total.saturating_sub(s.downloaded) as f64 / s.rate_bps).ceil() as u64);
    json!({
        "state": name,
        "progress": super::num_json((progress * 10_000.0).round() / 10_000.0),
        "rateBps": s.rate_bps.round() as u64,
        "etaS": eta,
        "ready": matches!(ready, crate::torrent::Readiness::Ready),
    })
}

/// Adds `torrent` to the rows of the magnet items of `items` (as listed, same order).
pub(crate) fn add_torrent_status(state: &AppState, items: &[QueueItem], json: &mut Value) {
    let Value::Array(rows) = json else { return };
    for (item, row) in items.iter().zip(rows.iter_mut()) {
        if let Value::Object(map) = row
            && crate::resolver::torrent::is_magnet(&item.source_url)
        {
            map.insert("torrent".into(), torrent_json(state, item));
        }
    }
}
