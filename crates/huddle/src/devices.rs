//! Microphone and speaker via cpal, converted to/from the session's 48 kHz mono `i16`
//! 20 ms frames. cpal streams are not `Send` on every backend, so each lives on its own thread
//! and stops when its handle is dropped.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::session::{FRAME_SAMPLES, SAMPLE_RATE};

/// Streaming linear resampler for mono `f32`.
#[derive(Debug, Clone)]
pub struct Resampler {
    step: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Resampler {
        Resampler { step: from_rate as f64 / to_rate as f64, pos: 0.0, last: 0.0 }
    }

    /// Resample a chunk; state carries over to the next call.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 1);
        // `pos` is relative to `last` (index -1) followed by `input`.
        while self.pos < input.len() as f64 {
            let i = self.pos.floor();
            let frac = (self.pos - i) as f32;
            let a = if i < 1.0 { self.last } else { input[i as usize - 1] };
            let b = input[i as usize];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= input.len() as f64;
        if let Some(l) = input.last() {
            self.last = *l;
        }
        out
    }
}

/// Mix interleaved device samples down to mono.
pub fn to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect()
}

/// Environment variables naming the devices to use (a case-insensitive part of the name).
pub const INPUT_ENV: &str = "SLACKTUI_AUDIO_INPUT";
pub const OUTPUT_ENV: &str = "SLACKTUI_AUDIO_OUTPUT";

/// Does a device called `name` match the user's `wanted` text?
pub fn name_matches(name: &str, wanted: &str) -> bool {
    name.to_lowercase().contains(&wanted.trim().to_lowercase())
}

/// The named device (`wanted`), or the default one. A device that was asked for by name but
/// is missing is an error, never a silent fallback to the default.
fn pick(input: bool, wanted: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match wanted.filter(|w| !w.trim().is_empty()) {
        Some(w) => {
            let mut devices = if input { host.input_devices()? } else { host.output_devices()? };
            devices.find(|d| name_matches(&device_label(d), w)).ok_or_else(|| anyhow!("no {} device matching {w:?}", if input { "input" } else { "output" }))
        }
        None => if input { host.default_input_device() } else { host.default_output_device() }.ok_or_else(|| anyhow!("no default {} device", if input { "input" } else { "output" })),
    }
}

/// "`<id>` — `<name>`": ALSA names repeat, the id tells devices apart; both can be matched.
fn device_label(d: &cpal::Device) -> String {
    match d.id() {
        Ok(id) => format!("{id} — {d}"),
        Err(_) => d.to_string(),
    }
}

/// Labels of the input and output devices.
pub fn list_devices() -> Result<(Vec<String>, Vec<String>)> {
    let host = cpal::default_host();
    let inputs = host.input_devices()?.map(|d| device_label(&d)).collect();
    let outputs = host.output_devices()?.map(|d| device_label(&d)).collect();
    Ok((inputs, outputs))
}

/// Turns device input (any rate, interleaved channels, `f32`) into 20 ms 48 kHz mono frames.
#[derive(Debug, Clone)]
pub struct MicConverter {
    channels: usize,
    resampler: Resampler,
    pending: Vec<i16>,
}

impl MicConverter {
    pub fn new(rate: u32, channels: usize) -> MicConverter {
        MicConverter { channels, resampler: Resampler::new(rate, SAMPLE_RATE), pending: Vec::with_capacity(FRAME_SAMPLES * 2) }
    }

    /// Feed one device buffer; returns the complete frames it produced.
    pub fn push(&mut self, data: &[f32]) -> Vec<Vec<i16>> {
        let mono = self.resampler.process(&to_mono(data, self.channels));
        self.pending.extend(mono.iter().map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16));
        let mut frames = Vec::new();
        while self.pending.len() >= FRAME_SAMPLES {
            frames.push(self.pending.drain(..FRAME_SAMPLES).collect());
        }
        frames
    }
}

/// Microphone capture; dropping it stops the stream.
pub struct Microphone {
    _stop: Sender<()>,
}

