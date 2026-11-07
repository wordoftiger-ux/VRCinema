use super::*;
use crate::i18n::Lang;

fn settings() -> TickerSettings {
    TickerSettings {
        enabled: true,
        show_address: false,
        ..TickerSettings::default()
    }
}

fn now(requester: Requester, next: Option<&str>) -> NowNext {
    NowNext {
        title: "Never Gonna Give You Up".into(),
        requester,
        next_title: next.map(String::from),
        address: "https://kanal.example.com/".into(),
    }
}

#[test]
fn the_text_names_the_film_who_added_it_and_the_next_one() {
    let text = compose_text(
        Lang::En,
        &now(Requester::Nick("Kasia".into()), Some("Darude — Sandstorm")),
        &settings(),
        0,
    );
    assert_eq!(
        text,
        "► Now: Never Gonna Give You Up   ·   added by Kasia   ·   next: Darude — Sandstorm"
    );
}

#[test]
fn a_film_the_owner_added_is_from_the_host_in_the_slate_language() {
    let text = compose_text(Lang::Pl, &now(Requester::Host, None), &settings(), 0);
    assert_eq!(
        text,
        "► Teraz: Never Gonna Give You Up   ·   dodane przez Gospodarza"
    );
}

/// Final wave M1: a video stored without a title carries the Polish default "Bez tytułu"
/// (shared with the old server); the ticker says "Untitled" in the slate language.
#[test]
fn the_stored_default_title_is_untitled_in_the_slate_language() {
    let n = |title: &str, next: &str| NowNext {
        title: title.into(),
        requester: Requester::Anonymous,
        next_title: Some(next.into()),
        address: String::new(),
    };
    assert_eq!(
        compose_text(Lang::En, &n("Bez tytułu", "Bez tytułu"), &settings(), 0),
        "► Now: Untitled   ·   next: Untitled"
    );
    assert_eq!(
        compose_text(Lang::De, &n("Bez tytułu", "Film"), &settings(), 0),
        "► Jetzt: Ohne Titel   ·   als Nächstes: Film"
    );
    assert_eq!(
        compose_text(Lang::Pl, &n("Bez tytułu", "Film"), &settings(), 0),
        "► Teraz: Bez tytułu   ·   następnie: Film"
    );
}

#[test]
fn without_a_nick_only_the_title_is_shown() {
    let text = compose_text(Lang::En, &now(Requester::Anonymous, None), &settings(), 0);
    assert_eq!(text, "► Now: Never Gonna Give You Up");
}

#[test]
fn next_and_address_can_be_switched_off_and_on() {
    let n = now(Requester::Anonymous, Some("Pies"));
    let mut s = settings();
    s.show_next = false;
    assert_eq!(
        compose_text(Lang::En, &n, &s, 0),
        "► Now: Never Gonna Give You Up"
    );
    s.show_address = true;
    assert_eq!(
        compose_text(Lang::En, &n, &s, 0),
        "► Now: Never Gonna Give You Up   ·   add a video: https://kanal.example.com/"
    );
    // No public address: nothing to show.
    let mut no_address = n.clone();
    no_address.address = String::new();
    assert_eq!(
        compose_text(Lang::En, &no_address, &s, 0),
        "► Now: Never Gonna Give You Up"
    );
}

#[test]
fn interval_mode_shows_the_next_message_at_every_appearance() {
    let mut s = settings();
    s.messages = vec!["A".into(), "B".into(), "C".into()];
    let n = now(Requester::Anonymous, None);
    let shown: Vec<String> = (0..5)
        .map(|i| compose_text(Lang::En, &n, &s, i))
        .map(|t| t.rsplit("   ·   ").next().unwrap().to_string())
        .collect();
    assert_eq!(shown, ["A", "B", "C", "A", "B"]);
}

#[test]
fn continuous_mode_loops_the_film_and_every_message() {
    let mut s = settings();
    s.mode = TickerMode::Continuous;
    s.messages = vec!["Witajcie!".into(), "Max 3 filmy".into()];
    let n = now(Requester::Anonymous, None);
    let text = compose_text(Lang::En, &n, &s, 7);
    assert_eq!(
        text,
        "► Now: Never Gonna Give You Up   ·   Witajcie!   ·   Max 3 filmy"
    );
    assert_eq!(compose_text(Lang::En, &n, &s, 0), text);
}

