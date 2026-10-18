//! Video playback with sound: `ffmpeg` (a runtime dependency on PATH) decodes to raw RGB
//! frames and f32 PCM over pipes; frames are drawn with ratatui-image, audio plays via cpal.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use image::{DynamicImage, RgbImage};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect, Size};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};
use serde_json::Value;


/// Frames per second we decode at (terminal graphics cannot keep up with more).
const FPS: f64 = 12.0;
/// Widest frame we decode, in pixels.
const MAX_WIDTH: u32 = 960;

#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub duration: Option<f64>,
    pub has_audio: bool,
}

/// Parse `ffprobe -show_streams -show_format -of json`.
pub fn parse_probe(v: &Value) -> Option<Probe> {
    let streams = v["streams"].as_array()?;
    let video = streams.iter().find(|s| s["codec_type"] == "video")?;
    Some(Probe {
        width: video["width"].as_u64()? as u32,
        height: video["height"].as_u64()? as u32,
        duration: v["format"]["duration"].as_str().and_then(|d| d.parse().ok()),
        has_audio: streams.iter().any(|s| s["codec_type"] == "audio"),
    })
}

/// Frame size that fits `max` pixels (and `MAX_WIDTH`) keeping the aspect ratio, never
/// upscaling; even dimensions as most pixel formats require.
pub fn fit_frame(src: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let (w, h) = (src.0.max(1) as f64, src.1.max(1) as f64);
    let scale = (max.0.min(MAX_WIDTH) as f64 / w).min(max.1 as f64 / h).min(1.0);
    let even = |x: f64| ((x as u32) & !1).max(2);
    (even(w * scale), even(h * scale))
}

/// Index of the frame to show after `played` seconds of playback.
pub fn frame_due(played: f64, fps: f64) -> u64 {
    (played.max(0.0) * fps).floor() as u64
}

enum AudioCmd {
    Play,
    Pause,
}

pub struct Player {
    pub title: String,
    frame_size: (u32, u32),
    frames: Receiver<Vec<u8>>,
    /// Index of the next frame the receiver will hand out.
    next_frame: u64,
    current: Option<DynamicImage>,
    encoded: Option<(Size, Protocol)>,
    started: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
    duration: Option<f64>,
    audio: Option<SyncSender<AudioCmd>>,
    children: Vec<Child>,
    file: PathBuf,
    pub finished: bool,
}

