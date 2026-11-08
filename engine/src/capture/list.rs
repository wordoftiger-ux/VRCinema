//! What can be captured here (windows, screens, cameras, sound devices) and the preview frame.
//! Windows: `EnumWindows` / `EnumDisplayMonitors` and ffmpeg's DirectShow listing; Linux: the X
//! display, `/sys/class/video4linux` and `pactl` (best effort); elsewhere nothing.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use super::{CaptureSpec, Catalog, Device, Platform, capture_inputs, fit_filter, x_display};

/// The longest a listing by ffmpeg (or pactl) may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
/// The longest a preview frame may take (a camera can be slow to open).
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(15);
/// The width of a preview frame.
const PREVIEW_WIDTH: u32 = 480;

/// Cameras and sound devices in ffmpeg's DirectShow listing (`-list_devices true -f dshow`),
/// both of its formats: `"Name" (video)` / `(audio)` / `(audio, video)` / `(none)` lines, and the
/// older sections "DirectShow video devices" / "DirectShow audio devices" with bare `"Name"`
/// lines. An `Alternative name "…"` line names the device before it; that name is what ffmpeg
/// opens (it tells devices of the same name apart). Devices are listed once each.
pub fn parse_dshow(output: &str) -> (Vec<Device>, Vec<Device>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Video,
        Audio,
        Both,
        None,
    }
    let mut section = Kind::None;
    let mut found: Vec<(Kind, Device)> = Vec::new();
    for raw in output.lines() {
        // `[dshow @ 000001…] text` / `[in#0 @ …] text`
        let line = match raw.trim_start().strip_prefix('[') {
            Some(rest) => rest.split_once("] ").map_or("", |(_, text)| text),
            None => raw,
        }
        .trim();
        if line.contains("DirectShow video devices") {
            section = Kind::Video;
            continue;
        }
        if line.contains("DirectShow audio devices") {
            section = Kind::Audio;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Alternative name") {
            let alt = rest.trim().trim_matches('"');
            if let Some((_, last)) = found.last_mut()
                && !alt.is_empty()
            {
                last.id = alt.to_string();
            }
            continue;
        }
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some(end) = rest.find('"') else {
            continue;
        };
        let name = &rest[..end];
        let tail = rest[end + 1..].trim();
        let kind = match tail {
            "(video)" => Kind::Video,
            "(audio)" => Kind::Audio,
            "(audio, video)" | "(video, audio)" => Kind::Both,
            "" => section,
            _ => Kind::None,
        };
        if name.is_empty() || kind == Kind::None {
            continue;
        }
        found.push((
            kind,
            Device {
                id: name.to_string(),
                name: name.to_string(),
            },
        ));
    }
    let mut cameras: Vec<Device> = Vec::new();
    let mut audio: Vec<Device> = Vec::new();
    for (kind, device) in found {
        if matches!(kind, Kind::Video | Kind::Both) && !cameras.iter().any(|d| d.id == device.id) {
            cameras.push(device.clone());
        }
        if matches!(kind, Kind::Audio | Kind::Both) && !audio.iter().any(|d| d.id == device.id) {
            audio.push(device);
        }
    }
    (cameras, audio)
}

/// ffmpeg has the `ddagrab` filter (Desktop Duplication, Windows 8 and later).
async fn has_ddagrab(ffmpeg: &Path) -> bool {
    let args: Vec<OsString> = ["-hide_banner", "-h", "filter=ddagrab"]
        .iter()
        .map(OsString::from)
        .collect();
    crate::process::run_process(ffmpeg, &args, LIST_TIMEOUT)
        .await
        .is_ok_and(|out| out.stdout.contains("Filter ddagrab"))
}

/// What can be captured on this computer now.
pub async fn catalog(ffmpeg: &Path) -> Catalog {
    match Platform::current() {
        Platform::Windows => windows_catalog(ffmpeg).await,
        Platform::Linux => linux_catalog().await,
        Platform::Other => Catalog {
            platform: "other",
            notes: vec!["unsupported"],
            ..Catalog::default()
        },
    }
}

