//! Screen capture for sharing. In a Wayland session the desktop's screen-cast portal asks the
//! user what to share (nothing is captured before they pick) and hands out a PipeWire stream,
//! which GStreamer's `pipewiresrc` (the `gst-launch-1.0` tool) turns into fixed-size RGB frames
//! on a pipe. Elsewhere the `ffmpeg` tool captures the whole (main) screen, as there is no
//! picker: `x11grab` on X11, `avfoundation` on macOS (needs the Screen Recording permission),
//! `gdigrab` on Windows — written for those systems but only the portal has been run.

use anyhow::{Result, anyhow};

/// Size and rate of a shared screen: large enough to read text.
pub const SHARE_WIDTH: usize = 1280;
pub const SHARE_HEIGHT: usize = 720;
pub const SHARE_FPS: u32 = 10;

/// Where a shared screen comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The screen-cast portal (Linux, Wayland): the desktop asks what to share.
    Portal,
    /// The whole X11 display (`$DISPLAY`).
    X11,
    MacOs,
    Windows,
}

impl Source {
    /// This system's source: on Linux the portal in a Wayland session, X11 otherwise.
    pub fn current() -> Option<Source> {
        let set = |v: &str| std::env::var_os(v).is_some_and(|s| !s.is_empty());
        if cfg!(target_os = "macos") {
            Some(Source::MacOs)
        } else if cfg!(target_os = "windows") {
            Some(Source::Windows)
        } else if set("WAYLAND_DISPLAY") {
            Some(Source::Portal)
        } else if set("DISPLAY") {
            Some(Source::X11)
        } else {
            None
        }
    }

    /// Whether the desktop asks the user what to share; otherwise the whole screen goes out.
    pub fn asks_what_to_share(self) -> bool {
        self == Source::Portal
    }
}

/// ffmpeg arguments to capture the whole screen from `source` as `width`×`height` RGB at
/// `fps` (`display`: the X11 display name), and what is being shared; None for the portal.
pub fn ffmpeg_screen_args(source: Source, display: &str, width: usize, height: usize, fps: u32) -> Option<(Vec<String>, &'static str)> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let rate = fps.to_string();
    let (input, what) = match source {
        Source::Portal => return None,
        Source::X11 => (s(&["-f", "x11grab", "-framerate", &rate, "-draw_mouse", "1", "-i", display]), "whole screen"),
        Source::MacOs => (s(&["-f", "avfoundation", "-capture_cursor", "1", "-framerate", "30", "-i", "Capture screen 0"]), "main screen"),
        Source::Windows => (s(&["-f", "gdigrab", "-framerate", &rate, "-draw_mouse", "1", "-i", "desktop"]), "whole desktop"),
    };
    Some((crate::camera::rgb_output_args(input, width, height, fps), what))
}

/// `gst-launch-1.0` arguments: read PipeWire node `node` through the portal's remote (passed
/// to the child as file descriptor `fd`), as `width`×`height` RGB at `fps`, letterboxed, on
/// standard output. `-q` keeps gst-launch's own messages off the frame pipe.
pub fn gst_args(node: u32, fd: i32, width: usize, height: usize, fps: u32) -> Vec<String> {
    let pipeline = format!(
        "pipewiresrc fd={fd} path={node} always-copy=true ! videorate ! video/x-raw,framerate={fps}/1 ! videoconvert ! videoscale add-borders=true ! video/x-raw,format=RGB,width={width},height={height},pixel-aspect-ratio=1/1 ! fdsink fd=1 sync=false"
    );
    std::iter::once("-q".to_string()).chain(pipeline.split(' ').map(String::from)).collect()
}

/// A running screen share capture; dropping it stops GStreamer and closes the portal session.
pub struct ScreenCapture {
    child: std::process::Child,
    #[cfg(target_os = "linux")]
    _portal: Option<(ashpd::desktop::screencast::Screencast, ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>)>,
}

impl ScreenCapture {
    /// Capture a screen as `width`×`height` RGB at `fps`, calling `on_frame` on a thread:
    /// with the portal the desktop first asks what to share (a dialog; this waits for the
    /// user), elsewhere the whole screen is captured at once. Returns the capture and a
    /// description of what was shared.
    pub async fn start(width: usize, height: usize, fps: u32, on_frame: impl FnMut(Vec<u8>) + Send + 'static) -> Result<(ScreenCapture, String)> {
        let source = Source::current().ok_or_else(|| anyhow!("no screen to share (neither a Wayland nor an X11 session)"))?;
        #[cfg(target_os = "linux")]
        if source == Source::Portal {
            return Self::start_portal(width, height, fps, on_frame).await;
        }
        let display = std::env::var("DISPLAY").unwrap_or_default();
        let (args, what) = ffmpeg_screen_args(source, &display, width, height, fps).ok_or_else(|| anyhow!("screen sharing needs the screen-cast portal here"))?;
        let child = crate::camera::spawn_ffmpeg(args, width * height * 3, "screen share".into(), on_frame)?;
        #[cfg(target_os = "linux")]
        return Ok((ScreenCapture { child, _portal: None }, what.to_string()));
        #[cfg(not(target_os = "linux"))]
        Ok((ScreenCapture { child }, what.to_string()))
    }

