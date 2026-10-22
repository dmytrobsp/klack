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
pub const INPUT_ENV: &str = "TTYAK_AUDIO_INPUT";
pub const OUTPUT_ENV: &str = "TTYAK_AUDIO_OUTPUT";

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
                tracing::info!("microphone: {} ({} Hz, {} ch)", device_label(&device), config.sample_rate, config.channels);
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

/// Echo cancellation (WebRTC's AEC3, pure Rust via `sonora`): removes what the speaker
/// plays from the microphone. Clones share one canceller: the playback side feeds it the
/// meeting audio ([`EchoCanceller::render`]), the microphone side cleans each frame with it
/// ([`EchoCanceller::capture`]); it finds the delay between the two itself.
#[derive(Clone)]
pub struct EchoCanceller(Arc<Mutex<sonora::AudioProcessing>>);

/// The canceller works on 10 ms chunks.
const AEC_CHUNK: usize = SAMPLE_RATE as usize / 100;

impl EchoCanceller {
    pub fn new() -> EchoCanceller {
        let config = sonora::Config { echo_canceller: Some(Default::default()), ..Default::default() };
        let stream = sonora::StreamConfig::new(SAMPLE_RATE, 1);
        EchoCanceller(Arc::new(Mutex::new(sonora::AudioProcessing::builder().config(config).capture_config(stream).render_config(stream).build())))
    }

    /// Note audio (48 kHz mono) as it goes to the speaker.
    pub fn render(&self, frame: &[i16]) {
        let mut apm = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = [0i16; AEC_CHUNK];
        for chunk in frame.chunks(AEC_CHUNK).filter(|c| c.len() == AEC_CHUNK) {
            let _ = apm.process_render_i16(chunk, &mut out);
        }
    }

    /// Remove the speaker's echo from a microphone frame (48 kHz mono) in place.
    pub fn capture(&self, frame: &mut [i16]) {
        let mut apm = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = [0i16; AEC_CHUNK];
        for chunk in frame.chunks_mut(AEC_CHUNK).filter(|c| c.len() == AEC_CHUNK) {
            // Required with echo cancellation on; only a hint, AEC3 estimates the delay.
            let _ = apm.set_stream_delay_ms(0);
            if apm.process_capture_i16(chunk, &mut out).is_ok() {
                chunk.copy_from_slice(&out);
            }
        }
    }
}

impl Default for EchoCanceller {
    fn default() -> Self {
        EchoCanceller::new()
    }
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
    /// Echo cancellation (needs a canceller fed with the playback, see [`MicProcessor::with_echo_canceller`]).
    pub cancel_echo: bool,
}

impl Default for MicConfig {
    fn default() -> Self {
        MicConfig { gain: 1.0, gate_db: None, suppress_noise: false, cancel_echo: false }
    }
}

/// Frames (20 ms) the gate stays open after the level falls below the threshold, so the
/// ends of words are not cut off.
pub const GATE_HANG_FRAMES: u32 = 15;

/// Samples faded in when the gate opens (2 ms), to avoid a click.
const GATE_FADE_IN: usize = 96;

/// Echo cancellation, noise suppression, gain and the speaking gate for 20 ms 48 kHz frames,
/// in that order.
#[derive(Default)]
pub struct MicProcessor {
    echo: Option<EchoCanceller>,
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

    /// With echo cancellation by `echo` (used while [`MicConfig::cancel_echo`] is on).
    pub fn with_echo_canceller(echo: EchoCanceller) -> MicProcessor {
        MicProcessor { echo: Some(echo), ..Default::default() }
    }

    /// Process `frame` in place; returns its level (0–1, before the gate) and whether the
    /// gate let it through (a closed gate leaves silence).
    pub fn process(&mut self, frame: &mut [i16], cfg: &MicConfig) -> (f32, bool) {
        if let Some(echo) = self.echo.as_ref().filter(|_| cfg.cancel_echo) {
            echo.capture(frame);
        }
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
                tracing::info!("speaker: {} ({} Hz, {} ch)", device_label(&device), config.sample_rate, config.channels);
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

    /// Speech-like noise for the echo tests: white noise (a fixed LCG) low-passed to the speech
    /// band (AEC3 drops everything above 16 kHz) whose loudness changes every 100 ms, as
    /// frames of `secs` seconds.
    fn bursty_noise(secs: f32, seed: u32) -> Vec<Vec<i16>> {
        let mut seed = seed;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 16) as f32 / 32_768.0 - 1.0
        };
        let n = (secs * SAMPLE_RATE as f32) as usize;
        let (mut amp, mut lp) = (0.0, [0.0f32; 3]);
        let all: Vec<i16> = (0..n)
            .map(|i| {
                if i % (SAMPLE_RATE as usize / 10) == 0 {
                    amp = 0.1 + 0.5 * (next() + 1.0) / 2.0;
                }
                // Three one-pole low-passes: about -18 dB/octave above ~2.5 kHz.
                let mut x = next();
                for y in lp.iter_mut() {
                    *y += 0.3 * (x - *y);
                    x = *y;
                }
                (x * 2.5 * amp * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16
            })
            .collect();
        all.chunks(FRAME_SAMPLES).map(|c| c.to_vec()).collect()
    }

    /// What a microphone hears of `played`: delayed by `delay` samples, attenuated, through
    /// a short room response.
    fn echo_of(played: &[Vec<i16>], delay: usize) -> Vec<Vec<i16>> {
        let all = played.concat();
        let at = |i: usize| if i >= delay && i - delay < all.len() { all[i - delay] as f32 } else { 0.0 };
        let mic: Vec<i16> = (0..all.len()).map(|i| (0.3 * at(i) + 0.12 * at(i.wrapping_sub(7)) - 0.05 * at(i.wrapping_sub(23))) as i16).collect();
        mic.chunks(FRAME_SAMPLES).map(|c| c.to_vec()).collect()
    }

