use super::*;
use serde_json::json;

const ID: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn settings_are_validated_clamped_and_stored() {
    let mut patch = UploadPatch::default();
    for (key, value) in [
        ("viewerUploads", json!(true)),
        ("viewerUploadMaxMb", json!(999)),
        ("viewerUploadMaxS", json!(1)),
        ("viewerUploadAudioMaxS", json!(99999)),
        ("viewerUploadQuotaMb", json!(5000.7)),
    ] {
        assert!(sanitize_key(key, &value, &mut patch).is_some_and(|r| r.is_ok()));
    }
    assert!(sanitize_key("viewerUploads", &json!(1), &mut patch).is_some_and(|r| r.is_err()));
    assert!(sanitize_key("viewerUploadMaxMb", &json!(-1), &mut patch).is_some_and(|r| r.is_err()));
    assert!(sanitize_key("uploadMbps", &json!(1), &mut patch).is_none());
    assert_eq!(
        patch.entries(),
        vec![
            ("viewerUploads", "true".to_string()),
            ("viewerUploadMaxMb", "200".to_string()),
            ("viewerUploadMaxS", "10".to_string()),
            ("viewerUploadAudioMaxS", "7200".to_string()),
            ("viewerUploadQuotaMb", "5000".to_string()),
        ]
    );
    let mut s = UploadSettings::default();
    assert!(!s.viewer_uploads, "off by default");
    assert_eq!(s.max_bytes(), 50 << 20);
    assert_eq!(
        (s.max_s(false), s.max_s(true)),
        (600, 600),
        "audio as long as video"
    );
    s.apply(&patch);
    assert_eq!(s.max_bytes(), 200 << 20);
    assert_eq!((s.max_s(false), s.max_s(true)), (10, 7200));
    assert!(
        s.read_stored("viewerUploadMaxMb", &json!("x")),
        "an upload key"
    );
    assert_eq!(s.viewer_upload_max_mb, 200, "a wrong stored value keeps it");
}

#[test]
fn only_binary_video_and_audio_containers_are_sniffed() {
    let mut ts = vec![0u8; SNIFF_BYTES];
    for at in [0, 188, 376] {
        ts[at] = 0x47;
    }
    let cases: [(&[u8], Option<&str>); 24] = [
        (b"\0\0\0\x20ftypisom", Some("mp4")),
        (b"\0\0\0\x20ftypM4A ", Some("mp4")),
        (b"\x1a\x45\xdf\xa3\x01", Some("mkv")),
        (b"RIFF\0\0\0\0AVI LIST", Some("avi")),
        (b"FLV\x01\x05", Some("flv")),
        (&ts, Some("ts")),
        (b"\0\0\x01\xba\x44", Some("mpg")),
        (b"OggS\0\x02", Some("ogv")),
        (b"\x30\x26\xb2\x75\x8e\x66\xcf\x11\xa6", Some("wmv")),
        (b"ID3\x04\0\0\0\0\x10\0", Some("mp3")),
        // MPEG-1 layer III, 128 kbit/s, 44.1 kHz; MPEG-2 layer III.
        (b"\xff\xfb\x90\x64", Some("mp3")),
        (b"\xff\xf3\x48\xc4", Some("mp3")),
        (b"\xff\xf1\x50\x80", Some("aac")),
        (b"\xff\xf9\x50\x80", Some("aac")),
        (b"fLaC\0\0\0\x22", Some("flac")),
        (b"RIFF\0\0\0\0WAVEfmt ", Some("wav")),
        // Not a frame: a reserved version, a "bad" bit rate, a reserved sample rate.
        (b"\xff\xeb\x90\x64", None),
        (b"\xff\xfb\xf0\x64", None),
        (b"\xff\xfb\x9c\x64", None),
        (b"#EXTM3U\n#EXTINF:1,\nfile:///etc/passwd", None),
        (b"ffconcat version 1.0\nfile a.mp4", None),
        (b"\x89PNG\r\n\x1a\n", None),
        (b"\xff\xd8\xff\xe0\0\x10JFIF", None),
        (b"", None),
    ];
    for (head, ext) in cases {
        assert_eq!(
            sniff(head).map(|f| f.ext),
            ext,
            "{:?}",
            &head[..head.len().min(12)]
        );
    }
}

