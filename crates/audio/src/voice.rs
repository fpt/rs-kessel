//! One voice: an oscillator, an amplitude envelope, and a pitch envelope.
//!
//! Everything here is fixed-size and allocation-free — a voice is a plain
//! struct that lives in the synth's array for the life of the process, so
//! `render_add` can run on an audio callback thread.

use crate::patch::{VoiceParams, Waveform};

/// Below this level a decaying envelope is finished. `-80 dB`: far under
/// anything a 16-bit output can carry, so nothing audible is cut off.
const SILENCE: f32 = 1e-4;

/// Which sound gets to keep a voice when they compete.
///
/// Music loses to sound effects: a bassline eating an explosion is much more
/// noticeable than the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Priority {
    Music = 0,
    #[default]
    Sfx = 1,
}

/// Where a voice is in its amplitude envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// A deterministic PRNG for the noise oscillator.
///
/// Never OS randomness: an offline render of the same event log has to produce
/// the same WAV, or the agent loop's audio observable means nothing.
#[derive(Debug, Clone, Copy)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        // Zero is a fixed point of xorshift; any nonzero substitute will do.
        Rng(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    /// Next sample in `[-1, 1)`.
    fn next_bipolar(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        // Top 24 bits into a float, so the mantissa is exact.
        ((x >> 8) as f32 / 8_388_608.0) - 1.0
    }
}

/// A single sounding note.
#[derive(Debug, Clone, Copy)]
pub struct Voice {
    params: VoiceParams,
    stage: Stage,

    /// Oscillator phase in `[0, 1)`.
    phase: f32,
    /// Phase step per sample for the played note, before the pitch envelope.
    base_inc: f32,
    /// Current pitch-envelope offset, in semitones, collapsing toward zero.
    pitch_off: f32,
    /// Held sample for [`Waveform::Noise`], refreshed when the phase wraps.
    noise: f32,
    rng: Rng,

    /// Envelope level, `0..=1`.
    env: f32,
    /// Velocity × instrument volume.
    gain: f32,

    /// Samples left before an auto-release (`Play`), or `None` for a held note.
    hold: Option<u32>,
    /// The game-owned channel that can release this voice, if any.
    pub chan: Option<u8>,
    pub priority: Priority,
    /// Allocation order, for oldest-first stealing.
    pub age: u64,
}

