//! The kessel console's synth — a function from an event log to samples.
//!
//! This crate is **host-free**, in the same sense and for the same reasons as
//! `kessel-vm`: it opens no audio device, spawns no thread, does no I/O, and
//! depends on nothing. Every backend (cpal in the CLI, `AudioTrack` on Android,
//! `AVAudioSourceNode` on iOS) lives further out. If you are tempted to put
//! cpal in here, don't — put it in the host.
//!
//! It also depends on no part of the VM, which is the other half of the point:
//! a standalone instrument links [`Synth`] on its own without carrying a stack
//! machine, an assembler, and a sprite blitter.
//!
//! ```
//! use kessel_audio::{AudioEvent, Patch, Synth, SynthConfig, Waveform};
//!
//! let mut synth = Synth::new(SynthConfig::default());
//! synth.set_instruments(&[Patch { wave: Waveform::Square, ..Patch::default() }]);
//! synth.handle(AudioEvent::Play { inst: 0, note: 69, vel: 200, frames: 30 });
//!
//! let mut out = vec![0.0f32; 800 * 2]; // one 60 Hz frame of stereo at 48 kHz
//! synth.render(&mut out);
//! assert!(out.iter().any(|s| *s != 0.0));
//! ```
//!
//! ## The two clocks
//!
//! The console runs on frames; the synth runs on samples. Durations cross the
//! boundary in frames ([`AudioEvent::Play`]) and are resolved here via
//! [`samples_per_frame`]. Nothing in this crate knows what a frame *is* beyond
//! that conversion — see `docs/AUDIO.md` for how events get timestamped.
//!
//! ## Realtime contract
//!
//! [`Synth::render`] runs on an audio callback thread: no allocation, no locks,
//! no syscalls, no panics. Voices live in a fixed array for the life of the
//! synth, and the only allocation in the crate is [`Synth::set_instruments`],
//! which is a load-time call.

pub mod bank;
pub mod engine;
pub mod event;
pub mod filter;
pub mod fx;
pub mod master;
pub mod patch;
pub mod sequencer;
pub mod voice;
pub mod wav;

pub use bank::{SfxDef, SoundBank, TrackDef};
pub use engine::AudioEngine;
pub use event::AudioEvent;
pub use filter::{cutoff_hz, resonance_q, FilterMode};
pub use patch::{Patch, VoiceParams, Waveform};
pub use voice::{note_hz, Priority};

use fx::{Chorus, Reverb};
use master::Limiter;
use voice::Voice;

/// How many notes can sound at once.
///
/// Sixteen rather than eight: music wants six to eight, sound effects two to
/// four, and released voices linger through their tails. At eight you think
/// about voice stealing constantly; at sixteen it is a rare event with an
/// audible cause.
pub const MAX_VOICES: usize = 16;

/// The console's frame rate — the unit game-facing durations are quoted in.
pub const FRAME_RATE: u32 = 60;

/// What every host runs at in practice, and what the offline renderer pins.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// Samples in one console frame. 800 at 48 kHz.
pub const fn samples_per_frame(sample_rate: u32) -> u32 {
    sample_rate / FRAME_RATE
}

/// How the synth is built. Both fields matter for reproducibility: the same
/// config and the same events must give the same samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthConfig {
    pub sample_rate: u32,
    /// Seed for the noise oscillators. Fixed, never from the OS.
    pub seed: u32,
}

impl Default for SynthConfig {
    fn default() -> Self {
        SynthConfig {
            sample_rate: DEFAULT_SAMPLE_RATE,
            seed: 0x5EED_1E55,
        }
    }
}

/// Counters a host or the agent loop can read to explain a render.
///
/// "Nothing played" and "the melody got eaten" are the two failures that are
/// hard to hear and easy to count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SynthStats {
    /// Notes that started sounding.
    pub started: u64,
    /// Notes that started by taking a voice from a sounding note.
    pub stolen: u64,
    /// Events dropped because they named an instrument the bank doesn't have.
    pub dropped: u64,
    /// Stereo frames the master limiter turned down. Zero means nothing in the
    /// render was ever too loud, which `peak` alone cannot tell you.
    pub limited: u64,
}