#[test]
fn only_names_made_here_map_into_the_folder() {
    let ok = format!("upload:{ID}.mp4");
    assert_eq!(source_file_name(&ok), Some(&ok[7..]));
    for ext in ["m4a", "mka", "ogg", "wma", "mp3", "aac", "flac", "wav"] {
        assert!(
            source_file_name(&format!("upload:{ID}.{ext}")).is_some(),
            "{ext}"
        );
    }
    for bad in [
        format!("upload:../{ID}.mp4"),
        format!("upload:{ID}.mp4/../../x"),
        format!("upload:..\\{ID}.mp4"),
        format!("upload:{}.mp4", ID.to_uppercase()),
        format!("upload:{ID}.exe"),
        format!("upload:{ID}.part"),
        format!("upload:{ID}.mp4.mp4"),
        format!("upload:{}.mp4", &ID[1..]),
        format!("upload:/{ID}.mp4"),
        format!("local:{ID}.mp4"),
        "upload:".to_string(),
    ] {
        assert_eq!(source_file_name(&bad), None, "{bad}");
    }
    assert!(is_upload_source(&ok));
    assert!(!is_upload_source("https://x/upload:a"));
}

#[test]
fn the_title_is_the_last_part_of_the_name_without_extension() {
    assert_eq!(title_from_name("Mój film.mp4").as_deref(), Some("Mój film"));
    assert_eq!(
        title_from_name("C:\\Users\\me\\Desktop\\clip.final.webm").as_deref(),
        Some("clip.final")
    );
    assert_eq!(
        title_from_name("../../etc/a\u{0}b\u{202e}.mkv").as_deref(),
        Some("ab")
    );
    assert_eq!(title_from_name(".mp4").as_deref(), Some(".mp4"));
    assert_eq!(title_from_name("  \u{200b} ").as_deref(), None);
    assert_eq!(
        title_from_name(&"x".repeat(300)).map(|t| t.len()),
        Some(100)
    );
}

fn video(w: u64, h: u64) -> Value {
    json!({"codec_type": "video", "width": w, "height": h, "disposition": {"attached_pic": 0}})
}

fn cover(w: u64, h: u64, attached: Value) -> Value {
    json!({"codec_type": "video", "width": w, "height": h, "disposition": {"attached_pic": attached}})
}

fn refused(info: &Value) -> String {
    read_probe(info).unwrap_err().key().to_string()
}

/// A fake ffprobe answer for each case: what a video needs, and the same choices as playback.
#[test]
fn the_probe_needs_a_picture_a_length_and_a_sane_size() {
    let ok = json!({"streams": [video(1280, 720), {"codec_type": "audio"}], "format": {"duration": "12.5"}});
    assert_eq!(
        read_probe(&ok),
        Ok(Probed {
            duration_s: 12.5,
            width: 1280,
            height: 720,
            audio: false,
        })
    );
    let huge = json!({"streams": [video(16000, 9000)], "format": {"duration": "5"}});
    assert_eq!(refused(&huge), "errors.upload_not_video");
    // The picture stream playback would use has no known size: refused, not taken as sound.
    let unknown =
        json!({"streams": [video(0, 0), {"codec_type": "audio"}], "format": {"duration": "5"}});
    assert_eq!(refused(&unknown), "errors.upload_not_video");
    let no_length = json!({"streams": [video(640, 360)], "format": {}});
    assert_eq!(refused(&no_length), "errors.upload_not_video");
    assert_eq!(refused(&json!({})), "errors.upload_not_video");
    // Nothing to play: no picture and no sound (a cover alone is no film).
    let only_cover = json!({"streams": [cover(500, 500, json!(1))], "format": {"duration": "60"}});
    assert_eq!(refused(&only_cover), "errors.upload_not_video");
}

