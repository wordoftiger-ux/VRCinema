//! Video files from viewers: `POST /api/channels/main/upload` (off by default), the limits
//! counted on the bytes received, the checks before anything is read, and the clean-up after
//! a bulk removal.

use super::*;
use crate::uploads::{UPLOADS_DIR, UploadPatch, UploadStore};

const UPLOAD: &str = "/api/channels/main/upload";

fn bin(name: &str) -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
    [format!("{name}.exe"), name.to_string()]
        .iter()
        .map(|n| root.join("bin").join(n))
        .find(|p| p.exists())
}

struct Uploads {
    h: Harness,
    store: Arc<UploadStore>,
    _dir: tempfile::TempDir,
}

fn harness() -> Uploads {
    let dir = tempfile::tempdir().unwrap();
    let ffprobe = bin("ffprobe").unwrap_or_else(|| PathBuf::from("ffprobe-missing"));
    let store = Arc::new(
        UploadStore::with_free_space(
            &dir.path().join(UPLOADS_DIR),
            ffprobe,
            Box::new(|_| Some(u64::MAX)),
        )
        .unwrap(),
    );
    let h = Harness::with_state(config(), |s| s.with_uploads(Some(Arc::clone(&store))));
    Uploads {
        h,
        store,
        _dir: dir,
    }
}

impl Uploads {
    fn allow(&self, patch: UploadPatch) {
        self.h.update_settings(SettingsPatch {
            uploads: UploadPatch {
                enabled: Some(true),
                ..patch
            },
            ..Default::default()
        });
    }

    fn files(&self) -> Vec<String> {
        std::fs::read_dir(self.store.dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }
}

fn upload(body: &[u8]) -> Call {
    let mut c = call("POST", UPLOAD).ip("1.1.1.1");
    c.body = Some(body.to_vec());
    c.header("content-type", "application/octet-stream")
}

/// The first bytes of an MP4 file (enough for the sniffing), then `size` bytes in all.
fn mp4_like(size: usize) -> Vec<u8> {
    let mut b = b"\0\0\0\x20ftypisom".to_vec();
    b.resize(size, 0);
    b
}

#[tokio::test]
async fn uploads_are_off_by_default_and_the_page_learns_the_limits() {
    let u = harness();
    let res = u.h.send(upload(&mp4_like(1000))).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(res.json()["code"], "uploads_off");
    let state = u.h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(state["rules"]["uploads"], Value::Null);
    u.allow(UploadPatch {
        max_mb: Some(20),
        ..Default::default()
    });
    let state = u.h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(
        state["rules"]["uploads"],
        json!({"maxMb": 20, "maxDurationS": 600, "maxAudioS": 600})
    );
    u.allow(UploadPatch {
        audio_max_s: Some(1800),
        ..Default::default()
    });
    let state = u.h.send(get("/api/channels/main/state")).await.json();
    assert_eq!(state["rules"]["uploads"]["maxAudioS"], 1800);
    assert!(u.files().is_empty());
}

#[tokio::test]
async fn the_size_is_counted_on_the_bytes_that_come_not_the_header() {
    let u = harness();
    u.allow(UploadPatch {
        max_mb: Some(1),
        ..Default::default()
    });
    let too_big = mp4_like((1 << 20) + 1);
    // Declared too big: refused before a byte is read.
    let res =
        u.h.send(upload(&too_big).header("content-length", &too_big.len().to_string()))
            .await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(res.json()["code"], "upload_too_big");
    // No length, or a false one: counted while it comes.
    for c in [
        upload(&too_big),
        upload(&too_big).header("content-length", "10"),
    ] {
        let res = u.h.send(c).await;
        assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", res.text());
        assert!(res.error().contains("1 MB"), "{}", res.error());
    }
    assert!(
        u.files().is_empty(),
        "no part file is left: {:?}",
        u.files()
    );
}

#[tokio::test]
async fn what_is_not_a_video_container_is_refused_and_leaves_nothing() {
    let u = harness();
    u.allow(UploadPatch::default());
    let mut playlist = b"#EXTM3U\n#EXTINF:1,\nfile:///C:/Windows/win.ini\n".to_vec();
    playlist.resize(4000, b'\n');
    for body in [playlist, b"tiny".to_vec(), Vec::new()] {
        let res = u.h.send(upload(&body)).await;
        assert_eq!(
            res.status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "{}",
            res.text()
        );
        assert_eq!(res.json()["code"], "upload_not_video");
        assert!(!res.error().contains(&*u.store.dir().to_string_lossy()));
    }
    assert!(u.files().is_empty());
    assert!(u.h.list(ItemStatus::Queued).is_empty());
}

#[tokio::test]
async fn the_viewer_limits_apply_before_anything_is_read() {
    let u = harness();
    u.allow(UploadPatch::default());
    u.h.update_settings(SettingsPatch {
        max_active_per_requester: Some(1),
        ..Default::default()
    });
    u.h.with_db(|c| {
        QueueRepo::new(c)
            .add(&NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url: "https://youtu.be/a".into(),
                title: "A".into(),
                duration_s: Some(1.0),
                is_live: false,
                status: ItemStatus::Queued,
                added_by: AddedBy::Public,
                requester_name: None,
                requester_ip: Some("1.1.1.1".into()),
                requester_key_hash: None,
            })
            .unwrap()
    });
    let res = u.h.send(upload(&mp4_like(1000))).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.json()["code"], "too_many_active");
    // A nick without a device key is refused like on a link.
    let res =
        u.h.send(
            upload(&mp4_like(1000))
                .ip("2.2.2.2")
                .header("x-viewer-name", "Ola"),
        )
        .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.json()["code"], "invalid_device_key");
    assert!(u.files().is_empty());
}

