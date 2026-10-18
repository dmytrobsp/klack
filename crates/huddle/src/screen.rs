//! Screen capture for sharing. On Linux the desktop's screen-cast portal asks the user what
//! to share (nothing is captured before they pick) and hands out a PipeWire stream, which
//! GStreamer's `pipewiresrc` (the `gst-launch-1.0` tool) turns into fixed-size RGB frames on
//! a pipe. Other systems are not supported yet.

use anyhow::Result;

/// Size and rate of a shared screen: large enough to read text.
pub const SHARE_WIDTH: usize = 1280;
pub const SHARE_HEIGHT: usize = 720;
pub const SHARE_FPS: u32 = 10;

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
    _portal: (ashpd::desktop::screencast::Screencast, ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>),
}

impl ScreenCapture {
    /// Ask the desktop what to share (a dialog; this waits for the user), then capture it as
    /// `width`×`height` RGB at `fps`, calling `on_frame` on a thread. Returns the capture and
    /// a description of what was shared.
    #[cfg(target_os = "linux")]
    pub async fn start(width: usize, height: usize, fps: u32, on_frame: impl FnMut(Vec<u8>) + Send + 'static) -> Result<(ScreenCapture, String)> {
        use anyhow::{Context, anyhow};
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
        Ok((ScreenCapture { child, _portal: (proxy, session) }, what))
    }

    #[cfg(not(target_os = "linux"))]
    pub async fn start(_width: usize, _height: usize, _fps: u32, _on_frame: impl FnMut(Vec<u8>) + Send + 'static) -> Result<(ScreenCapture, String)> {
        Err(anyhow::anyhow!("screen sharing is only implemented on Linux so far"))
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
}