/// Sound only is accepted with a known length; attached pictures count as cover art only (in
/// any of ffprobe's spellings, as playback reads them) and must be of a sane size.
#[test]
fn sound_only_is_accepted_and_cover_art_is_never_the_picture() {
    let audio = |covers: Vec<Value>, format: Value| {
        let mut streams = vec![json!({"codec_type": "audio", "duration": "61.5"})];
        streams.extend(covers);
        json!({"streams": streams, "format": format})
    };
    let sound = Probed {
        duration_s: 180.0,
        width: 0,
        height: 0,
        audio: true,
    };
    assert_eq!(
        read_probe(&audio(vec![], json!({"duration": "180"}))),
        Ok(sound)
    );
    for attached in [json!(1), json!(true), json!("1")] {
        let info = audio(
            vec![cover(600, 600, attached.clone())],
            json!({"duration": "180"}),
        );
        assert_eq!(read_probe(&info), Ok(sound), "attached_pic {attached}");
        // Playback sees the same: sound only, with this cover.
        let played = crate::pipeline::probe::parse_probe_output(&info).unwrap();
        assert!(played.audio.is_some(), "attached_pic {attached}");
    }
    // A cover too big (or of no known size) to draw is refused.
    for (w, h) in [(20000, 20000), (0, 0)] {
        let info = audio(vec![cover(w, h, json!(1))], json!({"duration": "180"}));
        assert_eq!(refused(&info), "errors.upload_not_video", "{w}x{h}");
    }
    // The length: the file's, else the audio stream's; neither: refused.
    assert_eq!(
        read_probe(&audio(vec![], json!({}))).map(|p| p.duration_s),
        Ok(61.5)
    );
    let no_length = json!({"streams": [{"codec_type": "audio"}], "format": {"duration": "N/A"}});
    assert_eq!(refused(&no_length), "errors.upload_not_video");
    // A real picture next to the cover: a video, checked as one.
    let both = json!({"streams": [cover(600, 600, json!(1)), video(640, 360), {"codec_type": "audio"}], "format": {"duration": "9"}});
    assert_eq!(
        read_probe(&both).map(|p| (p.audio, p.width)),
        Ok((false, 640))
    );
}

fn store_with(free: Option<u64>) -> (tempfile::TempDir, Arc<UploadStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = UploadStore::with_free_space(
        &dir.path().join(UPLOADS_DIR),
        PathBuf::from("ffprobe"),
        Box::new(move |_| free),
    )
    .unwrap();
    (dir, Arc::new(store))
}

fn put(store: &UploadStore, name: &str, size: usize) {
    std::fs::write(store.dir().join(name), vec![0u8; size]).unwrap();
}

fn now() -> i64 {
    unix_ms(SystemTime::now())
}

#[test]
fn part_files_of_an_earlier_run_go_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let uploads = dir.path().join(UPLOADS_DIR);
    std::fs::create_dir_all(&uploads).unwrap();
    std::fs::write(uploads.join(format!("{ID}.part")), b"x").unwrap();
    std::fs::write(uploads.join(format!("{ID}.mp4")), b"x").unwrap();
    let store =
        UploadStore::with_free_space(&uploads, PathBuf::new(), Box::new(|_| Some(u64::MAX)))
            .unwrap();
    assert!(!store.dir().join(format!("{ID}.part")).exists());
    assert!(store.dir().join(format!("{ID}.mp4")).exists());
}