fn ffmpeg(args: &[&str], out: &Path) -> bool {
    let Some(ffmpeg) = bin("ffmpeg") else {
        return false;
    };
    std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .unwrap()
        .success()
}

fn make_mp4(out: &Path) -> bool {
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=160x120:d=1",
            "-c:v",
            "libx264",
            "-f",
            "mp4",
        ],
        out,
    )
}

/// An MP3 of `seconds` with ID3 tags and a cover picture (an attached PNG).
fn make_mp3(out: &Path, seconds: u32) -> bool {
    let cover = out.with_extension("png");
    ffmpeg(
        &["-f", "lavfi", "-i", "color=c=red:s=64x64", "-frames:v", "1"],
        &cover,
    ) && ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("sine=f=440:d={seconds}"),
            "-i",
            &cover.to_string_lossy(),
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:a",
            "libmp3lame",
            "-c:v",
            "png",
            "-disposition:v",
            "attached_pic",
            "-id3v2_version",
            "3",
            "-metadata",
            "title=Tune",
            "-metadata",
            "artist=Band",
            "-f",
            "mp3",
        ],
        out,
    )
}

/// Real ffmpeg: an MP3 with cover art is queued as sound only (`.mp3`, its own length limit)
/// and airs through the audio picture (playback's probe sees sound only with that cover); the
/// cover never counts as the picture, so the audio limit applies, not the video one.
#[tokio::test]
async fn a_real_mp3_with_cover_art_is_queued_as_audio_with_its_own_limit() {
    let u = harness();
    let tmp = tempfile::tempdir().unwrap();
    let song = tmp.path().join("song.mp3");
    if bin("ffprobe").is_none() || !make_mp3(&song, 12) {
        eprintln!("no ffmpeg/ffprobe in bin/: skipped");
        return;
    }
    let bytes = std::fs::read(&song).unwrap();
    // Audio limited to 10 s (the video limit is longer): refused as too long, nothing kept.
    u.allow(UploadPatch {
        max_s: Some(600),
        audio_max_s: Some(10),
        ..Default::default()
    });
    let res = u.h.send(upload(&bytes)).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.text());
    assert_eq!(res.json()["code"], "too_long");
    assert!(u.files().is_empty(), "{:?}", u.files());
    // Audio may be longer than videos: accepted.
    u.allow(UploadPatch {
        max_s: Some(10),
        audio_max_s: Some(60),
        ..Default::default()
    });
    let res =
        u.h.send(upload(&bytes).header("x-upload-name", "C%3A%5Cmusic%5Cmy%20song.mp3"))
            .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    let item = &u.h.list(ItemStatus::Queued)[0];
    assert_eq!(item.title, "my song");
    assert!(
        item.source_url.starts_with("upload:") && item.source_url.ends_with(".mp3"),
        "{}",
        item.source_url
    );
    assert!((item.duration_s.unwrap() - 12.0).abs() < 0.5);
    let path = u.store.file_of(&item.source_url).expect("the file is kept");
    assert!(!res.text().contains(&*path.to_string_lossy()));
    let input = crate::resolver::StreamInput {
        url: path.to_string_lossy().into_owned(),
        headers: Vec::new(),
    };
    let played = crate::pipeline::probe::probe_video(&bin("ffprobe").unwrap(), &[input])
        .await
        .unwrap();
    let tags = played.audio.expect("plays as sound only");
    assert_eq!(
        (tags.title.as_str(), tags.artist.as_str()),
        ("Tune", "Band")
    );
    assert!(tags.cover_stream.is_some(), "with its cover");
}

