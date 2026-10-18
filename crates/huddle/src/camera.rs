//! Camera capture through the `ffmpeg` command-line tool (already needed for video playback):
//! V4L2 on Linux, AVFoundation on macOS, DirectShow on Windows. Frames come back over a pipe
//! as packed RGB of a fixed size. A camera is only ever opened by an explicit choice; there is
//! no default device and no fallback.

use std::io::Read;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, anyhow};

use crate::devices::name_matches;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    /// What ffmpeg opens: the device path on Linux, the device name elsewhere.
    pub id: String,
    pub name: String,
}

/// The cameras present. Linux reads `/sys/class/video4linux` (capture nodes only); macOS and
/// Windows ask ffmpeg (`-list_devices`; not run on those systems yet).
pub fn list_cameras() -> Vec<CameraInfo> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        out = parse_avfoundation_devices(&ffmpeg_device_list(&["-f", "avfoundation", "-list_devices", "true", "-i", ""]));
    } else if cfg!(target_os = "windows") {
        out = parse_dshow_devices(&ffmpeg_device_list(&["-list_devices", "true", "-f", "dshow", "-i", "dummy"]));
    } else if cfg!(target_os = "linux") {
        if let Ok(dir) = std::fs::read_dir("/sys/class/video4linux") {
            for entry in dir.flatten() {
                let read = |f: &str| std::fs::read_to_string(entry.path().join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
                // A camera has several nodes; index 0 is the one with pictures.
                if read("index") != "0" {
                    continue;
                }
                out.push(CameraInfo { id: format!("/dev/{}", entry.file_name().to_string_lossy()), name: read("name") });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
    }
    out
}

/// What ffmpeg prints when asked to list devices (on standard error; it then exits with an
/// error, which is expected).
fn ffmpeg_device_list(args: &[&str]) -> String {
    match Command::new("ffmpeg").arg("-hide_banner").args(args).stdin(Stdio::null()).output() {
        Ok(out) => String::from_utf8_lossy(&out.stderr).into_owned(),
        Err(e) => {
            tracing::warn!("cannot list cameras with ffmpeg: {e}");
            String::new()
        }
    }
}

/// Cameras in `ffmpeg -f avfoundation -list_devices true -i ""` output (macOS): the video
/// devices by index, without the "Capture screen N" entries (those are screens).
pub fn parse_avfoundation_devices(text: &str) -> Vec<CameraInfo> {
    let mut out = Vec::new();
    let mut video = false;
    for line in text.lines() {
        if line.contains("AVFoundation video devices") {
            video = true;
        } else if line.contains("AVFoundation audio devices") {
            video = false;
        } else if video {
            // "[AVFoundation indev @ 0x7f…] [0] FaceTime HD Camera"
            let Some(rest) = line.split_once("] [").map(|(_, r)| r) else { continue };
            let Some((index, name)) = rest.split_once("] ") else { continue };
            if index.chars().all(|c| c.is_ascii_digit()) && !name.starts_with("Capture screen") {
                out.push(CameraInfo { id: index.to_string(), name: name.trim().to_string() });
            }
        }
    }
    out
}

/// Cameras in `ffmpeg -list_devices true -f dshow -i dummy` output (Windows), opened by name:
/// current ffmpeg marks each device `"Name" (video)`, older ones list video devices under a
/// "DirectShow video devices" heading.
pub fn parse_dshow_devices(text: &str) -> Vec<CameraInfo> {
    let mut out = Vec::new();
    let mut video_section = false;
    for line in text.lines() {
        if line.contains("DirectShow video devices") {
            video_section = true;
            continue;
        }
        if line.contains("DirectShow audio devices") {
            video_section = false;
            continue;
        }
        if line.contains("Alternative name") {
            continue;
        }
        let Some((_, rest)) = line.split_once('"') else { continue };
        let Some((name, after)) = rest.split_once('"') else { continue };
        // "(video)", "(audio)", "(audio, video)", "(none)"; nothing in the older form.
        let tag = after.trim_start();
        let marked = tag.starts_with('(');
        let marked_video = marked && tag.split(')').next().is_some_and(|t| t.contains("video"));
        if marked_video || (video_section && !marked) {
            out.push(CameraInfo { id: name.to_string(), name: name.to_string() });
        }
    }
    out
}

/// The camera matching `wanted` (part of its path or name). Never a fallback: no match is an
/// error. Off Linux, where nothing is listed, the name is used as given.
pub fn find_camera(cameras: &[CameraInfo], wanted: &str) -> Result<CameraInfo> {
    if wanted.trim().is_empty() {
        return Err(anyhow!("no camera chosen"));
    }
    if cameras.is_empty() && !cfg!(target_os = "linux") {
        return Ok(CameraInfo { id: wanted.trim().into(), name: wanted.trim().into() });
    }
    cameras.iter().find(|c| c.id == wanted.trim() || name_matches(&c.name, wanted) || name_matches(&c.id, wanted)).cloned().ok_or_else(|| anyhow!("no camera matching {wanted:?}"))
}

/// ffmpeg's arguments to read `camera` as `width`×`height` RGB at `fps` (letterboxed, so any
/// aspect fits) on standard output.
pub fn ffmpeg_args(camera: &CameraInfo, width: usize, height: usize, fps: u32) -> Vec<String> {
    let input: Vec<String> = if cfg!(target_os = "macos") {
        vec!["-f".into(), "avfoundation".into(), "-framerate".into(), "30".into(), "-i".into(), camera.id.clone()]
    } else if cfg!(target_os = "windows") {
        vec!["-f".into(), "dshow".into(), "-i".into(), format!("video={}", camera.id)]
    } else {
        vec!["-f".into(), "v4l2".into(), "-i".into(), camera.id.clone()]
    };
    rgb_output_args(input, width, height, fps)
}

/// ffmpeg's arguments around `input`: quiet, then `width`×`height` RGB at `fps`, letterboxed,
/// as raw frames on standard output.
pub fn rgb_output_args(input: Vec<String>, width: usize, height: usize, fps: u32) -> Vec<String> {
    let filter = format!("fps={fps},scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,format=rgb24");
    ["-hide_banner", "-loglevel", "error", "-nostdin"].iter().map(|s| s.to_string()).chain(input).chain(["-vf".into(), filter, "-f".into(), "rawvideo".into(), "-".into()]).collect()
}

/// Run ffmpeg with `args` and call `on_frame` on a thread with each `frame_len`-byte frame it
/// writes; `what` names it in the log. Kill the child to stop.
pub fn spawn_ffmpeg(args: Vec<String>, frame_len: usize, what: String, on_frame: impl FnMut(Vec<u8>) + Send + 'static) -> Result<Child> {
    let mut child = Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run ffmpeg (needed for the {what})"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no ffmpeg output"))?;
    let mut stderr = child.stderr.take();
    std::thread::spawn(move || {
        match read_frames(stdout, frame_len, on_frame) {
            Ok(n) => tracing::info!("{what}: {n} frames"),
            Err(e) => tracing::warn!("{what}: {e}"),
        }
        let mut err = String::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_string(&mut err);
        }
        if !err.trim().is_empty() {
            tracing::warn!("{what}: ffmpeg: {}", err.trim());
        }
    });
    Ok(child)
}

