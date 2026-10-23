//! End-to-end test with real ffmpeg: two different clips (720p25 with audio, 1080p30 without)
//! plus slates go through the real clip factory, channel and broadcaster (accelerated clock)
//! and must form one stream without DTS going backwards. Skips when `bin/ffmpeg` or
//! `bin/ffprobe` is missing. Port of `tests/integration.test.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use vrcinema::config::{VideoEncoder, load_config};
use vrcinema::db::{
    AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, SettingsPatch, SettingsRepo,
    open_database,
};
use vrcinema::pipeline::broadcaster::{
    Broadcaster, BroadcasterOptions, PublishedSegment, SegmentSource, SourceSegment,
};
use vrcinema::pipeline::channel::{Channel, ChannelOptions, ChannelStatus, PauseReason};
use vrcinema::pipeline::clip_factory::{ClipFactoryDeps, create_clip_factory};
use vrcinema::pipeline::slates::{SlateOptions, generate_slates};
use vrcinema::resolver::{MediaMeta, MediaResolver, ResolveError, ResolvedStream, StreamInput};

/// Resolves a local file path to itself.
struct PathResolver;

#[async_trait]
impl MediaResolver for PathResolver {
    async fn fetch_meta(&self, _url: &str) -> Result<MediaMeta, ResolveError> {
        Err(ResolveError::new("nieużywane"))
    }

    async fn resolve(&self, url: &str) -> Result<ResolvedStream, ResolveError> {
        Ok(ResolvedStream {
            inputs: vec![StreamInput {
                url: url.to_string(),
                headers: vec![],
            }],
            title: url.to_string(),
            duration_s: Some(6.0),
            is_live: false,
        })
    }
}

