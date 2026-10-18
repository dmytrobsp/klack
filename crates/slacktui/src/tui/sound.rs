//! Notification sounds: short tones synthesized here and played on the speaker chosen in
//! the preferences, or the desktop's own notification sound ("system").

use std::f32::consts::PI;

/// The choices, in the order the preferences cycle through them.
pub const SOUNDS: &[&str] = &["off", "chime", "ping", "knock", "system"];

const RATE: f32 = huddle::session::SAMPLE_RATE as f32;

/// A decaying sine burst of `ms` milliseconds.
fn tone(hz: f32, ms: u32, volume: f32) -> Vec<f32> {
    let n = (RATE * ms as f32 / 1000.0) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE;
            let attack = (i as f32 / (RATE * 0.004)).min(1.0);
            let decay = (-t * 1000.0 / ms as f32 * 4.0).exp();
            (2.0 * PI * hz * t).sin() * attack * decay * volume
        })
        .collect()
}

fn silence(ms: u32) -> Vec<f32> {
    vec![0.0; (RATE * ms as f32 / 1000.0) as usize]
}

/// 48 kHz mono samples of a built-in sound (empty for `off`, `system` and unknown names).
pub fn samples(name: &str) -> Vec<i16> {
    let f: Vec<f32> = match name {
        "chime" => [tone(880.0, 140, 0.35), tone(1318.5, 220, 0.35)].concat(),
        "ping" => tone(1760.0, 180, 0.3),
        "knock" => [tone(196.0, 60, 0.6), silence(70), tone(196.0, 60, 0.6)].concat(),
        _ => vec![],
    };
    f.into_iter().map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).collect()
}

/// Play a built-in sound without blocking (no-op for `off`/`system`). `device`: part of an
/// output device's name, None for the default; `volume` in percent.
pub fn play(name: &str, device: Option<String>, volume: u32) {
    let samples = samples(name);
    if samples.is_empty() {
        return;
    }
    std::thread::spawn(move || match huddle::devices::Speaker::start(device) {
        Ok(mut speaker) => {
            speaker.volume().set(volume as f32 / 100.0);
            speaker.play(&samples);
            // Let it drain before the stream is dropped.
            std::thread::sleep(std::time::Duration::from_millis(samples.len() as u64 * 1000 / RATE as u64 + 250));
        }
        Err(e) => tracing::warn!("notification sound: {e}"),
    });
}

/// The notification server's sound name for "system".
pub fn system_sound_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "NSUserNotificationDefaultSoundName"
    } else if cfg!(target_os = "windows") {
        "Default"
    } else {
        "message-new-instant"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_sounds() {
        assert!(samples("off").is_empty() && samples("system").is_empty() && samples("nope").is_empty());
        let chime = samples("chime");
        assert_eq!(chime.len(), (RATE * 0.36) as usize, "two tones, 360 ms");
        assert!(chime.iter().map(|s| s.unsigned_abs()).max().unwrap() > 5_000, "audible");
        assert!(chime.len() < huddle::session::SAMPLE_RATE as usize / 2, "fits the speaker's 0.5 s queue");
        assert!(samples("knock").len() < samples("chime").len());
    }
}