#[test]
fn one_upload_per_address_a_few_at_once_and_the_part_goes_with_the_slot() {
    let (_dir, store) = store_with(Some(u64::MAX));
    let a = store.begin("1.1.1.1", 10, u64::MAX, &[], now()).unwrap();
    assert_eq!(
        store.begin("1.1.1.1", 10, u64::MAX, &[], now()).err(),
        Some(Refusal::Busy)
    );
    let _b = store.begin("2.2.2.2", 10, u64::MAX, &[], now()).unwrap();
    let _c = store.begin("3.3.3.3", 10, u64::MAX, &[], now()).unwrap();
    assert_eq!(
        store.begin("4.4.4.4", 10, u64::MAX, &[], now()).err(),
        Some(Refusal::Busy)
    );
    std::fs::write(a.part(), b"half").unwrap();
    let part = a.part().to_path_buf();
    assert!(part.starts_with(store.dir()));
    // The sweep leaves a part that is being written.
    assert_eq!(store.sweep(&[], now()), 0);
    drop(a);
    assert!(!part.exists(), "a dropped upload leaves nothing behind");
    assert!(store.begin("1.1.1.1", 10, u64::MAX, &[], now()).is_ok());
}

#[test]
fn the_disk_keeps_its_reserve() {
    let (_dir, store) = store_with(Some(FREE_RESERVE + 100));
    assert_eq!(
        store.begin("a", 101, u64::MAX, &[], now()).err(),
        Some(Refusal::NoRoom)
    );
    assert!(store.begin("a", 100, u64::MAX, &[], now()).is_ok());
    let (_dir, unknown) = store_with(None);
    assert_eq!(
        unknown.begin("a", 1, u64::MAX, &[], now()).err(),
        Some(Refusal::NoRoom)
    );
}

#[test]
fn room_in_the_quota_is_made_from_finished_files_oldest_first() {
    let (_dir, store) = store_with(Some(u64::MAX));
    let names: Vec<String> = (0..4).map(|n| format!("{n:032x}.mp4")).collect();
    for name in &names {
        put(&store, name, 100);
        std::thread::sleep(Duration::from_millis(20));
    }
    let t = now();
    let rows: Vec<UploadRow> = vec![
        (format!("upload:{}", names[0]), ItemStatus::Queued, None),
        (format!("upload:{}", names[1]), ItemStatus::Played, Some(t)),
        (format!("upload:{}", names[2]), ItemStatus::Removed, Some(t)),
        // names[3]: no item yet (written a moment ago): never taken for room.
    ];
    // 400 kept + 150 wanted in 450: one finished file must go, the oldest (names[1]).
    let slot = store.begin("a", 150, 450, &rows, t).unwrap();
    assert!(
        store.dir().join(&names[0]).exists(),
        "a waiting item's file stays"
    );
    assert!(!store.dir().join(&names[1]).exists());
    assert!(store.dir().join(&names[2]).exists());
    drop(slot);
    // Not enough even without the finished ones: refused, the waiting file kept.
    assert_eq!(
        store.begin("a", 300, 450, &rows, t).err(),
        Some(Refusal::NoRoom)
    );
    assert!(store.dir().join(&names[0]).exists());
    assert!(store.dir().join(&names[3]).exists());
}

#[test]
fn the_sweep_deletes_what_aired_or_went_after_the_undo_time() {
    let (_dir, store) = store_with(Some(u64::MAX));
    let name = |n: u32| format!("{n:032x}.mkv");
    for n in 0..5 {
        put(&store, &name(n), 10);
    }
    std::fs::write(store.dir().join(format!("{:032x}.part", 9)), b"x").unwrap();
    std::fs::write(store.dir().join("notes.txt"), b"x").unwrap();
    let t = now();
    let rows: Vec<UploadRow> = vec![
        (format!("upload:{}", name(0)), ItemStatus::Playing, None),
        (
            format!("upload:{}", name(1)),
            ItemStatus::Played,
            Some(t - FINISHED_GRACE_MS - 1),
        ),
        (
            format!("upload:{}", name(2)),
            ItemStatus::Removed,
            Some(t - 1000),
        ),
        // Played once, then queued again: kept.
        (
            format!("upload:{}", name(3)),
            ItemStatus::Played,
            Some(t - FINISHED_GRACE_MS - 1),
        ),
        (
            format!("upload:{}", name(3)),
            ItemStatus::PendingApproval,
            None,
        ),
        // name(4): no item, written just now.
    ];
    assert_eq!(
        store.sweep(&rows, t),
        2,
        "the played file and the stale part"
    );
    let exists = |n: u32| store.dir().join(name(n)).exists();
    assert!(exists(0) && !exists(1) && exists(2) && exists(3) && exists(4));
    assert!(
        store.dir().join("notes.txt").exists(),
        "not ours: left alone"
    );
    // After the undo time the removed one goes; a nameless file after ten minutes.
    assert_eq!(store.sweep(&rows, t + FINISHED_GRACE_MS), 1);
    assert!(!exists(2) && exists(4));
    assert_eq!(store.sweep(&rows, t + ORPHAN_GRACE_MS + 1000), 1);
    assert!(!exists(4) && exists(0) && exists(3));
}