impl Microphone {
    /// Capture `device` (part of its name; the default input if None) and call `on_frame`
    /// with each 20 ms 48 kHz mono frame.
    pub fn start(device: Option<String>, mut on_frame: impl FnMut(Vec<i16>) + Send + 'static) -> Result<Microphone> {
        let (stop_tx, stop_rx) = channel::<()>();
        let (ready_tx, ready_rx) = channel::<Result<()>>();
        std::thread::spawn(move || {
            let build = || -> Result<cpal::Stream> {
                let device = pick(true, device.as_deref())?;
                let config = device.default_input_config()?.config();
                let mut converter = MicConverter::new(config.sample_rate, config.channels as usize);
                let stream = device.build_input_stream::<f32, _, _>(
                    config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        for frame in converter.push(data) {
                            on_frame(frame);
                        }
                    },
                    |e| tracing::warn!("microphone error: {e}"),
                    None,
                )?;
                stream.play()?;
                Ok(stream)
            };
            match build() {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
        ready_rx.recv().map_err(|_| anyhow!("microphone thread died"))??;
        Ok(Microphone { _stop: stop_tx })
    }
}

/// Speaker playback; push frames with [`Speaker::play`]; dropping it stops the stream.
pub struct Speaker {
    queue: Arc<Mutex<VecDeque<f32>>>,
    resampler: Resampler,
    _stop: Sender<()>,
}

/// Cap on queued audio (0.5 s at 48 kHz) so latency cannot grow without bound.
const MAX_QUEUE: usize = SAMPLE_RATE as usize / 2;

impl Speaker {
    /// Play on `device` (part of its name; the default output if None).
    pub fn start(device: Option<String>) -> Result<Speaker> {
        let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let (stop_tx, stop_rx) = channel::<()>();
        let (ready_tx, ready_rx): (Sender<Result<u32>>, Receiver<Result<u32>>) = channel();
        let q = Arc::clone(&queue);
        std::thread::spawn(move || {
            let build = || -> Result<(cpal::Stream, u32)> {
                let device = pick(false, device.as_deref())?;
                let config = device.default_output_config()?.config();
                let (rate, channels) = (config.sample_rate, config.channels as usize);
                let stream = device.build_output_stream::<f32, _, _>(
                    config,
                    move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let mut q = q.lock().unwrap_or_else(|e| e.into_inner());
                        for frame in out.chunks_mut(channels) {
                            let s = q.pop_front().unwrap_or(0.0);
                            frame.iter_mut().for_each(|x| *x = s);
                        }
                    },
                    |e| tracing::warn!("speaker error: {e}"),
                    None,
                )?;
                stream.play()?;
                Ok((stream, rate))
            };
            match build() {
                Ok((stream, rate)) => {
                    let _ = ready_tx.send(Ok(rate));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
        let rate = ready_rx.recv().map_err(|_| anyhow!("speaker thread died"))??;
        Ok(Speaker { queue, resampler: Resampler::new(SAMPLE_RATE, rate), _stop: stop_tx })
    }

    /// Queue a 48 kHz mono frame for playback.
    pub fn play(&mut self, frame: &[i16]) {
        let samples: Vec<f32> = frame.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
        let out = self.resampler.process(&samples);
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.extend(out);
        let excess = q.len().saturating_sub(MAX_QUEUE);
        q.drain(..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_rates_and_continuity() {
        let mut r = Resampler::new(44_100, 48_000);
        let total: usize = (0..100).map(|_| r.process(&[0.5; 441]).len()).sum();
        // 44100 input samples → 48000 output samples (± one for the streaming edge).
        assert!((47_999..=48_001).contains(&total), "{total}");
        let mut down = Resampler::new(48_000, 16_000);
        assert_eq!(down.process(&[1.0; 960]).len(), 320);
        // A ramp stays monotonic across chunk boundaries.
        let mut r = Resampler::new(48_000, 44_100);
        let a = r.process(&(0..480).map(|i| i as f32).collect::<Vec<_>>());
        let b = r.process(&(480..960).map(|i| i as f32).collect::<Vec<_>>());
        let all: Vec<f32> = a.into_iter().chain(b).collect();
        assert!(all.windows(2).all(|w| w[1] >= w[0]), "not monotonic");
    }

    #[test]
    fn mic_converter_keeps_a_tone() {
        // One second of a 660 Hz tone from a 44.1 kHz stereo device, in 10 ms buffers.
        let mut conv = MicConverter::new(44_100, 2);
        let mut frames = Vec::new();
        for chunk in 0..100 {
            let buf: Vec<f32> = (0..441).flat_map(|i| {
                let t = (chunk * 441 + i) as f32 / 44_100.0;
                let s = (t * 660.0 * std::f32::consts::TAU).sin() * 0.5;
                [s, s]
            }).collect();
            frames.extend(conv.push(&buf));
        }
        assert_eq!(frames.len(), 50, "1 s = 50 frames of 20 ms");
        assert!(frames.iter().all(|f| f.len() == FRAME_SAMPLES));
        // Count zero crossings over the whole second: about 2 × 660.
        let all: Vec<i16> = frames.concat();
        let crossings = all.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        assert!((1300..=1340).contains(&crossings), "{crossings}");
        let rms = (all.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / all.len() as f64).sqrt();
        assert!((10_000.0..13_500.0).contains(&rms), "{rms}");
    }

    #[test]
    fn device_names() {
        assert!(name_matches("HyperX Cloud III Wireless Analog Stereo", "hyperx"));
        assert!(name_matches("pipewire", " PipeWire "));
        assert!(!name_matches("default", "hdmi"));
    }

    #[test]
    fn mono_mixdown() {
        assert_eq!(to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(to_mono(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }
}
