//! Instrument definitions: what a game (or a patch file) authors, and what the
//! voice actually runs.
//!
//! Two types, on purpose. [`Patch`] is the authoring surface — every field a
//! `u8`/`u16`/`i8`, which is both the byte world the VM already lives in and the
//! range a model writes correctly without a units table. [`VoiceParams`] is the
//! same instrument with the arithmetic already done: rates and coefficients in
//! float, at a known sample rate.
//!
//! The conversion happens **once**, when a bank is loaded — never in `render`.

/// The oscillator a voice runs.
///
/// Naive (un-bandlimited) shapes: `saw` and `square` alias at high notes, and
/// that is the sound of the thing this console is imitating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Waveform {
    Sine,
    #[default]
    Triangle,
    Saw,
    Square,
    /// Pitched noise: a fresh random sample each time the phase wraps, so noise
    /// still responds to the note and to the pitch envelope. That is what makes
    /// one waveform cover kicks, snares, lasers, and explosions.
    Noise,
}

/// An instrument, as authored.
///
/// Times are milliseconds, levels are `0..=255`, and `pitch_env` is in
/// semitones. Filter, LFO, and effect sends are not here yet — they arrive with
/// the steps that render them (see `docs/AUDIO.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Patch {
    pub wave: Waveform,

    /// Time to reach full level, linearly.
    pub attack_ms: u16,
    /// Time to fall from full level to `sustain`.
    pub decay_ms: u16,
    /// Level held while the note is on, `0..=255`.
    pub sustain: u8,
    /// Time to fall from `sustain` to silence after the note is released.
    pub release_ms: u16,

    /// Semitones the pitch starts above (or below, if negative) the note.
    ///
    /// With `Noise` this is a snare; with `Sine` and a short decay it is a
    /// kick; with `Saw` and a long one it is a laser. It earns its place ahead
    /// of an LFO for exactly that reason.
    pub pitch_env: i8,
    /// How long the pitch envelope takes to collapse to the played note.
    pub pitch_decay_ms: u16,

    /// Instrument level, `0..=255`.
    pub volume: u8,
}

impl Default for Patch {
    /// A plain, audible instrument — what an unspecified `instrument` block
    /// gets, and what a test can play without describing a sound first.
    fn default() -> Self {
        Patch {
            wave: Waveform::Triangle,
            attack_ms: 2,
            decay_ms: 60,
            sustain: 180,
            release_ms: 80,
            pitch_env: 0,
            pitch_decay_ms: 40,
            volume: 200,
        }
    }
}

/// A [`Patch`] with the arithmetic done, at one sample rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceParams {
    pub wave: Waveform,
    /// Envelope level gained per sample during attack (`>= 1.0` means instant).
    pub attack_rate: f32,
    /// Per-sample multiplier pulling the envelope toward `sustain`.
    pub decay_coef: f32,
    pub sustain: f32,
    /// Per-sample multiplier pulling the envelope toward zero.
    pub release_coef: f32,
    pub pitch_env: f32,
    pub pitch_coef: f32,
    pub volume: f32,
}

impl Patch {
    /// Resolve this patch against a sample rate. Call at load time, not in the
    /// render loop.
    pub fn compile(&self, sample_rate: u32) -> VoiceParams {
        let sr = sample_rate as f32;
        VoiceParams {
            wave: self.wave,
            attack_rate: linear_rate(self.attack_ms, sr),
            decay_coef: decay_coef(self.decay_ms, sr),
            sustain: self.sustain as f32 / 255.0,
            release_coef: decay_coef(self.release_ms, sr),
            pitch_env: self.pitch_env as f32,
            pitch_coef: decay_coef(self.pitch_decay_ms, sr),
            volume: self.volume as f32 / 255.0,
        }
    }
}

/// Level gained per sample to cover the full range in `ms`. Zero time is
/// instant, which is what a percussive attack wants.
fn linear_rate(ms: u16, sample_rate: f32) -> f32 {
    if ms == 0 {
        return 1.0;
    }
    1000.0 / (ms as f32 * sample_rate)
}

/// Per-sample multiplier for an exponential approach that covers 99% of the
/// distance in `ms`.
///
/// Exponential rather than linear because a linearly-decaying bell or kick
/// sounds like a mistake — the ear hears amplitude logarithmically.
fn decay_coef(ms: u16, sample_rate: f32) -> f32 {
    if ms == 0 {
        return 0.0;
    }
    // 99% covered when t = 4.6 tau, so tau = ms / 4.6 (in seconds / 1000).
    let tau_samples = (ms as f32 / 1000.0) * sample_rate / 4.6;
    (-1.0 / tau_samples).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_times_are_instant() {
        let p = Patch {
            attack_ms: 0,
            decay_ms: 0,
            release_ms: 0,
            ..Patch::default()
        };
        let v = p.compile(48_000);
        assert!(v.attack_rate >= 1.0);
        assert_eq!(v.decay_coef, 0.0);
        assert_eq!(v.release_coef, 0.0);
    }

    #[test]
    fn decay_coef_covers_99_percent_in_the_stated_time() {
        let sr = 48_000;
        let coef = decay_coef(100, sr as f32);
        // Apply it for 100 ms and see what is left of a unit level.
        let mut level = 1.0f32;
        for _ in 0..(sr / 10) {
            level *= coef;
        }
        assert!(level < 0.02, "left {level} after the stated decay time");
        assert!(level > 0.001, "collapsed too fast: {level}");
    }

    #[test]
    fn levels_map_to_unit_range() {
        let v = Patch {
            sustain: 255,
            volume: 255,
            ..Patch::default()
        }
        .compile(48_000);
        assert_eq!(v.sustain, 1.0);
        assert_eq!(v.volume, 1.0);
    }
}
