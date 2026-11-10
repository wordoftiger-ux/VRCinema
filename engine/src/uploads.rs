//! Small video and audio files viewers send from the request page (off by default): the
//! settings, the check of a received file, the folder `<data>/uploads` with its limits and
//! clean-up, and the resolver of the `upload:<name>` sources. The HTTP side is
//! `http/routes_uploads.rs`. A
//! sound-only file airs with a picture made from it.
//!
//! Every name in the folder is made here (`<32 hex>.<ext>`, `<32 hex>.part`); nothing a client
//! sends becomes a path.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

use crate::db::{ACTIVE_STATUSES, ItemStatus};
use crate::i18n::Msg;
use crate::resolver::local::FileDurationProbe;
use crate::resolver::{MediaMeta, MediaResolver, ResolveError, ResolvedStream, StreamInput};

/// The folder (in the data folder) of the received files.
pub const UPLOADS_DIR: &str = "uploads";
/// The scheme of a queue item that plays a received file.
pub const SCHEME: &str = "upload:";
/// The largest file the owner can allow (MB).
pub const HARD_MAX_MB: u32 = 200;
/// Uploads running at once (all viewers together); one per address.
pub const MAX_RUNNING: usize = 3;
/// What must stay free on the disk besides the file.
pub const FREE_RESERVE: u64 = 2 * 1024 * 1024 * 1024;
/// A file is kept this long after the last item using it finished (the bulk-removal undo of
/// the panel lasts a minute).
pub const FINISHED_GRACE_MS: i64 = 90_000;
/// A file no item names (the moment between its renaming and its queue row, or a crash) is
/// kept this long.
pub const ORPHAN_GRACE_MS: i64 = 10 * 60_000;
/// The same for making room: never a file that was written a moment ago.
const ROOM_ORPHAN_MS: i64 = 2 * 60_000;
/// How often the folder is cleaned.
pub const SWEEP_EVERY: Duration = Duration::from_secs(30);
/// How long ffprobe may take for one file.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest picture accepted (the channel converts every film).
const MAX_WIDTH: u64 = 7680;
const MAX_HEIGHT: u64 = 4320;
const PART_EXT: &str = "part";

// ---------- Settings ----------

/// The channel's upload settings (keys `viewerUploads`, `viewerUploadMaxMb`,
/// `viewerUploadMaxS`, `viewerUploadAudioMaxS`, `viewerUploadQuotaMb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSettings {
    pub viewer_uploads: bool,
    /// Largest file (MB), 1..=[`HARD_MAX_MB`].
    pub viewer_upload_max_mb: u32,
    /// Longest video (seconds).
    pub viewer_upload_max_s: u32,
    /// Longest audio file (seconds; it airs with a picture made from its sound).
    pub viewer_upload_audio_max_s: u32,
    /// All kept files together (MB).
    pub viewer_upload_quota_mb: u32,
}

impl Default for UploadSettings {
    fn default() -> Self {
        Self {
            viewer_uploads: false,
            viewer_upload_max_mb: 50,
            viewer_upload_max_s: 600,
            viewer_upload_audio_max_s: 600,
            viewer_upload_quota_mb: 2048,
        }
    }
}

const MAX_S_RANGE: (u32, u32) = (10, 7200);
const QUOTA_RANGE: (u32, u32) = (100, 100_000);

impl UploadSettings {
    pub fn max_bytes(&self) -> u64 {
        u64::from(self.viewer_upload_max_mb.clamp(1, HARD_MAX_MB)) << 20
    }

    pub fn quota_bytes(&self) -> u64 {
        u64::from(self.viewer_upload_quota_mb) << 20
    }

    /// The longest file of a kind (seconds): a video, or audio only.
    pub fn max_s(&self, audio: bool) -> u32 {
        if audio {
            self.viewer_upload_audio_max_s
        } else {
            self.viewer_upload_max_s
        }
    }

    /// Applies a stored value; `false`: not an upload key. A wrong value keeps the default.
    pub fn read_stored(&mut self, key: &str, value: &Value) -> bool {
        let mut patch = UploadPatch::default();
        match sanitize_key(key, value, &mut patch) {
            None => false,
            Some(Err(_)) => true,
            Some(Ok(())) => {
                self.apply(&patch);
                true
            }
        }
    }