/// Real ffprobe: a small MP4 is queued with the viewer's nick, waits for approval, and its file
/// goes some time after the owner cleared the queue; it cannot be played again then.
#[tokio::test]
async fn a_real_video_is_queued_with_the_nick_and_its_file_goes_after_a_clear() {
    let u = harness();
    let tmp = tempfile::tempdir().unwrap();
    let film = tmp.path().join("film.mp4");
    if bin("ffprobe").is_none() || !make_mp4(&film) {
        eprintln!("no ffmpeg/ffprobe in bin/: skipped");
        return;
    }
    u.allow(UploadPatch::default());
    u.h.update_settings(SettingsPatch {
        moderation: Some(true),
        ..Default::default()
    });
    let bytes = std::fs::read(&film).unwrap();
    let res =
        u.h.send(
            upload(&bytes)
                .header("x-upload-name", "..%2F..%2FM%C3%B3j%20film.mp4")
                .header("x-viewer-name", "Ola")
                .header("x-device-key", KEY_A),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    assert_eq!(res.json()["status"], "pending_approval");
    let item = &u.h.list(ItemStatus::PendingApproval)[0];
    assert_eq!(item.title, "Mój film");
    assert_eq!(item.requester_name.as_deref(), Some("Ola"));
    assert_eq!(item.added_by, AddedBy::Public);
    assert!((item.duration_s.unwrap() - 1.0).abs() < 0.2);
    let path = u.store.file_of(&item.source_url).expect("the file is kept");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(u.h.logs().iter().any(|l| l.contains("Mój film")));

    let cookie = u.h.login().await;
    let res =
        u.h.send(
            call("POST", "/api/admin/channels/main/clear")
                .cookie(&cookie)
                .json(json!({})),
        )
        .await;
    assert_eq!(res.json()["removed"], 1);
    let rows = u.h.with_db(|c| QueueRepo::new(c).upload_rows().unwrap());
    let now = crate::db::now_ms();
    assert_eq!(
        u.store.sweep(&rows, now),
        0,
        "kept while the undo may bring it back"
    );
    assert_eq!(
        u.store
            .sweep(&rows, now + crate::uploads::FINISHED_GRACE_MS),
        1
    );
    assert!(!path.exists());
    let res =
        u.h.send(
            call("POST", "/api/admin/channels/main/play-next")
                .cookie(&cookie)
                .json(json!({"itemId": item.id})),
        )
        .await;
    assert_eq!(res.status, StatusCode::CONFLICT);
    assert_eq!(res.json()["code"], "upload_gone");
}
