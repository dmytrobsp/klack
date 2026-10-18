//! Video for huddles: H.264 (Constrained Baseline, the WebRTC default) via OpenH264 built from
//! source, and a synthetic test pattern to send instead of a camera.

use anyhow::{Result, anyhow};
use openh264::OpenH264API;
use openh264::decoder::Decoder;
use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile};
use openh264::formats::{RgbSliceU8, YUVBuffer, YUVSource};

/// Size and rate of the test pattern.
pub const TEST_WIDTH: usize = 640;
pub const TEST_HEIGHT: usize = 360;
pub const TEST_FPS: u32 = 15;

/// The colour bars of the test pattern, left to right (RGB).
pub const BARS: [[u8; 3]; 7] = [[235, 235, 235], [235, 235, 16], [16, 235, 235], [16, 235, 16], [235, 16, 235], [235, 16, 16], [16, 16, 235]];

/// Frame `n` of the test pattern as packed RGB: colour bars over the top two thirds, and a
/// white square moving left to right across a dark band below, so motion is visible.
pub fn test_pattern(n: u64, width: usize, height: usize) -> Vec<u8> {
    let mut rgb = vec![0u8; width * height * 3];
    let band = height * 2 / 3;
    let side = (height - band) / 2;
    let x0 = (n as usize * 8) % width.saturating_sub(side).max(1);
    let y0 = band + (height - band - side) / 2;
    for y in 0..height {
        for x in 0..width {
            let px = if y < band {
                BARS[x * BARS.len() / width]
            } else if (x0..x0 + side).contains(&x) && (y0..y0 + side).contains(&y) {
                [235, 235, 235]
            } else {
                [40, 40, 40]
            };
            rgb[(y * width + x) * 3..][..3].copy_from_slice(&px);
        }
    }
    rgb
}

/// Where the moving square of frame `n` starts (its left column).
pub fn test_square_x(n: u64, width: usize, height: usize) -> usize {
    let side = (height - height * 2 / 3) / 2;
    (n as usize * 8) % width.saturating_sub(side).max(1)
}

/// H.264 encoder for real-time video: Annex-B access units, a keyframe every two seconds
/// and on request (a receiver's PLI).
pub struct H264Encoder {
    inner: Encoder,
    width: usize,
    height: usize,
}

impl H264Encoder {
    pub fn new(width: usize, height: usize, fps: u32, bitrate_bps: u32) -> Result<H264Encoder> {
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .profile(Profile::Baseline)
            .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 2))
            .skip_frames(false);
        let inner = Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| anyhow!("H.264 encoder: {e}"))?;
        Ok(H264Encoder { inner, width, height })
    }

    /// Encode one packed-RGB frame of the encoder's size.
    pub fn encode_rgb(&mut self, rgb: &[u8]) -> Result<Vec<u8>> {
        if rgb.len() != self.width * self.height * 3 {
            return Err(anyhow!("frame is {} bytes, expected {}×{} RGB", rgb.len(), self.width, self.height));
        }
        let yuv = YUVBuffer::from_rgb8_source(RgbSliceU8::new(rgb, (self.width, self.height)));
        Ok(self.inner.encode(&yuv).map_err(|e| anyhow!("H.264 encode: {e}"))?.to_vec())
    }

    /// Make the next frame a keyframe.
    pub fn request_keyframe(&mut self) {
        self.inner.force_intra_frame();
    }
}

/// H.264 decoder: Annex-B access units in, packed RGB frames out.
pub struct H264Decoder {
    inner: Decoder,
}

impl H264Decoder {
    pub fn new() -> Result<H264Decoder> {
        Ok(H264Decoder { inner: Decoder::new().map_err(|e| anyhow!("H.264 decoder: {e}"))? })
    }

