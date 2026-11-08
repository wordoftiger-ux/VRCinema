use super::list::{parse_dshow, parse_pactl};
use super::*;
use serde_json::json;

/// What ffmpeg 7+ prints for `-list_devices true -f dshow -i dummy` (this computer's listing,
/// shortened): `(video)` / `(audio)` lines, each followed by its alternative name.
const NEW_FORMAT: &str = r#"[in#0 @ 000001f1b6e63d80] "Brio 100" (video)
[in#0 @ 000001f1b6e63d80]   Alternative name "@device_pnp_\\?\usb#vid_046d&pid_094c&mi_00#6&24bd4b6&0&0000#{65e8773d-8f56-11d0-a3b9-00a0c9223196}\global"
[in#0 @ 000001f1b6e63d80] "VTubeStudioCam" (video)
[in#0 @ 000001f1b6e63d80]   Alternative name "@device_sw_{860BB310-5D01-11D0-BD3B-00A0C911CE86}\{5C2CD55C-92AD-4999-8666-912BD3E70010}"
[in#0 @ 000001f1b6e63d80] "Mikrofon (Scarlett Solo USB)" (audio)
[in#0 @ 000001f1b6e63d80]   Alternative name "@device_cm_{33D9A762-90C8-11D0-BD43-00A0C911CE86}\wave_{0A5639D6-701C-4C9C-8ADE-DAF2776026D1}"
[in#0 @ 000001f1b6e63d80] "Mikrofon (Scarlett Solo USB)" (audio)
[in#0 @ 000001f1b6e63d80]   Alternative name "@device_cm_{33D9A762-90C8-11D0-BD43-00A0C911CE86}\wave_{FFFF}"
[in#0 @ 000001f1b6e63d80] "Capture card" (audio, video)
[in#0 @ 000001f1b6e63d80] "Odd one" (none)
Error opening input file dummy.
"#;

/// What older ffmpeg builds print: sections, bare names.
const OLD_FORMAT: &str = r#"[dshow @ 0000020b] DirectShow video devices (some may be both video and audio devices)
[dshow @ 0000020b]  "Integrated Camera"
[dshow @ 0000020b]     Alternative name "@device_pnp_\\?\usb#vid_04f2"
[dshow @ 0000020b] DirectShow audio devices
[dshow @ 0000020b]  "Microphone Array (Realtek(R) Audio)"
[dshow @ 0000020b]     Alternative name "@device_cm_{33D9A762}\wave_{1}"
[dshow @ 0000020b]  "Stereo Mix (Realtek(R) Audio)"
dummy: Immediate exit requested
"#;

#[test]
fn the_dshow_listing_gives_cameras_and_sound_devices_by_their_alternative_names() {
    let (cameras, audio) = parse_dshow(NEW_FORMAT);
    let names: Vec<&str> = cameras.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["Brio 100", "VTubeStudioCam", "Capture card"]);
    assert!(cameras[0].id.starts_with(r"@device_pnp_\\?\usb#vid_046d"));
    assert_eq!(
        cameras[2].id, "Capture card",
        "no alternative name: the name"
    );
    let names: Vec<&str> = audio.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Mikrofon (Scarlett Solo USB)",
            "Mikrofon (Scarlett Solo USB)",
            "Capture card"
        ],
        "two devices of one name stay apart by their ids"
    );
    assert_ne!(audio[0].id, audio[1].id);

    let (cameras, audio) = parse_dshow(OLD_FORMAT);
    assert_eq!(cameras.len(), 1);
    assert_eq!(cameras[0].name, "Integrated Camera");
    assert_eq!(cameras[0].id, r"@device_pnp_\\?\usb#vid_04f2");
    let names: Vec<&str> = audio.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Microphone Array (Realtek(R) Audio)",
            "Stereo Mix (Realtek(R) Audio)"
        ]
    );
    assert_eq!(audio[1].id, "Stereo Mix (Realtek(R) Audio)");

    assert_eq!(parse_dshow(""), (vec![], vec![]));
    assert_eq!(parse_dshow("garbage\n\"unterminated"), (vec![], vec![]));
}

#[test]
fn pulse_sources_are_read_from_pactl() {
    let out = "0\talsa_output.pci.analog-stereo.monitor\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n\
               1\talsa_input.usb-mic.mono\tPipeWire\ts16le 1ch 48000Hz\tRUNNING\n\n";
    let list = parse_pactl(out);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "pulse:alsa_output.pci.analog-stereo.monitor");
    assert_eq!(list[1].name, "alsa_input.usb-mic.mono");
}

fn device(id: &str, name: &str) -> Device {
    Device {
        id: id.into(),
        name: name.into(),
    }
}

fn catalog() -> Catalog {
    Catalog {
        platform: "windows",
        desktop: true,
        region: true,
        screens: vec![
            Screen {
                index: 0,
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
                primary: true,
            },
            Screen {
                index: 1,
                x: 2560,
                y: 0,
                width: 1920,
                height: 1080,
                primary: false,
            },
        ],
        windows: vec!["Notatnik — szkic".into()],
        cameras: vec![device("@device_pnp_cam", "Brio 100")],
        audio: vec![device("@device_cm_mic", "Mikrofon")],
        fast: true,
        notes: vec![],
    }
}

#[test]
fn only_what_the_listing_named_can_be_captured() {
    let c = catalog();
    let spec = parse_spec(
        &json!({"kind": "window", "source": "Notatnik — szkic", "audio": "@device_cm_mic"}),
        &c,
    )
    .unwrap();
    assert_eq!(spec.video, Video::Window("Notatnik — szkic".into()));
    assert_eq!(spec.audio, Some(device("@device_cm_mic", "Mikrofon")));
    assert_eq!((spec.fps, spec.height, spec.fast), (30, 1080, false));
    for (body, error) in [
        (
            json!({"kind": "window", "source": "Bank"}),
            SpecError::UnknownSource,
        ),
        (
            json!({"kind": "camera", "source": "@device_pnp_other"}),
            SpecError::UnknownSource,
        ),
        (
            json!({"kind": "screen", "source": "7"}),
            SpecError::UnknownSource,
        ),
        (
            json!({"kind": "desktop", "audio": "@nope"}),
            SpecError::UnknownSource,
        ),
        (json!({"kind": "none"}), SpecError::Invalid),
        (json!({"kind": "desktop", "fps": 60}), SpecError::Invalid),
        (
            json!({"kind": "desktop", "height": 2160}),
            SpecError::Invalid,
        ),
        (
            json!({"kind": "url", "source": "https://x"}),
            SpecError::Invalid,
        ),
        (
            json!({"kind": "region", "region": {"x": 0, "y": 0, "width": 10, "height": 10}}),
            SpecError::Invalid,
        ),
        (json!("desktop"), SpecError::Invalid),
    ] {
        assert_eq!(parse_spec(&body, &c), Err(error), "{body}");
    }
    let screen = parse_spec(
        &json!({"kind": "screen", "source": "1", "fast": true, "fps": 15, "height": 720}),
        &c,
    )
    .unwrap();
    assert!(screen.fast);
    assert_eq!((screen.fps, screen.height), (15, 720));
    // Fast capture is for one screen, and only with an ffmpeg that has ddagrab.
    let desktop = parse_spec(&json!({"kind": "desktop", "fast": true}), &c).unwrap();
    assert!(!desktop.fast);
    let slow = Catalog {
        fast: false,
        ..catalog()
    };
    assert!(
        !parse_spec(
            &json!({"kind": "screen", "source": "1", "fast": true}),
            &slow
        )
        .unwrap()
        .fast
    );
    let none = parse_spec(&json!({"kind": "none", "audio": "@device_cm_mic"}), &c).unwrap();
    assert_eq!(none.video, Video::None);
    // A Linux computer without an X display has no desktop to capture.
    let headless = Catalog {
        desktop: false,
        region: false,
        ..catalog()
    };
    assert_eq!(
        parse_spec(&json!({"kind": "desktop"}), &headless),
        Err(SpecError::UnknownSource)
    );
}

fn spec(video: Video, audio: Option<Device>) -> CaptureSpec {
    CaptureSpec {
        video,
        audio,
        fps: 25,
        height: 720,
        fast: false,
    }
}

fn args_of(c: &CaptureInputs, n: usize) -> Vec<&str> {
    c.input_args[n].iter().map(String::as_str).collect()
}

#[test]
fn windows_inputs_use_gdigrab_ddagrab_and_dshow() {
    let c = capture_inputs(
        &spec(
            Video::Window("Notatnik".into()),
            Some(device("@device_cm_mic", "Mikrofon")),
        ),
        Platform::Windows,
        ":0",
    );
    assert_eq!(c.inputs[0].url, "title=Notatnik");
    assert_eq!(
        args_of(&c, 0),
        ["-f", "gdigrab", "-framerate", "25", "-draw_mouse", "1"]
    );
    assert_eq!(c.inputs[1].url, "audio=@device_cm_mic");
    assert_eq!(args_of(&c, 1), ["-f", "dshow"]);
    assert!(c.has_audio);

    let screen = catalog().screens[1].clone();
    let c = capture_inputs(
        &spec(Video::Screen(screen.clone()), None),
        Platform::Windows,
        ":0",
    );
    assert_eq!(c.inputs.len(), 1);
    assert!(!c.has_audio);
    assert_eq!(c.inputs[0].url, "desktop");
    assert_eq!(
        args_of(&c, 0)[6..],
        [
            "-offset_x",
            "2560",
            "-offset_y",
            "0",
            "-video_size",
            "1920x1080"
        ]
    );
    let fast = CaptureSpec {
        fast: true,
        ..spec(Video::Screen(screen), None)
    };
    let c = capture_inputs(&fast, Platform::Windows, ":0");
    assert_eq!(args_of(&c, 0), ["-f", "lavfi"]);
    assert_eq!(
        c.inputs[0].url,
        "ddagrab=output_idx=1:framerate=25:draw_mouse=1,hwdownload,format=bgra"
    );

    let c = capture_inputs(
        &spec(Video::Camera(device("@device_pnp_cam", "Brio 100")), None),
        Platform::Windows,
        ":0",
    );
    assert_eq!(c.inputs[0].url, "video=@device_pnp_cam");
    assert_eq!(args_of(&c, 0)[..2], ["-f", "dshow"]);

    let c = capture_inputs(
        &spec(Video::None, Some(device("@device_cm_mic", "Mikrofon"))),
        Platform::Windows,
        ":0",
    );
    assert_eq!(args_of(&c, 0), ["-f", "lavfi"]);
    assert_eq!(c.inputs[0].url, "color=c=0x0b1020:s=1280x720:r=25");
}

#[test]
fn linux_inputs_use_x11grab_v4l2_and_pulse_or_alsa() {
    let region = Video::Region {
        x: 100,
        y: 50,
        width: 800,
        height: 600,
    };
    let c = capture_inputs(
        &spec(region, Some(device("pulse:alsa_output.monitor", "x"))),
        Platform::Linux,
        ":1",
    );
    assert_eq!(c.inputs[0].url, ":1+100,50");
    assert_eq!(
        args_of(&c, 0),
        [
            "-f",
            "x11grab",
            "-framerate",
            "25",
            "-draw_mouse",
            "1",
            "-video_size",
            "800x600"
        ]
    );
    assert_eq!(c.inputs[1].url, "alsa_output.monitor");
    assert_eq!(args_of(&c, 1)[..2], ["-f", "pulse"]);

    let c = capture_inputs(
        &spec(
            Video::Desktop,
            Some(device("alsa:default", "default (ALSA)")),
        ),
        Platform::Linux,
        ":0",
    );
    assert_eq!(c.inputs[0].url, ":0");
    assert_eq!(c.inputs[1].url, "default");
    assert_eq!(args_of(&c, 1)[..2], ["-f", "alsa"]);

    let c = capture_inputs(
        &spec(Video::Camera(device("/dev/video0", "UVC")), None),
        Platform::Linux,
        ":0",
    );
    assert_eq!(c.inputs[0].url, "/dev/video0");
    assert_eq!(args_of(&c, 0)[..2], ["-f", "v4l2"]);
}

#[test]
fn the_fit_filter_makes_one_even_16_9_size() {
    assert_eq!(box_width(1080), 1920);
    assert_eq!(box_width(720), 1280);
    assert_eq!(box_width(480), 854);
    assert_eq!(
        fit_filter(720),
        "scale=1280:720:force_original_aspect_ratio=decrease:force_divisible_by=2,\
         pad=1280:720:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1"
    );
}

#[test]
fn window_titles_and_device_names_are_redacted() {
    let s = spec(
        Video::Window("Bank — konto".into()),
        Some(device("@device_cm_{1}\\wave_{2}", "Mikrofon (Brio)")),
    );
    let sensitive = sensitive_texts(&s);
    let text = "[gdigrab] Can't find window 'Bank — konto', aborting. audio=@device_cm_{1}\\wave_{2}: \
                Mikrofon (Brio) busy";
    let out = redact(text, &sensitive);
    assert!(!out.contains("Bank"), "{out}");
    assert!(!out.contains("wave_"), "{out}");
    assert!(!out.contains("Brio"), "{out}");
    assert!(out.contains("Can't find window"));
}

#[test]
fn sources_and_tokens() {
    assert!(is_capture_source("capture:ab12"));
    assert!(is_capture_source("CAPTURE:ab12"));
    assert!(!is_capture_source("https://capture:ab12"));
    assert_eq!(token_of("capture:ab12"), Some("ab12"));
    assert_eq!(token_of("capture:"), None);
    assert_eq!(token_of("upload:x"), None);
}

#[test]
fn a_capture_belongs_to_its_item_until_stop() {
    let registry = Arc::new(CaptureRegistry::new(PathBuf::from("ffmpeg")));
    let s = spec(Video::Desktop, None);
    let token = registry.register(s.clone());
    assert_eq!(token.len(), 32);
    assert_ne!(
        registry.register(s.clone()),
        token,
        "every capture gets its own token"
    );
    assert_eq!(registry.lookup(&token, 5), None, "not bound yet");
    registry.bind(&token, 5);
    assert_eq!(registry.lookup(&token, 5), Some(s.clone()));
    assert_eq!(
        registry.lookup(&token, 6),
        None,
        "a copy of the item finds nothing"
    );
    assert_eq!(registry.lookup("other", 5), None);
    assert_eq!(registry.bound_items(), [5]);

    let run = registry.start_run(5);
    let again = registry.start_run(5);
    assert_eq!(registry.running(), [5]);
    drop(run);
    assert_eq!(registry.running(), [5]);
    drop(again);
    assert!(registry.running().is_empty());

    assert!(registry.is_active(&token));
    assert_eq!(registry.stop_all(), 2);
    assert!(!registry.is_active(&token));
    assert_eq!(registry.lookup(&token, 5), None);

    let t = registry.register(s);
    registry.forget(&t);
    assert!(!registry.is_active(&t));

    assert!(registry.catalog().is_none());
    registry.set_catalog(catalog());
    assert_eq!(registry.catalog(), Some(catalog()));
}

#[test]
fn the_registry_keeps_a_bounded_number_of_captures() {
    let registry = CaptureRegistry::new(PathBuf::from("ffmpeg"));
    let first = registry.register(spec(Video::Desktop, None));
    for _ in 0..MAX_ENTRIES {
        registry.register(spec(Video::Desktop, None));
    }
    assert!(!registry.is_active(&first), "the oldest goes first");
    assert_eq!(registry.lock().entries.len(), MAX_ENTRIES);
}

/// `bin/<name>` of the repository, or of `VRCINEMA_BIN_DIR` (a worktree without the tools).
fn tool(name: &str) -> Option<PathBuf> {
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
    std::env::var_os("VRCINEMA_BIN_DIR")
        .map(PathBuf::from)
        .into_iter()
        .chain([root.join("bin")])
        .map(|dir| dir.join(&exe))
        .find(|p| p.is_file())
}

async fn probe(ffprobe: &Path, file: &Path, entries: &str, select: &str) -> String {
    let mut args: Vec<std::ffi::OsString> = ["-v", "error", "-select_streams", select]
        .iter()
        .map(Into::into)
        .collect();
    args.extend(
        ["-show_entries", entries, "-of", "csv=p=0"]
            .iter()
            .map(Into::into),
    );
    args.push(file.into());
    let out = crate::process::run_process(ffprobe, &args, Duration::from_secs(30))
        .await
        .unwrap();
    out.stdout.trim().to_string()
}

/// Checks the segments a capture made: the box size, a key frame first in each, AAC 48 kHz
/// stereo; returns how many there are.
async fn check_capture_segments(ffprobe: &Path, dir: &Path, width: u32, height: u32) -> usize {
    let mut segments: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ts"))
        .collect();
    segments.sort();
    assert!(!segments.is_empty(), "no segments in {}", dir.display());
    for seg in &segments {
        let video = probe(ffprobe, seg, "stream=codec_name,width,height", "v:0").await;
        assert_eq!(video, format!("h264,{width},{height}"), "{}", seg.display());
        let audio = probe(
            ffprobe,
            seg,
            "stream=codec_name,sample_rate,channels",
            "a:0",
        )
        .await;
        assert_eq!(audio, "aac,48000,2", "{}", seg.display());
        let mut args: Vec<std::ffi::OsString> = [
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "frame=key_frame",
            "-of",
            "csv=p=0",
        ]
        .iter()
        .map(Into::into)
        .collect();
        args.push(seg.into());
        let first = crate::process::run_process(ffprobe, &args, Duration::from_secs(30))
            .await
            .unwrap();
        // `1` (ffprobe may add an empty side-data column: `1,`).
        let key = first
            .stdout
            .lines()
            .next()
            .map(|l| l.trim().trim_end_matches(','));
        assert_eq!(key, Some("1"), "{} starts with a key frame", seg.display());
    }
    segments.len()
}

/// The real thing with stand-in devices: an odd-sized picture (like a window) and a separate
/// sound input become 1 s segments of the 1280×720 box, each starting with a key frame, with
/// AAC 48 kHz stereo — the arguments a capture airs with.
#[tokio::test]
async fn a_capture_is_fitted_into_its_box_with_a_key_frame_every_segment() {
    let (Some(ffmpeg), Some(ffprobe)) = (tool("ffmpeg"), tool("ffprobe")) else {
        eprintln!("skip: bin/ffmpeg missing");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let inputs = CaptureInputs {
        inputs: vec![
            StreamInput {
                url: "testsrc2=size=1001x557:rate=25:duration=3".into(),
                headers: vec![],
            },
            StreamInput {
                url: "sine=frequency=440:sample_rate=44100:duration=3".into(),
                headers: vec![],
            },
        ],
        input_args: vec![strings(&["-f", "lavfi"]), strings(&["-f", "lavfi"])],
        has_audio: true,
    };
    let args = crate::pipeline::clip_factory::capture_clip_args(
        inputs,
        25,
        720,
        crate::config::VideoEncoder::Libx264,
        false,
        1,
        dir.path().to_path_buf(),
        None,
    );
    let out = crate::process::run_process(&ffmpeg, &args, Duration::from_secs(120))
        .await
        .unwrap();
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let count = check_capture_segments(&ffprobe, dir.path(), 1280, 720).await;
    assert!((3..=4).contains(&count), "{count} segments");
}

/// Opt-in (`VRCINEMA_CAPTURE_TEST=1`, Windows): the real desktop through gdigrab and the first
/// sound device of the DirectShow listing, for about three seconds, into a temporary folder
/// that is removed afterwards (nothing is looked at but the stream properties).
#[cfg(windows)]
#[tokio::test]
async fn the_real_desktop_and_microphone_become_segments() {
    if std::env::var_os("VRCINEMA_CAPTURE_TEST").is_none() {
        eprintln!("skip: set VRCINEMA_CAPTURE_TEST=1 to capture this desktop for 3 s");
        return;
    }
    let (Some(ffmpeg), Some(ffprobe)) = (tool("ffmpeg"), tool("ffprobe")) else {
        eprintln!("skip: bin/ffmpeg missing");
        return;
    };
    let catalog = list::catalog(&ffmpeg).await;
    assert_eq!(catalog.platform, "windows");
    assert!(!catalog.screens.is_empty(), "{catalog:?}");
    assert!(!catalog.windows.is_empty(), "some window is open");
    eprintln!(
        "screens {:?}, {} windows, {} cameras, {} sound devices, ddagrab {}",
        catalog.screens,
        catalog.windows.len(),
        catalog.cameras.len(),
        catalog.audio.len(),
        catalog.fast
    );
    let mut specs = vec![CaptureSpec {
        video: Video::Desktop,
        audio: catalog.audio.first().cloned(),
        fps: 30,
        height: 720,
        fast: false,
    }];
    // The window in front (by its title).
    specs.push(CaptureSpec {
        video: Video::Window(catalog.windows[0].clone()),
        audio: None,
        fps: 30,
        height: 720,
        fast: false,
    });
    // The last screen by its rectangle, and through ddagrab when there is one.
    let screen = catalog.screens.last().cloned().unwrap();
    specs.push(CaptureSpec {
        video: Video::Screen(screen.clone()),
        audio: None,
        fps: 15,
        height: 480,
        fast: false,
    });
    if catalog.fast {
        specs.push(CaptureSpec {
            video: Video::Screen(screen),
            audio: None,
            fps: 30,
            height: 1080,
            fast: true,
        });
    }
    for spec in specs {
        let dir = tempfile::tempdir().unwrap();
        let inputs = capture_inputs(&spec, Platform::Windows, ":0");
        let args = crate::pipeline::clip_factory::capture_clip_args(
            inputs,
            spec.fps,
            spec.height,
            crate::config::VideoEncoder::Libx264,
            false,
            1,
            dir.path().to_path_buf(),
            None,
        );
        // A live input never ends: stopped after a while.
        let result = crate::process::run_process(&ffmpeg, &args, Duration::from_secs(6)).await;
        if let Ok(out) = &result {
            panic!("{spec:?} ended by itself: {}", out.stderr);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        // The segment being written when ffmpeg was stopped is cut short: not checked.
        let mut written: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "ts"))
            .collect();
        written.sort();
        if let Some(last) = written.pop() {
            std::fs::remove_file(last).unwrap();
        }
        let count =
            check_capture_segments(&ffprobe, dir.path(), box_width(spec.height), spec.height).await;
        assert!(count >= 2, "{spec:?}: {count} segments");
        let frame = list::grab_preview(&ffmpeg, &spec).await;
        assert!(
            frame
                .as_ref()
                .is_ok_and(|jpeg| jpeg.starts_with(&[0xff, 0xd8])),
            "{spec:?}: {:?}",
            frame.err()
        );
    }
}
