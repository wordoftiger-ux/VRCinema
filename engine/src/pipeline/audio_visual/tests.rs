use super::*;
use serde_json::json;
use std::path::PathBuf;

use crate::config::VideoEncoder;
use crate::pipeline::ffmpeg_args::{ClipArgsOptions, ClipExtras, build_clip_args_ext};
use crate::pipeline::probe::{VideoMode, probe_video};

#[test]
fn the_setting_is_one_of_five_looks_and_auto_by_default() {
    assert_eq!(AudioSettings::default().audio_visual, AudioLook::Auto);
    let mut patch = AudioPatch::default();
    assert!(
        sanitize_key("audioVisual", &json!("spectrum"), &mut patch)
            .unwrap()
            .is_ok()
    );
    assert_eq!(patch.visual, Some(AudioLook::Spectrum));
    assert_eq!(
        patch.entries(),
        [("audioVisual", "\"spectrum\"".to_string())]
    );
    let err = sanitize_key("audioVisual", &json!("bars"), &mut patch)
        .unwrap()
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "audioVisual: expected one of: auto, cover, waveform, spectrum, off"
    );
    assert!(sanitize_key("slateFrameStyle", &json!("dim"), &mut patch).is_none());

    let mut s = AudioSettings::default();
    assert!(s.read_stored("audioVisual", &json!("off")));
    assert_eq!(s.audio_visual, AudioLook::Off);
    assert!(s.read_stored("audioVisual", &json!(7)));
    assert_eq!(
        s.audio_visual,
        AudioLook::Off,
        "a wrong value keeps the last one"
    );
    assert!(!s.read_stored("other", &json!("off")));
    assert_eq!(
        serde_json::to_value(s).unwrap(),
        json!({"audioVisual": "off"})
    );
}

#[test]
fn the_setting_is_saved_and_read_back_with_the_channel_settings() {
    use crate::db::{MAIN_CHANNEL_ID, SettingsRepo, open_database, sanitize_settings_patch};
    let conn = open_database(Path::new(":memory:")).unwrap();
    let repo = SettingsRepo::new(&conn);
    assert_eq!(
        repo.get(MAIN_CHANNEL_ID).unwrap().audio.audio_visual,
        AudioLook::Auto
    );
    let patch = sanitize_settings_patch(&json!({"audioVisual": "cover"})).unwrap();
    let saved = repo.update(MAIN_CHANNEL_ID, &patch).unwrap();
    assert_eq!(saved.audio.audio_visual, AudioLook::Cover);
    assert_eq!(
        repo.get(MAIN_CHANNEL_ID).unwrap().audio.audio_visual,
        AudioLook::Cover
    );
    assert!(sanitize_settings_patch(&json!({"audioVisual": "x"})).is_err());
}

#[test]
fn auto_shows_the_cover_when_there_is_one_else_the_waveform() {
    use AudioLook::*;
    assert_eq!(picture_for(Auto, true), Picture::Cover);
    assert_eq!(picture_for(Auto, false), Picture::Waveform);
    assert_eq!(picture_for(Cover, true), Picture::Cover);
    assert_eq!(picture_for(Cover, false), Picture::Plain);
    assert_eq!(picture_for(Waveform, true), Picture::Waveform);
    assert_eq!(picture_for(Spectrum, true), Picture::Spectrum);
    assert_eq!(picture_for(Off, true), Picture::Plain);
    assert!(wants_cover(Auto) && wants_cover(Cover));
    assert!(!wants_cover(Waveform) && !wants_cover(Spectrum) && !wants_cover(Off));
}

#[test]
fn the_canvas_is_16_by_9_with_even_sides() {
    assert_eq!(canvas(1080), (1920, 1080));
    assert_eq!(canvas(720), (1280, 720));
    assert_eq!(canvas(480), (854, 480));
    assert_eq!(canvas(4000), (1920, 1080));
    assert_eq!(canvas(0), (256, 144));
}

