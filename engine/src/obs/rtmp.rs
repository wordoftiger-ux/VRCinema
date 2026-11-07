//! The server side of RTMP for one publisher (OBS): the plain handshake, the chunk stream,
//! the few AMF0 commands a publisher sends, and its audio, video and metadata turned into an
//! FLV byte stream for ffmpeg's stdin.
//!
//! Only what publishing needs: no playing, no AMF3 objects, no encrypted (RTMPE) handshake.
//! OBS, librtmp and ffmpeg accept the plain handshake (version bytes 0, S2 = C1).

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

/// Size of C1/C2/S1/S2.
const HANDSHAKE_SIZE: usize = 1536;
/// The chunk size until the peer sets another.
const DEFAULT_CHUNK_SIZE: usize = 128;
/// The chunk size of what this server sends (announced right after `connect`).
const OUT_CHUNK_SIZE: usize = 4096;
/// A larger message is refused (a video frame of a 1080p stream is well below it).
pub const MAX_MESSAGE: usize = 8 * 1024 * 1024;
/// More chunk streams at once than any publisher uses.
const MAX_CHUNK_STREAMS: usize = 64;
/// The acknowledgement window this server asks of the client, and its peer bandwidth.
const WINDOW: u32 = 5_000_000;
/// Nesting of AMF0 objects a command may have.
const MAX_AMF_DEPTH: usize = 8;

/// The FLV file header (audio and video) and the first "previous tag size".
pub const FLV_HEADER: [u8; 13] = [b'F', b'L', b'V', 1, 5, 0, 0, 0, 9, 0, 0, 0, 0];

// Message types.
const SET_CHUNK_SIZE: u8 = 1;
const ACKNOWLEDGEMENT: u8 = 3;
const WINDOW_ACK_SIZE: u8 = 5;
const SET_PEER_BANDWIDTH: u8 = 6;
const AUDIO: u8 = 8;
const VIDEO: u8 = 9;
const DATA_AMF3: u8 = 15;
const COMMAND_AMF3: u8 = 17;
const DATA_AMF0: u8 = 18;
const COMMAND_AMF0: u8 = 20;

#[derive(Debug, thiserror::Error)]
pub enum RtmpError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("RTMP: {0}")]
    Protocol(&'static str),
    #[error("RTMP: timed out")]
    Timeout,
}

fn bad(what: &'static str) -> RtmpError {
    RtmpError::Protocol(what)
}

// ---------- AMF0 ----------

/// An AMF0 value (what publishers send in commands and metadata).
#[derive(Debug, Clone, PartialEq)]
pub enum Amf {
    Number(f64),
    Bool(bool),
    Str(String),
    Object(Vec<(String, Amf)>),
    Null,
    Undefined,
    /// ECMA array: keys and values like an object.
    EcmaArray(Vec<(String, Amf)>),
    StrictArray(Vec<Amf>),
}

impl Amf {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }
}

fn take<'a>(data: &mut &'a [u8], n: usize) -> Result<&'a [u8], RtmpError> {
    if data.len() < n {
        return Err(bad("AMF0 value cut short"));
    }
    let (head, rest) = data.split_at(n);
    *data = rest;
    Ok(head)
}

fn amf_string(data: &mut &[u8], long: bool) -> Result<String, RtmpError> {
    let len = if long {
        u32::from_be_bytes(take(data, 4)?.try_into().unwrap_or_default()) as usize
    } else {
        u16::from_be_bytes(take(data, 2)?.try_into().unwrap_or_default()) as usize
    };
    Ok(String::from_utf8_lossy(take(data, len)?).into_owned())
}

fn amf_properties(data: &mut &[u8], depth: usize) -> Result<Vec<(String, Amf)>, RtmpError> {
    let mut out = Vec::new();
    loop {
        let key = amf_string(data, false)?;
        if key.is_empty() && data.first() == Some(&9) {
            *data = &data[1..];
            return Ok(out);
        }
        let value = amf_value(data, depth + 1)?;
        out.push((key, value));
    }
}