impl Player {
    /// Start decoding `file` (deleted when the player is dropped) for an area of
    /// `max_px` pixels.
    pub fn start(title: String, file: PathBuf, max_px: (u32, u32)) -> anyhow::Result<Player> {
        let probe = probe(&file)?;
        let (w, h) = fit_frame((probe.width, probe.height), max_px);
        let mut children = Vec::new();

        let mut video = Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(&file)
            .args(["-an", "-vf", &format!("fps={FPS},scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut out = video.stdout.take().ok_or_else(|| anyhow::anyhow!("no ffmpeg stdout"))?;
        children.push(video);
        let (tx, frames) = sync_channel::<Vec<u8>>(8);
        let frame_len = (w * h * 3) as usize;
        std::thread::spawn(move || {
            loop {
                let mut buf = vec![0u8; frame_len];
                if out.read_exact(&mut buf).is_err() || tx.send(buf).is_err() {
                    break;
                }
            }
        });

        let audio = if probe.has_audio {
            match start_audio(&file) {
                Ok((child, ctl)) => {
                    children.push(child);
                    Some(ctl)
                }
                Err(e) => {
                    tracing::warn!("no audio: {e}");
                    None
                }
            }
        } else {
            None
        };
        Ok(Player {
            title,
            frame_size: (w, h),
            frames,
            next_frame: 0,
            current: None,
            encoded: None,
            started: Instant::now(),
            paused_at: None,
            paused_total: Duration::ZERO,
            duration: probe.duration,
            audio,
            children,
            file,
            finished: false,
        })
    }

    fn played(&self) -> f64 {
        let now = self.paused_at.unwrap_or_else(Instant::now);
        (now - self.started).saturating_sub(self.paused_total).as_secs_f64()
    }

    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn toggle_pause(&mut self) {
        match self.paused_at.take() {
            Some(at) => {
                self.paused_total += at.elapsed();
                self.send_audio(AudioCmd::Play);
            }
            None => {
                self.paused_at = Some(Instant::now());
                self.send_audio(AudioCmd::Pause);
            }
        }
    }

    fn send_audio(&self, cmd: AudioCmd) {
        if let Some(a) = &self.audio {
            let _ = a.try_send(cmd);
        }
    }

    /// Take the frame due now (dropping late ones); true if the picture changed.
    pub fn advance(&mut self) -> bool {
        if self.paused() || self.finished {
            return false;
        }
        let due = frame_due(self.played(), FPS);
        let mut changed = false;
        while self.next_frame <= due {
            match self.frames.try_recv() {
                Ok(buf) => {
                    self.next_frame += 1;
                    if self.next_frame > due {
                        if let Some(img) = RgbImage::from_raw(self.frame_size.0, self.frame_size.1, buf) {
                            self.current = Some(DynamicImage::ImageRgb8(img));
                            self.encoded = None;
                            changed = true;
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        changed
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, picker: &Picker) {
        super::media::clear_popup(f, area);
        let clock = |s: f64| format!("{}:{:02}", s as u64 / 60, s as u64 % 60);
        let total = self.duration.map(|d| format!(" / {}", clock(d))).unwrap_or_default();
        let state = if self.finished {
            "ended"
        } else if self.paused() {
            "paused"
        } else {
            "playing"
        };
        let sound = if self.audio.is_some() { "" } else { " · no sound" };
        // Wall time keeps running after the last frame; the clock should not.
        let shown = self.duration.map_or(self.played(), |d| self.played().min(d));
        let title = format!(" {} · {state} {}{total}{sound} ", self.title, clock(shown));
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(super::theme::p().accent)).title(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(img) = &self.current else {
            f.render_widget(Paragraph::new("Starting video…").style(Style::default().fg(super::theme::p().dim)), inner);
            return;
        };
        let size = Size::new(inner.width, inner.height);
        if self.encoded.as_ref().is_none_or(|(s, _)| *s != size) {
            self.encoded = picker.new_protocol(img.clone(), size, Resize::Fit(None)).ok().map(|p| (size, p));
        }
        if let Some((_, proto)) = &self.encoded {
            let fit = proto.size();
            let [x] = Layout::horizontal([Constraint::Length(fit.width)]).flex(Flex::Center).areas(inner);
            let [cell] = Layout::vertical([Constraint::Length(fit.height)]).flex(Flex::Center).areas(x);
            f.render_widget(Image::new(proto), cell);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_file(&self.file);
    }
}

fn probe(file: &Path) -> anyhow::Result<Probe> {
    let out = Command::new("ffprobe").args(["-v", "error", "-show_streams", "-show_format", "-of", "json"]).arg(file).stdin(Stdio::null()).output()?;
    let v: Value = serde_json::from_slice(&out.stdout)?;
    parse_probe(&v).ok_or_else(|| anyhow::anyhow!("no video stream"))
}

/// Decode the audio track to f32 PCM at the output device's rate and play it on a
/// dedicated thread (cpal streams are not `Send` on every backend).
fn start_audio(file: &Path) -> anyhow::Result<(Child, SyncSender<AudioCmd>)> {
    let device = cpal::default_host().default_output_device().ok_or_else(|| anyhow::anyhow!("no audio output device"))?;
    let supported = device.default_output_config()?;
    let config = supported.config();
    let (rate, channels) = (config.sample_rate, config.channels);
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(file)
        .args(["-vn", "-f", "f32le", "-ac", &channels.to_string(), "-ar", &rate.to_string(), "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut out = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no ffmpeg stdout"))?;
    // ~0.25 s chunks; the bound gives backpressure while paused.
    let (pcm_tx, pcm_rx) = sync_channel::<Vec<f32>>(8);
    std::thread::spawn(move || {
        let chunk = (rate as usize * channels as usize / 4) * 4;
        loop {
            let mut bytes = vec![0u8; chunk];
            let n = match out.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(n) => n - n % 4,
            };
            let samples: Vec<f32> = bytes[..n].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            if pcm_tx.send(samples).is_err() {
                break;
            }
        }
    });
    let (ctl_tx, ctl_rx) = sync_channel::<AudioCmd>(4);
    std::thread::spawn(move || {
        let mut queue: VecDeque<f32> = VecDeque::new();
        let callback = move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
            while queue.len() < out.len() {
                match pcm_rx.try_recv() {
                    Ok(chunk) => queue.extend(chunk),
                    Err(_) => break,
                }
            }
            for s in out.iter_mut() {
                *s = queue.pop_front().unwrap_or(0.0);
            }
        };
        let stream = match device.build_output_stream::<f32, _, _>(config, callback, |e| tracing::warn!("audio stream error: {e}"), None) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("audio stream failed: {e}");
                return;
            }
        };
        let _ = stream.play();
        // Lives until the player drops its sender.
        while let Ok(cmd) = ctl_rx.recv() {
            let _ = match cmd {
                AudioCmd::Play => stream.play(),
                AudioCmd::Pause => stream.pause(),
            };
        }
    });
    Ok((child, ctl_tx))
}

/// Download a video attachment to a temporary file.
pub async fn download(client: slack_core::Client, file: slack_core::types::File) -> anyhow::Result<PathBuf> {
    let url = file.mp4.as_deref().or(file.url_private_download.as_deref()).or(file.url_private.as_deref()).ok_or_else(|| anyhow::anyhow!("no video URL"))?.to_string();
    let bytes = client.download(&url).await?;
    let ext = Path::new(&file.name).extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let path = std::env::temp_dir().join(format!("slacktui-{}-{}.{ext}", std::process::id(), file.id));
    tokio::fs::write(&path, bytes).await?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_parsing() {
        let v = serde_json::json!({"streams":[{"codec_type":"video","width":640,"height":360},{"codec_type":"audio"}],"format":{"duration":"6.000000"}});
        assert_eq!(parse_probe(&v), Some(Probe { width: 640, height: 360, duration: Some(6.0), has_audio: true }));
        assert_eq!(parse_probe(&serde_json::json!({"streams":[{"codec_type":"audio"}]})), None);
    }

    #[test]
    fn frame_fitting() {
        assert_eq!(fit_frame((1920, 1080), (1200, 900)), (960, 540));
        assert_eq!(fit_frame((640, 360), (2000, 2000)), (640, 360));
        assert_eq!(fit_frame((641, 361), (320, 1000)), (320, 180));
        assert_eq!(fit_frame((100, 1000), (500, 101)), (10, 100));
    }

    #[test]
    fn frames_due() {
        assert_eq!(frame_due(0.0, 12.0), 0);
        assert_eq!(frame_due(1.0, 12.0), 12);
        assert_eq!(frame_due(-1.0, 12.0), 0);
    }
}