#[test]
fn line_breaks_and_control_characters_never_reach_the_one_line_ticker() {
    let mut s = settings();
    s.messages = vec!["dwie\nlinie\tok".into()];
    let mut n = now(Requester::Nick("Ka\rsia".into()), None);
    n.title = "Tytuł\u{0}".into();
    assert_eq!(
        compose_text(Lang::En, &n, &s, 0),
        "► Now: Tytuł   ·   added by Ka sia   ·   dwie linie ok"
    );
}

#[test]
fn the_requester_comes_from_the_queue_item() {
    use crate::db::{AddedBy, ItemStatus, QueueItem};
    let mut item = QueueItem {
        id: 1,
        channel_id: 1,
        position: 0,
        source_url: String::new(),
        title: "t".into(),
        duration_s: None,
        is_live: false,
        status: ItemStatus::Queued,
        added_by: AddedBy::Admin,
        requester_name: Some("Kasia".into()),
        requester_ip: None,
        requester_key_hash: None,
        error: None,
        created_at: 0,
        started_at: None,
        finished_at: None,
        in_s: None,
        out_s: None,
        start_at: None,
    };
    assert_eq!(Requester::of(&item), Requester::Host);
    item.added_by = AddedBy::Public;
    assert_eq!(Requester::of(&item), Requester::Nick("Kasia".into()));
    item.requester_name = Some("  ".into());
    assert_eq!(Requester::of(&item), Requester::Anonymous);
    item.requester_name = None;
    assert_eq!(Requester::of(&item), Requester::Anonymous);
}

/// The Japanese font (Yu Gothic) has no ►: a ticker drawn with it starts with ▶ in every slate
/// language (an English ticker with a Japanese title is drawn with the Japanese font too).
#[test]
fn a_ticker_drawn_with_the_japanese_font_uses_the_arrow_that_font_has() {
    use crate::db::{AddedBy, ItemStatus, QueueItem};
    let item = QueueItem {
        id: 1,
        channel_id: 1,
        position: 0,
        source_url: String::new(),
        title: "夜に駆ける".into(),
        duration_s: None,
        is_live: false,
        status: ItemStatus::Queued,
        added_by: AddedBy::Admin,
        requester_name: None,
        requester_ip: None,
        requester_key_hash: None,
        error: None,
        created_at: 0,
        started_at: None,
        finished_at: None,
        in_s: None,
        out_s: None,
        start_at: None,
    };
    let schedule = Schedule {
        mode: TickerMode::Interval,
        interval_s: 60.0,
        prepare_after_s: 30.0,
    };
    let content = |lang| TickerContent {
        lang,
        settings: settings(),
        next_title: None,
        address: String::new(),
    };
    let dir = std::env::temp_dir();
    let text = |japanese_font, lang| {
        TickerWriter::new(
            dir.clone(),
            &item,
            schedule,
            None,
            Default::default(),
            japanese_font,
        )
        .texts(None, &content(lang))
        .0
    };
    assert_eq!(
        text(true, Lang::En),
        "▶ Now: 夜に駆ける   ·   added by Host"
    );
    assert_eq!(
        text(true, Lang::Ja),
        "▶ 再生中：夜に駆ける   ·   追加：ホスト"
    );
    // Arial (any other font) keeps the ►.
    assert_eq!(
        text(false, Lang::Pl),
        "► Teraz: 夜に駆ける   ·   dodane przez Gospodarza"
    );
    assert_eq!(for_font("► a ►".into(), true), "▶ a ▶");
    assert_eq!(for_font("► a".into(), false), "► a");
}

#[test]
fn a_text_longer_than_the_window_is_cut_with_an_ellipsis() {
    let long = "x".repeat(400);
    let fitted = fit_text(&long, 1000.0);
    assert!(fitted.ends_with('…'), "{fitted}");
    assert!(text_width_px(&fitted) <= 1000.0);
    assert!(fitted.chars().count() > 20);
    assert_eq!(fit_text("krótki", 1000.0), "krótki");
    // Wide characters count more.
    assert!(text_width_px("日本語") > text_width_px("abc"));
}