fn amf_value(data: &mut &[u8], depth: usize) -> Result<Amf, RtmpError> {
    if depth > MAX_AMF_DEPTH {
        return Err(bad("AMF0 nested too deep"));
    }
    let marker = take(data, 1)?[0];
    Ok(match marker {
        0 => Amf::Number(f64::from_be_bytes(
            take(data, 8)?.try_into().unwrap_or_default(),
        )),
        1 => Amf::Bool(take(data, 1)?[0] != 0),
        2 => Amf::Str(amf_string(data, false)?),
        3 => Amf::Object(amf_properties(data, depth)?),
        5 => Amf::Null,
        6 => Amf::Undefined,
        8 => {
            take(data, 4)?; // the count is a hint only
            Amf::EcmaArray(amf_properties(data, depth)?)
        }
        10 => {
            let count = u32::from_be_bytes(take(data, 4)?.try_into().unwrap_or_default());
            let mut items = Vec::new();
            for _ in 0..count {
                items.push(amf_value(data, depth + 1)?);
            }
            Amf::StrictArray(items)
        }
        11 => {
            take(data, 10)?; // a date: 8 bytes of time, 2 of time zone
            Amf::Undefined
        }
        12 => Amf::Str(amf_string(data, true)?),
        _ => return Err(bad("AMF0 type not supported")),
    })
}

/// All the values of an AMF0 payload, in order.
pub fn amf_decode(mut data: &[u8]) -> Result<Vec<Amf>, RtmpError> {
    let mut out = Vec::new();
    while !data.is_empty() {
        out.push(amf_value(&mut data, 0)?);
    }
    Ok(out)
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let bytes = &s.as_bytes()[..s.len().min(u16::MAX as usize)];
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

pub fn amf_encode(value: &Amf, out: &mut Vec<u8>) {
    match value {
        Amf::Number(n) => {
            out.push(0);
            out.extend_from_slice(&n.to_be_bytes());
        }
        Amf::Bool(b) => out.extend_from_slice(&[1, u8::from(*b)]),
        Amf::Str(s) => {
            out.push(2);
            put_str(out, s);
        }
        Amf::Object(props) | Amf::EcmaArray(props) => {
            if let Amf::EcmaArray(_) = value {
                out.push(8);
                out.extend_from_slice(&(props.len() as u32).to_be_bytes());
            } else {
                out.push(3);
            }
            for (key, value) in props {
                put_str(out, key);
                amf_encode(value, out);
            }
            out.extend_from_slice(&[0, 0, 9]);
        }
        Amf::Null => out.push(5),
        Amf::Undefined => out.push(6),
        Amf::StrictArray(items) => {
            out.push(10);
            out.extend_from_slice(&(items.len() as u32).to_be_bytes());
            for item in items {
                amf_encode(item, out);
            }
        }
    }
}

fn amf_payload(values: &[Amf]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        amf_encode(value, &mut out);
    }
    out
}

fn object(props: &[(&str, Amf)]) -> Amf {
    Amf::Object(
        props
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    )
}

// ---------- Chunk stream ----------

/// One whole RTMP message.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub type_id: u8,
    pub stream_id: u32,
    pub timestamp: u32,
    pub payload: Vec<u8>,
}

/// What a chunk stream remembers between its chunks (headers of types 1–3 leave fields out).
#[derive(Debug, Default)]
struct ChunkStream {
    timestamp: u32,
    delta: u32,
    length: usize,
    type_id: u8,
    stream_id: u32,
    /// The last full header used an extended timestamp (type 3 chunks then carry one too).
    extended: bool,
    buf: Vec<u8>,
}

/// Reads whole messages from the client's chunk stream.
pub struct ChunkReader<R> {
    r: R,
    chunk_size: usize,
    streams: HashMap<u32, ChunkStream>,
    /// Bytes read so far (for the acknowledgements).
    pub bytes: u64,
}

impl<R: AsyncRead + Unpin> ChunkReader<R> {
    pub fn new(r: R) -> Self {
        Self {
            r,
            chunk_size: DEFAULT_CHUNK_SIZE,
            streams: HashMap::new(),
            bytes: 0,
        }
    }

    async fn fill(&mut self, buf: &mut [u8]) -> Result<(), RtmpError> {
        self.r.read_exact(buf).await?;
        self.bytes += buf.len() as u64;
        Ok(())
    }