    pub fn apply(&mut self, p: &UploadPatch) {
        if let Some(v) = p.enabled {
            self.viewer_uploads = v;
        }
        if let Some(v) = p.max_mb {
            self.viewer_upload_max_mb = v;
        }
        if let Some(v) = p.max_s {
            self.viewer_upload_max_s = v;
        }
        if let Some(v) = p.audio_max_s {
            self.viewer_upload_audio_max_s = v;
        }
        if let Some(v) = p.quota_mb {
            self.viewer_upload_quota_mb = v;
        }
    }
}

/// A validated partial update of [`UploadSettings`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UploadPatch {
    pub enabled: Option<bool>,
    pub max_mb: Option<u32>,
    pub max_s: Option<u32>,
    pub audio_max_s: Option<u32>,
    pub quota_mb: Option<u32>,
}

/// A whole number within `range` (a fraction truncated, out of range clamped).
fn number_in(key: &str, value: &Value, (lo, hi): (u32, u32)) -> Result<u32, Msg> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| (n as u32).clamp(lo, hi))
        .ok_or_else(|| Msg::error("setting_not_number").arg("key", key))
}

/// Validates one settings key; `None`: not an upload key.
pub fn sanitize_key(key: &str, value: &Value, out: &mut UploadPatch) -> Option<Result<(), Msg>> {
    let result = match key {
        "viewerUploads" => value
            .as_bool()
            .map(|b| out.enabled = Some(b))
            .ok_or_else(|| Msg::error("setting_not_boolean").arg("key", key)),
        "viewerUploadMaxMb" => {
            number_in(key, value, (1, HARD_MAX_MB)).map(|n| out.max_mb = Some(n))
        }
        "viewerUploadMaxS" => number_in(key, value, MAX_S_RANGE).map(|n| out.max_s = Some(n)),
        "viewerUploadAudioMaxS" => {
            number_in(key, value, MAX_S_RANGE).map(|n| out.audio_max_s = Some(n))
        }
        "viewerUploadQuotaMb" => number_in(key, value, QUOTA_RANGE).map(|n| out.quota_mb = Some(n)),
        _ => return None,
    };
    Some(result)
}

impl UploadPatch {
    /// The stored JSON texts of the keys present.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(v) = self.enabled {
            out.push(("viewerUploads", v.to_string()));
        }
        if let Some(v) = self.max_mb {
            out.push(("viewerUploadMaxMb", v.to_string()));
        }
        if let Some(v) = self.max_s {
            out.push(("viewerUploadMaxS", v.to_string()));
        }
        if let Some(v) = self.audio_max_s {
            out.push(("viewerUploadAudioMaxS", v.to_string()));
        }
        if let Some(v) = self.quota_mb {
            out.push(("viewerUploadQuotaMb", v.to_string()));
        }
        out
    }
}

// ---------- Names and formats ----------

/// A container the channel accepts: the demuxer ffprobe is forced to and the extensions the
/// file is stored with (as a video, and with sound only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub demuxer: &'static str,
    pub ext: &'static str,
    pub audio_ext: &'static str,
}

const fn format(demuxer: &'static str, ext: &'static str, audio_ext: &'static str) -> Format {
    Format {
        demuxer,
        ext,
        audio_ext,
    }
}

const MP4: Format = format("mov", "mp4", "m4a");
const MKV: Format = format("matroska", "mkv", "mka");
const AVI: Format = format("avi", "avi", "avi");
const FLV: Format = format("flv", "flv", "flv");
const TS: Format = format("mpegts", "ts", "ts");
const MPG: Format = format("mpeg", "mpg", "mpg");
const OGG: Format = format("ogg", "ogv", "ogg");
const WMV: Format = format("asf", "wmv", "wma");
// Sound-only containers (an attached cover picture is all the picture they can carry).
const MP3: Format = format("mp3", "mp3", "mp3");
const AAC: Format = format("aac", "aac", "aac");
const FLAC: Format = format("flac", "flac", "flac");
const WAV: Format = format("wav", "wav", "wav");
const FORMATS: [Format; 12] = [MP4, MKV, AVI, FLV, TS, MPG, OGG, WMV, MP3, AAC, FLAC, WAV];

