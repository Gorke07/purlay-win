//! Purlay's notification tones, drawn here from sine waves rather than shipped as files:
//! short, soft (peaks at about −8 dBFS) and free of clicks (every tone fades in and out).
//! Each is rendered once to a 16-bit mono WAV and kept for the life of the process.

use std::f32::consts::TAU;
use std::sync::OnceLock;

const RATE: u32 = 44_100;

/// A notification tone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    /// A water drop: a quick rising blip.
    Drop,
    /// Two bell notes, a fifth apart.
    Chime,
    /// A soft wooden knock, like a marimba bar.
    Marimba,
    /// Three glassy notes of a major chord, rolled upwards.
    Glass,
}

impl Tone {
    pub const ALL: [Tone; 4] = [Tone::Drop, Tone::Chime, Tone::Marimba, Tone::Glass];
}

/// One struck note: partials `(ratio, gain)` over `freq`, decaying with `decay` seconds
/// (the time to fall to 1/e), starting at `at` seconds.
struct Note {
    at: f32,
    freq: f32,
    decay: f32,
    gain: f32,
    partials: &'static [(f32, f32, f32)],
}

/// Adds a note to the buffer. Each partial is `(ratio, gain, decay factor)`: higher
/// partials usually die faster, which is what makes a bell or a bar sound struck.
fn strike(buf: &mut [f32], n: &Note) {
    let start = (n.at * RATE as f32) as usize;
    let attack = (0.004 * RATE as f32) as usize;
    for (i, s) in buf.iter_mut().enumerate().skip(start) {
        let k = i - start;
        let t = k as f32 / RATE as f32;
        let env_attack = if k < attack { k as f32 / attack as f32 } else { 1.0 };
        let mut v = 0.0;
        for &(ratio, gain, decay_factor) in n.partials {
            let env = (-t / (n.decay * decay_factor)).exp();
            v += gain * env * (TAU * n.freq * ratio * t).sin();
        }
        *s += n.gain * env_attack * v;
    }
}

fn render(tone: Tone) -> Vec<f32> {
    match tone {
        Tone::Drop => {
            // A sine whose pitch rises fast, then rings out: the "bloop" of a drop.
            let len = (0.32 * RATE as f32) as usize;
            let mut buf = vec![0.0; len];
            let mut phase = 0.0f32;
            for (i, s) in buf.iter_mut().enumerate() {
                let t = i as f32 / RATE as f32;
                let glide = 1.0 - (-t / 0.018).exp();
                let freq = 620.0 + 900.0 * glide;
                phase += TAU * freq / RATE as f32;
                let attack = (t / 0.003).min(1.0);
                let env = attack * (-t / 0.07).exp();
                *s = env * (phase.sin() + 0.18 * (2.0 * phase).sin());
            }
            buf
        }
        Tone::Chime => {
            // Bell partials (1, 2.76, 5.4 are the classic inharmonic bell ratios), softened.
            const BELL: &[(f32, f32, f32)] = &[(1.0, 1.0, 1.0), (2.0, 0.28, 0.6), (2.76, 0.16, 0.4), (5.4, 0.05, 0.2)];
            let mut buf = vec![0.0; (0.95 * RATE as f32) as usize];
            strike(&mut buf, &Note { at: 0.0, freq: 1046.5, decay: 0.32, gain: 0.8, partials: BELL });
            strike(&mut buf, &Note { at: 0.13, freq: 1568.0, decay: 0.38, gain: 0.75, partials: BELL });
            buf
        }
        Tone::Marimba => {
            // A bar: the fundamental and its fourth partial, the latter gone almost at once.
            const BAR: &[(f32, f32, f32)] = &[(1.0, 1.0, 1.0), (3.93, 0.35, 0.18), (9.2, 0.06, 0.08)];
            let mut buf = vec![0.0; (0.5 * RATE as f32) as usize];
            strike(&mut buf, &Note { at: 0.0, freq: 784.0, decay: 0.12, gain: 0.9, partials: BAR });
            strike(&mut buf, &Note { at: 0.085, freq: 1174.7, decay: 0.14, gain: 0.8, partials: BAR });
            buf
        }
        Tone::Glass => {
            // A rolled A-major chord in a glassy register, with a little shimmer on top.
            const GLASS: &[(f32, f32, f32)] = &[(1.0, 1.0, 1.0), (2.0, 0.22, 0.5), (3.0, 0.08, 0.3), (4.2, 0.04, 0.2)];
            let mut buf = vec![0.0; (0.9 * RATE as f32) as usize];
            for (i, f) in [1760.0f32, 2217.5, 2637.0].into_iter().enumerate() {
                strike(&mut buf, &Note { at: i as f32 * 0.065, freq: f, decay: 0.22, gain: 0.55, partials: GLASS });
            }
            buf
        }
    }
}

/// The tone as a complete WAV file (16-bit PCM, mono, 44.1 kHz).
pub fn wav(tone: Tone) -> &'static [u8] {
    static CACHE: [OnceLock<Vec<u8>>; 4] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let i = Tone::ALL.iter().position(|t| *t == tone).unwrap_or(0);
    CACHE[i].get_or_init(|| encode(&finish(render(tone))))
}

/// Normalises to the target peak and fades the last 30 ms to silence.
fn finish(mut buf: Vec<f32>) -> Vec<f32> {
    const PEAK: f32 = 0.40; // about −8 dBFS: noticeable, never startling.
    let max = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if max > 0.0 {
        let k = PEAK / max;
        buf.iter_mut().for_each(|s| *s *= k);
    }
    let fade = (0.03 * RATE as f32) as usize;
    let n = buf.len();
    for i in 0..fade.min(n) {
        buf[n - 1 - i] *= i as f32 / fade as f32;
    }
    buf
}

fn encode(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Plays the tone without waiting; a tone already playing is cut off by the new one.
#[cfg(windows)]
pub fn play(tone: Tone) {
    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    let bytes = wav(tone);
    // SAFETY: with SND_MEMORY the "name" is the WAV image itself; it is 'static, so it
    // outlives the asynchronous playback.
    unsafe {
        let _ = PlaySoundW(PCWSTR(bytes.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

/// Plays the notification sound chosen in Windows (for a preview; toasts play it themselves).
#[cfg(windows)]
pub fn play_system() {
    use windows::core::w;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC};
    unsafe {
        let _ = PlaySoundW(w!("Notification.Default"), None, SND_ALIAS | SND_ASYNC);
    }
}

#[cfg(not(windows))]
pub fn play(_tone: Tone) {}

#[cfg(not(windows))]
pub fn play_system() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tone_is_a_short_valid_wav_without_clipping() {
        for t in Tone::ALL {
            let w = wav(t);
            assert_eq!(&w[0..4], b"RIFF");
            assert_eq!(&w[8..16], b"WAVEfmt ");
            let data_len = u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize;
            assert_eq!(w.len(), 44 + data_len);
            let secs = data_len as f32 / 2.0 / RATE as f32;
            assert!(secs > 0.2 && secs < 1.0, "{t:?} lasts {secs}s");
            let peak = w[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap();
            assert!(peak > 10_000 && peak < 16_000, "{t:?} peaks at {peak}");
            // Starts and ends in silence: no click.
            let first = i16::from_le_bytes([w[44], w[45]]).unsigned_abs();
            let last = i16::from_le_bytes([w[w.len() - 2], w[w.len() - 1]]).unsigned_abs();
            assert!(first < 200 && last < 200, "{t:?} edges {first} {last}");
        }
    }
}