    async fn u8(&mut self) -> Result<u8, RtmpError> {
        let mut b = [0u8; 1];
        self.fill(&mut b).await?;
        Ok(b[0])
    }

    async fn u24(&mut self) -> Result<u32, RtmpError> {
        let mut b = [0u8; 3];
        self.fill(&mut b).await?;
        Ok(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }

    async fn u32_be(&mut self) -> Result<u32, RtmpError> {
        let mut b = [0u8; 4];
        self.fill(&mut b).await?;
        Ok(u32::from_be_bytes(b))
    }

    async fn u32_le(&mut self) -> Result<u32, RtmpError> {
        let mut b = [0u8; 4];
        self.fill(&mut b).await?;
        Ok(u32::from_le_bytes(b))
    }

    /// The next whole message (chunks of others streams in between are gathered meanwhile).
    pub async fn read_message(&mut self) -> Result<Message, RtmpError> {
        loop {
            if let Some(message) = self.read_chunk().await? {
                return Ok(message);
            }
        }
    }

    async fn read_chunk(&mut self) -> Result<Option<Message>, RtmpError> {
        let first = self.u8().await?;
        let fmt = first >> 6;
        let csid = match first & 0x3f {
            0 => 64 + u32::from(self.u8().await?),
            1 => {
                let low = u32::from(self.u8().await?);
                let high = u32::from(self.u8().await?);
                64 + low + high * 256
            }
            n => u32::from(n),
        };
        if !self.streams.contains_key(&csid) && self.streams.len() >= MAX_CHUNK_STREAMS {
            return Err(bad("too many chunk streams"));
        }
        let was_extended = self.streams.get(&csid).is_some_and(|s| s.extended);
        // The header fields this chunk carries.
        let mut time = None;
        let mut length = None;
        let mut type_id = None;
        let mut stream_id = None;
        let mut extended = was_extended;
        if fmt <= 2 {
            let mut t = self.u24().await?;
            if fmt <= 1 {
                length = Some(self.u24().await? as usize);
                type_id = Some(self.u8().await?);
            }
            if fmt == 0 {
                stream_id = Some(self.u32_le().await?);
            }
            extended = t == 0x00ff_ffff;
            if extended {
                t = self.u32_be().await?;
            }
            time = Some(t);
        } else if was_extended {
            self.u32_be().await?; // repeats the timestamp already known
        }
        let chunk_size = self.chunk_size;
        let st = self.streams.entry(csid).or_default();
        st.extended = extended;
        if fmt <= 2 || st.buf.is_empty() {
            // A new message (types 0–2 always start one; a type 3 chunk does when nothing is
            // pending): timestamp, length, type and stream from the header or the last one.
            st.buf.clear();
            if let Some(len) = length {
                st.length = len;
            }
            if let Some(t) = type_id {
                st.type_id = t;
            }
            if let Some(s) = stream_id {
                st.stream_id = s;
            }
            match (fmt, time) {
                (0, Some(t)) => {
                    st.timestamp = t;
                    st.delta = 0;
                }
                (_, Some(d)) => {
                    st.delta = d;
                    st.timestamp = st.timestamp.wrapping_add(d);
                }
                (_, None) => st.timestamp = st.timestamp.wrapping_add(st.delta),
            }
            if st.length > MAX_MESSAGE {
                return Err(bad("message too large"));
            }
        }
        let want = chunk_size.min(st.length - st.buf.len());
        let start = st.buf.len();
        st.buf.resize(start + want, 0);
        let mut part = std::mem::take(&mut st.buf);
        let read = self.fill(&mut part[start..]).await;
        let st = self.streams.entry(csid).or_default();
        st.buf = part;
        read?;
        if st.buf.len() < st.length {
            return Ok(None);
        }
        Ok(Some(Message {
            type_id: st.type_id,
            stream_id: st.stream_id,
            timestamp: st.timestamp,
            payload: std::mem::take(&mut st.buf),
        }))
    }
}

/// `m` as chunks of chunk stream `csid` (a full header, then type 3 chunks).
pub fn encode_chunks(csid: u8, m: &Message, chunk_size: usize) -> Vec<u8> {
    let extended = m.timestamp >= 0x00ff_ffff;
    let mut out = Vec::with_capacity(m.payload.len() + 16);
    out.push(csid & 0x3f);
    let ts = if extended { 0x00ff_ffff } else { m.timestamp };
    out.extend_from_slice(&ts.to_be_bytes()[1..]);
    out.extend_from_slice(&(m.payload.len() as u32).to_be_bytes()[1..]);
    out.push(m.type_id);
    out.extend_from_slice(&m.stream_id.to_le_bytes());
    if extended {
        out.extend_from_slice(&m.timestamp.to_be_bytes());
    }
    for (i, chunk) in m.payload.chunks(chunk_size.max(1)).enumerate() {
        if i > 0 {
            out.push(0xc0 | (csid & 0x3f));
            if extended {
                out.extend_from_slice(&m.timestamp.to_be_bytes());
            }
        }
        out.extend_from_slice(chunk);
    }
    out
}

fn control(type_id: u8, payload: Vec<u8>) -> Vec<u8> {
    encode_chunks(
        2,
        &Message {
            type_id,
            stream_id: 0,
            timestamp: 0,
            payload,
        },
        OUT_CHUNK_SIZE,
    )
}

fn command(csid: u8, stream_id: u32, values: &[Amf]) -> Vec<u8> {
    encode_chunks(
        csid,
        &Message {
            type_id: COMMAND_AMF0,
            stream_id,
            timestamp: 0,
            payload: amf_payload(values),
        },
        OUT_CHUNK_SIZE,
    )
}

// ---------- FLV ----------

/// One FLV tag (with its trailing "previous tag size").
pub fn flv_tag(tag_type: u8, timestamp: u32, data: &[u8]) -> Vec<u8> {
    let size = data.len() as u32;
    let mut out = Vec::with_capacity(data.len() + 15);
    out.push(tag_type);
    out.extend_from_slice(&size.to_be_bytes()[1..]);
    out.extend_from_slice(&timestamp.to_be_bytes()[1..]);
    out.push((timestamp >> 24) as u8);
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(data);
    out.extend_from_slice(&(size + 11).to_be_bytes());
    out
}

/// A data message as FLV script data: `@setDataFrame` (how publishers wrap `onMetaData`) is
/// taken off; `None`: not metadata (cue points and the like are left out).
fn script_data(payload: &[u8]) -> Option<Vec<u8>> {
    let mut rest = payload;
    let first = amf_value(&mut rest, 0).ok()?;
    let body = if first.as_str() == Some("@setDataFrame") {
        rest
    } else {
        payload
    };
    let mut probe = body;
    match amf_value(&mut probe, 0).ok()?.as_str() {
        Some("onMetaData") => Some(body.to_vec()),
        _ => None,
    }
}

// ---------- Session ----------

/// What a session asks of its owner.
pub trait Ingest: Send {
    /// The stream key (the name of `publish`) is right.
    fn key_ok(&mut self, name: &str) -> bool;
    /// The publisher starts: where its FLV goes. `None`: another publisher is on air, refused.
    fn open(&mut self) -> Option<mpsc::Sender<Vec<u8>>>;
}

/// How a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionEnd {
    /// The client left (or ended its stream).
    Closed,
    /// `publish` with a wrong stream key.
    WrongKey,
    /// Another publisher streams already.
    Busy,
    /// The channel stopped taking the stream (it is gone or restarting).
    Dropped,
}

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// The client must have published (with the right key) within this time.
    pub auth: Duration,
    /// A publisher that sends nothing for this long is dropped.
    pub idle: Duration,
}

