//! What the console asks the synth to do.
//!
//! This is the wire between a machine that runs on frames and a synth that runs
//! on samples, so it is deliberately plain data: `Copy`, no allocation, no
//! lifetimes. The same value crosses a lock-free queue, the observation record,
//! and (as a flat array) the C ABI, with no conversion layer at any of them.

/// A sound the game asked for.
///
/// The first three variants are the console's existing sound ports (`0x9`
/// registers 0–2). The note-level variants are the low-level API for games that
/// want to sequence their own sound, and the surface a standalone instrument
/// drives directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEvent {
    /// Trigger sound effect `id` from the bank.
    PlaySfx { id: u16 },
    /// Start music track `id` from the bank, replacing whatever was playing.
    PlayMusic { id: u16 },
    /// Stop the music (sound effects keep ringing).
    StopMusic,
    /// Fire-and-forget note: hold for `frames`, then release.
    ///
    /// The primary API. It needs no bookkeeping from the game — no voice
    /// handle to keep, nothing to release, nothing to leak.
    Play {
        inst: u8,
        note: u8,
        vel: u8,
        frames: u16,
    },
    /// Start a note held on a game-owned channel, released by [`Self::NoteOff`].
    ///
    /// `chan` is a slot the *game* names, not a voice the engine allocated:
    /// voices get stolen, channels do not, so a game can always turn off the
    /// note it turned on.
    NoteOn {
        chan: u8,
        inst: u8,
        note: u8,
        vel: u8,
    },
    /// Release whatever [`Self::NoteOn`] started on this channel.
    NoteOff { chan: u8 },
    /// Everything off *including tails* — voices, envelopes, and (once they
    /// exist) delay lines and reverb.
    ///
    /// Emitted on reset, snapshot restore, and ROM load. A rewound timeline
    /// with the old one's reverb still ringing over it sounds broken.
    Panic,
}