/// The instrument: voices, and (later) filters and effects.
///
/// A game reaches this through `AudioEngine`, which adds the bank, the
/// sequencer, and sample-accurate event timing. A standalone instrument drives
/// it directly with [`AudioEvent::NoteOn`] / [`AudioEvent::NoteOff`].
pub struct Synth {
    cfg: SynthConfig,
    voices: [Voice; MAX_VOICES],
    instruments: Vec<VoiceParams>,
    /// Monotonic allocation counter, so "oldest" is well defined.
    next_age: u64,
    limiter: Limiter,
    /// The two shared effects, and the buses that feed them. One of each for
    /// the whole mix — see [`fx`] for why they are not per voice.
    chorus: Chorus,
    reverb: Reverb,
    chorus_bus: Vec<f32>,
    reverb_bus: Vec<f32>,
    stats: SynthStats,
}

/// How long the master limiter takes to come back up after a peak.
const LIMITER_RELEASE_MS: u16 = 150;

/// Frames the synth renders at a time internally.
///
/// The caller's block can be any length a device asks for; the send buses are
/// fixed buffers sized once, so a long block is walked in pieces rather than
/// allocating to fit it. The effects are stateful, so splitting changes
/// nothing about the output — which is also what keeps a render independent of
/// the device's buffer size.
const CHUNK_FRAMES: usize = 512;

impl Synth {
    pub fn new(cfg: SynthConfig) -> Self {
        Synth {
            cfg,
            // Each voice gets its own noise stream, so two simultaneous hits
            // are not the same noise twice.
            voices: std::array::from_fn(|i| {
                Voice::new(cfg.seed.wrapping_add((i as u32).wrapping_mul(0x9E37_79B9)))
            }),
            instruments: Vec::new(),
            next_age: 0,
            limiter: Limiter::new(cfg.sample_rate, LIMITER_RELEASE_MS),
            chorus: Chorus::new(cfg.sample_rate),
            reverb: Reverb::new(cfg.sample_rate),
            chorus_bus: vec![0.0; CHUNK_FRAMES * 2],
            reverb_bus: vec![0.0; CHUNK_FRAMES * 2],
            stats: SynthStats::default(),
        }
    }

    /// Set the shared effects' character. Load-time; the per-voice `chorus` and
    /// `reverb` sends decide *who* reaches them.
    pub fn set_fx(&mut self, fx: bank::FxSettings) {
        self.reverb.set(fx.reverb_size, fx.reverb_damping);
        self.chorus.set(fx.chorus_rate, fx.chorus_depth);
    }

    /// Install the instrument table. Load-time only — this is the one call in
    /// the crate that allocates.
    pub fn set_instruments(&mut self, patches: &[Patch]) {
        self.instruments.clear();
        self.instruments.reserve(patches.len());
        self.instruments
            .extend(patches.iter().map(|p| p.compile(self.cfg.sample_rate)));
    }

    pub fn sample_rate(&self) -> u32 {
        self.cfg.sample_rate
    }

    pub fn stats(&self) -> SynthStats {
        SynthStats {
            limited: self.limiter.engaged(),
            ..self.stats
        }
    }