async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S) -> Result<(), RtmpError> {
    let mut c0c1 = vec![0u8; 1 + HANDSHAKE_SIZE];
    s.read_exact(&mut c0c1).await?;
    if c0c1[0] != 3 {
        return Err(bad("not a plain RTMP handshake"));
    }
    let mut out = Vec::with_capacity(1 + 2 * HANDSHAKE_SIZE);
    out.push(3);
    // S1: time 0, version 0 (no digest), random bytes.
    let mut s1 = vec![0u8; HANDSHAKE_SIZE];
    getrandom::fill(&mut s1[8..]).map_err(|_| bad("no random numbers"))?;
    out.extend_from_slice(&s1);
    // S2: C1 echoed.
    out.extend_from_slice(&c0c1[1..]);
    s.write_all(&out).await?;
    s.flush().await?;
    let mut c2 = vec![0u8; HANDSHAKE_SIZE];
    s.read_exact(&mut c2).await?;
    Ok(())
}

/// The stream key out of a `publish` name (`key?query` from some clients).
fn stream_key(name: &str) -> &str {
    name.split('?').next().unwrap_or_default()
}

fn status(code: &str, level: &str, description: &str) -> Amf {
    object(&[
        ("level", Amf::Str(level.into())),
        ("code", Amf::Str(code.into())),
        ("description", Amf::Str(description.into())),
    ])
}

