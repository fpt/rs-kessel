//! The console's engine: a bank, a timestamped event queue, and a [`Synth`].
//!
//! [`Synth`] applies events *now*. That is right for an instrument, where a key
//! press happens when it happens, and wrong for a console, where a game emits
//! all of frame N's sounds in one burst and they have to land at frame N's
//! place in the audio stream. [`AudioEngine`] is the layer that knows about
//! time.
//!
//! ```text
//! submit(ev, at_sample) ──► queue ──► render() splits its block at each
//!                                      pending timestamp and dispatches
//! ```
//!
//! Both halves are still called from one thread here. The lock-free handoff
//! between a game thread and an audio callback is the host's business, and it
//! arrives with the first host that has an audio device.

use crate::bank::{SoundBank, MAX_TRACK_CHANNELS};
use crate::event::AudioEvent;
use crate::sequencer::{SeqNote, Sequencer};
use crate::{samples_per_frame, Priority, Synth, SynthConfig, SynthStats};

/// How many scheduled events can be pending at once.
///
/// A sound effect expands into one event per note the moment it is triggered,
/// so this is "notes in flight", not "events this frame". 256 is several
/// simultaneous effects deep.
pub const QUEUE_CAPACITY: usize = 256;

/// What the engine did, for a host or the agent loop to read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EngineStats {
    /// Events that never got scheduled because the queue was full.
    pub queue_overflow: u64,
    /// `PlaySfx` for an id the bank doesn't have.
    pub unknown_sfx: u64,
    /// `PlayMusic` for an id the bank doesn't have.
    pub unknown_track: u64,
    /// The high-water mark of the queue, so a host can tell "nearly full" from
    /// "never busy" without waiting for the first dropped event.
    pub queue_peak: usize,
}

/// One scheduled event.
#[derive(Debug, Clone, Copy)]
struct Pending {
    at: u64,
    /// Submission order, so events sharing a timestamp dispatch in the order
    /// the game emitted them — `music_stop()` then `music()` must not swap.
    seq: u64,
    event: AudioEvent,
}

/// The scheduled events, as their own type.
///
/// Separate from [`AudioEngine`] so that expanding a sound effect can read the
/// bank and write the queue at once — two disjoint fields rather than two
/// borrows of the whole engine. The alternative was collecting the effect's
/// notes into a `Vec` first, which allocates on a path a host may well call
/// from its game loop.
struct Queue {
    /// Fixed-capacity and unsorted: submission is O(1), and finding the next
    /// event is a 256-entry scan once per block, which is nothing next to
    /// rendering that block.
    slots: [Option<Pending>; QUEUE_CAPACITY],
    len: usize,
    next_seq: u64,
    overflow: u64,
    peak: usize,
}

impl Queue {
    fn new() -> Self {
        Queue {
            slots: [None; QUEUE_CAPACITY],
            len: 0,
            next_seq: 0,
            overflow: 0,
            peak: 0,
        }
    }

    fn push(&mut self, event: AudioEvent, at: u64) {
        let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) else {
            self.overflow += 1;
            return;
        };
        *slot = Some(Pending {
            at,
            seq: self.next_seq,
            event,
        });
        self.next_seq += 1;
        self.len += 1;
        self.peak = self.peak.max(self.len);
    }

    /// Drop everything scheduled at or after `at`.
    ///
    /// Not "everything": a `Panic` timestamped for frame 10 must not cancel
    /// the sounds of frames 0–9 that are still sitting in the queue waiting to
    /// be rendered. Cancelling the future is what stopping means; cancelling
    /// the past is losing audio the game already asked for.
    fn clear_from(&mut self, at: u64) {
        for slot in self.slots.iter_mut() {
            if slot.is_some_and(|p| p.at >= at) {
                *slot = None;
                self.len -= 1;
            }
        }
    }

    /// Timestamp of the earliest pending event.
    fn next_time(&self) -> Option<u64> {
        self.slots.iter().flatten().map(|p| p.at).min()
    }

    /// Remove and return the earliest event due at or before `now`.
    fn take_due(&mut self, now: u64) -> Option<AudioEvent> {
        let mut best: Option<(usize, u64, u64)> = None;
        for (i, slot) in self.slots.iter().enumerate() {
            let Some(p) = slot else { continue };
            if p.at > now {
                continue;
            }
            if best.is_none_or(|(_, at, seq)| (p.at, p.seq) < (at, seq)) {
                best = Some((i, p.at, p.seq));
            }
        }
        let (i, _, _) = best?;
        self.len -= 1;
        self.slots[i].take().map(|p| p.event)
    }
}

