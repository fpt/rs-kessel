//! The music player: walks a [`TrackDef`]'s rows and starts notes.
//!
//! **It runs on the audio clock, not the game's.** A row falls due after so
//! many *samples*, computed once from the track's tempo, and nothing about it
//! depends on when the game last ticked. That is the whole reason it is
//! separate from the frame-timestamped event queue: a game that drops a frame
//! should drop a frame, not stutter the music. Sound effects go the other way —
//! they are the game's own timing and must stay with it.
//!
//! What the sequencer does *not* do is render. It reports which notes fall due
//! and when the next row is, and [`crate::AudioEngine`] splits its render there
//! and starts them. Keeping the decision and the sound apart is what lets the
//! offline renderer and a device callback share one implementation.

use crate::bank::{Row, TrackDef};

/// A note the sequencer wants started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeqNote {
    pub inst: u8,
    pub note: u8,
    /// The track's velocity, carried through so the engine does not have to
    /// look the track up again to find it.
    pub vel: u8,
    /// How long to hold it, in frames — a note plus its holds, exactly as in a
    /// sound effect.
    pub frames: u16,
}

/// Position in a track, measured in samples.
pub struct Sequencer {
    /// Which track is playing, if any.
    track: Option<u16>,
    /// Next row to play.
    row: usize,
    /// Absolute sample time the next row falls due.
    next_row_at: u64,
    /// Samples per row, from the track's tempo. Zero when stopped.
    row_samples: u64,
    looping: bool,
    rows: usize,
}

impl Default for Sequencer {
    fn default() -> Self {
        Self::new()
    }
}

impl Sequencer {
    pub fn new() -> Self {
        Sequencer {
            track: None,
            row: 0,
            next_row_at: 0,
            row_samples: 0,
            looping: false,
            rows: 0,
        }
    }

    pub fn playing(&self) -> Option<u16> {
        self.track
    }

    /// The row about to play — for a host that wants to show position.
    pub fn row(&self) -> usize {
        self.row
    }

    /// Start `track` at `now`, from the top.
    ///
    /// Always from the top: resuming where a previous track left off would tie
    /// two unrelated pieces of music together, and "start the level theme"
    /// means start it.
    pub fn start(&mut self, id: u16, def: &TrackDef, now: u64, samples_per_frame: u64) {
        self.track = Some(id);
        self.row = 0;
        self.rows = def.rows();
        self.looping = def.looping;
        self.row_samples = def.tempo.max(1) as u64 * samples_per_frame;
        // The first row is due immediately, not one row from now.
        self.next_row_at = now;
        if self.rows == 0 {
            self.stop();
        }
    }

    pub fn stop(&mut self) {
        self.track = None;
        self.row = 0;
        self.row_samples = 0;
    }

    /// When the next row falls due, or `None` when nothing is playing.
    pub fn next_row_at(&self) -> Option<u64> {
        self.track.map(|_| self.next_row_at)
    }