#[test]
fn long_titles_wrap_and_end_with_an_ellipsis() {
    // 0.6 em per Latin character: 10 characters of 10 px fit in 60 px.
    assert_eq!(wrap("ab cd ef", 60.0, 10, 2), ["ab cd ef"]);
    assert_eq!(wrap("abcd efgh ijkl", 60.0, 10, 3), ["abcd efgh", "ijkl"]);
    assert_eq!(
        wrap("abcd efgh ijkl", 50.0, 10, 3),
        ["abcd", "efgh", "ijkl"]
    );
    assert_eq!(wrap("abcd efgh ijkl", 50.0, 10, 2), ["abcd", "efgh…"]);
    // A word longer than a line, and Japanese without spaces, are cut between characters.
    assert_eq!(wrap("abcdefghijklmn", 60.0, 10, 2), ["abcdefghij", "klmn"]);
    assert_eq!(
        wrap("あいうえおかきくけこ", 30.0, 10, 2),
        ["あいう", "えお…"]
    );
    assert!(wrap("  ", 60.0, 10, 2).is_empty());
}

#[test]
fn the_layout_places_title_and_artist_for_every_picture() {
    let plain = layout(Picture::Plain, (1920, 1080), "Song", "Band");
    assert_eq!(plain.len(), 2);
    assert_eq!(
        (plain[0].file.as_str(), plain[0].x),
        ("av-title-0.txt", None)
    );
    assert_eq!(
        (plain[1].file.as_str(), plain[1].color),
        ("av-artist-0.txt", ARTIST_COLOR)
    );
    assert!(plain[0].y < 540 && plain[1].y > 540, "{plain:?}");

    let waves = layout(Picture::Waveform, (1280, 720), "Song", "");
    assert_eq!(waves.len(), 1, "no artist line without an artist");
    assert_eq!(waves[0].y, 73);
    assert_eq!(waves[0].size, 43);

    let cover = layout(Picture::Cover, (1920, 1080), "Song", "Band");
    let x = cover[0].x.unwrap();
    assert!(x > 172 + 670 && x < 1100, "{cover:?}");
    assert_eq!(cover[1].x, Some(x));
}

#[test]
fn the_graphs_draw_from_files_in_the_clip_folder() {
    let lines = layout(Picture::Cover, (1920, 1080), "A%{b}", "C");
    let still = still_graph(Picture::Cover, (1920, 1080), "../_fonts/a.ttf", &lines);
    assert!(
        still.starts_with("movie=cover.png,split[av_c1][av_c2];"),
        "{still}"
    );
    assert!(still.contains(
        ",drawtext=fontfile=../_fonts/a.ttf:textfile=av-title-0.txt:expansion=none:fontsize=60:"
    ));
    assert!(!still.contains("A%{b}"), "texts only through files");
    assert!(still.ends_with(",format=rgb24"));
    let plain = still_graph(Picture::Plain, (1280, 720), "f.ttf", &[]);
    assert_eq!(plain, "color=c=0x14161c:s=1280x720:d=1,format=rgb24");

    assert_eq!(
        clip_graph(Picture::Waveform, (1920, 1080), true),
        "movie=bg.png,format=yuv420p,loop=loop=-1:size=1,fps=30[av_bg];\
         [0:a:0]aformat=channel_layouts=mono,showwaves=s=1920x410:mode=cline:rate=30:\
         colors=0xf2b84b:scale=sqrt:draw=full[av_v];[av_bg][av_v]overlay=x=0:y=540:shortest=1,\
         format=yuv420p"
    );
    let spectrum = clip_graph(Picture::Spectrum, (1280, 720), false);
    assert!(spectrum.starts_with("color=c=0x14161c:s=1280x720:r=30,format=yuv420p[av_bg];"));
    assert!(spectrum.contains("showfreqs=s=1074x274:mode=bar:fscale=log:rate=30"));
    let cover = clip_graph(Picture::Cover, (1920, 1080), true);
    assert!(
        cover.contains(
            "[0:a:0]showwaves=s=2x2:rate=30[av_v];[av_bg][av_v]overlay=x=W:y=H:shortest=1"
        )
    );
}