/// The synth, plus a bank and a clock.
pub struct AudioEngine {
    synth: Synth,
    bank: SoundBank,
    queue: Queue,
    /// The music player. Its rows fall due on *this* clock, not the game's —
    /// see [`crate::sequencer`].
    seq: Sequencer,
    /// Scratch for one row's notes. Sized for the channel limit so the render
    /// loop never allocates.
    row_notes: [SeqNote; MAX_TRACK_CHANNELS],
    /// Samples rendered since the engine was created.
    now: u64,
    unknown_sfx: u64,
    unknown_track: u64,
}

impl AudioEngine {
    pub fn new(cfg: SynthConfig) -> Self {
        AudioEngine {
            synth: Synth::new(cfg),
            bank: SoundBank::default(),
            queue: Queue::new(),
            seq: Sequencer::new(),
            row_notes: [SeqNote {
                inst: 0,
                note: 0,
                vel: 0,
                frames: 0,
            }; MAX_TRACK_CHANNELS],
            now: 0,
            unknown_sfx: 0,
            unknown_track: 0,
        }
    }

    /// Install a bank. Load-time only — this allocates.
    pub fn set_bank(&mut self, bank: SoundBank) {
        self.synth.set_instruments(&bank.instruments);
        self.synth.set_fx(bank.fx);
        self.bank = bank;
    }

    pub fn bank(&self) -> &SoundBank {
        &self.bank
    }

    pub fn sample_rate(&self) -> u32 {
        self.synth.sample_rate()
    }

    /// Samples rendered so far — the engine's clock, and what `at_sample`
    /// timestamps are relative to.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// The sample a given console frame starts at.
    pub fn frame_at(&self, frame: u64) -> u64 {
        frame * samples_per_frame(self.sample_rate()) as u64
    }

    pub fn stats(&self) -> EngineStats {
        EngineStats {
            queue_overflow: self.queue.overflow,
            unknown_sfx: self.unknown_sfx,
            unknown_track: self.unknown_track,
            queue_peak: self.queue.peak,
        }
    }

    pub fn synth_stats(&self) -> SynthStats {
        self.synth.stats()
    }

    pub fn active_voices(&self) -> usize {
        self.synth.active_voices()
    }

    /// Number of events waiting to be dispatched.
    pub fn pending(&self) -> usize {
        self.queue.len
    }

    /// The track currently playing, if any.
    pub fn playing_music(&self) -> Option<u16> {
        self.seq.playing()
    }

    /// Schedule an event for `at_sample`.
    ///
    /// A timestamp already in the past is dispatched at the next render rather
    /// than dropped: a host whose game thread ran late should sound late, not
    /// silent.
    pub fn submit(&mut self, event: AudioEvent, at_sample: u64) {
        match event {
            // A sound effect is a static list of notes, so it expands the
            // moment it is triggered rather than being re-read each frame by a
            // cursor. `Panic` clearing the queue then cancels it for free.
            AudioEvent::PlaySfx { id } => {
                let Some(def) = self.bank.sfx.get(id as usize) else {
                    self.unknown_sfx += 1;
                    return;
                };
                let spf = samples_per_frame(self.synth.sample_rate()) as u64;
                let (inst, vel) = (def.inst, def.vel);
                for (offset, note, frames) in def.notes() {
                    self.queue.push(
                        AudioEvent::Play {
                            inst,
                            note,
                            vel,
                            frames,
                        },
                        at_sample + offset as u64 * spf,
                    );
                }
            }
            AudioEvent::Panic => {
                // Cancels the rest of any effect in flight, but nothing
                // already due — see `clear_from`.
                self.queue.clear_from(at_sample);
                self.seq.stop();
                self.queue.push(event, at_sample);
            }
            _ => self.queue.push(event, at_sample),
        }
    }

    /// Apply an event immediately, bypassing the queue. For a host with no
    /// clock of its own — a standalone instrument, or a test.
    pub fn handle(&mut self, event: AudioEvent) {
        self.submit(event, self.now);
    }