#[test]
fn the_interval_window_fits_the_longest_text_and_leaves_the_band_hidden_for_a_while() {
    let mut s = settings();
    s.interval_s = 60;
    s.speed = TickerSpeed::Normal;
    let short = window_budget_px(&s, &["abc".to_string()]);
    let long = window_budget_px(&s, &["abc".to_string(), "x".repeat(80)]);
    assert!(long > short);
    assert!(long >= text_width_px(&"x".repeat(80)));
    // Never longer than the interval minus the hidden part.
    let huge = window_budget_px(&s, &["x".repeat(5000)]);
    let visible_s = (1920.0 + huge) / TickerSpeed::Normal.px_per_s();
    assert!(visible_s <= 60.0 - 8.0 + 1e-9, "{visible_s}");
}

fn ctx(duration_s: Option<f64>) -> OverlayContext<'static> {
    OverlayContext {
        duration_s,
        start_at_s: 20.0,
        text_file: TEXT_FILE,
        font: "../_fonts/arial.ttf",
        qr: Some(QrOverlay {
            png: QR_FILE,
            size_px: 185,
            caption_file: CAPTION_FILE,
        }),
        frame_height: 1080,
        text_budget_px: 2000.0,
    }
}

#[test]
fn the_filter_reads_its_text_font_and_code_by_relative_names_only() {
    let f = overlay_filter(&settings(), &ctx(Some(60.0)));
    assert!(
        f.contains("textfile=ticker.txt:reload=1:expansion=none"),
        "{f}"
    );
    assert!(f.contains("fontfile=../_fonts/arial.ttf"), "{f}");
    assert!(f.contains("movie=qr.png"), "{f}");
    assert!(f.contains("textfile=qr.txt"), "{f}");
    assert!(!f.contains('\\') && !f.contains("C:"), "{f}");
}

#[test]
fn the_progress_line_follows_the_position_in_the_film_but_not_for_a_live_stream() {
    let f = overlay_filter(&settings(), &ctx(Some(60.0)));
    assert!(f.contains("color=c=0xf2b84b@0.85"), "{f}");
    assert!(f.contains("x='W*min(1,(20+t)/60)-w'"), "{f}");
    assert!(f.contains("shortest=1"), "{f}");
    let live = overlay_filter(&settings(), &ctx(None));
    assert!(!live.contains("0xf2b84b"), "{live}");
    let mut off = settings();
    off.progress = false;
    assert!(!overlay_filter(&off, &ctx(Some(60.0))).contains("0xf2b84b"));
}

#[test]
fn interval_mode_shows_the_band_for_a_window_every_interval_continuous_always() {
    let mut s = settings();
    s.interval_s = 120;
    let f = overlay_filter(&s, &ctx(Some(60.0)));
    assert!(
        f.contains("enable='lt(mod(t,120),min((w+2000)/180,116))'"),
        "{f}"
    );
    assert!(f.contains("x='w-mod(t,120)*180'"), "{f}");
    s.mode = TickerMode::Continuous;
    let f = overlay_filter(&s, &ctx(Some(60.0)));
    assert!(!f.contains("enable="), "{f}");
    assert!(f.contains("x='w-mod(t*180,w+tw)'"), "{f}");
}

#[test]
fn the_code_shows_with_the_ticker_always_or_never() {
    let mut s = settings();
    s.qr = TickerQr::WithTicker;
    let f = overlay_filter(&s, &ctx(Some(60.0)));
    let qr_part = &f[f.find("movie=").unwrap()..];
    assert!(qr_part.contains("enable='lt(mod(t,60)"), "{f}");
    s.qr = TickerQr::Always;
    let f = overlay_filter(&s, &ctx(Some(60.0)));
    let qr_part = &f[f.find("movie=").unwrap()..];
    assert!(!qr_part.contains("enable="), "{f}");
    s.qr = TickerQr::Off;
    assert!(!overlay_filter(&s, &ctx(Some(60.0))).contains("movie="));
    // No public address, no code.
    let mut c = ctx(Some(60.0));
    c.qr = None;
    s.qr = TickerQr::Always;
    assert!(!overlay_filter(&s, &c).contains("movie="));
}

#[test]
fn sizes_follow_the_frame_height() {
    let mut c = ctx(Some(60.0));
    c.frame_height = 720;
    let f = overlay_filter(&settings(), &c);
    assert!(f.contains("fontsize=24:"), "{f}");
    assert!(f.contains("x='w-mod(t,60)*120'"), "{f}");
}