impl Voice {
    pub fn new(seed: u32) -> Self {
        Voice {
            params: crate::patch::Patch::default().compile(48_000),
            stage: Stage::Idle,
            phase: 0.0,
            base_inc: 0.0,
            pitch_off: 0.0,
            noise: 0.0,
            rng: Rng::new(seed),
            env: 0.0,
            gain: 0.0,
            hold: None,
            chan: None,
            priority: Priority::Sfx,
            age: 0,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    pub fn is_releasing(&self) -> bool {
        self.stage == Stage::Release
    }

    pub fn env(&self) -> f32 {
        self.env
    }

    /// Start a note. Reuses this voice whatever it was doing — the caller has
    /// already decided it is the one to take.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        params: VoiceParams,
        note: u8,
        vel: u8,
        hold: Option<u32>,
        chan: Option<u8>,
        priority: Priority,
        age: u64,
        sample_rate: u32,
    ) {
        self.params = params;
        self.stage = Stage::Attack;
        self.phase = 0.0;
        self.base_inc = note_hz(note) / sample_rate as f32;
        self.pitch_off = params.pitch_env;
        self.env = 0.0;
        self.gain = (vel as f32 / 255.0) * params.volume;
        self.hold = hold;
        self.chan = chan;
        self.priority = priority;
        self.age = age;
        // Note deliberately *not* reset: `rng` and `noise`. Restarting the
        // noise sequence on every hit makes a machine-gun sound identical
        // shot to shot, which is exactly the artifact this avoids.
    }

    /// Enter the release stage. A voice already releasing or idle is untouched.
    pub fn release(&mut self) {
        if !matches!(self.stage, Stage::Idle | Stage::Release) {
            self.stage = Stage::Release;
            self.hold = None;
        }
    }

    /// Stop immediately, with no release tail.
    pub fn kill(&mut self) {
        self.stage = Stage::Idle;
        self.env = 0.0;
        self.chan = None;
        self.hold = None;
    }

    /// Add this voice into an interleaved stereo buffer.
    ///
    /// Panning lands with the filter and master stage; for now a voice sits in
    /// the middle of the image.
    pub fn render_add(&mut self, out: &mut [f32]) {
        if self.stage == Stage::Idle {
            return;
        }
        for frame in out.chunks_exact_mut(2) {
            if !self.advance_env() {
                break;
            }
            let s = self.next_sample() * self.env * self.gain;
            frame[0] += s;
            frame[1] += s;
        }
    }

    /// Step the amplitude envelope, the hold counter, and the pitch envelope by
    /// one sample. Returns `false` once the voice has gone idle.
    fn advance_env(&mut self) -> bool {
        if let Some(left) = self.hold {
            if left == 0 {
                self.release();
            } else {
                self.hold = Some(left - 1);
            }
        }
        match self.stage {
            Stage::Idle => return false,
            Stage::Attack => {
                self.env += self.params.attack_rate;
                if self.env >= 1.0 {
                    self.env = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                let sustain = self.params.sustain;
                self.env = sustain + (self.env - sustain) * self.params.decay_coef;
                if self.env - sustain <= SILENCE {
                    self.env = sustain;
                    // A patch that sustains at zero is percussive: it is done
                    // when the decay finishes, without waiting for a release
                    // that a fire-and-forget note may never send.
                    self.stage = if sustain <= SILENCE {
                        Stage::Idle
                    } else {
                        Stage::Sustain
                    };
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                self.env *= self.params.release_coef;
                if self.env <= SILENCE {
                    self.kill();
                    return false;
                }
            }
        }
        self.pitch_off *= self.params.pitch_coef;
        true
    }

    /// Advance the oscillator by one sample and return its value in `[-1, 1]`.
    fn next_sample(&mut self) -> f32 {
        let inc = if self.pitch_off.abs() > 1e-3 {
            self.base_inc * exp2(self.pitch_off / 12.0)
        } else {
            self.base_inc
        };
        self.phase += inc;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.noise = self.rng.next_bipolar();
        }
        let p = self.phase;
        match self.params.wave {
            Waveform::Sine => (p * std::f32::consts::TAU).sin(),
            Waveform::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
            Waveform::Saw => 2.0 * p - 1.0,
            Waveform::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Waveform::Noise => self.noise,
        }
    }
}

/// MIDI note number → Hz (note 69 = A440).
pub fn note_hz(note: u8) -> f32 {
    440.0 * exp2((note as f32 - 69.0) / 12.0)
}

fn exp2(x: f32) -> f32 {
    x.exp2()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::Patch;

    fn params(p: Patch) -> VoiceParams {
        p.compile(48_000)
    }

    fn play(p: Patch, note: u8, samples: usize) -> Vec<f32> {
        let mut v = Voice::new(1);
        v.start(params(p), note, 255, None, None, Priority::Sfx, 0, 48_000);
        let mut buf = vec![0.0; samples * 2];
        v.render_add(&mut buf);
        buf
    }

    #[test]
    fn note_69_is_a440() {
        assert!((note_hz(69) - 440.0).abs() < 0.001);
        assert!((note_hz(81) - 880.0).abs() < 0.01);
        assert!((note_hz(57) - 220.0).abs() < 0.01);
    }

    #[test]
    fn every_waveform_makes_sound_inside_unit_range() {
        for wave in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Saw,
            Waveform::Square,
            Waveform::Noise,
        ] {
            let buf = play(
                Patch {
                    wave,
                    sustain: 255,
                    ..Patch::default()
                },
                69,
                4800,
            );
            let peak = buf.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            assert!(peak > 0.1, "{wave:?} was silent (peak {peak})");
            assert!(peak <= 1.0, "{wave:?} left unit range (peak {peak})");
            assert!(buf.iter().all(|s| s.is_finite()), "{wave:?} produced NaN");
        }
    }

    #[test]
    fn oscillator_runs_at_the_played_frequency() {
        // Count rising zero crossings of a sine over a second.
        let sr = 48_000;
        let mut v = Voice::new(1);
        v.start(
            params(Patch {
                wave: Waveform::Sine,
                attack_ms: 0,
                sustain: 255,
                decay_ms: 0,
                ..Patch::default()
            }),
            69,
            255,
            None,
            None,
            Priority::Sfx,
            0,
            sr,
        );
        let mut buf = vec![0.0; sr as usize * 2];
        v.render_add(&mut buf);
        let mut crossings: i32 = 0;
        for w in buf.chunks_exact(2).collect::<Vec<_>>().windows(2) {
            if w[0][0] <= 0.0 && w[1][0] > 0.0 {
                crossings += 1;
            }
        }
        assert!(
            (crossings - 440).abs() <= 1,
            "measured {crossings} Hz, expected 440"
        );
    }

    #[test]
    fn a_percussive_patch_ends_without_a_release() {
        // sustain = 0: the voice must free itself when the decay finishes,
        // or a fire-and-forget note would occupy a voice forever.
        let mut v = Voice::new(1);
        v.start(
            params(Patch {
                attack_ms: 0,
                decay_ms: 20,
                sustain: 0,
                ..Patch::default()
            }),
            69,
            255,
            None,
            None,
            Priority::Sfx,
            0,
            48_000,
        );
        let mut buf = vec![0.0; 48_000 * 2 / 10]; // 100 ms
        v.render_add(&mut buf);
        assert!(v.is_idle());
    }

    #[test]
    fn a_held_note_stays_until_released() {
        let mut v = Voice::new(1);
        v.start(
            params(Patch {
                sustain: 200,
                release_ms: 10,
                ..Patch::default()
            }),
            69,
            255,
            None,
            Some(3),
            Priority::Sfx,
            0,
            48_000,
        );
        let mut buf = vec![0.0; 48_000 * 2]; // a full second
        v.render_add(&mut buf);
        assert!(!v.is_idle(), "a held note released itself");
        v.release();
        let mut tail = vec![0.0; 48_000 * 2 / 10];
        v.render_add(&mut tail);
        assert!(v.is_idle(), "release never finished");
    }

    #[test]
    fn play_duration_releases_on_its_own() {
        let hold = 4_800; // 100 ms
        let mut v = Voice::new(1);
        v.start(
            params(Patch {
                sustain: 200,
                release_ms: 5,
                ..Patch::default()
            }),
            69,
            255,
            Some(hold),
            None,
            Priority::Sfx,
            0,
            48_000,
        );
        let mut buf = vec![0.0; 48_000 * 2 / 2]; // 500 ms, well past hold+release
        v.render_add(&mut buf);
        assert!(v.is_idle(), "a timed note outlived its duration");
    }

    #[test]
    fn pitch_envelope_starts_high_and_settles() {
        // A downward sweep should cross zero more often early than late.
        let sr = 48_000usize;
        let mut v = Voice::new(1);
        v.start(
            params(Patch {
                wave: Waveform::Sine,
                attack_ms: 0,
                decay_ms: 0,
                sustain: 255,
                pitch_env: 36,
                pitch_decay_ms: 400,
                ..Patch::default()
            }),
            60,
            255,
            None,
            None,
            Priority::Sfx,
            0,
            sr as u32,
        );
        let mut buf = vec![0.0; sr * 2 * 3 / 2]; // 1.5 s
        v.render_add(&mut buf);
        let count = |s: &[f32]| {
            s.chunks_exact(2)
                .collect::<Vec<_>>()
                .windows(2)
                .filter(|w| w[0][0] <= 0.0 && w[1][0] > 0.0)
                .count()
        };
        let window = sr / 20 * 2; // 50 ms of interleaved stereo
        let head = count(&buf[..window]);
        let tail = count(&buf[buf.len() - window..]);
        assert!(
            head > tail * 3,
            "pitch envelope did not sweep: {head} then {tail}"
        );
    }

    #[test]
    fn noise_is_seeded_and_reproducible() {
        let one = play(
            Patch {
                wave: Waveform::Noise,
                sustain: 255,
                ..Patch::default()
            },
            60,
            1000,
        );
        let two = play(
            Patch {
                wave: Waveform::Noise,
                sustain: 255,
                ..Patch::default()
            },
            60,
            1000,
        );
        assert_eq!(one, two);
    }
}
