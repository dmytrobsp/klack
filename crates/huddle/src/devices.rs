//! Microphone and speaker via cpal, converted to/from the session's 48 kHz mono `i16`
//! 20 ms frames. cpal streams are not `Send` on every backend, so each lives on its own thread
//! and stops when its handle is dropped.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
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

/// A frame's RMS in dBFS (-100 for silence).
pub fn frame_db(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return -100.0;
    }
    let mean = frame.iter().map(|s| (*s as f32 / i16::MAX as f32).powi(2)).sum::<f32>() / frame.len() as f32;
    10.0 * mean.max(1e-10).log10()
}

/// Loudness of a frame as a level (0–1): its RMS in dBFS mapped from -50 dB (0) to
/// -10 dB (1), roughly how the meeting reports speakers.
pub fn frame_level(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    ((frame_db(frame) + 50.0) / 40.0).clamp(0.0, 1.0)
}

/// How the microphone is processed before sending; shared with a running call (behind a
/// mutex) so a change applies to the next frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MicConfig {
    /// Linear gain (1.0: unchanged).
    pub gain: f32,
    /// Frames quieter than this (dBFS, after the gain) are sent as silence; None: always open.
    pub gate_db: Option<f32>,
    /// RNNoise noise suppression.
    pub suppress_noise: bool,
}

impl Default for MicConfig {
    fn default() -> Self {
        MicConfig { gain: 1.0, gate_db: None, suppress_noise: false }
    }
}

/// Frames (20 ms) the gate stays open after the level falls below the threshold, so the
/// ends of words are not cut off.
pub const GATE_HANG_FRAMES: u32 = 15;

/// Samples faded in when the gate opens (2 ms), to avoid a click.
const GATE_FADE_IN: usize = 96;

/// Noise suppression, gain and the speaking gate for 20 ms 48 kHz frames, in that order.
#[derive(Default)]
pub struct MicProcessor {
    /// Created on first use (RNNoise keeps state across frames).
    denoise: Option<Box<nnnoiseless::DenoiseState<'static>>>,
    /// Frames left before the gate closes.
    hang: u32,
    open: bool,
}

impl MicProcessor {
    pub fn new() -> MicProcessor {
        MicProcessor::default()
    }

    /// Process `frame` in place; returns its level (0–1, before the gate) and whether the
    /// gate let it through (a closed gate leaves silence).
    pub fn process(&mut self, frame: &mut [i16], cfg: &MicConfig) -> (f32, bool) {
        if cfg.suppress_noise {
            let state = self.denoise.get_or_insert_with(nnnoiseless::DenoiseState::new);
            let n = nnnoiseless::DenoiseState::FRAME_SIZE;
            let mut out = [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE];
            for chunk in frame.chunks_mut(n).filter(|c| c.len() == n) {
                // RNNoise works on samples in the i16 range.
                let input: Vec<f32> = chunk.iter().map(|s| *s as f32).collect();
                state.process_frame(&mut out, &input);
                for (s, o) in chunk.iter_mut().zip(out) {
                    *s = o.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                }
            }
        }
        if cfg.gain != 1.0 {
            for s in frame.iter_mut() {
                *s = (*s as f32 * cfg.gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            }
        }
        let level = frame_level(frame);
        let was_open = self.open;
        match cfg.gate_db {
            None => self.open = true,
            Some(t) if frame_db(frame) >= t => {
                self.open = true;
                self.hang = GATE_HANG_FRAMES;
            }
            Some(_) if self.hang > 0 => self.hang -= 1,
            Some(_) => self.open = false,
        }
        let len = frame.len().max(1) as f32;
        match (was_open, self.open) {
            (false, true) => frame.iter_mut().take(GATE_FADE_IN).enumerate().for_each(|(i, s)| *s = (*s as f32 * i as f32 / GATE_FADE_IN as f32) as i16),
            // Closing: fade this frame out instead of cutting it.
            (true, false) => frame.iter_mut().enumerate().for_each(|(i, s)| *s = (*s as f32 * (1.0 - i as f32 / len)) as i16),
            (false, false) => frame.fill(0),
            (true, true) => {}
        }
        (level, self.open)
    }
}

/// A playback volume (1.0: unchanged) shared with the output stream, so it can change
/// while playing.
#[derive(Debug, Clone)]
pub struct Volume(Arc<AtomicU32>);

impl Volume {
    pub fn new(volume: f32) -> Volume {
        Volume(Arc::new(AtomicU32::new(volume.to_bits())))
    }