#[test]
fn an_appearance_is_prepared_once_the_previous_one_has_scrolled_away() {
    let schedule = Schedule {
        mode: TickerMode::Interval,
        interval_s: 60.0,
        prepare_after_s: 20.0,
    };
    assert_eq!(schedule.appearance(None), 0);
    assert_eq!(schedule.appearance(Some(0.0)), 0);
    assert_eq!(schedule.appearance(Some(19.9)), 0);
    assert_eq!(schedule.appearance(Some(20.0)), 1);
    assert_eq!(schedule.appearance(Some(61.0)), 1);
    assert_eq!(schedule.appearance(Some(85.0)), 2);
    let continuous = Schedule {
        mode: TickerMode::Continuous,
        ..schedule
    };
    assert_eq!(continuous.appearance(Some(500.0)), 0);
}

#[test]
fn a_slow_conversion_warns_after_30_s_and_clears_when_it_keeps_up() {
    let mut w = SlowWatch::default();
    assert_eq!(w.observe(0, Some(0.9)), None);
    assert_eq!(w.observe(29_999, Some(0.94)), None);
    assert_eq!(w.observe(30_000, Some(0.94)), Some(true));
    assert!(w.is_slow());
    assert_eq!(w.observe(31_000, Some(0.9)), None);
    assert_eq!(w.observe(32_000, None), None, "no report: no change");
    assert_eq!(w.observe(33_000, Some(1.2)), Some(false));
    // A short dip does not count.
    assert_eq!(w.observe(40_000, Some(0.5)), None);
    assert_eq!(w.observe(50_000, Some(1.5)), None);
    assert_eq!(w.observe(75_000, Some(0.5)), None);
    assert_eq!(w.observe(104_999, Some(0.5)), None);
    assert_eq!(w.observe(105_000, Some(0.5)), Some(true));
}

/// A clip with the ticker is read at real time (`-readrate 1.0`), so a speed just under 1.0×
/// is normal and not a warning; only below 0.95× counts.
#[test]
fn a_conversion_at_about_real_time_is_not_slow() {
    let mut w = SlowWatch::default();
    for s in 0..=120 {
        assert_eq!(w.observe(s * 1000, Some(0.96)), None, "{s} s");
    }
    assert_eq!(w.observe(121_000, Some(0.95)), None);
    assert!(!w.is_slow());
}

#[test]
fn the_text_file_keeps_its_size_and_a_shorter_text_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEXT_FILE);
    TickerText::write_in_place(&path, "drugi, dłuższy tekst").unwrap();
    assert_eq!(TickerText::read(&path).unwrap(), "drugi, dłuższy tekst");
    TickerText::write_in_place(&path, "krótki").unwrap();
    assert_eq!(TickerText::read(&path).unwrap(), "krótki");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), TEXT_FILE_BYTES);
    assert!(bytes["krótki".len()..].iter().all(|&b| b == 0));
    // Nothing else in the folder (no temporary file).
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [TEXT_FILE]);
}

#[test]
fn a_text_too_long_for_the_file_is_cut_at_a_character() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEXT_FILE);
    let long = "ż".repeat(TEXT_FILE_BYTES);
    TickerText::write_in_place(&path, &long).unwrap();
    let read = TickerText::read(&path).unwrap();
    assert!(read.len() < TEXT_FILE_BYTES);
    assert!(read.chars().all(|c| c == 'ż'));
    assert_eq!(std::fs::read(&path).unwrap().len(), TEXT_FILE_BYTES);
    // A NUL inside the text would end it early for ffmpeg: it is left out.
    TickerText::write_in_place(&path, "a\0b").unwrap();
    assert_eq!(TickerText::read(&path).unwrap(), "ab");
}

/// ffmpeg opens the file with read and write sharing (`_wopen`) and maps it; a new text is
/// written while it holds it, without waiting.
#[cfg(windows)]
#[test]
fn a_reader_holding_the_file_does_not_hold_up_a_new_text() {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEXT_FILE);
    TickerText::write_in_place(&path, "stary").unwrap();
    let mut reader = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&path)
        .unwrap();
    TickerText::write_in_place(&path, "nowy").unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert!(bytes.starts_with(b"nowy\0"));
    drop(reader);
    assert_eq!(TickerText::read(&path).unwrap(), "nowy");
}

// ---------- With the real ffmpeg (skipped when bin/ffmpeg or the font is missing) ----------