async fn windows_catalog(ffmpeg: &Path) -> Catalog {
    let args: Vec<OsString> = [
        "-hide_banner",
        "-list_devices",
        "true",
        "-f",
        "dshow",
        "-i",
        "dummy",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let (cameras, audio) = match crate::process::run_process(ffmpeg, &args, LIST_TIMEOUT).await {
        Ok(out) => parse_dshow(&out.stderr),
        Err(_) => (Vec::new(), Vec::new()),
    };
    let (screens, windows) = tokio::task::spawn_blocking(|| (win::screens(), win::windows()))
        .await
        .unwrap_or_default();
    Catalog {
        platform: "windows",
        desktop: true,
        region: true,
        screens,
        windows,
        cameras,
        audio,
        fast: has_ddagrab(ffmpeg).await,
        notes: vec!["loopback"],
    }
}

/// Sound sources of PulseAudio / PipeWire (`pactl list short sources`: index, name, …);
/// `.monitor` sources are what the computer plays.
pub fn parse_pactl(output: &str) -> Vec<Device> {
    output
        .lines()
        .filter_map(|line| line.split('\t').nth(1))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| Device {
            id: format!("pulse:{name}"),
            name: name.to_string(),
        })
        .collect()
}

async fn linux_catalog() -> Catalog {
    let display = std::env::var("DISPLAY")
        .ok()
        .filter(|d| !d.trim().is_empty());
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let mut notes = vec!["no_windows_linux"];
    if wayland {
        notes.push("wayland");
    }
    if display.is_none() {
        notes.push("no_display");
    }
    let args: Vec<OsString> = ["list", "short", "sources"]
        .iter()
        .map(OsString::from)
        .collect();
    let mut audio = match crate::process::run_process(Path::new("pactl"), &args, LIST_TIMEOUT).await
    {
        Ok(out) if out.code == Some(0) => parse_pactl(&out.stdout),
        _ => Vec::new(),
    };
    if audio.is_empty() {
        audio.push(Device {
            id: "alsa:default".into(),
            name: "default (ALSA)".into(),
        });
    }
    let cameras = tokio::task::spawn_blocking(v4l2_cameras)
        .await
        .unwrap_or_default();
    Catalog {
        platform: "linux",
        desktop: display.is_some(),
        region: display.is_some(),
        screens: Vec::new(),
        windows: Vec::new(),
        cameras,
        audio,
        fast: false,
        notes,
    }
}

/// The capture nodes of `/sys/class/video4linux` (index 0 of each device), by their names.
fn v4l2_cameras() -> Vec<Device> {
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return Vec::new();
    };
    let read = |dir: &Path, file: &str| {
        std::fs::read_to_string(dir.join(file))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let mut cameras: Vec<Device> = entries
        .flatten()
        .filter_map(|entry| {
            let node = entry.file_name().to_string_lossy().into_owned();
            let dir = entry.path();
            let index = read(&dir, "index");
            (node.starts_with("video") && (index.is_empty() || index == "0")).then(|| Device {
                id: format!("/dev/{node}"),
                name: match read(&dir, "name") {
                    n if n.is_empty() => node.clone(),
                    n => n,
                },
            })
        })
        .collect();
    cameras.sort_by(|a, b| a.id.cmp(&b.id));
    cameras
}

