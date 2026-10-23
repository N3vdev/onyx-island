//! The soft start/stop/done tones for dictation.
//!
//! Generated at first use (a few milliseconds of sine with a smooth
//! envelope) and played with `PlaySound` straight from memory, so there are
//! no audio files and no output stream to keep open.

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

const SAMPLE_RATE: u32 = 24_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Recording started: a short rising blip.
    Start,
    /// Recording stopped: a short falling blip.
    Stop,
    /// Text was inserted: a soft two-note chime.
    Done,
}

/// A mono 16-bit PCM WAV of one or more notes `(hz, seconds)`.
fn wav(notes: &[(f32, f32)], volume: f32) -> Vec<u8> {
    let mut samples: Vec<i16> = Vec::new();
    for &(hz, seconds) in notes {
        let count = (SAMPLE_RATE as f32 * seconds) as usize;
        for index in 0..count {
            let t = index as f32 / SAMPLE_RATE as f32;
            let position = index as f32 / count as f32;
            // Raised-cosine fade in and out: no clicks.
            let envelope = (std::f32::consts::PI * position).sin().powi(2);
            let value = (std::f32::consts::TAU * hz * t).sin() * envelope * volume;
            samples.push((value * f32::from(i16::MAX)) as i16);
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn bytes_for(tone: Tone) -> &'static [u8] {
    static START: OnceLock<Vec<u8>> = OnceLock::new();
    static STOP: OnceLock<Vec<u8>> = OnceLock::new();
    static DONE: OnceLock<Vec<u8>> = OnceLock::new();
    match tone {
        Tone::Start => START.get_or_init(|| wav(&[(660.0, 0.045), (880.0, 0.045)], 0.16)),
        Tone::Stop => STOP.get_or_init(|| wav(&[(880.0, 0.045), (587.0, 0.055)], 0.16)),
        Tone::Done => DONE.get_or_init(|| wav(&[(784.0, 0.06), (1047.0, 0.09)], 0.14)),
    }
}

/// Plays a tone without blocking. The buffer is `'static`, as `SND_ASYNC |
/// SND_MEMORY` requires it to outlive the call.
pub fn play(tone: Tone) {
    let bytes = bytes_for(tone);
    // SAFETY: `bytes` is a complete in-memory WAV that lives for the whole
    // program; the flags tell Windows to read it from memory asynchronously.
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(bytes.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_consistent() {
        let bytes = wav(&[(440.0, 0.05)], 0.2);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        let data_len = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
        assert_eq!(bytes.len(), 44 + data_len);
        let riff_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        assert_eq!(riff_len, bytes.len() - 8);
        assert_eq!(data_len, (SAMPLE_RATE as f32 * 0.05) as usize * 2);
    }

    #[test]
    fn tones_start_and_end_silent_and_stay_quiet() {
        for tone in [Tone::Start, Tone::Stop, Tone::Done] {
            let bytes = bytes_for(tone);
            let samples: Vec<i16> = bytes[44..]
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            assert!(samples[0].abs() < 50 && samples[samples.len() - 1].abs() < 50);
            let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(peak > 2_000, "audible");
            assert!(peak < 8_000, "soft: {peak}");
        }
    }
}
