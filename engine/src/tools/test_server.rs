//! Local HTTP server for the tools tests (never the real internet): serves fixture bytes by
//! path, supports `Range`, can drop the connection halfway through the first response and can
//! pretend not to know `Range`.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::get;

const CHUNK: usize = 16 * 1024;

#[derive(Default)]
struct Shared {
    files: Mutex<HashMap<String, Vec<u8>>>,
    /// Cut the next response longer than this after this many body bytes.
    drop_after: Mutex<Option<usize>>,
    ignore_range: AtomicBool,
    /// Answer a `Range` request with 206 for one byte later than asked.
    wrong_range_start: AtomicBool,
    requests: Mutex<Vec<(String, Option<String>)>>,
    /// Paths that answer with this status instead of their file.
    failing: Mutex<HashMap<String, StatusCode>>,
}

pub struct TestServer {
    pub base: String,
    shared: Arc<Shared>,
}

impl TestServer {
    pub async fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let app = Router::new()
            .route("/{*path}", get(serve))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestServer {
            base: format!("http://127.0.0.1:{port}"),
            shared,
        }
    }

    pub fn put(&self, path: &str, bytes: impl Into<Vec<u8>>) {
        self.shared.failing.lock().unwrap().remove(path);
        self.shared
            .files
            .lock()
            .unwrap()
            .insert(path.to_string(), bytes.into());
    }

    /// Makes `path` answer with HTTP `status` (and an empty body) from now on.
    pub fn fail_with(&self, path: &str, status: u16) {
        self.shared.failing.lock().unwrap().insert(
            path.to_string(),
            StatusCode::from_u16(status).expect("valid status"),
        );
    }

    pub fn drop_next_response_after(&self, bytes: usize) {
        *self.shared.drop_after.lock().unwrap() = Some(bytes);
    }

    pub fn set_ignore_range(&self, ignore: bool) {
        self.shared.ignore_range.store(ignore, Ordering::SeqCst);
    }

    pub fn set_wrong_range_start(&self, wrong: bool) {
        self.shared.wrong_range_start.store(wrong, Ordering::SeqCst);
    }

    /// `(path, Range header)` of every request so far.
    pub fn requests(&self) -> Vec<(String, Option<String>)> {
        self.shared.requests.lock().unwrap().clone()
    }

    pub fn requested_paths(&self) -> Vec<String> {
        self.requests().into_iter().map(|(p, _)| p).collect()
    }
}

fn parse_range_start(value: &str) -> Option<usize> {
    value
        .strip_prefix("bytes=")?
        .split('-')
        .next()?
        .parse()
        .ok()
}

async fn serve(
    State(shared): State<Arc<Shared>>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Response {
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    shared
        .requests
        .lock()
        .unwrap()
        .push((path.clone(), range.clone()));
    let failing = shared.failing.lock().unwrap().get(&path).copied();
    if let Some(status) = failing {
        return Response::builder()
            .status(status)
            .body(Body::empty())
            .unwrap();
    }
    let Some(data) = shared.files.lock().unwrap().get(&path).cloned() else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap();
    };
    let total = data.len();
    let mut start = if shared.ignore_range.load(Ordering::SeqCst) {
        0
    } else {
        range.as_deref().and_then(parse_range_start).unwrap_or(0)
    };
    if start > 0 && shared.wrong_range_start.load(Ordering::SeqCst) {
        start += 1;
    }
    if start >= total && start > 0 {
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .body(Body::empty())
            .unwrap();
    }
    let body_bytes = data[start..].to_vec();
    let remaining = body_bytes.len();
    // Only a response longer than the threshold is cut (small sums files pass untouched).
    let drop_after = {
        let mut slot = shared.drop_after.lock().unwrap();
        if slot.is_some_and(|n| n < remaining) {
            slot.take()
        } else {
            None
        }
    };
    let limit = drop_after.unwrap_or(remaining);
    let broken = drop_after.is_some();

    // (offset, bytes, already failed)
    let stream = futures_util::stream::unfold(
        (0usize, body_bytes, false),
        move |(pos, buf, done)| async move {
            if done {
                return None;
            }
            if pos >= limit {
                return if broken {
                    Some((
                        Err(io::Error::new(io::ErrorKind::ConnectionReset, "dropped")),
                        (pos, buf, true),
                    ))
                } else {
                    None
                };
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
            let end = (pos + CHUNK).min(limit);
            let chunk = Bytes::copy_from_slice(&buf[pos..end]);
            Some((Ok::<Bytes, io::Error>(chunk), (end, buf, false)))
        },
    );

    let mut builder = Response::builder()
        .header(header::CONTENT_LENGTH, remaining)
        .header(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if start > 0 {
        builder = builder.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{}/{total}", total - 1),
        );
    }
    builder.body(Body::from_stream(stream)).unwrap()
}

/// Builds an in-memory zip archive.
pub fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in files {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}