    pub fn set(&self, volume: f32) {
        self.0.store(volume.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Fill an interleaved output buffer from the mono queue at `volume` (silence when empty).
fn fill_output(out: &mut [f32], channels: usize, q: &mut VecDeque<f32>, volume: f32) {
    for frame in out.chunks_mut(channels.max(1)) {
        let s = (q.pop_front().unwrap_or(0.0) * volume).clamp(-1.0, 1.0);
        frame.iter_mut().for_each(|x| *x = s);
    }
}

/// Speaker playback; push frames with [`Speaker::play`]; dropping it stops the stream.
pub struct Speaker {
    queue: Arc<Mutex<VecDeque<f32>>>,
    resampler: Resampler,
    volume: Volume,
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
        let volume = Volume::new(1.0);
        let vol = volume.clone();
        std::thread::spawn(move || {
            let build = || -> Result<(cpal::Stream, u32)> {
                let device = pick(false, device.as_deref())?;
                let config = device.default_output_config()?.config();
                let (rate, channels) = (config.sample_rate, config.channels as usize);
                let stream = device.build_output_stream::<f32, _, _>(
                    config,
                    move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let mut q = q.lock().unwrap_or_else(|e| e.into_inner());
                        fill_output(out, channels, &mut q, vol.get());
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
        Ok(Speaker { queue, resampler: Resampler::new(SAMPLE_RATE, rate), volume, _stop: stop_tx })
    }

    /// The playback volume; changing it affects audio already queued too.
    pub fn volume(&self) -> Volume {
        self.volume.clone()
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
    fn frame_levels() {
        assert_eq!(frame_level(&[0; 960]), 0.0);
        let full: Vec<i16> = (0..960).map(|i| ((i as f32 * 0.1).sin() * i16::MAX as f32) as i16).collect();
        assert_eq!(frame_level(&full), 1.0, "a full-scale sine (-3 dBFS) is at the top");
        let quiet: Vec<i16> = full.iter().map(|s| s / 100).collect();
        let l = frame_level(&quiet);
        assert!(l > 0.0 && l < 0.4, "-43 dBFS is low: {l}");
    }

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

    /// `secs` of a sine at `amp` (0–1), as 20 ms frames.
    fn sine_frames(hz: f32, amp: f32, secs: f32) -> Vec<Vec<i16>> {
        let n = (secs * SAMPLE_RATE as f32) as usize;
        let all: Vec<i16> = (0..n).map(|i| ((i as f32 / SAMPLE_RATE as f32 * hz * std::f32::consts::TAU).sin() * amp * i16::MAX as f32) as i16).collect();
        all.chunks(FRAME_SAMPLES).map(|c| c.to_vec()).collect()
    }

    fn rms(frames: &[Vec<i16>]) -> f64 {
        let all = frames.concat();
        (all.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / all.len().max(1) as f64).sqrt()
    }

    #[test]
    fn mic_gain() {
        let mut p = MicProcessor::new();
        let input = sine_frames(440.0, 0.1, 0.2);
        let mut out = input.clone();
        let cfg = MicConfig { gain: 2.0, ..Default::default() };
        out.iter_mut().for_each(|f| assert!(p.process(f, &cfg).1, "no gate: always open"));
        let ratio = rms(&out) / rms(&input);
        assert!((1.98..2.02).contains(&ratio), "{ratio}");
        // A big gain clips instead of wrapping around.
        let mut loud = sine_frames(440.0, 0.5, 0.02);
        p.process(&mut loud[0], &MicConfig { gain: 10.0, ..Default::default() });
        assert_eq!(loud[0].iter().max(), Some(&i16::MAX));
        assert_eq!(loud[0].iter().min(), Some(&i16::MIN));
        // Unity gain, no gate, no suppression: untouched.
        let mut same = input[0].clone();
        p.process(&mut same, &MicConfig::default());
        assert_eq!(same, input[0]);
    }

    #[test]
    fn speaking_gate_opens_holds_and_closes() {
        let mut p = MicProcessor::new();
        let cfg = MicConfig { gate_db: Some(-40.0), ..Default::default() };
        // -63 dBFS: below the threshold, sent as silence.
        let quiet = sine_frames(300.0, 0.001, 0.02).remove(0);
        let mut f = quiet.clone();
        assert!(!p.process(&mut f, &cfg).1);
        assert!(f.iter().all(|s| *s == 0));
        // -23 dBFS opens it at once (faded in over the first 2 ms).
        let mut f = sine_frames(300.0, 0.1, 0.02).remove(0);
        let (level, open) = p.process(&mut f, &cfg);
        assert!(open && level > 0.5, "{level}");
        assert_eq!(f[0], 0);
        assert!(f[100..].iter().any(|s| s.abs() > 3000));
        // Quiet again: held open for the hang time, then faded out, then silent.
        for n in 0..GATE_HANG_FRAMES {
            let mut f = quiet.clone();
            assert!(p.process(&mut f, &cfg).1, "frame {n} still open");
            assert_eq!(f, quiet, "held frames pass unchanged");
        }
        // -50 dBFS DC, so the fade is easy to see.
        let mut f = vec![100i16; FRAME_SAMPLES];
        assert!(!p.process(&mut f, &cfg).1, "closes after {GATE_HANG_FRAMES} frames");
        assert!(f[0] == 100 && f[FRAME_SAMPLES / 2] == 50 && f[FRAME_SAMPLES - 1] == 0, "faded out: {} … {}", f[0], f[FRAME_SAMPLES - 1]);
        let mut f = quiet.clone();
        assert!(!p.process(&mut f, &cfg).1);
        assert!(f.iter().all(|s| *s == 0));
        // The threshold is judged after the gain.
        let mut f = quiet.clone();
        assert!(p.process(&mut f, &MicConfig { gain: 200.0, ..cfg }).1);
    }

    #[test]
    fn noise_suppression_reduces_noise() {
        // Two seconds of white noise at about -30 dBFS (a fixed LCG, so the test is repeatable).
        let mut seed = 0x2545F491u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 16) as f32 / 32_768.0 - 1.0) * 0.055 * i16::MAX as f32
        };
        let input: Vec<Vec<i16>> = (0..100).map(|_| (0..FRAME_SAMPLES).map(|_| noise() as i16).collect()).collect();
        let mut p = MicProcessor::new();
        let cfg = MicConfig { suppress_noise: true, ..Default::default() };
        let mut out = input.clone();
        out.iter_mut().for_each(|f| {
            p.process(f, &cfg);
        });
        // Judge the second half, after RNNoise has settled.
        let reduction = 20.0 * (rms(&input[50..]) / rms(&out[50..]).max(1.0)).log10();
        assert!(reduction > 10.0, "noise reduced by only {reduction:.1} dB");
    }

    #[test]
    fn playback_volume() {
        let mut q: VecDeque<f32> = [0.4, -0.4, 0.8].into_iter().collect();
        let mut out = [9.0f32; 8];
        fill_output(&mut out, 2, &mut q, 0.5);
        assert_eq!(out, [0.2, 0.2, -0.2, -0.2, 0.4, 0.4, 0.0, 0.0], "scaled, then silence once empty");
        let mut q: VecDeque<f32> = [0.8].into_iter().collect();
        let mut out = [0.0f32; 1];
        fill_output(&mut out, 1, &mut q, 2.0);
        assert_eq!(out, [1.0], "loud samples are clipped");
        let v = Volume::new(1.0);
        let shared = v.clone();
        v.set(0.25);
        assert_eq!(shared.get(), 0.25);
        v.set(-1.0);
        assert_eq!(shared.get(), 0.0);
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