    #[cfg(target_os = "linux")]
    async fn start_portal(width: usize, height: usize, fps: u32, on_frame: impl FnMut(Vec<u8>) + Send + 'static) -> Result<(ScreenCapture, String)> {
        use anyhow::Context;
        use ashpd::desktop::PersistMode;
        use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
        use std::os::fd::{AsRawFd, OwnedFd};
        use std::os::unix::process::CommandExt;

        let proxy = Screencast::new().await.context("no screen-cast portal")?;
        let session = proxy.create_session(Default::default()).await?;
        // Cancelling the desktop's picker surfaces as different portal errors (Hyprland's
        // reports an invalid session); say what happened, keep the details in the log.
        let not_started = |e: ashpd::Error| {
            tracing::warn!("screen share portal: {e}");
            anyhow!("not started (cancelled, or refused by the desktop)")
        };
        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default().set_cursor_mode(CursorMode::Embedded).set_sources(SourceType::Monitor | SourceType::Window).set_multiple(false).set_persist_mode(PersistMode::DoNot),
            )
            .await
            .map_err(not_started)?;
        let streams = proxy.start(&session, None, Default::default()).await.map_err(not_started)?.response().map_err(not_started)?;
        let stream = streams.streams().first().ok_or_else(|| anyhow!("nothing was chosen to share"))?;
        let node = stream.pipe_wire_node_id();
        let kind = stream.source_type().map_or("screen".to_string(), |t| format!("{t:?}").to_lowercase());
        let what = format!("{kind}{}", stream.size().map(|(w, h)| format!(" {w}×{h}")).unwrap_or_default());
        let remote: OwnedFd = proxy.open_pipe_wire_remote(&session, Default::default()).await?;

        // The child gets the portal's remote as fd 3.
        const CHILD_FD: i32 = 3;
        let raw = remote.as_raw_fd();
        let mut command = std::process::Command::new("gst-launch-1.0");
        command.args(gst_args(node, CHILD_FD, width, height, fps)).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        // SAFETY: only async-signal-safe calls (dup2, fcntl) between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(raw, CHILD_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let flags = libc::fcntl(CHILD_FD, libc::F_GETFD);
                libc::fcntl(CHILD_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                Ok(())
            });
        }
        let mut child = command.spawn().context("cannot run gst-launch-1.0 (GStreamer with the PipeWire plugin is needed to share a screen)")?;
        drop(remote);
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no gst-launch output"))?;
        let mut stderr = child.stderr.take();
        std::thread::spawn(move || {
            match crate::camera::read_frames(stdout, width * height * 3, on_frame) {
                Ok(n) => tracing::info!("screen share: {n} frames"),
                Err(e) => tracing::warn!("screen share: {e}"),
            }
            let mut err = String::new();
            if let Some(s) = stderr.as_mut() {
                use std::io::Read;
                let _ = s.read_to_string(&mut err);
            }
            if !err.trim().is_empty() {
                tracing::warn!("screen share: gst-launch: {}", err.trim());
            }
        });
        Ok((ScreenCapture { child, _portal: Some((proxy, session)) }, what))
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gstreamer_pipeline() {
        let args = gst_args(42, 3, 1280, 720, 10);
        assert_eq!(args[0], "-q", "gst-launch's own messages would corrupt the frame pipe");
        let line = args.join(" ");
        assert!(line.starts_with("-q pipewiresrc fd=3 path=42 "));
        assert!(line.contains("framerate=10/1") && line.contains("format=RGB,width=1280,height=720"));
        assert!(line.ends_with("fdsink fd=1 sync=false"));
    }

    #[test]
    fn ffmpeg_screens() {
        assert_eq!(ffmpeg_screen_args(Source::Portal, ":0", 1280, 720, 10), None);
        let input = |source| {
            let (args, what) = ffmpeg_screen_args(source, ":1", 1280, 720, 10).unwrap();
            let at = args.iter().position(|a| a == "-i").unwrap();
            assert!(args.join(" ").contains("-vf fps=10,scale=1280:720:"), "{source:?}: scaled and letterboxed");
            assert_eq!(args.last().map(String::as_str), Some("-"));
            (args[at - 1].clone(), args[at + 1].clone(), args[1 + args.iter().position(|a| a == "-f").unwrap()].clone(), what)
        };
        assert_eq!(input(Source::X11), ("1".into(), ":1".into(), "x11grab".into(), "whole screen"));
        assert_eq!(input(Source::MacOs), ("30".into(), "Capture screen 0".into(), "avfoundation".into(), "main screen"));
        assert_eq!(input(Source::Windows), ("1".into(), "desktop".into(), "gdigrab".into(), "whole desktop"));
        assert!(Source::Portal.asks_what_to_share() && !Source::X11.asks_what_to_share());
    }
}