impl Format {
    /// The extension of the stored file.
    pub fn ext_for(&self, audio: bool) -> &'static str {
        if audio { self.audio_ext } else { self.ext }
    }
}

/// How many first bytes [`sniff`] needs to decide (MPEG-TS: three packets).
pub const SNIFF_BYTES: usize = 3 * 188 + 1;

/// An MPEG audio frame header (MP3 and its relatives) at the start: the 11-bit sync, a known
/// version and layer, a bit rate other than "bad" and a known sample rate.
fn mpeg_audio_frame(head: &[u8]) -> bool {
    let [a, b, c, ..] = *head else {
        return false;
    };
    a == 0xff
        && b & 0xe0 == 0xe0
        && (b >> 3) & 0b11 != 0b01
        && (b >> 1) & 0b11 != 0
        && c >> 4 != 0xf
        && (c >> 2) & 0b11 != 0b11
}

/// An ADTS header (raw AAC) at the start: the 12-bit sync and layer 0.
fn adts_frame(head: &[u8]) -> bool {
    matches!(*head, [0xff, b, ..] if b & 0xf6 == 0xf0)
}

/// What the first bytes say the file is. Only binary video and audio containers are accepted:
/// never a playlist, a text format or a picture (they could make ffmpeg open other files or
/// addresses, or are no film).
pub fn sniff(head: &[u8]) -> Option<Format> {
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    if at(4, b"ftyp") {
        Some(MP4)
    } else if at(0, b"\x1a\x45\xdf\xa3") {
        Some(MKV)
    } else if at(0, b"RIFF") && at(8, b"AVI ") {
        Some(AVI)
    } else if at(0, b"RIFF") && at(8, b"WAVE") {
        Some(WAV)
    } else if at(0, b"FLV\x01") {
        Some(FLV)
    } else if at(0, b"\x47") && at(188, b"\x47") && at(376, b"\x47") {
        Some(TS)
    } else if at(0, b"\x00\x00\x01\xba") {
        Some(MPG)
    } else if at(0, b"OggS") {
        Some(OGG)
    } else if at(0, b"\x30\x26\xb2\x75\x8e\x66\xcf\x11") {
        Some(WMV)
    } else if at(0, b"fLaC") {
        Some(FLAC)
    } else if at(0, b"ID3") || mpeg_audio_frame(head) {
        Some(MP3)
    } else if adts_frame(head) {
        Some(AAC)
    } else {
        None
    }
}

/// An `upload:` source (whatever follows).
pub fn is_upload_source(url: &str) -> bool {
    url.starts_with(SCHEME)
}

/// A name this module made: 32 lower-case hex digits and the extension of an accepted format.
fn valid_name(name: &str) -> bool {
    let Some((id, ext)) = name.split_once('.') else {
        return false;
    };
    id.len() == 32
        && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && FORMATS.iter().any(|f| f.ext == ext || f.audio_ext == ext)
}

/// The file name inside the uploads folder of an `upload:` source, only when it is exactly
/// one of ours (no separators, no dots but the extension's).
pub fn source_file_name(url: &str) -> Option<&str> {
    let name = url.strip_prefix(SCHEME)?;
    valid_name(name).then_some(name)
}

fn random_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system random number generator");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The title of a received file: the name the browser gave (only its last part, without the
/// extension), control and invisible characters removed, at most 100 characters; `None` when
/// nothing is left.
pub fn title_from_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let stem = match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 => stem,
        _ => base,
    };
    let clean: String = stem
        .chars()
        .filter(|c| !c.is_control() && !crate::nicks::is_invisible(*c))
        .take(100)
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

// ---------- The check of a file ----------

/// What ffprobe found in an accepted file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probed {
    pub duration_s: f64,
    /// The picture's size (sound only: 0 × 0).
    pub width: u64,
    pub height: u64,
    /// Sound only (cover art at most): it airs with a picture made from its sound.
    pub audio: bool,
}

/// A positive finite length from ffprobe's text (or number).
fn seconds(v: &Value) -> Option<f64> {
    v.as_str()
        .and_then(|d| d.trim().parse::<f64>().ok())
        .or_else(|| v.as_f64())
        .filter(|d| d.is_finite() && *d > 0.0)
}