    /// Decode an access unit; a picture when one is complete: (width, height, RGB).
    pub fn decode(&mut self, annex_b: &[u8]) -> Result<Option<(usize, usize, Vec<u8>)>> {
        match self.inner.decode(annex_b).map_err(|e| anyhow!("H.264 decode: {e}"))? {
            Some(yuv) => {
                let (w, h) = yuv.dimensions();
                let mut rgb = vec![0u8; w * h * 3];
                yuv.write_rgb8(&mut rgb);
                Ok(Some((w, h, rgb)))
            }
            None => Ok(None),
        }
    }
}

/// Reassembles H.264 access units (Annex-B) from RTP packets: payloads are depacketized
/// (single NALs, STAP-A, FU-A) and collected until the marker bit. A sequence gap spoils the
/// unit being collected (it is dropped; the caller should ask for a keyframe).
#[derive(Default)]
pub struct AccessUnitAssembler {
    depacketizer: rtc::rtp::codec::h264::H264Packet,
    buf: Vec<u8>,
    last_seq: Option<u16>,
    broken: bool,
    /// Units dropped because of a gap or a bad payload.
    pub dropped: u64,
}

impl AccessUnitAssembler {
    /// Feed one packet; a complete access unit when this packet ends one.
    pub fn push(&mut self, packet: &rtc::rtp::packet::Packet) -> Option<Vec<u8>> {
        use rtc::rtp::packetizer::Depacketizer;
        let seq = packet.header.sequence_number;
        if self.last_seq.is_some_and(|last| seq != last.wrapping_add(1)) {
            self.broken = true;
        }
        self.last_seq = Some(seq);
        match self.depacketizer.depacketize(&packet.payload) {
            Ok(bytes) => self.buf.extend_from_slice(&bytes),
            Err(_) => self.broken = true,
        }
        if !packet.header.marker {
            return None;
        }
        let unit = std::mem::take(&mut self.buf);
        let broken = std::mem::replace(&mut self.broken, false);
        if broken || unit.is_empty() {
            self.dropped += 1;
            self.depacketizer = Default::default();
            return None;
        }
        Some(unit)
    }
}

