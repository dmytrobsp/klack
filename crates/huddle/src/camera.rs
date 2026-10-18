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

/// The cameras present. Linux reads `/sys/class/video4linux` (capture nodes only); other
/// systems are not listed yet (choose a camera by its exact name there).
pub fn list_cameras() -> Vec<CameraInfo> {
    let mut out = Vec::new();
    if cfg!(target_os = "linux") {
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
    let filter = format!("fps={fps},scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,format=rgb24");
    ["-hide_banner", "-loglevel", "error", "-nostdin"].iter().map(|s| s.to_string()).chain(input).chain(["-vf".into(), filter, "-f".into(), "rawvideo".into(), "-".into()]).collect()
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
        let mut child = Command::new("ffmpeg")
            .args(ffmpeg_args(camera, width, height, fps))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot run ffmpeg (needed for the camera)")?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no ffmpeg output"))?;
        let mut stderr = child.stderr.take();
        let name = camera.name.clone();
        std::thread::spawn(move || {
            match read_frames(stdout, width * height * 3, on_frame) {
                Ok(n) => tracing::info!("camera {name}: {n} frames"),
                Err(e) => tracing::warn!("camera {name}: {e}"),
            }
            let mut err = String::new();
            if let Some(s) = stderr.as_mut() {
                let _ = s.read_to_string(&mut err);
            }
            if !err.trim().is_empty() {
                tracing::warn!("camera {name}: ffmpeg: {}", err.trim());
            }
        });
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

    #[test]
    fn frames_from_a_pipe() {
        let data: Vec<u8> = (0..25u8).collect();
        let mut got = Vec::new();
        // Two whole 12-byte frames; the trailing byte is an incomplete frame.
        assert_eq!(read_frames(&data[..], 12, |f| got.push(f)).unwrap(), 2);
        assert_eq!(got[1], (12..24).collect::<Vec<u8>>());
    }
}