fn make_clip(ffmpeg: &Path, file: &Path, size: &str, rate: u32, with_audio: bool, seconds: u32) {
    let mut args: Vec<String> = ["-v", "error", "-y", "-f", "lavfi", "-i"]
        .map(String::from)
        .into();
    args.push(format!(
        "testsrc2=size={size}:rate={rate}:duration={seconds}"
    ));
    if with_audio {
        args.extend(["-f", "lavfi", "-i"].map(String::from));
        args.push(format!("sine=frequency=440:duration={seconds}"));
    }
    args.extend(
        [
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-g",
        ]
        .map(String::from),
    );
    args.push(rate.to_string());
    if with_audio {
        args.extend(["-c:a", "aac"].map(String::from));
    }
    args.push(file.to_string_lossy().into_owned());
    let out = Command::new(ffmpeg).args(&args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn packet_dts(ffprobe: &Path, file: &Path, stream: &str) -> Vec<f64> {
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", stream])
        .args(["-show_entries", "packet=dts", "-of", "csv=p=0"])
        .arg(file)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split(',').next()?.trim().parse::<f64>().ok())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clips_and_slates_form_one_stream_without_dts_going_backwards() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let env: HashMap<String, String> = HashMap::from([(
        "ADMIN_PASSWORD".to_string(),
        "haslo-testowe-123".to_string(),
    )]);
    let cfg = load_config(&env, &root, cfg!(windows)).unwrap();
    if !cfg.ffmpeg_path.exists() || !cfg.ffprobe_path.exists() {
        eprintln!("pomijam: brak bin/ffmpeg lub bin/ffprobe");
        return;
    }

    let work = tempfile::tempdir().unwrap();
    let work_dir = work.path();
    make_clip(
        &cfg.ffmpeg_path,
        &work_dir.join("a.mp4"),
        "1280x720",
        25,
        true,
        6,
    );
    make_clip(
        &cfg.ffmpeg_path,
        &work_dir.join("b.mp4"),
        "1920x1080",
        30,
        false,
        6,
    );

    let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
    {
        let conn = db.lock().unwrap();
        let queue = QueueRepo::new(&conn);
        for name in ["a.mp4", "b.mp4"] {
            queue
                .add(&NewItem {
                    channel_id: MAIN_CHANNEL_ID,
                    source_url: work_dir.join(name).to_string_lossy().into_owned(),
                    title: name.to_string(),
                    duration_s: Some(6.0),
                    is_live: false,
                    status: ItemStatus::Queued,
                    added_by: AddedBy::Admin,
                    requester_name: None,
                    requester_ip: None,
                    requester_key_hash: None,
                })
                .unwrap();
        }
    }

    let slates = generate_slates(&SlateOptions {
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        font_file: cfg.font_file.clone(),
        dir: work_dir.join("slates"),
        request_url: String::new(),
        connecting: false,
        segment_seconds: 4,
        generation: 0,
        lang: vrcinema::i18n::Lang::En,
        japanese_font_dir: vrcinema::pipeline::slates::system_fonts_dir(),
    })
    .await
    .unwrap();

    let clips_dir = work_dir.join("clips");
    let factory = create_clip_factory(ClipFactoryDeps {
        resolver: Arc::new(PathResolver),
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        ffprobe_path: cfg.ffprobe_path.clone(),
        encoder: vrcinema::live::LiveValue::new(VideoEncoder::Libx264),
        segment_seconds: 4,
        clips_dir: clips_dir.clone(),
        temp_dir: clips_dir.join("temp"),
        db: Arc::clone(&db),
        channel_id: MAIN_CHANNEL_ID,
        log: None,
        font_file: cfg.font_file.clone(),
        public_url: vrcinema::live::LiveValue::new(String::new()),
        host_resolver: std::sync::Arc::new(vrcinema::rules::SystemHostResolver),
        check_viewer_media: true,
    });
    let mut channel = Channel::new(four_second_lag(ChannelOptions::new(
        MAIN_CHANNEL_ID,
        Arc::clone(&db),
        Arc::new(factory),
        slates,
    )));
    channel.start().unwrap();

    // Accelerated broadcaster clock; ffmpeg runs at its own pace.
    let clock = Arc::new(AtomicU64::new(0));
    let live_dir = work_dir.join("live");
    let clock_for_broadcaster = Arc::clone(&clock);
    let mut broadcaster = Broadcaster::new(
        BroadcasterOptions {
            dir: live_dir.clone(),
            window_size: 10,
            segment_seconds: 4.0,
            path_prefix: String::new(),
            discontinuity_tags: Box::new(|| true),
            retain_s: Some(100_000),
            log: None,
        },
        Box::new(move || clock_for_broadcaster.load(Ordering::SeqCst)),
    );
    broadcaster.start_initial(&mut channel);

    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        let done = {
            let conn = db.lock().unwrap();
            QueueRepo::new(&conn)
                .list_by_status(MAIN_CHANNEL_ID, &[ItemStatus::Queued, ItemStatus::Playing])
                .unwrap()
                .is_empty()
        };
        if done && broadcaster.known_segments().len() > 8 {
            break;
        }
        clock.fetch_add(1000, Ordering::SeqCst);
        broadcaster.tick(&mut channel);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let published = broadcaster.known_segments();
    channel.stop();

    let statuses: Vec<(String, ItemStatus, Option<String>)> = {
        let conn = db.lock().unwrap();
        QueueRepo::new(&conn)
            .history(MAIN_CHANNEL_ID, 10)
            .unwrap()
            .into_iter()
            .map(|i| (i.title, i.status, i.error))
            .collect()
    };
    assert_eq!(
        statuses,
        vec![
            ("b.mp4".to_string(), ItemStatus::Played, None),
            ("a.mp4".to_string(), ItemStatus::Played, None),
        ]
    );
    assert!(
        published.len() > 8,
        "opublikowano {} fragmentów",
        published.len()
    );

    let joined = work_dir.join("joined.ts");
    let mut bytes = Vec::new();
    for name in &published {
        bytes.extend(std::fs::read(live_dir.join(name)).unwrap());
    }
    std::fs::write(&joined, bytes).unwrap();

    for stream in ["v", "a"] {
        let dts = packet_dts(&cfg.ffprobe_path, &joined, stream);
        assert!(dts.len() > 100, "strumień {stream}: {} pakietów", dts.len());
        let backwards: Vec<f64> = dts
            .windows(2)
            .filter(|w| w[1] <= w[0])
            .map(|w| w[1])
            .collect();
        assert!(
            backwards.is_empty(),
            "cofnięcia DTS w strumieniu {stream}: {backwards:?}"
        );
        let max_gap = dts.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
        assert!(
            max_gap < 0.5 * 90_000.0,
            "największa luka w strumieniu {stream}: {max_gap}"
        );
    }

    // Disposal is asynchronous: wait until ffmpeg exited and the clip directories are gone.
    let cleanup_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let leftover = std::fs::read_dir(&clips_dir)
            .map(|d| d.count())
            .unwrap_or(0);
        if leftover == 0 || Instant::now() > cleanup_deadline {
            assert_eq!(leftover, 0, "katalogi klipów nie zostały usunięte");
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Records what the channel hands to the broadcaster.
struct Tap<'a> {
    inner: &'a mut Channel,
    pulled: &'a mut Vec<(String, f64)>,
}

impl SegmentSource for Tap<'_> {
    fn next(&mut self) -> anyhow::Result<SourceSegment> {
        let seg = self.inner.next()?;
        self.pulled.push((seg.source_key.clone(), seg.duration));
        Ok(seg)
    }

    fn on_published(&mut self, seg: &PublishedSegment) {
        self.inner.on_published(seg);
    }
}

fn clip_seconds(pulled: &[(String, f64)]) -> f64 {
    pulled
        .iter()
        .filter(|(key, _)| key.starts_with("clip-"))
        .map(|(_, d)| d)
        .sum()
}

fn assert_no_dts_backwards(ffprobe: &Path, joined: &Path) {
    for stream in ["v", "a"] {
        let dts = packet_dts(ffprobe, joined, stream);
        assert!(dts.len() > 100, "strumień {stream}: {} pakietów", dts.len());
        let backwards: Vec<f64> = dts
            .windows(2)
            .filter(|w| w[1] <= w[0])
            .map(|w| w[1])
            .collect();
        assert!(
            backwards.is_empty(),
            "cofnięcia DTS w strumieniu {stream}: {backwards:?}"
        );
        let max_gap = dts.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
        assert!(
            max_gap < 0.5 * 90_000.0,
            "największa luka w strumieniu {stream}: {max_gap}"
        );
    }
}

/// Review Focus 2 (pause): a film paused midway and resumed continues from the remembered
/// position (the same title, positions never jump back), and the stream across the pause slates
/// and the relaunched clip has no DTS going backwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_and_resume_continue_the_same_film_without_dts_going_backwards() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let env: HashMap<String, String> = HashMap::from([(
        "ADMIN_PASSWORD".to_string(),
        "haslo-testowe-123".to_string(),
    )]);
    let cfg = load_config(&env, &root, cfg!(windows)).unwrap();
    if !cfg.ffmpeg_path.exists() || !cfg.ffprobe_path.exists() {
        eprintln!("pomijam: brak bin/ffmpeg lub bin/ffprobe");
        return;
    }

    let work = tempfile::tempdir().unwrap();
    let work_dir = work.path();
    make_clip(
        &cfg.ffmpeg_path,
        &work_dir.join("p.mp4"),
        "1280x720",
        25,
        true,
        32,
    );
    let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
    let film = {
        let conn = db.lock().unwrap();
        QueueRepo::new(&conn)
            .add(&NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url: work_dir.join("p.mp4").to_string_lossy().into_owned(),
                title: "p.mp4".to_string(),
                duration_s: Some(32.0),
                is_live: false,
                status: ItemStatus::Queued,
                added_by: AddedBy::Admin,
                requester_name: None,
                requester_ip: None,
                requester_key_hash: None,
            })
            .unwrap()
    };
    let slates = generate_slates(&SlateOptions {
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        font_file: cfg.font_file.clone(),
        dir: work_dir.join("slates"),
        request_url: String::new(),
        connecting: false,
        segment_seconds: 4,
        generation: 0,
        lang: vrcinema::i18n::Lang::En,
        japanese_font_dir: vrcinema::pipeline::slates::system_fonts_dir(),
    })
    .await
    .unwrap();
    let clips_dir = work_dir.join("clips");
    let factory = create_clip_factory(ClipFactoryDeps {
        resolver: Arc::new(PathResolver),
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        ffprobe_path: cfg.ffprobe_path.clone(),
        encoder: vrcinema::live::LiveValue::new(VideoEncoder::Libx264),
        segment_seconds: 4,
        clips_dir: clips_dir.clone(),
        temp_dir: clips_dir.join("temp"),
        db: Arc::clone(&db),
        channel_id: MAIN_CHANNEL_ID,
        log: None,
        font_file: cfg.font_file.clone(),
        public_url: vrcinema::live::LiveValue::new(String::new()),
        host_resolver: std::sync::Arc::new(vrcinema::rules::SystemHostResolver),
        check_viewer_media: true,
    });
    let mut channel = Channel::new(four_second_lag(ChannelOptions::new(
        MAIN_CHANNEL_ID,
        Arc::clone(&db),
        Arc::new(factory),
        slates,
    )));
    channel.start().unwrap();

    let clock = Arc::new(AtomicU64::new(0));
    let live_dir = work_dir.join("live");
    let clock_for_broadcaster = Arc::clone(&clock);
    let mut broadcaster = Broadcaster::new(
        BroadcasterOptions {
            dir: live_dir.clone(),
            window_size: 10,
            segment_seconds: 4.0,
            path_prefix: String::new(),
            discontinuity_tags: Box::new(|| true),
            retain_s: Some(100_000),
            log: None,
        },
        Box::new(move || clock_for_broadcaster.load(Ordering::SeqCst)),
    );
    broadcaster.start_initial(&mut channel);

    // What viewers saw, one entry per observed change: (status, film position in seconds).
    let mut seen: Vec<(ChannelStatus, i64)> = Vec::new();
    // Every segment the channel handed to the broadcaster: (source key, duration in seconds).
    let mut pulled: Vec<(String, f64)> = Vec::new();
    let (mut pause_mark, mut resume_mark) = (0, 0);
    let mut paused_at: Option<i64> = None;
    let mut pause_slates = 0;
    let mut resumed = false;
    let mut finished = false;
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        clock.fetch_add(1000, Ordering::SeqCst);
        broadcaster.tick(&mut Tap {
            inner: &mut channel,
            pulled: &mut pulled,
        });
        let state = channel.state();
        if let Some(cur) = &state.current {
            assert_eq!(cur.title, "p.mp4");
            if seen.last() != Some(&(state.status, cur.position_s)) {
                seen.push((state.status, cur.position_s));
            }
        }
        match (paused_at, resumed) {
            (None, _) => {
                if state.status == ChannelStatus::Playing
                    && state.current.as_ref().is_some_and(|c| c.position_s >= 12)
                {
                    pause_mark = pulled.len();
                    channel.pause(PauseReason::Tunnel);
                    paused_at = state.current.map(|c| c.position_s);
                }
            }
            (Some(_), false) => {
                if state.status == ChannelStatus::Paused {
                    pause_slates += 1;
                }
                if pause_slates >= 8 {
                    resume_mark = pulled.len();
                    channel.resume(PauseReason::Tunnel);
                    resumed = true;
                }
            }
            (Some(_), true) => {}
        }
        if resumed {
            let done = {
                let conn = db.lock().unwrap();
                QueueRepo::new(&conn).get(film.id).unwrap().unwrap().status == ItemStatus::Played
            };
            if done && channel.state().status == ChannelStatus::Idle {
                finished = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let published = broadcaster.known_segments();
    channel.stop();
    assert!(finished, "film nie dokończył się po wznowieniu: {seen:?}");
    let paused_at = paused_at.expect("film nie doszedł do pozycji pauzy");

    // The pause shows the paused film at the position it will resume from, and playing
    // positions never go back, not even across the pause.
    let paused_pos = seen
        .iter()
        .find(|(s, _)| *s == ChannelStatus::Paused)
        .map(|(_, p)| *p)
        .expect("brak stanu pauzy");
    assert!(paused_pos >= paused_at, "{seen:?}");
    let playing: Vec<i64> = seen
        .iter()
        .filter(|(s, _)| *s == ChannelStatus::Playing)
        .map(|(_, p)| *p)
        .collect();
    assert!(
        playing.windows(2).all(|w| w[1] >= w[0]),
        "pozycje cofnęły się: {seen:?}"
    );
    let after_pause: Vec<i64> = seen
        .iter()
        .skip_while(|(s, _)| *s != ChannelStatus::Paused)
        .filter(|(s, _)| *s == ChannelStatus::Playing)
        .map(|(_, p)| *p)
        .collect();
    let first_after = *after_pause.first().expect("brak odtwarzania po wznowieniu");
    assert!(
        first_after >= paused_pos && first_after <= paused_pos + 8,
        "wznowiono od {first_after} s zamiast od ok. {paused_pos} s: {seen:?}"
    );
    assert!(
        after_pause.last().copied().unwrap_or(0) >= 24,
        "film nie doszedł do końca: {seen:?}"
    );
    // Content check that the resumed clip really seeked (without -ss it would air ~the whole
    // 32 s again): the film seconds aired after the resume are what was left at the pause.
    let before_pause = clip_seconds(&pulled[..pause_mark]);
    let after_resume = clip_seconds(&pulled[resume_mark..]);
    assert!(
        (after_resume - (32.0 - before_pause)).abs() <= 4.5,
        "po wznowieniu wyemitowano {after_resume} s filmu, a po pauzie (od {before_pause} s) \
         zostało {} s",
        32.0 - before_pause
    );
    assert!(
        pulled[resume_mark..]
            .iter()
            .any(|(k, _)| k.ends_with("-r1") && k.starts_with("clip-")),
        "klip po wznowieniu ma ten sam klucz źródła co przed pauzą"
    );
    assert_eq!(
        {
            let conn = db.lock().unwrap();
            QueueRepo::new(&conn)
                .history(MAIN_CHANNEL_ID, 10)
                .unwrap()
                .into_iter()
                .map(|i| (i.title, i.status, i.error))
                .collect::<Vec<_>>()
        },
        vec![("p.mp4".to_string(), ItemStatus::Played, None)]
    );

    let joined = work_dir.join("joined.ts");
    let mut bytes = Vec::new();
    for name in &published {
        bytes.extend(std::fs::read(live_dir.join(name)).unwrap());
    }
    std::fs::write(&joined, bytes).unwrap();
    assert_no_dts_backwards(&cfg.ffprobe_path, &joined);

    let cleanup_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let leftover = std::fs::read_dir(&clips_dir)
            .map(|d| d.count())
            .unwrap_or(0);
        if leftover == 0 || Instant::now() > cleanup_deadline {
            assert_eq!(leftover, 0, "katalogi klipów nie zostały usunięte");
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ---------- Seek (stage 4) ----------

/// GOP of the counter film: a keyframe every 2 s (50 frames at 25 fps).
const COUNTER_GOP_S: i64 = 1;

/// A film whose picture is its own counter: the brightness of every frame is
/// `16 + 3 × whole second` (luma), with a tone. Keyframes every [`COUNTER_GOP_S`].
fn make_counter_clip(ffmpeg: &Path, file: &Path, seconds: u32) {
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!(
            "color=c=black:s=320x240:r=25:d={seconds},format=yuv420p,\
             geq=lum='16+3*floor(T)':cb=128:cr=128"
        ))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={seconds}"))
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .args(["-g", "25", "-c:a", "aac"])
        .arg(file)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The counter shown in the first frame of a segment: the mean of its luma plane as coded
/// (raw yuv420p, no range conversion; the Y plane is the first two thirds).
fn counter_in(ffmpeg: &Path, segment: &Path) -> i64 {
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(segment)
        .args([
            "-frames:v",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success() && !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let luma = &out.stdout[..out.stdout.len() * 2 / 3];
    let mean = luma.iter().map(|&b| f64::from(b)).sum::<f64>() / luma.len() as f64;
    ((mean - 16.0) / 3.0).round() as i64
}

/// Records what the broadcaster published: (sequence number, source key).
struct PublishTap<'a> {
    inner: &'a mut Channel,
    published: &'a mut Vec<(u64, String)>,
}

impl SegmentSource for PublishTap<'_> {
    fn next(&mut self) -> anyhow::Result<SourceSegment> {
        self.inner.next()
    }

    fn on_published(&mut self, seg: &PublishedSegment) {
        self.published.push((seg.seq, seg.source_key.clone()));
        self.inner.on_published(seg);
    }
}

/// Review Focus 1 with real ffmpeg: a seek to 30 s of a 60 s film. The first segment after it
/// shows 30 s (±1 s converted; copied: the keyframe at or before 30 s), the DTS of the whole
/// stream never go back (also across the buffering slate aired while the new clip starts), and
/// every source change is exactly one discontinuity.
async fn seek_lands_on_its_target(force_transcode: bool) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let env: HashMap<String, String> = HashMap::from([(
        "ADMIN_PASSWORD".to_string(),
        "haslo-testowe-123".to_string(),
    )]);
    let cfg = load_config(&env, &root, cfg!(windows)).unwrap();
    if !cfg.ffmpeg_path.exists() || !cfg.ffprobe_path.exists() {
        eprintln!("pomijam: brak bin/ffmpeg lub bin/ffprobe");
        return;
    }
    let work = tempfile::tempdir().unwrap();
    let work_dir = work.path();
    let source = work_dir.join("licznik.mp4");
    make_counter_clip(&cfg.ffmpeg_path, &source, 60);

    let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
    {
        let conn = db.lock().unwrap();
        SettingsRepo::new(&conn)
            .update(
                MAIN_CHANNEL_ID,
                &SettingsPatch {
                    force_transcode: Some(force_transcode),
                    ..Default::default()
                },
            )
            .unwrap();
        QueueRepo::new(&conn)
            .add(&NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url: source.to_string_lossy().into_owned(),
                title: "licznik.mp4".to_string(),
                duration_s: Some(60.0),
                is_live: false,
                status: ItemStatus::Queued,
                added_by: AddedBy::Admin,
                requester_name: None,
                requester_ip: None,
                requester_key_hash: None,
            })
            .unwrap();
    }
    let slates = generate_slates(&SlateOptions {
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        font_file: cfg.font_file.clone(),
        dir: work_dir.join("slates"),
        request_url: String::new(),
        connecting: false,
        segment_seconds: 4,
        generation: 0,
        lang: vrcinema::i18n::Lang::En,
        japanese_font_dir: vrcinema::pipeline::slates::system_fonts_dir(),
    })
    .await
    .unwrap();
    let clips_dir = work_dir.join("clips");
    let factory = create_clip_factory(ClipFactoryDeps {
        resolver: Arc::new(PathResolver),
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        ffprobe_path: cfg.ffprobe_path.clone(),
        encoder: vrcinema::live::LiveValue::new(VideoEncoder::Libx264),
        segment_seconds: 4,
        clips_dir: clips_dir.clone(),
        temp_dir: clips_dir.join("temp"),
        db: Arc::clone(&db),
        channel_id: MAIN_CHANNEL_ID,
        log: None,
        font_file: cfg.font_file.clone(),
        public_url: vrcinema::live::LiveValue::new(String::new()),
        host_resolver: std::sync::Arc::new(vrcinema::rules::SystemHostResolver),
        check_viewer_media: true,
    });
    let mut channel = Channel::new(four_second_lag(ChannelOptions::new(
        MAIN_CHANNEL_ID,
        Arc::clone(&db),
        Arc::new(factory),
        slates,
    )));
    channel.start().unwrap();
    let clock = Arc::new(AtomicU64::new(0));
    let live_dir = work_dir.join("live");
    let clock_for_broadcaster = Arc::clone(&clock);
    let mut broadcaster = Broadcaster::new(
        BroadcasterOptions {
            dir: live_dir.clone(),
            // The whole run stays in the playlist.
            window_size: 500,
            segment_seconds: 4.0,
            path_prefix: String::new(),
            discontinuity_tags: Box::new(|| true),
            retain_s: Some(100_000),
            log: None,
        },
        Box::new(move || clock_for_broadcaster.load(Ordering::SeqCst)),
    );
    let mut published: Vec<(u64, String)> = Vec::new();
    broadcaster.start_initial(&mut PublishTap {
        inner: &mut channel,
        published: &mut published,
    });
    let clip_segments = |published: &[(u64, String)], relaunched: bool| {
        published
            .iter()
            .filter(|(_, k)| k.starts_with("clip-") && k.ends_with("-r1") == relaunched)
            .count()
    };

    // Two segments of the film from its start, then the seek.
    let deadline = Instant::now() + Duration::from_secs(90);
    while clip_segments(&published, false) < 2 {
        assert!(Instant::now() < deadline, "film nie wystartował");
        clock.fetch_add(1000, Ordering::SeqCst);
        channel.on_tick();
        broadcaster.tick(&mut PublishTap {
            inner: &mut channel,
            published: &mut published,
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let last_before = published
        .iter()
        .rev()
        .find(|(_, k)| k.starts_with("clip-"))
        .unwrap()
        .0;
    channel.seek_to(30.0);
    let control = channel.state().control;
    assert!(control.seek_pending);
    assert_eq!(control.position_s, 30.0);
    // The broadcaster keeps going as in the real loop: the old clip, the buffering slate while
    // the new clip starts, then the new clip.
    let deadline = Instant::now() + Duration::from_secs(90);
    while clip_segments(&published, true) < 3 {
        assert!(Instant::now() < deadline, "po skoku za mało fragmentów");
        clock.fetch_add(1000, Ordering::SeqCst);
        channel.on_tick();
        broadcaster.tick(&mut PublishTap {
            inner: &mut channel,
            published: &mut published,
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let approx = channel.state().control.position_approx;
    let playlist = broadcaster.playlist();
    let all = broadcaster.known_segments();
    channel.stop();

    assert_eq!(approx, !force_transcode, "positionApprox");
    let (first_after, key) = published
        .iter()
        .find(|(_, k)| k.starts_with("clip-") && k.ends_with("-r1"))
        .cloned()
        .unwrap();
    let counter = counter_in(
        &cfg.ffmpeg_path,
        &live_dir.join(format!("{first_after}.ts")),
    );
    if force_transcode {
        assert!(
            (counter - 30).abs() <= 1,
            "konwersja: licznik {counter} ({key})"
        );
    } else {
        assert!(
            (30 - COUNTER_GOP_S..=30).contains(&counter),
            "kopiowanie: licznik {counter} ({key})"
        );
    }
    let before = counter_in(
        &cfg.ffmpeg_path,
        &live_dir.join(format!("{last_before}.ts")),
    );
    assert!(before < 12, "przed skokiem licznik {before}");

    // Exactly one discontinuity per source change (old clip → slate → new clip), none
    // elsewhere; the whole run is in the playlist.
    let keys: HashMap<u64, String> = published.iter().cloned().collect();
    let mut discontinuity = false;
    let mut listed: Vec<(u64, bool)> = Vec::new();
    for line in playlist.lines() {
        if line == "#EXT-X-DISCONTINUITY" {
            discontinuity = true;
        } else if let Some(seq) = line.strip_suffix(".ts").and_then(|s| s.parse::<u64>().ok()) {
            listed.push((seq, discontinuity));
            discontinuity = false;
        }
    }
    assert_eq!(listed.len(), published.len(), "{playlist}");
    for pair in listed.windows(2) {
        let (prev, (seq, tagged)) = (pair[0].0, pair[1]);
        let changed = keys[&prev] != keys[&seq];
        assert_eq!(
            tagged, changed,
            "segment {seq} ({} after {}): {playlist}",
            keys[&seq], keys[&prev]
        );
    }
    // The old clip may air a little longer while the seek waits for its quiet; after its last
    // segment: old clip → new clip directly (one change), or through the buffering slate (two).
    let last_old = published
        .iter()
        .filter(|(_, k)| k.starts_with("clip-") && !k.ends_with("-r1"))
        .map(|(seq, _)| *seq)
        .max()
        .unwrap();
    assert!(last_old < first_after, "{published:?}");
    let changes_after_old = listed
        .iter()
        .filter(|(seq, tagged)| *seq > last_old && *tagged)
        .count();
    let slates_between = published
        .iter()
        .filter(|(seq, _)| *seq > last_old && *seq < first_after)
        .count();
    assert!(
        published
            .iter()
            .filter(|(seq, _)| *seq > last_old && *seq < first_after)
            .all(|(_, k)| k.starts_with("buffering")),
        "{published:?}"
    );
    assert_eq!(
        changes_after_old,
        if slates_between > 0 { 2 } else { 1 },
        "{published:?}"
    );

    let joined = work_dir.join("joined.ts");
    let mut bytes = Vec::new();
    for name in &all {
        bytes.extend(std::fs::read(live_dir.join(name)).unwrap());
    }
    std::fs::write(&joined, bytes).unwrap();
    assert_no_dts_backwards(&cfg.ffprobe_path, &joined);

    let cleanup_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let leftover = std::fs::read_dir(&clips_dir)
            .map(|d| d.count())
            .unwrap_or(0);
        if leftover == 0 || Instant::now() > cleanup_deadline {
            assert_eq!(leftover, 0, "katalogi klipów nie zostały usunięte");
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seek_of_a_copied_film_lands_on_the_keyframe_at_or_before_the_target() {
    seek_lands_on_its_target(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seek_of_a_converted_film_lands_on_the_target() {
    seek_lands_on_its_target(true).await;
}

// ---------- Info ticker (stage 4, task 6) ----------

/// The counter of the first frame of a segment, read from the top half of the picture only
/// (the ticker draws at the bottom).
fn counter_in_top(ffmpeg: &Path, segment: &Path) -> i64 {
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(segment)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "crop=iw:ih/2:0:0",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success() && !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let luma = &out.stdout[..out.stdout.len() * 2 / 3];
    let mean = luma.iter().map(|&b| f64::from(b)).sum::<f64>() / luma.len() as f64;
    ((mean - 16.0) / 3.0).round() as i64
}

/// A new look of the ticker while a film airs with it: the film starts again once, from where
/// the channel is (the first new segment follows the last old one: no part repeated or
/// skipped), with one discontinuity per source change and no DTS going backwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_ticker_look_starts_the_film_again_once_from_where_it_is() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let env: HashMap<String, String> = HashMap::from([(
        "ADMIN_PASSWORD".to_string(),
        "haslo-testowe-123".to_string(),
    )]);
    let cfg = load_config(&env, &root, cfg!(windows)).unwrap();
    if !cfg.ffmpeg_path.exists() || !cfg.ffprobe_path.exists() || !cfg.font_file.is_file() {
        eprintln!("pomijam: brak bin/ffmpeg, bin/ffprobe lub czcionki");
        return;
    }
    let work = tempfile::tempdir().unwrap();
    let work_dir = work.path();
    let source = work_dir.join("licznik.mp4");
    make_counter_clip(&cfg.ffmpeg_path, &source, 60);

    let db = Arc::new(Mutex::new(open_database(Path::new(":memory:")).unwrap()));
    let update = |patch: serde_json::Value| {
        let conn = db.lock().unwrap();
        let patch = vrcinema::db::sanitize_settings_patch(&patch).unwrap();
        SettingsRepo::new(&conn)
            .update(MAIN_CHANNEL_ID, &patch)
            .unwrap();
    };
    update(serde_json::json!({ "tickerEnabled": true, "tickerQr": "off" }));
    {
        let conn = db.lock().unwrap();
        QueueRepo::new(&conn)
            .add(&NewItem {
                channel_id: MAIN_CHANNEL_ID,
                source_url: source.to_string_lossy().into_owned(),
                title: "licznik.mp4".to_string(),
                duration_s: Some(60.0),
                is_live: false,
                status: ItemStatus::Queued,
                added_by: AddedBy::Admin,
                requester_name: None,
                requester_ip: None,
                requester_key_hash: None,
            })
            .unwrap();
    }
    let slates = generate_slates(&SlateOptions {
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        font_file: cfg.font_file.clone(),
        dir: work_dir.join("slates"),
        request_url: String::new(),
        connecting: false,
        segment_seconds: 4,
        generation: 0,
        lang: vrcinema::i18n::Lang::En,
        japanese_font_dir: vrcinema::pipeline::slates::system_fonts_dir(),
    })
    .await
    .unwrap();
    let clips_dir = work_dir.join("clips");
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&logs);
    let factory = create_clip_factory(ClipFactoryDeps {
        resolver: Arc::new(PathResolver),
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        ffprobe_path: cfg.ffprobe_path.clone(),
        encoder: vrcinema::live::LiveValue::new(VideoEncoder::Libx264),
        segment_seconds: 4,
        clips_dir: clips_dir.clone(),
        temp_dir: clips_dir.join("temp"),
        db: Arc::clone(&db),
        channel_id: MAIN_CHANNEL_ID,
        log: Some(Arc::new(move |m| sink.lock().unwrap().push(m.to_string()))),
        font_file: cfg.font_file.clone(),
        public_url: vrcinema::live::LiveValue::new(String::new()),
        host_resolver: std::sync::Arc::new(vrcinema::rules::SystemHostResolver),
        check_viewer_media: true,
    });
    let mut options =
        ChannelOptions::new(MAIN_CHANNEL_ID, Arc::clone(&db), Arc::new(factory), slates);
    let sink = Arc::clone(&logs);
    options.log = Some(Box::new(move |m| sink.lock().unwrap().push(m.to_string())));
    options.viewer_lag_segments = Some(3);
    let mut channel = Channel::new(options);
    channel.start().unwrap();
    let clock = Arc::new(AtomicU64::new(0));
    let live_dir = work_dir.join("live");
    let clock_for_broadcaster = Arc::clone(&clock);
    let mut broadcaster = Broadcaster::new(
        BroadcasterOptions {
            dir: live_dir.clone(),
            window_size: 500,
            segment_seconds: 4.0,
            path_prefix: String::new(),
            discontinuity_tags: Box::new(|| true),
            retain_s: Some(100_000),
            log: None,
        },
        Box::new(move || clock_for_broadcaster.load(Ordering::SeqCst)),
    );
    let mut published: Vec<(u64, String)> = Vec::new();
    broadcaster.start_initial(&mut PublishTap {
        inner: &mut channel,
        published: &mut published,
    });
    let clip_segments = |published: &[(u64, String)], suffix: Option<&str>| {
        published
            .iter()
            .filter(|(_, k)| {
                k.starts_with("clip-")
                    && match suffix {
                        Some(suffix) => k.ends_with(suffix),
                        None => !k.contains("-r"),
                    }
            })
            .count()
    };

    let deadline = Instant::now() + Duration::from_secs(90);
    while clip_segments(&published, None) < 2 {
        assert!(Instant::now() < deadline, "film nie wystartował");
        clock.fetch_add(1000, Ordering::SeqCst);
        channel.on_tick();
        broadcaster.tick(&mut PublishTap {
            inner: &mut channel,
            published: &mut published,
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    // The owner switches the ticker to continuous mode (twice, quickly: one restart).
    update(serde_json::json!({ "tickerMode": "continuous" }));
    channel.apply_ticker();
    update(serde_json::json!({ "tickerSpeed": "fast" }));
    channel.apply_ticker();
    let deadline = Instant::now() + Duration::from_secs(90);
    while clip_segments(&published, Some("-r1")) < 3 {
        assert!(
            Instant::now() < deadline,
            "po zmianie paska za mało fragmentów"
        );
        clock.fetch_add(1000, Ordering::SeqCst);
        channel.on_tick();
        broadcaster.tick(&mut PublishTap {
            inner: &mut channel,
            published: &mut published,
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let playlist = broadcaster.playlist();
    let all = broadcaster.known_segments();
    channel.stop();

    assert_eq!(
        clip_segments(&published, Some("-r2")),
        0,
        "one restart only"
    );
    let logs = logs.lock().unwrap().clone();
    assert_eq!(
        logs.iter()
            .filter(|l| l.starts_with("Info ticker settings changed: video #"))
            .count(),
        1,
        "{logs:?}"
    );
    let last_old = published
        .iter()
        .filter(|(_, k)| k.starts_with("clip-") && !k.contains("-r"))
        .map(|(seq, _)| *seq)
        .max()
        .unwrap();
    let first_new = published
        .iter()
        .find(|(_, k)| k.starts_with("clip-") && k.ends_with("-r1"))
        .map(|(seq, _)| *seq)
        .unwrap();
    assert!(last_old < first_new);
    let old = counter_in_top(&cfg.ffmpeg_path, &live_dir.join(format!("{last_old}.ts")));
    let new = counter_in_top(&cfg.ffmpeg_path, &live_dir.join(format!("{first_new}.ts")));
    // The old segment covers 4 s: the new one starts right after it.
    assert!(
        (3..=5).contains(&(new - old)),
        "old segment at {old} s, new one at {new} s"
    );

    let keys: HashMap<u64, String> = published.iter().cloned().collect();
    let mut discontinuity = false;
    let mut listed: Vec<(u64, bool)> = Vec::new();
    for line in playlist.lines() {
        if line == "#EXT-X-DISCONTINUITY" {
            discontinuity = true;
        } else if let Some(seq) = line.strip_suffix(".ts").and_then(|s| s.parse::<u64>().ok()) {
            listed.push((seq, discontinuity));
            discontinuity = false;
        }
    }
    for pair in listed.windows(2) {
        let (prev, (seq, tagged)) = (pair[0].0, pair[1]);
        assert_eq!(
            tagged,
            keys[&prev] != keys[&seq],
            "segment {seq} ({} after {}): {playlist}",
            keys[&seq],
            keys[&prev]
        );
    }

    let joined = work_dir.join("joined.ts");
    let mut bytes = Vec::new();
    for name in &all {
        bytes.extend(std::fs::read(live_dir.join(name)).unwrap());
    }
    std::fs::write(&joined, bytes).unwrap();
    assert_no_dts_backwards(&cfg.ffprobe_path, &joined);

    // Clip folders are removed (the ticker's font copy stays for the next clip).
    let cleanup_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let leftover: Vec<_> = std::fs::read_dir(&clips_dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n != "_fonts")
                    .collect()
            })
            .unwrap_or_default();
        if leftover.is_empty() || Instant::now() > cleanup_deadline {
            assert!(leftover.is_empty(), "katalogi klipów: {leftover:?}");
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// With the graphics card this computer has (skipped without one), a converted film is cut
/// into 4-second segments. The encoder's forced key frames must be IDR frames: otherwise the
/// segmenter waits for the encoder's own GOP and h264_nvenc gave 10-second segments (stage 4
/// live run).
#[tokio::test]
async fn a_gpu_encoder_cuts_the_converted_film_into_four_second_segments() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let env: HashMap<String, String> = HashMap::from([(
        "ADMIN_PASSWORD".to_string(),
        "haslo-testowe-123".to_string(),
    )]);
    let cfg = load_config(&env, &root, cfg!(windows)).unwrap();
    if !cfg.ffmpeg_path.exists() {
        eprintln!("skipped: no bin/ffmpeg");
        return;
    }
    let encoder = vrcinema::tools::encoder::detect_encoder(&cfg.ffmpeg_path).await;
    if encoder == VideoEncoder::Libx264 {
        eprintln!("skipped: no graphics card encoder on this computer");
        return;
    }
    let work = tempfile::tempdir().unwrap();
    // 25 fps and 13 s: the encoders' default GOP (250 frames) would be one 10 s segment.
    let film = work.path().join("film.mp4");
    make_clip(&cfg.ffmpeg_path, &film, "1280x720", 25, true, 13);
    let out_dir = work.path().join("clip");
    std::fs::create_dir_all(&out_dir).unwrap();
    let args = vrcinema::pipeline::ffmpeg_args::build_clip_args(
        &vrcinema::pipeline::ffmpeg_args::ClipArgsOptions {
            inputs: vec![StreamInput {
                url: film.to_string_lossy().into_owned(),
                headers: vec![],
            }],
            input_format: None,
            mode: vrcinema::pipeline::probe::VideoMode::Transcode,
            info: vrcinema::pipeline::probe::VideoInfo {
                codec: "mpeg4".into(),
                height: 720,
                pix_fmt: "yuv420p".into(),
                fps: 25.0,
                has_audio: true,
                duration_s: Some(13.0),
                portrait: false,
                keyframe_gap_s: Some(1.0),
            },
            encoder,
            segment_seconds: 4,
            out_dir: out_dir.clone(),
            start_at_s: 0.0,
            readrate: None,
            overlay: None,
        },
    );
    let out = Command::new(&cfg.ffmpeg_path).args(&args).output().unwrap();
    assert!(
        out.status.success(),
        "{encoder:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let list = std::fs::read_to_string(out_dir.join("list.csv")).unwrap();
    let lengths: Vec<f64> = list
        .lines()
        .filter_map(|l| {
            // `seg_00000.ts,<start>,<end>`
            let cols: Vec<&str> = l.split(',').collect();
            let start: f64 = cols.get(1)?.trim().parse().ok()?;
            let end: f64 = cols.get(2)?.trim().parse().ok()?;
            Some(end - start)
        })
        .collect();
    assert!(lengths.len() >= 4, "{encoder:?}: segments {lengths:?}");
    assert!(
        lengths.iter().all(|s| *s <= 4.2),
        "{encoder:?}: segments {lengths:?}"
    );
}

/// The tests cut 4 s segments: the viewers' 12 s are 3 of them.
fn four_second_lag(mut o: ChannelOptions) -> ChannelOptions {
    o.viewer_lag_segments = Some(3);
    o
}