/// NAL unit types in an Annex-B access unit (start codes 00 00 01 or 00 00 00 01).
pub fn nal_types(annex_b: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut i = 0;
    while i + 3 <= annex_b.len() {
        if annex_b[i..i + 3] == [0, 0, 1] {
            if let Some(b) = annex_b.get(i + 3) {
                types.push(b & 0x1f);
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    types
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(rgb: &[u8], w: usize, x: usize, y: usize) -> [i32; 3] {
        let i = (y * w + x) * 3;
        [rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32]
    }

    #[test]
    fn pattern_has_bars_and_a_moving_square() {
        let (w, h) = (TEST_WIDTH, TEST_HEIGHT);
        let f = test_pattern(0, w, h);
        assert_eq!(f.len(), w * h * 3);
        assert_eq!(pixel(&f, w, 5, 5), [235, 235, 235]);
        assert_eq!(pixel(&f, w, w - 5, 5), [16, 16, 235]);
        let x = test_square_x(10, w, h);
        assert_eq!(x, 80);
        assert_eq!(pixel(&test_pattern(10, w, h), w, x + 5, h * 5 / 6), [235, 235, 235]);
        assert_eq!(pixel(&test_pattern(0, w, h), w, x + 5, h * 5 / 6), [40, 40, 40], "the square has moved on");
    }

    #[test]
    fn h264_round_trip_keeps_the_picture() {
        let (w, h) = (TEST_WIDTH, TEST_HEIGHT);
        let mut enc = H264Encoder::new(w, h, TEST_FPS, 600_000).unwrap();
        let mut dec = H264Decoder::new().unwrap();
        let mut last = None;
        for n in 0..10 {
            let au = enc.encode_rgb(&test_pattern(n, w, h)).unwrap();
            let types = nal_types(&au);
            if n == 0 {
                assert!(types.contains(&7) && types.contains(&8) && types.contains(&5), "first frame: SPS, PPS, IDR: {types:?}");
            }
            if let Some(pic) = dec.decode(&au).unwrap() {
                last = Some((n, pic));
            }
        }
        let (n, (dw, dh, rgb)) = last.expect("decoded pictures");
        assert_eq!((dw, dh), (w, h));
        // Each bar's colour survives (within compression error).
        for (i, bar) in BARS.iter().enumerate() {
            let x = (2 * i + 1) * w / (2 * BARS.len());
            let got = pixel(&rgb, w, x, h / 3);
            assert!(got.iter().zip(bar).all(|(g, b)| (g - *b as i32).abs() < 30), "bar {i}: {got:?} vs {bar:?}");
        }
        // The square is where frame n put it.
        let x = test_square_x(n, w, h);
        assert!(pixel(&rgb, w, x + 10, h * 5 / 6)[0] > 180, "square at {x}");
        // Periodic keyframes: every two seconds.
        let mut enc2 = H264Encoder::new(w, h, TEST_FPS, 600_000).unwrap();
        let idr: Vec<u64> = (0..61).filter(|n| nal_types(&enc2.encode_rgb(&test_pattern(*n, w, h)).unwrap()).contains(&5)).collect();
        assert_eq!(idr, vec![0, 30, 60], "a keyframe every {} frames", TEST_FPS * 2);
        // A requested keyframe is an IDR.
        enc.request_keyframe();
        assert!(nal_types(&enc.encode_rgb(&test_pattern(11, w, h)).unwrap()).contains(&5));
        assert!(enc.encode_rgb(&[0; 3]).is_err(), "wrong size");
    }

    /// RTP packets for `units`, as webrtc-rs's own H.264 payloader makes them.
    fn packetize(units: &[Vec<u8>], first_seq: u16) -> Vec<rtc::rtp::packet::Packet> {
        use rtc::rtp::packetizer::Payloader;
        let mut payloader = rtc::rtp::codec::h264::H264Payloader::default();
        let mut seq = first_seq;
        let mut out = Vec::new();
        for unit in units {
            let payloads = payloader.payload(1200, &bytes::Bytes::from(unit.clone())).unwrap();
            let n = payloads.len();
            for (i, payload) in payloads.into_iter().enumerate() {
                let mut p = rtc::rtp::packet::Packet { payload, ..Default::default() };
                p.header.sequence_number = seq;
                p.header.marker = i + 1 == n;
                seq = seq.wrapping_add(1);
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn rtp_round_trip_and_gaps() {
        let (w, h) = (TEST_WIDTH, TEST_HEIGHT);
        let mut enc = H264Encoder::new(w, h, TEST_FPS, 600_000).unwrap();
        // Noise in the dark band makes the keyframe span many packets.
        let mut seed = 7u32;
        let noisy = |n: u64, seed: &mut u32| {
            let mut rgb = test_pattern(n, w, h);
            for px in rgb[w * h * 2..].iter_mut() {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *px = (*seed >> 24) as u8;
            }
            rgb
        };
        let units: Vec<Vec<u8>> = (0..5).map(|n| enc.encode_rgb(&noisy(n, &mut seed)).unwrap()).collect();
        // Across the sequence number wrap.
        let packets = packetize(&units, u16::MAX - 2);
        assert!(packets.len() > units.len(), "the keyframe needs several packets");
        let mut asm = AccessUnitAssembler::default();
        let mut dec = H264Decoder::new().unwrap();
        let mut pictures = 0;
        for p in &packets {
            if let Some(unit) = asm.push(p) {
                if let Some((dw, dh, rgb)) = dec.decode(&unit).unwrap() {
                    assert_eq!((dw, dh), (w, h));
                    assert!(pixel(&rgb, w, 5, 5)[0] > 200, "white bar");
                    pictures += 1;
                }
            }
        }
        assert_eq!((pictures, asm.dropped), (5, 0));
        // A lost packet inside the keyframe spoils that unit only.
        let mut packets = packetize(&units, 100);
        let end = packets.iter().position(|p| p.header.marker).unwrap();
        assert!(end >= 2);
        packets.remove(end - 1);
        let mut asm = AccessUnitAssembler::default();
        let got = packets.iter().filter_map(|p| asm.push(p)).count();
        assert_eq!((got, asm.dropped), (4, 1));
    }
}
