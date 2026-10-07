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
use vrcinema::db::{AddedBy, ItemStatus, MAIN_CHANNEL_ID, NewItem, QueueRepo, open_database};
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
    args.push((rate * 2).to_string());
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
                })
                .unwrap();
        }
    }

    let slates = generate_slates(&SlateOptions {
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        font_file: cfg.font_file.clone(),
        dir: work_dir.join("slates"),
        request_url: String::new(),
        segment_seconds: 4,
        generation: 0,
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
        db: Arc::clone(&db),
        channel_id: MAIN_CHANNEL_ID,
        log: None,
    });
    let mut channel = Channel::new(ChannelOptions::new(
        MAIN_CHANNEL_ID,
        Arc::clone(&db),
        Arc::new(factory),
        slates,
    ));
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
            })
            .unwrap()
    };
    let slates = generate_slates(&SlateOptions {
        ffmpeg_path: cfg.ffmpeg_path.clone(),
        font_file: cfg.font_file.clone(),
        dir: work_dir.join("slates"),
        request_url: String::new(),
        segment_seconds: 4,
        generation: 0,
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
        db: Arc::clone(&db),
        channel_id: MAIN_CHANNEL_ID,
        log: None,
    });
    let mut channel = Channel::new(ChannelOptions::new(
        MAIN_CHANNEL_ID,
        Arc::clone(&db),
        Arc::new(factory),
        slates,
    ));
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