/// Call `on_frame` with each complete frame of `frame_len` bytes from `reader`, until it ends;
/// returns the number of frames.
pub fn read_frames(mut reader: impl Read, frame_len: usize, mut on_frame: impl FnMut(Vec<u8>)) -> std::io::Result<u64> {
    let mut n = 0;
    loop {
        let mut frame = vec![0u8; frame_len];
        match reader.read_exact(&mut frame) {
            Ok(()) => {
                on_frame(frame);
                n += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(n),
            Err(e) => return Err(e),
        }
    }
}

/// A running capture; dropping it stops ffmpeg.
pub struct Camera {
    child: Child,
}

impl Camera {
    /// Capture `camera` as `width`×`height` RGB at `fps`, calling `on_frame` on a thread.
    pub fn start(camera: &CameraInfo, width: usize, height: usize, fps: u32, on_frame: impl FnMut(Vec<u8>) + Send + 'static) -> Result<Camera> {
        let child = spawn_ffmpeg(ffmpeg_args(camera, width, height, fps), width * height * 3, format!("camera {}", camera.name), on_frame)?;
        Ok(Camera { child })
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cams() -> Vec<CameraInfo> {
        vec![CameraInfo { id: "/dev/video0".into(), name: "Integrated Webcam".into() }, CameraInfo { id: "/dev/video42".into(), name: "slacktui test camera".into() }]
    }

    #[test]
    fn choosing_a_camera() {
        assert_eq!(find_camera(&cams(), "test camera").unwrap().id, "/dev/video42");
        assert_eq!(find_camera(&cams(), "/dev/video42").unwrap().name, "slacktui test camera");
        assert_eq!(find_camera(&cams(), "video0").unwrap().name, "Integrated Webcam");
        if cfg!(target_os = "linux") {
            assert!(find_camera(&cams(), "usb cam").is_err(), "no fallback to another camera");
            assert!(find_camera(&[], "anything").is_err());
        }
        assert!(find_camera(&cams(), " ").is_err(), "nothing chosen, nothing opened");
    }

    #[test]
    fn ffmpeg_reads_rgb_of_a_fixed_size() {
        let args = ffmpeg_args(&cams()[1], 640, 360, 15);
        let at = |a: &str| args.iter().position(|x| x == a).unwrap();
        let input = if cfg!(target_os = "windows") { "video=/dev/video42" } else { "/dev/video42" };
        assert_eq!(args[at("-i") + 1], input);
        assert!(args[at("-vf") + 1].starts_with("fps=15,scale=640:360:"));
        assert!(args[at("-vf") + 1].ends_with("format=rgb24"));
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    /// The device list as ffmpeg's AVFoundation input prints it (format from ffmpeg's
    /// documentation and source; not captured on a Mac).
    #[test]
    fn macos_cameras_from_ffmpeg() {
        let text = "[AVFoundation indev @ 0x7fb4d1c04a40] AVFoundation video devices:\n\
            [AVFoundation indev @ 0x7fb4d1c04a40] [0] FaceTime HD Camera\n\
            [AVFoundation indev @ 0x7fb4d1c04a40] [1] OBS Virtual Camera\n\
            [AVFoundation indev @ 0x7fb4d1c04a40] [2] Capture screen 0\n\
            [AVFoundation indev @ 0x7fb4d1c04a40] AVFoundation audio devices:\n\
            [AVFoundation indev @ 0x7fb4d1c04a40] [0] MacBook Pro Microphone\n\
            : Input/output error";
        let cams = parse_avfoundation_devices(text);
        assert_eq!(cams, vec![CameraInfo { id: "0".into(), name: "FaceTime HD Camera".into() }, CameraInfo { id: "1".into(), name: "OBS Virtual Camera".into() }]);
        assert_eq!(find_camera(&cams, "obs").unwrap().id, "1");
    }

    /// Both forms of ffmpeg's DirectShow device list (from its documentation and source; not
    /// captured on Windows).
    #[test]
    fn windows_cameras_from_ffmpeg() {
        let current = "[dshow @ 000001f8c5f0a0c0] \"Integrated Camera\" (video)\n\
            [dshow @ 000001f8c5f0a0c0]   Alternative name \"@device_pnp_\\\\?\\usb#vid_04f2&pid_b6d9\"\n\
            [dshow @ 000001f8c5f0a0c0] \"Microphone Array (Realtek(R) Audio)\" (audio)\n\
            [dshow @ 000001f8c5f0a0c0]   Alternative name \"@device_cm_{33D9A762}\\wave_{A1B2}\"\n\
            dummy: Immediate exit requested";
        assert_eq!(parse_dshow_devices(current), vec![CameraInfo { id: "Integrated Camera".into(), name: "Integrated Camera".into() }]);
        let older = "[dshow @ 0000000000c1e0a0] DirectShow video devices (some may be both video and audio devices)\n\
            [dshow @ 0000000000c1e0a0]  \"USB2.0 HD UVC WebCam\"\n\
            [dshow @ 0000000000c1e0a0]     Alternative name \"@device_pnp_\\\\?\\usb#vid_13d3\"\n\
            [dshow @ 0000000000c1e0a0] DirectShow audio devices\n\
            [dshow @ 0000000000c1e0a0]  \"Microphone (Realtek High Definition Audio)\"";
        assert_eq!(parse_dshow_devices(older), vec![CameraInfo { id: "USB2.0 HD UVC WebCam".into(), name: "USB2.0 HD UVC WebCam".into() }]);
    }

    #[test]
    fn frames_from_a_pipe() {
        let data: Vec<u8> = (0..25u8).collect();
        let mut got = Vec::new();
        // Two whole 12-byte frames; the trailing byte is an incomplete frame.
        assert_eq!(read_frames(&data[..], 12, |f| got.push(f)).unwrap(), 2);
        assert_eq!(got[1], (12..24).collect::<Vec<u8>>());
    }
}