/// Serves one RTMP client until it leaves; its stream goes to `ingest` once it published with
/// the right key.
pub async fn run_session<S, I>(
    mut stream: S,
    ingest: &mut I,
    timeouts: Timeouts,
) -> Result<SessionEnd, RtmpError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: Ingest,
{
    let deadline = tokio::time::Instant::now() + timeouts.auth;
    tokio::time::timeout_at(deadline, handshake(&mut stream))
        .await
        .map_err(|_| RtmpError::Timeout)??;
    let (rd, mut wr) = tokio::io::split(stream);
    let mut reader = ChunkReader::new(rd);
    let mut sink: Option<mpsc::Sender<Vec<u8>>> = None;
    // The client's acknowledgement window (how often it wants to hear how much arrived).
    let mut ack_window: Option<u64> = None;
    let mut acked = 0u64;
    loop {
        let next = reader.read_message();
        let message = if sink.is_some() {
            tokio::time::timeout(timeouts.idle, next).await
        } else {
            tokio::time::timeout_at(deadline, next).await
        };
        let message = match message {
            Err(_) => return Err(RtmpError::Timeout),
            Ok(Err(RtmpError::Io(e))) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(SessionEnd::Closed);
            }
            Ok(r) => r?,
        };
        if let Some(window) = ack_window
            && reader.bytes - acked >= window
        {
            acked = reader.bytes;
            wr.write_all(&control(
                ACKNOWLEDGEMENT,
                (reader.bytes as u32).to_be_bytes().to_vec(),
            ))
            .await?;
        }
        match message.type_id {
            SET_CHUNK_SIZE => {
                let size = message
                    .payload
                    .get(..4)
                    .and_then(|b| b.try_into().ok())
                    .map(u32::from_be_bytes)
                    .ok_or_else(|| bad("bad chunk size"))?
                    & 0x7fff_ffff;
                if size == 0 || size as usize > MAX_MESSAGE {
                    return Err(bad("bad chunk size"));
                }
                reader.chunk_size = size as usize;
            }
            WINDOW_ACK_SIZE => {
                ack_window = message
                    .payload
                    .get(..4)
                    .and_then(|b| b.try_into().ok())
                    .map(|b| u64::from(u32::from_be_bytes(b)))
                    .filter(|w| *w > 0);
            }
            AUDIO | VIDEO | DATA_AMF0 | DATA_AMF3 => {
                let Some(tx) = &sink else {
                    continue;
                };
                let tag = match message.type_id {
                    AUDIO | VIDEO => flv_tag(message.type_id, message.timestamp, &message.payload),
                    _ => {
                        let payload = match message.type_id {
                            DATA_AMF3 => message.payload.get(1..).unwrap_or_default(),
                            _ => &message.payload[..],
                        };
                        match script_data(payload) {
                            Some(data) => flv_tag(DATA_AMF0, message.timestamp, &data),
                            None => continue,
                        }
                    }
                };
                if tx.send(tag).await.is_err() {
                    return Ok(SessionEnd::Dropped);
                }
            }
            COMMAND_AMF0 | COMMAND_AMF3 => {
                let payload = match message.type_id {
                    COMMAND_AMF3 => message.payload.get(1..).unwrap_or_default(),
                    _ => &message.payload[..],
                };
                let values = amf_decode(payload)?;
                let name = values.first().and_then(Amf::as_str).unwrap_or_default();
                let txn = values.get(1).and_then(Amf::as_number).unwrap_or(0.0);
                match name {
                    "connect" => {
                        let mut out = control(WINDOW_ACK_SIZE, WINDOW.to_be_bytes().to_vec());
                        let mut bw = WINDOW.to_be_bytes().to_vec();
                        bw.push(2);
                        out.extend(control(SET_PEER_BANDWIDTH, bw));
                        out.extend(control(
                            SET_CHUNK_SIZE,
                            (OUT_CHUNK_SIZE as u32).to_be_bytes().to_vec(),
                        ));
                        out.extend(command(
                            3,
                            0,
                            &[
                                Amf::Str("_result".into()),
                                Amf::Number(txn),
                                object(&[
                                    ("fmsVer", Amf::Str("FMS/3,0,1,123".into())),
                                    ("capabilities", Amf::Number(31.0)),
                                ]),
                                object(&[
                                    ("level", Amf::Str("status".into())),
                                    ("code", Amf::Str("NetConnection.Connect.Success".into())),
                                    ("description", Amf::Str("Connection succeeded.".into())),
                                    ("objectEncoding", Amf::Number(0.0)),
                                ]),
                            ],
                        ));
                        wr.write_all(&out).await?;
                    }
                    "createStream" => {
                        wr.write_all(&command(
                            3,
                            0,
                            &[
                                Amf::Str("_result".into()),
                                Amf::Number(txn),
                                Amf::Null,
                                Amf::Number(1.0),
                            ],
                        ))
                        .await?;
                    }
                    "publish" => {
                        if sink.is_some() {
                            continue;
                        }
                        let key = values.get(3).and_then(Amf::as_str).unwrap_or_default();
                        let (end, code) = if !ingest.key_ok(stream_key(key)) {
                            (SessionEnd::WrongKey, "NetStream.Publish.BadName")
                        } else {
                            match ingest.open() {
                                Some(tx) => {
                                    if tx.send(FLV_HEADER.to_vec()).await.is_err() {
                                        return Ok(SessionEnd::Dropped);
                                    }
                                    sink = Some(tx);
                                    (SessionEnd::Closed, "NetStream.Publish.Start")
                                }
                                None => (SessionEnd::Busy, "NetStream.Publish.BadName"),
                            }
                        };
                        let started = sink.is_some();
                        let (level, description) = if started {
                            ("status", "Start publishing.")
                        } else {
                            ("error", "Refused.")
                        };
                        wr.write_all(&command(
                            5,
                            message.stream_id,
                            &[
                                Amf::Str("onStatus".into()),
                                Amf::Number(0.0),
                                Amf::Null,
                                status(code, level, description),
                            ],
                        ))
                        .await?;
                        wr.flush().await?;
                        if !started {
                            return Ok(end);
                        }
                    }
                    "FCUnpublish" | "deleteStream" | "closeStream" => {
                        if sink.is_some() {
                            return Ok(SessionEnd::Closed);
                        }
                    }
                    _ => {}
                }
                wr.flush().await?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client side of what the tests need: handshake, chunked commands, media.
    pub(crate) struct Client<S> {
        pub s: S,
        pub chunk: usize,
    }

    impl<S: AsyncRead + AsyncWrite + Unpin> Client<S> {
        pub async fn connect(mut s: S) -> Self {
            let mut c0c1 = vec![3u8];
            c0c1.extend(std::iter::repeat_n(7u8, HANDSHAKE_SIZE));
            s.write_all(&c0c1).await.unwrap();
            let mut s0s1s2 = vec![0u8; 1 + 2 * HANDSHAKE_SIZE];
            s.read_exact(&mut s0s1s2).await.unwrap();
            assert_eq!(s0s1s2[0], 3);
            assert_eq!(&s0s1s2[1 + HANDSHAKE_SIZE..], &c0c1[1..], "S2 echoes C1");
            s.write_all(&s0s1s2[1..=HANDSHAKE_SIZE]).await.unwrap();
            Self { s, chunk: 128 }
        }

        pub async fn send(&mut self, csid: u8, m: &Message) {
            let bytes = encode_chunks(csid, m, self.chunk);
            self.s.write_all(&bytes).await.unwrap();
        }

        pub async fn command(&mut self, values: &[Amf], stream_id: u32) {
            self.send(
                3,
                &Message {
                    type_id: COMMAND_AMF0,
                    stream_id,
                    timestamp: 0,
                    payload: amf_payload(values),
                },
            )
            .await;
        }

        pub async fn publish(&mut self, key: &str) {
            self.command(
                &[
                    Amf::Str("connect".into()),
                    Amf::Number(1.0),
                    object(&[("app", Amf::Str("live".into()))]),
                ],
                0,
            )
            .await;
            self.command(
                &[
                    Amf::Str("releaseStream".into()),
                    Amf::Number(2.0),
                    Amf::Null,
                    Amf::Str(key.into()),
                ],
                0,
            )
            .await;
            self.command(
                &[Amf::Str("createStream".into()), Amf::Number(3.0), Amf::Null],
                0,
            )
            .await;
            self.command(
                &[
                    Amf::Str("publish".into()),
                    Amf::Number(4.0),
                    Amf::Null,
                    Amf::Str(key.into()),
                    Amf::Str("live".into()),
                ],
                1,
            )
            .await;
        }

        /// Every message the server sent until `code` appears in an `onStatus` or `_result`.
        pub async fn read_until(&mut self, code: &str) -> Vec<Message> {
            let mut reader = ChunkReader::new(&mut self.s);
            let mut seen = Vec::new();
            loop {
                let m = reader.read_message().await.unwrap();
                if m.type_id == SET_CHUNK_SIZE {
                    reader.chunk_size =
                        u32::from_be_bytes(m.payload[..4].try_into().unwrap()) as usize;
                }
                let found = m.type_id == COMMAND_AMF0
                    && amf_decode(&m.payload).unwrap().iter().any(|v| match v {
                        Amf::Object(props) => props
                            .iter()
                            .any(|(k, v)| k == "code" && v.as_str() == Some(code)),
                        Amf::Str(s) => s == code,
                        _ => false,
                    });
                seen.push(m);
                if found {
                    return seen;
                }
            }
        }
    }

    struct TestIngest {
        key: &'static str,
        tx: Option<mpsc::Sender<Vec<u8>>>,
    }

    impl Ingest for TestIngest {
        fn key_ok(&mut self, name: &str) -> bool {
            name == self.key
        }
        fn open(&mut self) -> Option<mpsc::Sender<Vec<u8>>> {
            self.tx.take()
        }
    }

    const T: Timeouts = Timeouts {
        auth: Duration::from_secs(5),
        idle: Duration::from_secs(5),
    };

    #[test]
    fn amf_round_trips_the_command_values() {
        let values = vec![
            Amf::Str("connect".into()),
            Amf::Number(1.0),
            object(&[
                ("app", Amf::Str("live".into())),
                ("flag", Amf::Bool(true)),
                ("nested", object(&[("n", Amf::Null)])),
            ]),
            Amf::EcmaArray(vec![("w".into(), Amf::Number(1920.0))]),
            Amf::StrictArray(vec![Amf::Undefined]),
        ];
        assert_eq!(amf_decode(&amf_payload(&values)).unwrap(), values);
        assert!(amf_decode(&[2, 0, 9, b'a']).is_err(), "a string cut short");
        assert!(amf_decode(&[0x0d]).is_err(), "an unsupported type");
    }

    #[tokio::test]
    async fn chunks_of_any_size_and_interleaved_streams_make_whole_messages() {
        let (a, b) = tokio::io::duplex(1 << 20);
        let mut reader = ChunkReader::new(b);
        let big = Message {
            type_id: VIDEO,
            stream_id: 1,
            timestamp: 0x0123_4567, // extended
            payload: (0..1000u32).map(|i| i as u8).collect(),
        };
        let small = Message {
            type_id: AUDIO,
            stream_id: 1,
            timestamp: 40,
            payload: vec![0xaf, 1, 2, 3],
        };
        let mut wire = encode_chunks(6, &big, 128);
        // Another stream's whole message between the first two chunks of the big one.
        let audio = encode_chunks(4, &small, 128);
        let split = 1 + 11 + 4 + 128;
        let rest = wire.split_off(split);
        let mut a = a;
        a.write_all(&wire).await.unwrap();
        a.write_all(&audio).await.unwrap();
        a.write_all(&rest).await.unwrap();
        // A type 1 header (delta 20, same stream) then a type 3 new message (delta again).
        let mut next = vec![0x40 | 4, 0, 0, 20, 0, 0, 2, AUDIO, 9, 9];
        next.extend([0xc0 | 4, 8, 8]);
        a.write_all(&next).await.unwrap();
        assert_eq!(reader.read_message().await.unwrap(), small);
        assert_eq!(reader.read_message().await.unwrap(), big);
        let m = reader.read_message().await.unwrap();
        assert_eq!((m.timestamp, m.payload), (60, vec![9, 9]));
        let m = reader.read_message().await.unwrap();
        assert_eq!((m.timestamp, m.payload), (80, vec![8, 8]));
    }

    #[tokio::test]
    async fn a_message_over_the_limit_is_refused() {
        let (mut a, b) = tokio::io::duplex(1024);
        let mut reader = ChunkReader::new(b);
        a.write_all(&[3, 0, 0, 0, 0xff, 0xff, 0xff, VIDEO, 1, 0, 0, 0])
            .await
            .unwrap();
        assert!(matches!(
            reader.read_message().await,
            Err(RtmpError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn a_publisher_with_the_key_streams_flv() {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (tx, mut rx) = mpsc::channel(64);
        let session = tokio::spawn(async move {
            let mut ingest = TestIngest {
                key: "sekret",
                tx: Some(tx),
            };
            run_session(server, &mut ingest, T).await.unwrap()
        });
        let mut c = Client::connect(client).await;
        c.publish("sekret?x=1").await;
        let answers = c.read_until("NetStream.Publish.Start").await;
        assert!(answers.iter().any(|m| m.type_id == WINDOW_ACK_SIZE));
        // Metadata (wrapped as publishers do), then audio and video.
        let mut meta = amf_payload(&[
            Amf::Str("@setDataFrame".into()),
            Amf::Str("onMetaData".into()),
        ]);
        amf_encode(
            &Amf::EcmaArray(vec![("width".into(), Amf::Number(1280.0))]),
            &mut meta,
        );
        c.send(
            4,
            &Message {
                type_id: DATA_AMF0,
                stream_id: 1,
                timestamp: 0,
                payload: meta,
            },
        )
        .await;
        c.send(
            6,
            &Message {
                type_id: VIDEO,
                stream_id: 1,
                timestamp: 33,
                payload: vec![0x17, 0, 0, 0, 0],
            },
        )
        .await;
        assert_eq!(rx.recv().await.unwrap(), FLV_HEADER.to_vec());
        let script = rx.recv().await.unwrap();
        assert_eq!(script[0], DATA_AMF0);
        let body = &script[11..script.len() - 4];
        assert_eq!(
            amf_decode(body).unwrap()[0],
            Amf::Str("onMetaData".into()),
            "@setDataFrame is taken off"
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            [
                vec![VIDEO, 0, 0, 5, 0, 0, 33, 0, 0, 0, 0],
                vec![0x17, 0, 0, 0, 0],
                16u32.to_be_bytes().to_vec()
            ]
            .concat()
        );
        drop(c);
        assert_eq!(session.await.unwrap(), SessionEnd::Closed);
    }

    #[tokio::test]
    async fn a_wrong_key_is_refused_before_anything_flows() {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (tx, mut rx) = mpsc::channel(4);
        let session = tokio::spawn(async move {
            let mut ingest = TestIngest {
                key: "sekret",
                tx: Some(tx),
            };
            run_session(server, &mut ingest, T).await.unwrap()
        });
        let mut c = Client::connect(client).await;
        c.publish("zgadywanie").await;
        c.read_until("NetStream.Publish.BadName").await;
        assert_eq!(session.await.unwrap(), SessionEnd::WrongKey);
        assert!(rx.try_recv().is_err(), "nothing reached the channel");
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_that_never_publishes_times_out() {
        let (client, server) = tokio::io::duplex(1 << 20);
        let session = tokio::spawn(async move {
            let mut ingest = TestIngest { key: "k", tx: None };
            run_session(server, &mut ingest, T).await
        });
        let _c = Client::connect(client).await;
        assert!(matches!(session.await.unwrap(), Err(RtmpError::Timeout)));
    }
}