#[test]
fn the_cover_is_read_from_its_stream_or_the_thumbnail() {
    let strs = |a: Vec<OsString>| -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    };
    let input = StreamInput {
        url: "https://cdn.example/a.mp3".into(),
        headers: vec![("Referer".into(), "https://x".into())],
    };
    let embedded = strs(cover_args(&CoverSource::Embedded {
        input: input.clone(),
        stream: 2,
    }));
    let at = embedded.iter().position(|a| a == "-map").unwrap();
    assert_eq!(embedded[at + 1], "0:2");
    assert!(embedded.contains(&"-headers".to_string()));
    assert!(embedded.contains(&"-reconnect".to_string()));
    assert_eq!(embedded.last().map(String::as_str), Some(COVER_FILE));
    let thumb = strs(cover_args(&CoverSource::Thumbnail(input)));
    let at = thumb.iter().position(|a| a == "-map").unwrap();
    assert_eq!(thumb[at + 1], "0:v:0");
}

// ---- real ffmpeg ----

fn bin(name: &str) -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
    [format!("{name}.exe"), name.to_string()]
        .iter()
        .map(|n| root.join("bin").join(n))
        .find(|p| p.exists())
}

fn system_font() -> Option<PathBuf> {
    [
        "C:/Windows/Fonts/arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

async fn run(program: &Path, args: &[&str], cwd: &Path) -> Vec<u8> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    run_tool(program, args, Some(cwd), Duration::from_secs(90))
        .await
        .unwrap_or_else(|e| panic!("{}: {e}", program.display()))
}

/// A tagged 4 s mono mp3 at 44.1 kHz with a cover picture.
async fn make_song(ffmpeg: &Path, dir: &Path) -> PathBuf {
    run(
        ffmpeg,
        &[
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x320",
            "-frames:v",
            "1",
            "art.png",
        ],
        dir,
    )
    .await;
    run(
        ffmpeg,
        &[
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=4:sample_rate=44100",
            "-i",
            "art.png",
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:v",
            "mjpeg",
            "-disposition:v",
            "attached_pic",
            "-metadata",
            "title=Żółta piosenka o bardzo długim tytule, który się nie mieści",
            "-metadata",
            "artist=Zespół",
            "-c:a",
            "libmp3lame",
            "song.mp3",
        ],
        dir,
    )
    .await;
    dir.join("song.mp3")
}

/// The segments of `out` (from its list): each starts with a key frame and holds H.264 at
/// `height` lines and AAC 48 kHz stereo. Returns their count.
async fn check_segments(ffprobe: &Path, out: &Path, height: u32) -> usize {
    let list = std::fs::read_to_string(out.join("list.csv")).unwrap();
    let names: Vec<&str> = list.lines().filter_map(|l| l.split(',').next()).collect();
    for name in &names {
        let streams = run(
            ffprobe,
            &[
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name,height,sample_rate,channels",
                "-of",
                "csv=p=0",
                name,
            ],
            out,
        )
        .await;
        let streams = String::from_utf8_lossy(&streams).replace("\r", "");
        assert!(
            streams.contains(&format!("h264,{height}")),
            "{name}: {streams}"
        );
        assert!(streams.contains("aac,48000,2"), "{name}: {streams}");
        let first = run(
            ffprobe,
            &[
                "-v",
                "error",
                "-select_streams",
                "v",
                "-show_entries",
                "frame=key_frame",
                "-read_intervals",
                "%+#1",
                "-of",
                "csv=p=0",
                name,
            ],
            out,
        )
        .await;
        assert!(
            String::from_utf8_lossy(&first).starts_with('1'),
            "{name} does not start with a key frame"
        );
    }
    names.len()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_look_airs_as_one_second_segments_with_key_frames_and_uniform_sound() {
    let (Some(ffmpeg), Some(ffprobe)) = (bin("ffmpeg"), bin("ffprobe")) else {
        eprintln!("skip: bin/ffmpeg or bin/ffprobe missing");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let song = make_song(&ffmpeg, dir.path()).await;
    let input = StreamInput {
        url: song.to_string_lossy().into_owned(),
        headers: vec![],
    };
    let info = probe_video(&ffprobe, std::slice::from_ref(&input))
        .await
        .unwrap();
    let tags = info.audio.clone().expect("audio-only");
    assert_eq!(tags.artist, "Zespół");
    assert_eq!(tags.cover_stream, Some(1));
    assert!(
        info.duration_s.is_some_and(|d| (d - 4.0).abs() < 0.2),
        "{info:?}"
    );
    let font = system_font().map(|f| {
        std::fs::copy(&f, dir.path().join("font.ttf")).unwrap();
        "../font.ttf"
    });

    for (n, (look, expected, start_at_s)) in [
        (AudioLook::Auto, Picture::Cover, 0.0),
        (AudioLook::Waveform, Picture::Waveform, 1.0),
        (AudioLook::Spectrum, Picture::Spectrum, 0.0),
        (AudioLook::Off, Picture::Plain, 0.0),
    ]
    .into_iter()
    .enumerate()
    {
        let out = dir.path().join(format!("clip-{n}"));
        let visual = prepare(VisualJob {
            ffmpeg: &ffmpeg,
            out_dir: &out,
            look,
            height: 720,
            title: &tags.title,
            artist: &tags.artist,
            font,
            cover: Some(CoverSource::Embedded {
                input: input.clone(),
                stream: 1,
            }),
        })
        .await;
        assert_eq!(visual.picture, expected, "{look:?}: {:?}", visual.problems);
        assert!(visual.problems.is_empty(), "{:?}", visual.problems);
        assert!(out.join(STILL_FILE).is_file());
        let args = build_clip_args_ext(
            &ClipArgsOptions {
                inputs: vec![input.clone()],
                input_format: None,
                mode: VideoMode::Transcode,
                info: info.clone(),
                encoder: VideoEncoder::Libx264,
                segment_seconds: 1,
                out_dir: out.clone(),
                start_at_s,
                readrate: None,
                overlay: None,
                duration_limit_s: None,
            },
            &ClipExtras {
                max_height: Some(720),
                fade_in_s: Some(0.5),
                fade_out: Some((3.0, 0.5)),
                loudness_target: Some(-16),
                audio_visual: Some(visual.graph.clone()),
                ..ClipExtras::default()
            },
        );
        run_tool(&ffmpeg, args, Some(&out), Duration::from_secs(120))
            .await
            .unwrap_or_else(|e| panic!("{look:?}: {e}"));
        let count = check_segments(&ffprobe, &out, 720).await;
        // 4 s of sound in 1 s segments (3 after a seek to 1 s), give or take the last bit.
        let want = if start_at_s > 0.0 { 3 } else { 4 };
        assert!(
            (want..=want + 1).contains(&count),
            "{look:?}: {count} segments"
        );
        if std::env::var_os("AUDIO_VISUAL_SHOTS").is_some() {
            let shot = format!("shot-{}.png", expected.as_str());
            run(
                &ffmpeg,
                &[
                    "-v",
                    "error",
                    "-y",
                    "-i",
                    "seg_00002.ts",
                    "-frames:v",
                    "1",
                    &shot,
                ],
                &out,
            )
            .await;
            let keep = PathBuf::from(std::env::var_os("AUDIO_VISUAL_SHOTS").unwrap());
            std::fs::copy(out.join(&shot), keep.join(&shot)).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cover_that_cannot_be_read_falls_back_to_the_waveform() {
    let Some(ffmpeg) = bin("ffmpeg") else {
        eprintln!("skip: bin/ffmpeg missing");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("clip");
    let visual = prepare(VisualJob {
        ffmpeg: &ffmpeg,
        out_dir: &out,
        look: AudioLook::Auto,
        height: 1080,
        title: "T",
        artist: "",
        font: None,
        cover: Some(CoverSource::Thumbnail(StreamInput {
            url: dir.path().join("nie-ma.jpg").to_string_lossy().into_owned(),
            headers: vec![],
        })),
    })
    .await;
    assert_eq!(visual.picture, Picture::Waveform);
    assert_eq!(visual.problems.len(), 1);
    assert_eq!(visual.problems[0].0, "log.audio_cover_failed");
    assert!(
        visual.graph.starts_with("movie=bg.png,"),
        "{}",
        visual.graph
    );
    assert!(!visual.plain_graph.contains("movie="));
}