/// Reads ffprobe's JSON. A video: its picture stream (the first video stream that is not an
/// attached picture, chosen as playback chooses it) of a known, sane size. Sound only: an
/// audio stream; attached pictures count as cover art only, and each must have a known, sane
/// size too (the channel draws the cover). Either needs a known length (sound only: the
/// file's, else the audio stream's, as playback reads it). Errors are for the viewer.
pub fn read_probe(info: &Value) -> Result<Probed, Msg> {
    use crate::pipeline::probe::{is_cover_stream, main_video_stream};
    let not_video = || Msg::error("upload_not_video");
    let streams = info["streams"].as_array().ok_or_else(not_video)?;
    let sane = |s: &Value| {
        let (w, h) = (
            s["width"].as_u64().unwrap_or(0),
            s["height"].as_u64().unwrap_or(0),
        );
        (w > 0 && h > 0 && w <= MAX_WIDTH && h <= MAX_HEIGHT).then_some((w, h))
    };
    let format_length = seconds(&info["format"]["duration"]);
    if let Some(video) = main_video_stream(streams) {
        let (width, height) = sane(video).ok_or_else(not_video)?;
        return Ok(Probed {
            duration_s: format_length.ok_or_else(not_video)?,
            width,
            height,
            audio: false,
        });
    }
    let sound = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .ok_or_else(not_video)?;
    let covers_sane = streams
        .iter()
        .filter(|s| s["codec_type"] == "video" && is_cover_stream(s))
        .all(|s| sane(s).is_some());
    if !covers_sane {
        return Err(not_video());
    }
    Ok(Probed {
        duration_s: format_length
            .or_else(|| seconds(&sound["duration"]))
            .ok_or_else(not_video)?,
        width: 0,
        height: 0,
        audio: true,
    })
}

/// Runs ffprobe on `file` with the demuxer of `format` forced and only local files allowed.
pub async fn probe_file(ffprobe: &Path, file: &Path, format: Format) -> Result<Probed, Msg> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-f",
        format.demuxer,
        "-show_entries",
        "format=duration:stream=index,codec_type,width,height,duration:stream_disposition=attached_pic",
        "-of",
        "json",
        "-i",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(file.as_os_str().to_owned());
    let out = crate::pipeline::slate_look::run_tool(ffprobe, args, None, PROBE_TIMEOUT)
        .await
        .map_err(|_| Msg::error("upload_not_video"))?;
    let info: Value = serde_json::from_slice(&out).map_err(|_| Msg::error("upload_not_video"))?;
    read_probe(&info)
}

// ---------- The folder ----------

/// A queue row that plays a received file: its source, status and when it finished.
pub type UploadRow = (String, ItemStatus, Option<i64>);

/// Free bytes on the disk holding a folder (`None`: unknown).
pub type FreeSpaceFn = Box<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// Why an upload cannot start now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// This address is sending a file already, or [`MAX_RUNNING`] run.
    Busy,
    /// Too little room on the disk or in the quota.
    NoRoom,
}

#[derive(Default)]
struct Running {
    addresses: HashSet<String>,
    /// Part files being written (their names).
    parts: HashSet<String>,
    /// Bytes promised to the running uploads.
    reserved: u64,
}

/// The uploads folder: the running uploads, the room they may take, the clean-up.
pub struct UploadStore {
    dir: PathBuf,
    ffprobe: PathBuf,
    free_space: FreeSpaceFn,
    running: Mutex<Running>,
}

struct FileInfo {
    name: String,
    size: u64,
    /// Unix ms of the last write.
    modified: i64,
}

fn unix_ms(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// What becomes of one file: kept for now, or deletable (by the sweep / to make room).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// An item using it waits, is approved or on air.
    Active,
    /// Deletable to make room; by the sweep only once `sweep_at` (Unix ms) has passed.
    Done { sweep_at: i64, room: bool },
}

fn fate(rows: Option<&Vec<(ItemStatus, Option<i64>)>>, modified: i64) -> Fate {
    match rows {
        Some(rows) if rows.iter().any(|(s, _)| ACTIVE_STATUSES.contains(s)) => Fate::Active,
        Some(rows) => {
            let last = rows
                .iter()
                .filter_map(|(_, f)| *f)
                .max()
                .unwrap_or(modified);
            Fate::Done {
                sweep_at: last.saturating_add(FINISHED_GRACE_MS),
                room: true,
            }
        }
        None => Fate::Done {
            sweep_at: modified.saturating_add(ORPHAN_GRACE_MS),
            room: false,
        },
    }
}