    #[test]
    fn echo_cancellation_removes_the_speaker() {
        // 6 s of "far end" played with a 60 ms echo path; judge the last 2 s.
        let played = bursty_noise(6.0, 7);
        let heard = echo_of(&played, 60 * SAMPLE_RATE as usize / 1000);
        let echo = EchoCanceller::new();
        let mut p = MicProcessor::with_echo_canceller(echo.clone());
        let cfg = MicConfig { cancel_echo: true, ..Default::default() };
        let mut out = heard.clone();
        for (far, near) in played.iter().zip(out.iter_mut()) {
            echo.render(far);
            p.process(near, &cfg);
        }
        let tail = out.len() - 100;
        let reduction = 20.0 * (rms(&heard[tail..]) / rms(&out[tail..]).max(1.0)).log10();
        assert!(reduction > 20.0, "echo reduced by only {reduction:.1} dB");
        // Switched off in the config, the canceller is not used.
        let mut f = heard[tail].clone();
        p.process(&mut f, &MicConfig::default());
        assert_eq!(f, heard[tail]);
    }

    /// Power of `x` at `hz` (Goertzel), per sample.
    fn power_at(x: &[i16], hz: f32) -> f64 {
        let w = std::f64::consts::TAU * hz as f64 / SAMPLE_RATE as f64;
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for v in x {
            let s = *v as f64 + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2) / x.len() as f64
    }

    #[test]
    fn echo_cancellation_in_double_talk() {
        // The far end (below ~600 Hz) plays alone for 4 s, then the near end talks too
        // (speech-like noise, high-passed) for 2 s: per band, the voice must stay and the echo
        // go. (Steady pure tones from the near end are suppressed in double talk, ~-24 dB.)
        let lowpass = |frames: &mut Vec<Vec<i16>>, a: f32, gain: f32| {
            let mut lp = [0.0f32; 3];
            for s in frames.iter_mut().flatten() {
                let mut x = *s as f32;
                for y in lp.iter_mut() {
                    *y += a * (x - *y);
                    x = *y;
                }
                *s = (x * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            }
        };
        let mut played = bursty_noise(6.0, 3);
        lowpass(&mut played, 0.08, 3.0);
        let heard = echo_of(&played, 2_400);
        // Second differences: a high-pass that leaves little below 1 kHz.
        let (mut x1, mut x2) = (0.0f32, 0.0f32);
        let voice: Vec<Vec<i16>> = bursty_noise(2.0, 17).iter().map(|f| f.iter().map(|s| {
            let x = *s as f32;
            let y = x - 2.0 * x1 + x2;
            (x2, x1) = (x1, x);
            (y * 4.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16
        }).collect()).collect();
        let mic: Vec<Vec<i16>> = heard.iter().enumerate().map(|(n, e)| match n.checked_sub(200) {
            Some(k) => e.iter().zip(&voice[k]).map(|(a, b)| a.saturating_add(*b)).collect(),
            None => e.clone(),
        }).collect();
        let echo = EchoCanceller::new();
        let mut p = MicProcessor::with_echo_canceller(echo.clone());
        let cfg = MicConfig { cancel_echo: true, ..Default::default() };
        let mut out = mic.clone();
        for (far, near) in played.iter().zip(out.iter_mut()) {
            echo.render(far);
            p.process(near, &cfg);
        }
        // The last 1.5 s of double talk.
        let (before, after) = (mic[225..].concat(), out[225..].concat());
        let band = |x: &[i16], from: usize, to: usize| (from..to).map(|k| power_at(x, k as f32 * 50.0)).sum::<f64>();
        let kept = 10.0 * (band(&after, 50, 80) / band(&before, 50, 80)).log10();
        let removed = 10.0 * (band(&before, 2, 12) / band(&after, 2, 12).max(1e-9)).log10();
        assert!(kept > -3.0, "the voice lost {:.1} dB in double talk", -kept);
        assert!(removed > 20.0, "echo reduced by only {removed:.1} dB in double talk");
    }

    #[test]
    fn echo_cancellation_keeps_the_voice() {
        // Nothing played: the near-end voice passes (AEC3's band split costs ~0.4 dB).
        let voice = bursty_noise(3.0, 11);
        let echo = EchoCanceller::new();
        let mut p = MicProcessor::with_echo_canceller(echo.clone());
        let cfg = MicConfig { cancel_echo: true, ..Default::default() };
        let silence = vec![0i16; FRAME_SAMPLES];
        let mut out = voice.clone();
        for near in out.iter_mut() {
            echo.render(&silence);
            p.process(near, &cfg);
        }
        let loss = 20.0 * (rms(&voice[50..]) / rms(&out[50..]).max(1.0)).log10();
        assert!(loss < 1.0, "voice lost {loss:.1} dB with nothing playing");
        // The microphone opened after the far end had been playing for a while (render
        // without capture while muted) still works.
        let echo = EchoCanceller::new();
        let played = bursty_noise(4.0, 5);
        played[..100].iter().for_each(|f| echo.render(f));
        let heard = echo_of(&played[100..], 2_880);
        let mut p = MicProcessor::with_echo_canceller(echo.clone());
        let mut out = heard.clone();
        for (far, near) in played[100..].iter().zip(out.iter_mut()) {
            echo.render(far);
            p.process(near, &cfg);
        }
        let reduction = 20.0 * (rms(&heard[50..]) / rms(&out[50..]).max(1.0)).log10();
        assert!(reduction > 20.0, "after unmuting: echo reduced by only {reduction:.1} dB");
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