#[tokio::test]
async fn the_resolver_plays_only_files_of_the_folder() {
    let (_dir, store) = store_with(Some(u64::MAX));
    put(&store, &format!("{ID}.mp4"), 10);
    let r = UploadResolver::new(
        store.dir().to_path_buf(),
        Box::new(|_| Box::pin(async { Some(5.0) })),
    );
    let s = r.resolve(&format!("upload:{ID}.mp4")).await.unwrap();
    assert_eq!(
        PathBuf::from(&s.inputs[0].url),
        store.dir().join(format!("{ID}.mp4"))
    );
    assert_eq!(
        r.fetch_meta(&format!("upload:{ID}.mp4"))
            .await
            .unwrap()
            .duration_s,
        Some(5.0)
    );
    for bad in [format!("upload:{ID}.mkv"), format!("upload:../{ID}.mp4")] {
        assert_eq!(
            r.resolve(&bad).await.unwrap_err().0.key(),
            "errors.upload_gone"
        );
    }
}

fn bin(name: &str) -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
    [format!("{name}.exe"), name.to_string()]
        .iter()
        .map(|n| root.join("bin").join(n))
        .find(|p| p.exists())
}

fn make(ffmpeg: &Path, args: &[&str], out: &Path) {
    let status = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success());
}

/// Real ffprobe: a short MP4 passes with its length, sound only as audio; a fake header does
/// not.
#[tokio::test]
async fn real_files_are_checked_with_the_forced_demuxer() {
    let (Some(ffmpeg), Some(ffprobe)) = (bin("ffmpeg"), bin("ffprobe")) else {
        eprintln!("no ffmpeg/ffprobe in bin/: skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mp4 = dir.path().join("a.part");
    make(
        &ffmpeg,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=320x240:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=d=2",
            "-shortest",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-f",
            "mp4",
        ],
        &mp4,
    );
    let head = std::fs::read(&mp4).unwrap();
    let format = sniff(&head).unwrap();
    assert_eq!(format.ext, "mp4");
    let p = probe_file(&ffprobe, &mp4, format).await.unwrap();
    assert!((p.duration_s - 2.0).abs() < 0.2, "{p:?}");
    assert_eq!((p.width, p.height), (320, 240));

    let mka = dir.path().join("b.part");
    make(
        &ffmpeg,
        &[
            "-f", "lavfi", "-i", "sine=d=1", "-c:a", "libopus", "-f", "matroska",
        ],
        &mka,
    );
    let format = sniff(&std::fs::read(&mka).unwrap()).unwrap();
    let p = probe_file(&ffprobe, &mka, format).await.unwrap();
    assert!(p.audio && (p.duration_s - 1.0).abs() < 0.2, "{p:?}");
    assert_eq!(format.ext_for(p.audio), "mka");

    // An MP4 header in front of garbage: ffprobe refuses it.
    let fake = dir.path().join("c.part");
    std::fs::write(&fake, b"\0\0\0\x20ftypisom garbage garbage garbage").unwrap();
    assert_eq!(
        probe_file(&ffprobe, &fake, MP4).await.unwrap_err().key(),
        "errors.upload_not_video"
    );
}