    /// Apply an event that has come due.
    fn dispatch(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::PlayMusic { id } => {
                let spf = samples_per_frame(self.synth.sample_rate()) as u64;
                let Some(def) = self.bank.tracks.get(id as usize) else {
                    self.unknown_track += 1;
                    return;
                };
                // Replacing a track releases the one it replaces: two pieces
                // of music over each other is never what `music()` meant.
                self.synth.release_music();
                self.seq.start(id, def, self.now, spf);
            }
            AudioEvent::StopMusic | AudioEvent::Panic => {
                self.seq.stop();
                self.synth.handle(event);
            }
            other => self.synth.handle(other),
        }
    }

    /// Start whatever the music has due at the current sample.
    fn advance_music(&mut self) {
        let Some(id) = self.seq.playing() else {
            return;
        };
        let Some(def) = self.bank.tracks.get(id as usize) else {
            self.seq.stop();
            return;
        };
        let n = self.seq.take_due(self.now, def, &mut self.row_notes);
        for i in 0..n {
            let note = self.row_notes[i];
            self.synth
                .play_note(note.inst, note.note, note.vel, note.frames, Priority::Music);
        }
    }

    /// Render interleaved stereo, dispatching scheduled events at their
    /// sample.
    ///
    /// The block is split at each pending timestamp, so an event lands on the
    /// sample it asked for regardless of the buffer size the device chose.
    pub fn render(&mut self, out: &mut [f32]) {
        let total = out.len() / 2;
        let mut done = 0;
        while done < total {
            while let Some(ev) = self.queue.take_due(self.now) {
                self.dispatch(ev);
            }
            self.advance_music();

            // Render up to whichever comes first: the next queued event, or
            // the next row of music.
            let until = match (self.queue.next_time(), self.seq.next_row_at()) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => u64::MAX,
            };
            let room = total - done;
            let run = until.saturating_sub(self.now).min(room as u64).max(1) as usize;
            let run = run.min(room);
            self.synth.render(&mut out[done * 2..(done + run) * 2]);
            self.now += run as u64;
            done += run;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::{parse, SfxDef};
    use crate::{Patch, Waveform, MAX_VOICES};

    const SRC: &str = r#"
instrument blip {
  wave = square
  attack = 0  decay = 0  sustain = 255  release = 10
}
sfx arp {
  inst = blip
  speed = 2
  notes = "60 - 64 67"
}
"#;

    fn engine() -> AudioEngine {
        let (bank, errors) = parse(SRC);
        assert_eq!(errors, vec![]);
        let mut e = AudioEngine::new(SynthConfig::default());
        e.set_bank(bank);
        e
    }

    /// Render `frames` console frames in one call.
    fn render(e: &mut AudioEngine, frames: usize) -> Vec<f32> {
        let n = samples_per_frame(e.sample_rate()) as usize * frames;
        let mut out = vec![0.0; n * 2];
        e.render(&mut out);
        out
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }

    #[test]
    fn sfx_plays_its_notes() {
        let mut e = engine();
        e.submit(AudioEvent::PlaySfx { id: 0 }, 0);
        // Four rows at speed 2 = 8 frames, three notes (60 held, 64, 67).
        assert_eq!(e.pending(), 3);
        let out = render(&mut e, 12);
        assert!(peak(&out) > 0.1);
        assert_eq!(e.pending(), 0, "not everything was dispatched");
        assert_eq!(e.synth_stats().started, 3);
    }

    #[test]
    fn an_event_lands_on_the_sample_it_asked_for() {
        let mut e = engine();
        let at = e.frame_at(5);
        e.submit(
            AudioEvent::Play {
                inst: 0,
                note: 69,
                vel: 255,
                frames: 4,
            },
            at,
        );
        let out = render(&mut e, 10);
        // Silent until frame 5, sounding after.
        let spf = samples_per_frame(e.sample_rate()) as usize;
        assert_eq!(peak(&out[..at as usize * 2]), 0.0, "sound arrived early");
        assert!(
            peak(&out[at as usize * 2..(at as usize + spf) * 2]) > 0.1,
            "sound never arrived"
        );
    }

    #[test]
    fn the_device_block_size_does_not_move_events() {
        // The offline renderer uses whole frames; a device asks for whatever it
        // wants. Both must place the event on the same sample.
        let mut whole = engine();
        let mut chunked = engine();
        let at = whole.frame_at(3) + 137; // deliberately not on a frame edge
        let ev = AudioEvent::Play {
            inst: 0,
            note: 72,
            vel: 255,
            frames: 6,
        };
        whole.submit(ev, at);
        chunked.submit(ev, at);

        let one = render(&mut whole, 12);
        let mut many = Vec::new();
        let mut buf = vec![0.0; 61 * 2]; // an awkward device buffer
        while many.len() < one.len() {
            chunked.render(&mut buf);
            many.extend_from_slice(&buf);
        }
        many.truncate(one.len());
        assert_eq!(one, many);
    }

    #[test]
    fn events_at_the_same_time_keep_their_order() {
        // Two notes on one channel at the same sample: the second must win,
        // which only holds if they dispatch in submission order.
        let mut e = engine();
        e.submit(
            AudioEvent::NoteOn {
                chan: 0,
                inst: 0,
                note: 60,
                vel: 255,
            },
            100,
        );
        e.submit(
            AudioEvent::NoteOn {
                chan: 0,
                inst: 0,
                note: 72,
                vel: 255,
            },
            100,
        );
        e.submit(AudioEvent::NoteOff { chan: 0 }, 100);
        render(&mut e, 30);
        // NoteOff came last, so nothing is left holding.
        assert_eq!(e.active_voices(), 0, "the note_off did not arrive last");
    }

    #[test]
    fn panic_cancels_a_sound_effect_in_flight() {
        let mut e = engine();
        e.submit(AudioEvent::PlaySfx { id: 0 }, 0);
        assert_eq!(e.pending(), 3);
        e.submit(AudioEvent::Panic, e.frame_at(1));
        // The note already due at frame 0 survives; the two after the panic do
        // not. Pending is that note plus the panic itself.
        assert_eq!(e.pending(), 2, "panic cancelled the wrong events");
        let out = render(&mut e, 20);
        assert_eq!(e.active_voices(), 0);
        // The first note sounded before the panic; nothing after it did.
        let spf = samples_per_frame(e.sample_rate()) as usize;
        assert!(peak(&out[..spf * 2]) > 0.0);
        assert_eq!(peak(&out[spf * 4..]), 0.0, "a cancelled note still played");
    }

    #[test]
    fn a_late_event_is_played_late_not_dropped() {
        let mut e = engine();
        render(&mut e, 10); // clock is now well past zero
        e.submit(
            AudioEvent::Play {
                inst: 0,
                note: 60,
                vel: 255,
                frames: 4,
            },
            0, // a timestamp from the past
        );
        let out = render(&mut e, 4);
        assert!(peak(&out) > 0.1, "a late event was dropped");
    }

    #[test]
    fn an_unknown_sfx_is_counted() {
        let mut e = engine();
        e.submit(AudioEvent::PlaySfx { id: 99 }, 0);
        assert_eq!(e.pending(), 0);
        assert_eq!(e.stats().unknown_sfx, 1);
    }

    #[test]
    fn a_full_queue_drops_and_says_so() {
        let mut e = AudioEngine::new(SynthConfig::default());
        let mut bank = SoundBank::default();
        bank.add_instrument(
            "i",
            Patch {
                wave: Waveform::Square,
                ..Patch::default()
            },
        );
        // One effect longer than the whole queue.
        bank.add_sfx(
            "long",
            SfxDef {
                inst: 0,
                speed: 1,
                vel: 255,
                rows: crate::bank::parse_rows(&vec!["60"; 128].join(" ")).unwrap(),
            },
        );
        e.set_bank(bank);

        e.submit(AudioEvent::PlaySfx { id: 0 }, 0);
        e.submit(AudioEvent::PlaySfx { id: 0 }, 0);
        e.submit(AudioEvent::PlaySfx { id: 0 }, 0);
        assert_eq!(e.pending(), QUEUE_CAPACITY);
        assert!(e.stats().queue_overflow > 0);
        assert_eq!(e.stats().queue_peak, QUEUE_CAPACITY);
        // And it still renders: a full queue is a dropped sound, not a fault.
        let out = render(&mut e, 30);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    const MUSIC: &str = r#"
instrument bass { wave = triangle  attack = 0  decay = 0  sustain = 255  release = 20 }
instrument lead { wave = square    attack = 0  decay = 0  sustain = 255  release = 20 }
track song {
  tempo = 4
  bass = "36 - - - 43 - - -"
  lead = "60 64 67 72 . . . ."
}
"#;

    /// [`MUSIC`] plus a long sound effect, for the tests that need both.
    const MUSIC_AND_SFX: &str = r#"
instrument bass { wave = triangle  attack = 0  decay = 0  sustain = 255  release = 20 }
instrument lead { wave = square    attack = 0  decay = 0  sustain = 255  release = 20 }
instrument hit  { wave = noise     attack = 0  decay = 0  sustain = 255 }
track song {
  tempo = 4
  bass = "36 - - - 43 - - -"
  lead = "60 64 67 72 . . . ."
}
sfx bang { inst = hit  speed = 60  notes = "60" }
"#;

    fn engine_with(src: &str) -> AudioEngine {
        let (bank, errors) = parse(src);
        assert_eq!(errors, vec![], "{src}");
        let mut e = AudioEngine::new(SynthConfig::default());
        e.set_bank(bank);
        e
    }

    #[test]
    fn music_plays_and_keeps_playing() {
        let mut e = engine_with(MUSIC);
        e.submit(AudioEvent::PlayMusic { id: 0 }, 0);
        let out = render(&mut e, 60);
        assert!(peak(&out) > 0.1, "the track was silent");
        assert_eq!(e.playing_music(), Some(0));
        // Eight rows at tempo 4 is 32 frames, so a 60-frame render has been
        // round once and is still going.
        assert!(e.synth_stats().started > 8);
    }

    #[test]
    fn music_runs_on_the_audio_clock_not_the_frame_clock() {
        // The point of the whole module. Same total samples, one rendered in
        // tidy frame-sized blocks and one in blocks that have nothing to do
        // with a frame: the music must come out identical, because a game that
        // renders late or unevenly must not shift the beat.
        let render_in = |chunk: usize| {
            let mut e = engine_with(MUSIC);
            e.submit(AudioEvent::PlayMusic { id: 0 }, 0);
            let total = samples_per_frame(e.sample_rate()) as usize * 40;
            let mut out = Vec::new();
            let mut buf = vec![0.0; chunk * 2];
            while out.len() < total * 2 {
                e.render(&mut buf);
                out.extend_from_slice(&buf);
            }
            out.truncate(total * 2);
            out
        };
        assert_eq!(render_in(800), render_in(137));
    }

    #[test]
    fn music_stop_releases_the_music_and_leaves_effects_alone() {
        let mut e = engine_with(MUSIC_AND_SFX);
        e.submit(AudioEvent::PlayMusic { id: 0 }, 0);
        e.submit(AudioEvent::PlaySfx { id: 0 }, 0);
        render(&mut e, 4);
        assert!(e.active_voices() >= 2, "expected music and an effect");

        e.submit(AudioEvent::StopMusic, e.now());
        render(&mut e, 4);
        assert_eq!(e.playing_music(), None);
        // The long sound effect is still going; the music is not.
        assert!(e.active_voices() >= 1, "music_stop took the sfx with it");
    }

    #[test]
    fn starting_a_track_replaces_the_one_playing() {
        let mut e = engine_with(
            r#"
            instrument a { wave = triangle  sustain = 255 }
            track one { tempo = 4  a = "36 - - -" }
            track two { tempo = 4  a = "48 - - -" }
            "#,
        );
        e.submit(AudioEvent::PlayMusic { id: 0 }, 0);
        render(&mut e, 8);
        e.submit(AudioEvent::PlayMusic { id: 1 }, e.now());
        render(&mut e, 8);
        assert_eq!(e.playing_music(), Some(1), "the first track kept playing");
    }

    #[test]
    fn panic_stops_the_music_too() {
        let mut e = engine_with(MUSIC);
        e.submit(AudioEvent::PlayMusic { id: 0 }, 0);
        render(&mut e, 8);
        e.submit(AudioEvent::Panic, e.now());
        render(&mut e, 8);
        assert_eq!(e.playing_music(), None);
        assert_eq!(e.active_voices(), 0);
        assert_eq!(peak(&render(&mut e, 30)), 0.0, "music survived Panic");
    }

    #[test]
    fn an_unknown_track_is_counted() {
        let mut e = engine_with(MUSIC);
        e.submit(AudioEvent::PlayMusic { id: 42 }, 0);
        render(&mut e, 4);
        assert_eq!(e.stats().unknown_track, 1);
        assert_eq!(e.playing_music(), None);
    }

    #[test]
    fn a_sound_effect_can_take_a_voice_from_the_music() {
        // Music is allocated at a lower priority on purpose: an explosion
        // eaten by a bassline is much more noticeable than the reverse.
        let mut e = engine_with(MUSIC_AND_SFX);
        e.submit(AudioEvent::PlayMusic { id: 0 }, 0);
        render(&mut e, 2);
        for _ in 0..MAX_VOICES {
            e.submit(AudioEvent::PlaySfx { id: 0 }, e.now());
        }
        render(&mut e, 2);
        assert!(e.synth_stats().stolen > 0, "nothing was ever stolen");
    }

    #[test]
    fn the_same_events_render_the_same_samples() {
        let run = || {
            let mut e = engine();
            e.submit(AudioEvent::PlaySfx { id: 0 }, 0);
            e.submit(
                AudioEvent::Play {
                    inst: 0,
                    note: 48,
                    vel: 200,
                    frames: 10,
                },
                e.frame_at(4),
            );
            render(&mut e, 30)
        };
        assert_eq!(run(), run());
    }
}