mod drawn {
    use super::super::*;
    use crate::config::VideoEncoder;
    use crate::pipeline::ffmpeg_args::{ClipArgsOptions, build_clip_args};
    use crate::pipeline::probe::{VideoInfo, VideoMode};
    use crate::pipeline::segmenter::{ClipSegmenter, SegmenterState};
    use crate::pipeline::slates::qr_image;
    use crate::resolver::types::StreamInput;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const W: usize = 1920;
    const H: usize = 1080;
    const ADDRESS: &str = "https://kanal.example.com/";
    /// Rows of the band (1080p: 972..1032), a little inside its edges.
    const BAND_ROWS: std::ops::Range<usize> = 976..1028;

    fn ffmpeg() -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
        ["ffmpeg.exe", "ffmpeg"]
            .iter()
            .map(|name| root.join("bin").join(name))
            .find(|p| p.exists())
    }

    fn font() -> PathBuf {
        if cfg!(windows) {
            "C:/Windows/Fonts/arial.ttf".into()
        } else {
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into()
        }
    }

    fn tools() -> Option<PathBuf> {
        let ffmpeg = ffmpeg()?;
        if !font().is_file() {
            return None;
        }
        Some(ffmpeg)
    }

    /// A clip folder with the ticker's files, as the clip factory makes it.
    struct Clip {
        _dir: tempfile::TempDir,
        dir: PathBuf,
    }

    fn clip(text: &str, with_qr: bool) -> (Clip, Option<u32>) {
        let root = tempfile::tempdir().unwrap();
        let fonts = root.path().join(FONTS_DIR);
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::copy(font(), fonts.join("arial.ttf")).unwrap();
        let dir = root.path().join("clip");
        std::fs::create_dir_all(&dir).unwrap();
        TickerText::write_in_place(&dir.join(TEXT_FILE), text).unwrap();
        TickerText::write_in_place(&dir.join(CAPTION_FILE), "Add a video").unwrap();
        let qr = with_qr.then(|| {
            let (png, size) = qr_image(ADDRESS, QR_PX as u32).unwrap();
            std::fs::write(dir.join(QR_FILE), png).unwrap();
            size
        });
        (Clip { _dir: root, dir }, qr)
    }

    /// Starts ffmpeg drawing the ticker on `seconds` of a mid-grey 1080p30 picture
    /// (`realtime`: at the pace it plays, as a converted film is read).
    fn start(
        c: &Clip,
        settings: &TickerSettings,
        qr: Option<u32>,
        duration_s: Option<f64>,
        start_at_s: f64,
        seconds: u32,
        realtime: bool,
    ) -> ClipSegmenter {
        let overlay = overlay_filter(
            settings,
            &OverlayContext {
                duration_s,
                start_at_s,
                text_file: TEXT_FILE,
                font: "../_fonts/arial.ttf",
                qr: qr.map(|size_px| QrOverlay {
                    png: QR_FILE,
                    size_px,
                    caption_file: CAPTION_FILE,
                }),
                frame_height: 1080,
                text_budget_px: window_budget_px(
                    settings,
                    &[TickerText::read(&c.dir.join(TEXT_FILE)).unwrap()],
                ),
            },
        );
        let pace = if realtime { ",realtime" } else { "" };
        let args = build_clip_args(&ClipArgsOptions {
            inputs: vec![StreamInput {
                url: format!("color=c=gray:s=1920x1080:r=30:d={seconds}{pace}"),
                headers: vec![],
            }],
            input_format: Some("lavfi".into()),
            mode: VideoMode::Transcode,
            info: VideoInfo {
                codec: "rawvideo".into(),
                height: 1080,
                pix_fmt: "yuv420p".into(),
                fps: 30.0,
                has_audio: false,
                duration_s: Some(f64::from(seconds)),
                portrait: false,
                keyframe_gap_s: Some(1.0),
                ..VideoInfo::default()
            },
            encoder: VideoEncoder::Libx264,
            segment_seconds: 4,
            out_dir: c.dir.clone(),
            start_at_s: 0.0,
            readrate: None,
            overlay: Some(overlay),
            duration_limit_s: None,
        });
        ClipSegmenter::start(&ffmpeg().unwrap(), args, c.dir.clone(), Some(c.dir.clone()))
    }

    /// Waits for ffmpeg and joins its segments into one file.
    async fn finish(c: &Clip, seg: &ClipSegmenter) -> PathBuf {
        assert_eq!(seg.wait().await, SegmenterState::Done, "{:?}", seg.error());
        let joined = c.dir.join("joined.ts");
        let mut bytes = Vec::new();
        for s in seg.poll() {
            bytes.extend(std::fs::read(&s.path).unwrap());
        }
        assert!(!bytes.is_empty());
        std::fs::write(&joined, bytes).unwrap();
        joined
    }

    /// RGB frame at `at_s` seconds.
    fn frame(joined: &Path, at_s: f64) -> Vec<u8> {
        let out = std::process::Command::new(ffmpeg().unwrap())
            .args(["-v", "error", "-i"])
            .arg(joined)
            .args(["-ss", &at_s.to_string(), "-frames:v", "1"])
            .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
            .output()
            .unwrap();
        assert!(
            out.status.success() && out.stdout.len() == W * H * 3,
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn rgb(f: &[u8], x: usize, y: usize) -> (i32, i32, i32) {
        let i = (y * W + x) * 3;
        (i32::from(f[i]), i32::from(f[i + 1]), i32::from(f[i + 2]))
    }

    fn luma(f: &[u8], x: usize, y: usize) -> i32 {
        let (r, g, b) = rgb(f, x, y);
        (r * 299 + g * 587 + b * 114) / 1000
    }

    /// The band darkens the grey picture (black at 55 %).
    fn band_visible(f: &[u8]) -> bool {
        let mut dark = 0;
        let mut all = 0;
        for y in BAND_ROWS.step_by(4) {
            for x in (0..W).step_by(8) {
                all += 1;
                if luma(f, x, y) < 90 {
                    dark += 1;
                }
            }
        }
        dark * 2 > all
    }

    /// Last column of the bottom row in the accent colour (`None`: no progress line).
    fn progress_end(f: &[u8]) -> Option<usize> {
        (0..W).rev().find(|&x| {
            let (r, _, b) = rgb(f, x, H - 3);
            r - b > 80
        })
    }

    fn qr_codes(f: &[u8]) -> Vec<String> {
        // The bottom-right corner above the band.
        let (x0, y0, w, h) = (W - 420, 560, 420, 420);
        let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
            luma(f, x0 + x, y0 + y).clamp(0, 255) as u8
        });
        prepared
            .detect_grids()
            .into_iter()
            .filter_map(|g| g.decode().ok().map(|(_, text)| text))
            .collect()
    }

    /// Bright (text) pixels of each frame's band, for every frame of `joined`.
    fn band_text_per_frame(joined: &Path) -> Vec<usize> {
        let rows = BAND_ROWS.len();
        let out = std::process::Command::new(ffmpeg().unwrap())
            .args(["-v", "error", "-i"])
            .arg(joined)
            .args(["-vf", &format!("crop={W}:{rows}:0:{}", BAND_ROWS.start)])
            .args(["-pix_fmt", "gray", "-f", "rawvideo", "-"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
            .chunks(W * rows)
            .map(|f| f.iter().filter(|&&v| v > 200).count())
            .collect()
    }

    /// Interval mode: the band (with the code) shows at the start of the interval and is
    /// gone half a minute later; the progress line ends at `(start + t) / duration` of the
    /// width.
    #[tokio::test(flavor = "multi_thread")]
    async fn interval_mode_shows_the_band_code_and_progress_where_expected() {
        let Some(_) = tools() else {
            eprintln!("skip: no ffmpeg or font");
            return;
        };
        let settings = TickerSettings {
            enabled: true,
            ..TickerSettings::default()
        };
        let (c, qr) = clip("► Now: Test   ·   added by Host", true);
        let seg = start(&c, &settings, qr, Some(60.0), 20.0, 31, false);
        let joined = finish(&c, &seg).await;

        let early = frame(&joined, 1.0);
        assert!(band_visible(&early));
        assert_eq!(qr_codes(&early), [ADDRESS]);
        let end = progress_end(&early).expect("progress line");
        let expected = (20.0 + 1.0) / 60.0 * W as f64;
        assert!((end as f64 - expected).abs() <= 4.0, "{end} vs {expected}");

        let late = frame(&joined, 30.0);
        assert!(!band_visible(&late));
        assert!(
            qr_codes(&late).is_empty(),
            "the code shows only with the band"
        );
        let end = progress_end(&late).expect("progress line");
        let expected = (20.0 + 30.0) / 60.0 * W as f64;
        assert!((end as f64 - expected).abs() <= 4.0, "{end} vs {expected}");
    }

    /// Continuous mode: the band is always there; a live stream has no progress line; the
    /// code set to "always" shows too.
    #[tokio::test(flavor = "multi_thread")]
    async fn continuous_mode_keeps_the_band_and_a_live_stream_has_no_progress_line() {
        let Some(_) = tools() else {
            eprintln!("skip: no ffmpeg or font");
            return;
        };
        let settings = TickerSettings {
            enabled: true,
            mode: TickerMode::Continuous,
            qr: TickerQr::Always,
            ..TickerSettings::default()
        };
        let (c, qr) = clip("► Now: Live   ·   added by Host", true);
        let seg = start(&c, &settings, qr, None, 0.0, 11, false);
        let joined = finish(&c, &seg).await;
        for at in [1.0, 5.0, 10.0] {
            let f = frame(&joined, at);
            assert!(band_visible(&f), "{at}");
            assert_eq!(progress_end(&f), None, "{at}");
            assert_eq!(qr_codes(&f), [ADDRESS], "{at}");
        }
    }

    /// A new text written while ffmpeg runs shows in a later frame (`reload=1`).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_text_shows_in_a_later_frame() {
        let Some(_) = tools() else {
            eprintln!("skip: no ffmpeg or font");
            return;
        };
        let settings = TickerSettings {
            enabled: true,
            mode: TickerMode::Continuous,
            speed: TickerSpeed::Fast,
            qr: TickerQr::Off,
            ..TickerSettings::default()
        };
        // Dense letters, then a single dot: the band's bright pixels drop.
        let dense = "W".repeat(120);
        let sparse = ".".to_string();
        let (c, _) = clip(&dense, false);
        let seg = start(&c, &settings, None, None, 0.0, 9, true);
        // Written once ffmpeg's output is past 3 s (its filters are ahead of that) and
        // well before the end (it runs at the pace the picture plays).
        let deadline = Instant::now() + Duration::from_secs(60);
        while seg.out_time_s().is_none_or(|t| t < 3.0) {
            assert!(Instant::now() < deadline, "{:?}", seg.error());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let written_at = seg.out_time_s().unwrap();
        TickerText::write_in_place(&c.dir.join(TEXT_FILE), &sparse).unwrap();
        let joined = finish(&c, &seg).await;
        assert!(written_at < 7.0, "the change came too late: {written_at}");
        let bright = band_text_per_frame(&joined);
        // 30 frames per second; the text has crossed the whole width after 2 s.
        let before = bright[60..90].iter().min().copied().unwrap();
        let after = bright[bright.len() - 30..].iter().max().copied().unwrap();
        assert!(before > 4 * after.max(1), "before {before}, after {after}");
    }

    /// Review Focus 2: the text file replaced 100 times while ffmpeg reads it every frame:
    /// ffmpeg never fails and no frame has an empty band.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_hundred_rewrites_while_ffmpeg_reads_never_leave_an_empty_band() {
        let Some(_) = tools() else {
            eprintln!("skip: no ffmpeg or font");
            return;
        };
        let settings = TickerSettings {
            enabled: true,
            mode: TickerMode::Continuous,
            speed: TickerSpeed::Fast,
            qr: TickerQr::Off,
            ..TickerSettings::default()
        };
        let a = format!("► Now: {}", "Ala ma kota ".repeat(20));
        let b = format!("► Teraz: {}", "Kot ma Alę ".repeat(20));
        let (c, _) = clip(&a, false);
        let seg = start(&c, &settings, None, None, 0.0, 10, true);
        let path = c.dir.join(TEXT_FILE);
        let writer = std::thread::spawn(move || {
            for i in 0..100 {
                let text = if i % 2 == 0 { &b } else { &a };
                TickerText::write_in_place(&path, text).unwrap();
                std::thread::sleep(Duration::from_millis(70));
            }
        });
        let joined = finish(&c, &seg).await;
        writer.join().unwrap();
        let bright = band_text_per_frame(&joined);
        assert!(bright.len() >= 290, "{} frames", bright.len());
        // From 0.5 s on the text has entered the frame.
        let empty: Vec<usize> = (15..bright.len()).filter(|&i| bright[i] < 30).collect();
        assert!(empty.is_empty(), "frames with an empty band: {empty:?}");
    }
}