/// One frame of `spec`'s picture as a JPEG `PREVIEW_WIDTH` wide (never stored); `Err`: ffmpeg's
/// last error line (not redacted: the caller redacts it).
pub async fn grab_preview(ffmpeg: &Path, spec: &CaptureSpec) -> Result<Vec<u8>, String> {
    let c = capture_inputs(spec, Platform::current(), &x_display());
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    args.extend(c.input_args[0].iter().cloned());
    args.push("-i".into());
    args.push(c.inputs[0].url.clone());
    args.extend(
        [
            "-frames:v",
            "1",
            "-vf",
            &format!("{},scale={PREVIEW_WIDTH}:-2", fit_filter(spec.height)),
            "-f",
            "image2pipe",
            "-c:v",
            "mjpeg",
            "-q:v",
            "5",
            "pipe:1",
        ]
        .iter()
        .map(|s| (*s).to_string()),
    );
    let mut cmd = crate::process::command(ffmpeg);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = crate::process::spawn(&mut cmd).map_err(|e| e.to_string())?;
    // A timeout drops the child: `kill_on_drop` ends it.
    match tokio::time::timeout(PREVIEW_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) if out.status.success() && !out.stdout.is_empty() => Ok(out.stdout),
        Ok(Ok(out)) => Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("ffmpeg")
            .trim()
            .to_string()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("timeout ({} s)", PREVIEW_TIMEOUT.as_secs())),
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, MONITORINFOF_PRIMARY,
        WS_EX_TOOLWINDOW,
    };
    use windows_sys::core::BOOL;

    use super::super::Screen;

    /// Titles that are never offered (the desktop itself).
    const SKIPPED: [&str; 1] = ["Program Manager"];

    unsafe extern "system" fn each_window(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<HWND>` passed to `EnumWindows` below, alive for the call.
        let list = unsafe { &mut *(data as *mut Vec<HWND>) };
        list.push(hwnd);
        1
    }

    fn capturable_title(hwnd: HWND, own_pid: u32) -> Option<String> {
        // SAFETY: plain Win32 queries on a window handle EnumWindows gave; a window that went
        // away meanwhile just answers 0 / fails.
        unsafe {
            if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
                return None;
            }
            if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == own_pid {
                return None;
            }
            let mut cloaked = 0u32;
            let hr = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as _,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            if hr == 0 && cloaked != 0 {
                return None;
            }
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return None;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            let title = String::from_utf16_lossy(&buf[..got.max(0) as usize]);
            let title = title.trim().to_string();
            (!title.is_empty() && !SKIPPED.contains(&title.as_str())).then_some(title)
        }
    }

    /// The titles of the top-level windows a person sees (no tool windows, owned dialogs or
    /// cloaked windows of other virtual desktops, and none of this program), front to back,
    /// each title once.
    pub fn windows() -> Vec<String> {
        let mut handles: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes into `handles`, which outlives the call.
        unsafe {
            EnumWindows(Some(each_window), &mut handles as *mut Vec<HWND> as LPARAM);
        }
        // SAFETY: no arguments.
        let own_pid = unsafe { GetCurrentProcessId() };
        let mut titles: Vec<String> = Vec::new();
        for hwnd in handles {
            if let Some(title) = capturable_title(hwnd, own_pid)
                && !titles.contains(&title)
            {
                titles.push(title);
            }
        }
        titles
    }

    unsafe extern "system" fn each_monitor(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<Screen>` passed below, alive for the call.
        let list = unsafe { &mut *(data as *mut Vec<Screen>) };
        // SAFETY: a zeroed MONITORINFO with its size set, for a monitor the system gave.
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let r = info.rcMonitor;
            list.push(Screen {
                index: list.len() as u32,
                x: r.left,
                y: r.top,
                width: (r.right - r.left).max(0) as u32,
                height: (r.bottom - r.top).max(0) as u32,
                primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        1
    }

    /// The screens in the system's order (`ddagrab`'s output index follows it on most
    /// computers), each with its rectangle in desktop coordinates.
    pub fn screens() -> Vec<Screen> {
        let mut list: Vec<Screen> = Vec::new();
        // SAFETY: the callback only pushes into `list`, which outlives the call.
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(each_monitor),
                &mut list as *mut Vec<Screen> as LPARAM,
            );
        }
        list
    }
}

#[cfg(not(windows))]
mod win {
    pub fn windows() -> Vec<String> {
        Vec::new()
    }
    pub fn screens() -> Vec<super::super::Screen> {
        Vec::new()
    }
}