    /// If a row is due at `now`, advance past it and write its notes into
    /// `out`, returning how many there are.
    ///
    /// `out` is a caller-owned slice rather than a returned `Vec` because this
    /// runs inside the render loop, where allocating is not allowed.
    pub fn take_due(&mut self, now: u64, def: &TrackDef, out: &mut [SeqNote]) -> usize {
        if self.track.is_none() || now < self.next_row_at {
            return 0;
        }
        let row = self.row;
        let mut n = 0;
        for channel in &def.channels {
            let Some(Row::Note(note)) = channel.rows.get(row).copied() else {
                continue;
            };
            if n == out.len() {
                break;
            }
            // A note plus the holds after it is one long note, as in an sfx.
            let mut len = 1u32;
            while channel.rows.get(row + len as usize) == Some(&Row::Hold) {
                len += 1;
            }
            out[n] = SeqNote {
                inst: channel.inst,
                note,
                vel: def.vel,
                frames: (len * def.tempo.max(1) as u32).min(u16::MAX as u32) as u16,
            };
            n += 1;
        }

        self.row += 1;
        self.next_row_at += self.row_samples;
        if self.row >= self.rows {
            if self.looping {
                self.row = 0;
            } else {
                // Let the last row's notes finish; only the *scheduling* stops.
                self.stop();
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::parse;

    const SPF: u64 = 800; // 48 kHz at 60 fps

    fn track(src: &str) -> TrackDef {
        let (bank, errors) = parse(src);
        assert!(errors.is_empty(), "{errors:?}");
        bank.tracks[0].clone()
    }

    fn two_channels() -> TrackDef {
        track(
            r#"
            instrument bass { wave = triangle }
            instrument lead { wave = square }
            track t {
              tempo = 2
              bass = "36 - . 36"
              lead = "60 64 67 ."
            }
            "#,
        )
    }

    /// Collect every note the sequencer produces over `samples`, with the time
    /// it fell due.
    fn play(seq: &mut Sequencer, def: &TrackDef, samples: u64) -> Vec<(u64, SeqNote)> {
        let mut out = Vec::new();
        let mut buf = [SeqNote {
            inst: 0,
            note: 0,
            vel: 0,
            frames: 0,
        }; 8];
        for now in 0..samples {
            let n = seq.take_due(now, def, &mut buf);
            for note in &buf[..n] {
                out.push((now, *note));
            }
        }
        out
    }

    #[test]
    fn rows_fall_due_on_the_audio_clock() {
        let def = two_channels();
        let mut seq = Sequencer::new();
        seq.start(0, &def, 0, SPF);
        let notes = play(&mut seq, &def, SPF * 8);

        // tempo 2 = 1600 samples a row. Row 0 at 0, row 1 at 1600, and so on —
        // no reference to any frame the game may or may not have run.
        let times: Vec<u64> = notes.iter().map(|(t, _)| *t).collect();
        assert_eq!(times, [0, 0, 1600, 3200, 4800]);
    }

    #[test]
    fn holds_make_one_long_note() {
        let def = two_channels();
        let mut seq = Sequencer::new();
        seq.start(0, &def, 0, SPF);
        let notes = play(&mut seq, &def, SPF * 8);

        // bass row 0 is "36 -", so four frames rather than two.
        let bass: Vec<SeqNote> = notes
            .iter()
            .filter(|(_, n)| n.inst == 0)
            .map(|(_, n)| *n)
            .collect();
        assert_eq!(bass[0].note, 36);
        assert_eq!(bass[0].frames, 4);
        // The second 36 is a fresh hit of one row.
        assert_eq!(bass[1].frames, 2);
    }

    #[test]
    fn a_rest_starts_nothing() {
        let def = two_channels();
        let mut seq = Sequencer::new();
        seq.start(0, &def, 0, SPF);
        let notes = play(&mut seq, &def, SPF * 8);
        // Four rows, two channels, minus the two rests and one hold.
        assert_eq!(notes.len(), 5);
    }

    #[test]
    fn a_looping_track_comes_back_around() {
        let def = two_channels(); // loops by default
        let mut seq = Sequencer::new();
        seq.start(0, &def, 0, SPF);
        let notes = play(&mut seq, &def, SPF * 20);
        assert!(seq.playing().is_some(), "a looping track stopped");
        // Two passes' worth and counting.
        assert!(
            notes.len() > 8,
            "it did not come back around: {}",
            notes.len()
        );
    }

    #[test]
    fn a_one_shot_track_stops_at_the_end() {
        let def = track(
            r#"
            instrument i { wave = sine }
            track t { tempo = 2  loop = 0  i = "60 62 64" }
            "#,
        );
        let mut seq = Sequencer::new();
        seq.start(0, &def, 0, SPF);
        let notes = play(&mut seq, &def, SPF * 20);
        assert_eq!(notes.len(), 3, "a one-shot track repeated");
        assert!(seq.playing().is_none(), "it never stopped");
    }

    #[test]
    fn starting_a_track_starts_it_at_the_top() {
        let def = two_channels();
        let mut seq = Sequencer::new();
        seq.start(0, &def, 0, SPF);
        play(&mut seq, &def, SPF * 5);
        assert_ne!(seq.row(), 0);

        // Restarting is from row 0, not from wherever the last one had got to.
        seq.start(0, &def, SPF * 5, SPF);
        assert_eq!(seq.row(), 0);
    }

    #[test]
    fn the_first_row_is_due_immediately() {
        // Otherwise `music()` would be silent for a row, which reads as a bug
        // at any slow tempo.
        let def = two_channels();
        let mut seq = Sequencer::new();
        seq.start(0, &def, 12_345, SPF);
        assert_eq!(seq.next_row_at(), Some(12_345));
    }

    #[test]
    fn an_empty_track_stops_rather_than_spinning() {
        let def = track(
            r#"
            instrument i { wave = sine }
            track t { tempo = 4 }
            "#,
        );
        let mut seq = Sequencer::new();
        seq.start(0, &def, 0, SPF);
        assert!(seq.playing().is_none(), "an empty track kept playing");
        assert_eq!(seq.next_row_at(), None);
    }

    #[test]
    fn more_notes_than_the_buffer_holds_are_not_lost_forever() {
        // The engine sizes `out` for MAX_TRACK_CHANNELS, so this cannot happen
        // in practice — but a short buffer must truncate, not corrupt.
        let def = two_channels();
        let mut seq = Sequencer::new();
        seq.start(0, &def, 0, SPF);
        let mut one = [SeqNote {
            inst: 0,
            note: 0,
            vel: 0,
            frames: 0,
        }; 1];
        assert_eq!(seq.take_due(0, &def, &mut one), 1);
        assert_eq!(seq.row(), 1, "the row still advanced");
    }
}