impl UploadStore {
    /// The store of `dir` (made absolute, created); part files of an earlier run are removed.
    pub fn new(dir: &Path, ffprobe: PathBuf) -> std::io::Result<Self> {
        Self::with_free_space(dir, ffprobe, Box::new(crate::preview_source::free_space))
    }

    /// Tests: the free disk space from `free_space`.
    pub fn with_free_space(
        dir: &Path,
        ffprobe: PathBuf,
        free_space: FreeSpaceFn,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let dir = std::path::absolute(dir)?;
        let store = Self {
            dir,
            ffprobe,
            free_space,
            running: Mutex::new(Running::default()),
        };
        for path in store.entries().into_iter().map(|(path, _)| path) {
            if path.extension().is_some_and(|e| e == PART_EXT) {
                let _ = std::fs::remove_file(path);
            }
        }
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn ffprobe(&self) -> &Path {
        &self.ffprobe
    }

    fn running(&self) -> std::sync::MutexGuard<'_, Running> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The path of a source's file, when the source is one of ours and the file is there.
    pub fn file_of(&self, url: &str) -> Option<PathBuf> {
        let path = self.dir.join(source_file_name(url)?);
        path.is_file().then_some(path)
    }

    fn entries(&self) -> Vec<(PathBuf, std::fs::Metadata)> {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        read.flatten()
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                meta.is_file().then(|| (e.path(), meta))
            })
            .collect()
    }

    /// The finished files (ours only; part files and anything else are left out).
    fn files(&self) -> Vec<FileInfo> {
        self.entries()
            .into_iter()
            .filter_map(|(path, meta)| {
                let name = path.file_name()?.to_str()?.to_string();
                valid_name(&name).then(|| FileInfo {
                    name,
                    size: meta.len(),
                    modified: meta.modified().map_or(0, unix_ms),
                })
            })
            .collect()
    }

    fn by_name(rows: &[UploadRow]) -> HashMap<&str, Vec<(ItemStatus, Option<i64>)>> {
        let mut map: HashMap<&str, Vec<(ItemStatus, Option<i64>)>> = HashMap::new();
        for (url, status, finished) in rows {
            if let Some(name) = source_file_name(url) {
                map.entry(name).or_default().push((*status, *finished));
            }
        }
        map
    }

    /// Starts an upload from `address` that may take `reserve` bytes: one per address and
    /// [`MAX_RUNNING`] at once; the disk keeps [`FREE_RESERVE`] free; the quota is made room in
    /// by deleting files no item needs any more, oldest first. `rows`: the queue's
    /// [`UploadRow`]s, `now`: Unix ms.
    pub fn begin(
        self: &Arc<Self>,
        address: &str,
        reserve: u64,
        quota: u64,
        rows: &[UploadRow],
        now: i64,
    ) -> Result<Slot, Refusal> {
        let mut running = self.running();
        if running.addresses.contains(address) || running.addresses.len() >= MAX_RUNNING {
            return Err(Refusal::Busy);
        }
        if (self.free_space)(&self.dir).is_none_or(|free| free < reserve + FREE_RESERVE) {
            return Err(Refusal::NoRoom);
        }
        let names = Self::by_name(rows);
        let mut files = self.files();
        let mut used: u64 = files.iter().map(|f| f.size).sum::<u64>() + running.reserved;
        if used + reserve > quota {
            files.sort_by_key(|f| f.modified);
            for f in &files {
                if used + reserve <= quota {
                    break;
                }
                let removable = match fate(names.get(f.name.as_str()), f.modified) {
                    Fate::Active => false,
                    Fate::Done { room, .. } => room || now - f.modified >= ROOM_ORPHAN_MS,
                };
                if removable && std::fs::remove_file(self.dir.join(&f.name)).is_ok() {
                    used = used.saturating_sub(f.size);
                }
            }
            if used + reserve > quota {
                return Err(Refusal::NoRoom);
            }
        }
        let id = random_id();
        let part_name = format!("{id}.{PART_EXT}");
        running.addresses.insert(address.to_string());
        running.parts.insert(part_name.clone());
        running.reserved += reserve;
        Ok(Slot {
            store: Arc::clone(self),
            address: address.to_string(),
            reserve,
            part: self.dir.join(&part_name),
            part_name,
            id,
        })
    }

    /// Deletes the files no item needs any more (see [`fate`]) and part files nobody writes.
    /// Returns how many files went.
    pub fn sweep(&self, rows: &[UploadRow], now: i64) -> usize {
        let names = Self::by_name(rows);
        let parts = self.running().parts.clone();
        let mut removed = 0;
        for (path, meta) in self.entries() {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let gone = if path.extension().is_some_and(|e| e == PART_EXT) {
                !parts.contains(name)
            } else if valid_name(name) {
                let modified = meta.modified().map_or(0, unix_ms);
                matches!(fate(names.get(name), modified), Fate::Done { sweep_at, .. } if now >= sweep_at)
            } else {
                false
            };
            if gone && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// A running upload: its part file and its share of the limits, given back (and the part file
/// deleted) when it is dropped — also when the connection breaks off mid-way.
pub struct Slot {
    store: Arc<UploadStore>,
    address: String,
    reserve: u64,
    part: PathBuf,
    part_name: String,
    id: String,
}

impl Slot {
    /// Where the bytes go.
    pub fn part(&self) -> &Path {
        &self.part
    }

    /// Moves the checked part file to its final name and returns the item's source
    /// (`upload:<id>.<ext>`; `audio`: the extension of a sound-only file of the format).
    pub fn finish(&self, format: Format, audio: bool) -> std::io::Result<String> {
        let name = format!("{}.{}", self.id, format.ext_for(audio));
        std::fs::rename(&self.part, self.store.dir.join(&name))?;
        Ok(format!("{SCHEME}{name}"))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        // After `finish` there is no part file any more.
        let _ = std::fs::remove_file(&self.part);
        let mut running = self.store.running();
        running.addresses.remove(&self.address);
        running.parts.remove(&self.part_name);
        running.reserved = running.reserved.saturating_sub(self.reserve);
    }
}

/// Cleans the folder every [`SWEEP_EVERY`] for as long as it runs.
pub async fn run_sweeper(store: Arc<UploadStore>, db: Arc<Mutex<rusqlite::Connection>>) {
    let mut interval = tokio::time::interval(SWEEP_EVERY);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let rows = {
            let conn = db.lock().unwrap_or_else(PoisonError::into_inner);
            crate::db::QueueRepo::new(&conn).upload_rows()
        };
        if let Ok(rows) = rows {
            let store = Arc::clone(&store);
            let _ =
                tokio::task::spawn_blocking(move || store.sweep(&rows, crate::db::now_ms())).await;
        }
    }
}

// ---------- The resolver ----------

/// Plays `upload:<name>` sources: only files of the uploads folder with names made here.
pub struct UploadResolver {
    dir: PathBuf,
    probe_duration: FileDurationProbe,
}

impl UploadResolver {
    pub fn new(dir: PathBuf, probe_duration: FileDurationProbe) -> Self {
        Self {
            dir: std::path::absolute(&dir).unwrap_or(dir),
            probe_duration,
        }
    }

    fn file(&self, url: &str) -> Result<PathBuf, ResolveError> {
        let path = self
            .dir
            .join(source_file_name(url).ok_or_else(|| ResolveError::coded("upload_gone"))?);
        if path.is_file() {
            Ok(path)
        } else {
            Err(ResolveError::coded("upload_gone"))
        }
    }
}

#[async_trait]
impl MediaResolver for UploadResolver {
    async fn fetch_meta(&self, url: &str) -> Result<MediaMeta, ResolveError> {
        let file = self.file(url)?;
        let title = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(MediaMeta {
            title,
            duration_s: (self.probe_duration)(file).await,
            is_live: false,
            is_playlist: false,
            entries: None,
        })
    }

    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        let file = self.file(url)?;
        Ok(ResolvedStream {
            title: String::new(),
            inputs: vec![StreamInput {
                url: file.to_string_lossy().into_owned(),
                headers: Vec::new(),
            }],
            duration_s: None,
            is_live: false,
            thumbnail: None,
            artist: None,
        })
    }
}

#[cfg(test)]
mod tests;