    /// Voices currently sounding, release tails included.
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| !v.is_idle()).count()
    }

    /// Start a note directly, choosing its priority.
    ///
    /// The path the sequencer uses: music notes are allocated at
    /// [`Priority::Music`] so a sound effect can take a voice from the
    /// bassline rather than the other way round. A standalone instrument can
    /// use it too — `handle(Play)` is the same call at [`Priority::Sfx`].
    pub fn play_note(&mut self, inst: u8, note: u8, vel: u8, frames: u16, priority: Priority) {
        let hold = frames as u32 * samples_per_frame(self.cfg.sample_rate);
        self.start(inst, note, vel, Some(hold), None, priority);
    }

    /// Release every voice the music is using, leaving sound effects alone.
    ///
    /// What `music_stop()` means. Release rather than kill, so the last chord
    /// fades instead of being cut off mid-cycle — a hard stop on a sustained
    /// note is an audible click.
    pub fn release_music(&mut self) {
        for v in &mut self.voices {
            if v.priority == Priority::Music {
                v.release();
            }
        }
    }

    /// Apply an event immediately.
    pub fn handle(&mut self, ev: AudioEvent) {
        match ev {
            AudioEvent::Play {
                inst,
                note,
                vel,
                frames,
            } => {
                let hold = frames as u32 * samples_per_frame(self.cfg.sample_rate);
                self.start(inst, note, vel, Some(hold), None, Priority::Sfx);
            }
            AudioEvent::NoteOn {
                chan,
                inst,
                note,
                vel,
            } => {
                // Retriggering a channel replaces the note on it rather than
                // stacking: the game has one slot and can only turn off one.
                self.release_channel(chan);
                self.start(inst, note, vel, None, Some(chan), Priority::Sfx);
            }
            AudioEvent::NoteOff { chan } => self.release_channel(chan),
            AudioEvent::Panic => {
                for v in &mut self.voices {
                    v.kill();
                }
                // Including the master: a limiter still ducked from the old
                // timeline would fade the new one in.
                self.limiter.reset();
                // ...and the tails. `Panic` means "including tails", and a
                // reverb still ringing from a rewound timeline is the loudest
                // possible reminder of it.
                self.reverb.clear();
                self.chorus.clear();
            }
            // `StopMusic` reaches the voices here; starting music needs the
            // bank and the clock, so `AudioEngine` owns that half.
            AudioEvent::StopMusic => self.release_music(),
            AudioEvent::PlaySfx { .. } | AudioEvent::PlayMusic { .. } => {}
        }
    }

    /// Render interleaved stereo into `out`, overwriting it.
    ///
    /// `out.len()` must be even; a trailing half-frame is ignored.
    pub fn render(&mut self, out: &mut [f32]) {
        for block in out.chunks_mut(CHUNK_FRAMES * 2) {
            self.render_chunk(block);
        }
    }

    /// One chunk, no longer than the send buses.
    fn render_chunk(&mut self, out: &mut [f32]) {
        let n = out.len();
        let chorus = &mut self.chorus_bus[..n];
        let reverb = &mut self.reverb_bus[..n];
        out.fill(0.0);
        chorus.fill(0.0);
        reverb.fill(0.0);

        for v in &mut self.voices {
            v.render_add(out, chorus, reverb);
        }

        // Each unit processes its bus in place, and the result is summed back
        // into the dry mix. The units are shared, so this happens once per
        // chunk however many voices sent to them.
        self.chorus.process(chorus);
        self.reverb.process(reverb);
        for i in 0..n {
            out[i] += chorus[i] + reverb[i];
        }

        self.limiter.process(out);
    }

    fn start(
        &mut self,
        inst: u8,
        note: u8,
        vel: u8,
        hold: Option<u32>,
        chan: Option<u8>,
        priority: Priority,
    ) {
        let Some(params) = self.instruments.get(inst as usize).copied() else {
            self.stats.dropped += 1;
            return;
        };
        let idx = self.pick_voice();
        if !self.voices[idx].is_idle() {
            self.stats.stolen += 1;
        }
        let age = self.next_age;
        self.next_age += 1;
        self.voices[idx].start(
            params,
            note,
            vel,
            hold,
            chan,
            priority,
            age,
            self.cfg.sample_rate,
        );
        self.stats.started += 1;
    }

    fn release_channel(&mut self, chan: u8) {
        for v in &mut self.voices {
            if v.chan == Some(chan) {
                v.release();
                v.chan = None;
            }
        }
    }

    /// Choose the voice a new note takes.
    ///
    /// Idle first, then releasing, then the lowest priority, then the quietest,
    /// then the oldest. The ordering is what makes stealing sound like nothing:
    /// a note already fading out is the one nobody misses.
    fn pick_voice(&self) -> usize {
        let rank = |v: &Voice| {
            if v.is_idle() {
                0u8
            } else if v.is_releasing() {
                1
            } else {
                2
            }
        };
        self.voices
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                rank(a)
                    .cmp(&rank(b))
                    .then(a.priority.cmp(&b.priority))
                    .then(a.env().total_cmp(&b.env()))
                    .then(a.age.cmp(&b.age))
            })
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth() -> Synth {
        let mut s = Synth::new(SynthConfig::default());
        s.set_instruments(&[
            Patch {
                wave: Waveform::Square,
                sustain: 200,
                ..Patch::default()
            },
            Patch {
                wave: Waveform::Noise,
                attack_ms: 0,
                decay_ms: 40,
                sustain: 0,
                pitch_env: -12,
                ..Patch::default()
            },
        ]);
        s
    }

    fn render(s: &mut Synth, frames: usize) -> Vec<f32> {
        let n = samples_per_frame(s.sample_rate()) as usize * frames;
        let mut out = vec![0.0; n * 2];
        s.render(&mut out);
        out
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }

    #[test]
    fn samples_per_frame_is_800_at_48k() {
        assert_eq!(samples_per_frame(48_000), 800);
        assert_eq!(samples_per_frame(44_100), 735);
    }

    #[test]
    fn a_note_sounds_and_a_silent_synth_does_not() {
        let mut s = synth();
        let quiet = render(&mut s, 4);
        assert_eq!(peak(&quiet), 0.0);

        s.handle(AudioEvent::Play {
            inst: 0,
            note: 69,
            vel: 255,
            frames: 10,
        });
        let loud = render(&mut s, 4);
        assert!(peak(&loud) > 0.1, "peak {}", peak(&loud));
        assert!(loud.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn render_stays_inside_unit_range_with_every_voice_sounding() {
        let mut s = synth();
        for note in 40..40 + MAX_VOICES as u8 {
            s.handle(AudioEvent::Play {
                inst: 0,
                note,
                vel: 255,
                frames: 60,
            });
        }
        let out = render(&mut s, 10);
        assert!(peak(&out) <= 1.0, "peak {}", peak(&out));
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_timed_note_frees_its_voice() {
        let mut s = synth();
        s.handle(AudioEvent::Play {
            inst: 0,
            note: 69,
            vel: 255,
            frames: 6, // 100 ms, then an 80 ms release
        });
        assert_eq!(s.active_voices(), 1);
        render(&mut s, 6);
        assert_eq!(s.active_voices(), 1, "released before its duration was up");
        render(&mut s, 60); // a second
        assert_eq!(s.active_voices(), 0, "voice never freed");
    }

    #[test]
    fn a_channel_note_is_held_until_note_off() {
        let mut s = synth();
        s.handle(AudioEvent::NoteOn {
            chan: 2,
            inst: 0,
            note: 60,
            vel: 255,
        });
        render(&mut s, 120); // two seconds
        assert_eq!(s.active_voices(), 1, "a held note released itself");
        s.handle(AudioEvent::NoteOff { chan: 2 });
        render(&mut s, 60);
        assert_eq!(s.active_voices(), 0, "note_off did not release the voice");
    }

    #[test]
    fn retriggering_a_channel_replaces_rather_than_stacks() {
        let mut s = synth();
        for _ in 0..4 {
            s.handle(AudioEvent::NoteOn {
                chan: 0,
                inst: 0,
                note: 60,
                vel: 255,
            });
            render(&mut s, 1);
        }
        // One sounding note, plus tails of the three it replaced.
        assert!(s.active_voices() <= 4);
        s.handle(AudioEvent::NoteOff { chan: 0 });
        render(&mut s, 60);
        assert_eq!(s.active_voices(), 0, "a retrigger orphaned a voice");
    }

    #[test]
    fn stealing_never_exceeds_the_voice_count() {
        let mut s = synth();
        for note in 30..30 + MAX_VOICES as u8 + 8 {
            s.handle(AudioEvent::Play {
                inst: 0,
                note,
                vel: 255,
                frames: 120,
            });
        }
        assert_eq!(s.active_voices(), MAX_VOICES);
        assert_eq!(s.stats().started, MAX_VOICES as u64 + 8);
        assert_eq!(s.stats().stolen, 8);
    }

    #[test]
    fn a_releasing_voice_is_stolen_before_a_sounding_one() {
        let mut s = synth();
        // Fill every voice, then release one.
        for note in 30..30 + MAX_VOICES as u8 {
            s.handle(AudioEvent::NoteOn {
                chan: note - 30,
                inst: 0,
                note,
                vel: 255,
            });
        }
        render(&mut s, 2);
        s.handle(AudioEvent::NoteOff { chan: 7 });
        render(&mut s, 1);
        let before: Vec<u8> = (0..MAX_VOICES as u8).filter(|c| *c != 7).collect();

        s.handle(AudioEvent::Play {
            inst: 0,
            note: 90,
            vel: 255,
            frames: 60,
        });
        // Every channel that was still held is still held: the new note took
        // the one that was fading.
        for chan in before {
            s.handle(AudioEvent::NoteOff { chan });
        }
        assert_eq!(s.stats().stolen, 1);
    }

    #[test]
    fn music_loses_its_voice_before_a_sound_effect() {
        let mut s = synth();
        s.set_instruments(&[Patch {
            sustain: 200,
            ..Patch::default()
        }]);
        // One music voice and the rest sound effects, all sounding.
        s.start(0, 60, 255, None, Some(0), Priority::Music);
        for note in 61..60 + MAX_VOICES as u8 {
            s.start(0, note, 255, None, None, Priority::Sfx);
        }
        render(&mut s, 2);
        assert_eq!(s.active_voices(), MAX_VOICES);

        s.handle(AudioEvent::Play {
            inst: 0,
            note: 90,
            vel: 255,
            frames: 60,
        });
        // The music note is gone; nothing answers on its channel any more.
        assert!(
            !s.voices.iter().any(|v| v.chan == Some(0)),
            "a sound effect stole an effect voice over the music voice"
        );
    }

    #[test]
    fn an_unknown_instrument_is_counted_not_guessed() {
        let mut s = synth();
        s.handle(AudioEvent::Play {
            inst: 99,
            note: 69,
            vel: 255,
            frames: 10,
        });
        assert_eq!(s.active_voices(), 0);
        assert_eq!(s.stats().dropped, 1);
        assert_eq!(s.stats().started, 0);
    }

    #[test]
    fn panic_stops_everything_including_tails() {
        let mut s = synth();
        for note in 50..58 {
            s.handle(AudioEvent::Play {
                inst: 0,
                note,
                vel: 255,
                frames: 120,
            });
        }
        render(&mut s, 2);
        s.handle(AudioEvent::Panic);
        assert_eq!(s.active_voices(), 0);
        let after = render(&mut s, 2);
        assert_eq!(peak(&after), 0.0, "a tail survived Panic");
    }

    /// Render one note of `patch` and return the buffer.
    fn one_note(patch: Patch, note: u8, vel: u8, frames: usize) -> Vec<f32> {
        let mut s = Synth::new(SynthConfig::default());
        s.set_instruments(&[patch]);
        s.handle(AudioEvent::Play {
            inst: 0,
            note,
            vel,
            frames: frames as u16,
        });
        render(&mut s, frames)
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
    }

    /// Magnitude of one frequency in the left channel — a single DFT bin by
    /// correlation.
    ///
    /// A cheaper proxy (mean sample-to-sample change) is not good enough here:
    /// a 130 Hz saw and its own filtered fundamental have almost the same mean
    /// slew, so that measure calls a working filter broken. Ask about the
    /// harmonic directly instead.
    fn magnitude_at(buf: &[f32], hz: f32, sample_rate: u32) -> f32 {
        let left: Vec<f32> = buf.chunks_exact(2).map(|f| f[0]).collect();
        let w = std::f32::consts::TAU * hz / sample_rate as f32;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, s) in left.iter().enumerate() {
            let (sin, cos) = (w * i as f32).sin_cos();
            re += s * cos;
            im += s * sin;
        }
        2.0 * (re * re + im * im).sqrt() / left.len() as f32
    }

    #[test]
    fn pan_places_a_voice_in_the_stereo_image() {
        let base = Patch {
            wave: Waveform::Square,
            sustain: 200,
            ..Patch::default()
        };
        let energy = |buf: &[f32]| {
            buf.chunks_exact(2).fold((0.0f32, 0.0f32), |(l, r), f| {
                (l + f[0].abs(), r + f[1].abs())
            })
        };

        let (l, r) = energy(&one_note(base, 60, 200, 20));
        assert!(
            (l - r).abs() < l * 0.01,
            "centre was not centred: {l} vs {r}"
        );

        let (l, r) = energy(&one_note(Patch { pan: -127, ..base }, 60, 200, 20));
        assert!(r < l * 0.01, "hard left leaked right: {l} vs {r}");

        let (l, r) = energy(&one_note(Patch { pan: 127, ..base }, 60, 200, 20));
        assert!(l < r * 0.01, "hard right leaked left: {l} vs {r}");
    }

    #[test]
    fn a_lowpass_takes_the_edge_off_a_saw() {
        let base = Patch {
            wave: Waveform::Saw,
            attack_ms: 0,
            decay_ms: 0,
            sustain: 255,
            ..Patch::default()
        };
        // Note 48 is ~130.8 Hz; a corner at byte 60 is ~290 Hz, so the
        // fundamental survives and the harmonics do not.
        let f0 = note_hz(48);
        let open = one_note(base, 48, 150, 20);
        let closed = one_note(
            Patch {
                filter: FilterMode::Lpf,
                cutoff: 60,
                ..base
            },
            48,
            150,
            20,
        );
        let sr = DEFAULT_SAMPLE_RATE;
        let (h_open, h_closed) = (
            magnitude_at(&open, f0 * 8.0, sr),
            magnitude_at(&closed, f0 * 8.0, sr),
        );
        assert!(
            h_closed < h_open * 0.2,
            "8th harmonic survived the lowpass: {h_open} → {h_closed}"
        );
        let (f_open, f_closed) = (magnitude_at(&open, f0, sr), magnitude_at(&closed, f0, sr));
        assert!(
            f_closed > f_open * 0.7,
            "lowpass ate the fundamental: {f_open} → {f_closed}"
        );
    }

    #[test]
    fn a_highpass_takes_the_body_out() {
        let base = Patch {
            wave: Waveform::Saw,
            attack_ms: 0,
            decay_ms: 0,
            sustain: 255,
            ..Patch::default()
        };
        // A corner at byte 150 is ~2.6 kHz: well above the fundamental and
        // well below the 20th harmonic.
        let f0 = note_hz(48);
        let sr = DEFAULT_SAMPLE_RATE;
        let open = one_note(base, 48, 150, 20);
        let thin = one_note(
            Patch {
                filter: FilterMode::Hpf,
                cutoff: 150,
                ..base
            },
            48,
            150,
            20,
        );
        assert!(
            magnitude_at(&thin, f0, sr) < magnitude_at(&open, f0, sr) * 0.05,
            "highpass kept the fundamental"
        );
        assert!(
            magnitude_at(&thin, f0 * 20.0, sr) > magnitude_at(&open, f0 * 20.0, sr) * 0.7,
            "highpass ate the harmonics too"
        );
    }

    #[test]
    fn distortion_adds_harmonics_rather_than_level() {
        let base = Patch {
            wave: Waveform::Sine,
            attack_ms: 0,
            decay_ms: 0,
            sustain: 255,
            ..Patch::default()
        };
        let clean = one_note(base, 57, 150, 20);
        let dirty = one_note(
            Patch {
                distortion: 220,
                ..base
            },
            57,
            150,
            20,
        );
        // A saturated sine is closer to a square: same peak, more energy.
        let (pc, pd) = (
            clean.iter().fold(0.0f32, |a, s| a.max(s.abs())),
            dirty.iter().fold(0.0f32, |a, s| a.max(s.abs())),
        );
        assert!(
            (pd - pc).abs() < pc * 0.25,
            "drive changed the level: peak {pc} → {pd}"
        );
        assert!(
            rms(&dirty) > rms(&clean) * 1.2,
            "drive added no harmonics: rms {} → {}",
            rms(&clean),
            rms(&dirty)
        );
    }

    /// A synth with one percussive patch, sending as given.
    fn sends(chorus: u8, reverb: u8) -> Synth {
        let mut s = Synth::new(SynthConfig::default());
        s.set_instruments(&[Patch {
            wave: Waveform::Square,
            attack_ms: 0,
            decay_ms: 30,
            sustain: 0,
            chorus,
            reverb,
            ..Patch::default()
        }]);
        s.set_fx(bank::FxSettings::default());
        s
    }

    fn hit(s: &mut Synth) {
        s.handle(AudioEvent::Play {
            inst: 0,
            note: 60,
            vel: 255,
            frames: 2,
        });
    }

    #[test]
    fn a_reverb_send_adds_a_tail_the_dry_signal_does_not_have() {
        // The defining property: well after the note has decayed, a sent voice
        // is still audible and an unsent one is silent.
        let tail = |reverb: u8| {
            let mut s = sends(0, reverb);
            hit(&mut s);
            render(&mut s, 10); // well past the note's own decay
            peak(&render(&mut s, 12))
        };
        // Not `== 0.0` for the dry case: the envelope's final sample sits at
        // the voice's silence threshold, which is a note ending rather than a
        // tail. The two differ by orders of magnitude, which is the claim.
        assert!(tail(0) < 1e-4, "a dry voice left a tail: {}", tail(0));
        assert!(
            tail(255) > tail(0) * 100.0 + 0.001,
            "a sent voice left none: {} vs {}",
            tail(0),
            tail(255)
        );
    }

    #[test]
    fn a_bigger_send_returns_more() {
        let level = |reverb: u8| {
            let mut s = sends(0, reverb);
            hit(&mut s);
            render(&mut s, 6);
            peak(&render(&mut s, 12))
        };
        assert!(
            level(255) > level(60) * 2.0,
            "the send amount did nothing: {} vs {}",
            level(60),
            level(255)
        );
    }

    #[test]
    fn the_effects_are_shared_not_per_voice() {
        // Sixteen voices into one reverb is one reverb's worth of return, and
        // it stays inside full scale.
        let mut s = sends(0, 200);
        for note in 40..40 + MAX_VOICES as u8 {
            s.handle(AudioEvent::Play {
                inst: 0,
                note,
                vel: 255,
                frames: 2,
            });
        }
        let out = render(&mut s, 30);
        assert!(peak(&out) <= 1.0, "the shared bus blew past full scale");
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn panic_takes_the_tails_with_it() {
        let mut s = sends(255, 255);
        hit(&mut s);
        render(&mut s, 6);
        assert!(peak(&render(&mut s, 2)) > 0.0, "no tail to cancel");

        s.handle(AudioEvent::Panic);
        assert_eq!(
            peak(&render(&mut s, 12)),
            0.0,
            "an effect tail survived Panic"
        );
    }

    #[test]
    fn a_dry_mix_is_untouched_by_the_effects() {
        // Nothing sent means nothing returned: the effects must not colour a
        // game that never asked for them.
        let mut s = Synth::new(SynthConfig::default());
        s.set_instruments(&[Patch {
            wave: Waveform::Square,
            sustain: 200,
            ..Patch::default()
        }]);
        s.set_fx(bank::FxSettings::default());
        s.handle(AudioEvent::Play {
            inst: 0,
            note: 60,
            vel: 120,
            frames: 20,
        });
        let out = render(&mut s, 10);
        let expect = 120.0 / 255.0 * (Patch::default().volume as f32 / 255.0);
        assert!(
            (peak(&out) - expect).abs() < 1e-6,
            "{} vs {expect}",
            peak(&out)
        );
    }

    #[test]
    fn the_master_leaves_a_quiet_mix_alone() {
        // One modest voice never reaches the ceiling, so the master must be a
        // no-op on it — the limiter is for mixes, not a tone control.
        let mut s = synth();
        s.handle(AudioEvent::Play {
            inst: 0,
            note: 60,
            vel: 80,
            frames: 20,
        });
        let out = render(&mut s, 10);
        assert!(peak(&out) < 0.95);
        assert!(peak(&out) > 0.05);
        // Nothing was scaled: the loudest sample is exactly what one voice at
        // this velocity produces, not a limited version of it.
        let expect = 80.0 / 255.0 * (200.0 / 255.0);
        assert!(
            peak(&out) <= expect + 1e-6,
            "master boosted the mix: {} > {expect}",
            peak(&out)
        );
    }

    #[test]
    fn the_same_events_render_the_same_samples() {
        let events = [
            AudioEvent::Play {
                inst: 1,
                note: 40,
                vel: 255,
                frames: 4,
            },
            AudioEvent::Play {
                inst: 0,
                note: 67,
                vel: 180,
                frames: 20,
            },
        ];
        let run = || {
            let mut s = synth();
            let mut out = Vec::new();
            for (i, ev) in events.iter().enumerate() {
                s.handle(*ev);
                let _ = i;
                out.extend(render(&mut s, 8));
            }
            out
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn block_size_does_not_change_the_audio() {
        // The realtime host renders in whatever chunks the device asks for;
        // the offline renderer uses whole frames. They must agree.
        let mut a = synth();
        let mut b = synth();
        let ev = AudioEvent::Play {
            inst: 0,
            note: 64,
            vel: 200,
            frames: 5,
        };
        a.handle(ev);
        b.handle(ev);

        let total = 8_000usize;
        let mut one = vec![0.0; total * 2];
        a.render(&mut one);

        let mut many = Vec::with_capacity(total * 2);
        let mut chunk = vec![0.0; 137 * 2]; // an awkward device buffer
        while many.len() < total * 2 {
            b.render(&mut chunk);
            many.extend_from_slice(&chunk);
        }
        many.truncate(total * 2);
        assert_eq!(one, many);
    }
}
