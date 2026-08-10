//! The audio device for `kessel run`: a cpal output stream fed by the game
//! thread through a lock-free queue.
//!
//! ```text
//! game thread              audio callback thread
//! ───────────              ─────────────────────
//! tick() → events ──► Ring ──► AudioEngine::render() ──► device
//! ```
//!
//! Three rules hold this together, and all three are about the callback thread:
//!
//! - **It never blocks.** Not on a mutex, not on the game thread. A stalled
//!   game keeps envelopes decaying and the last notes ringing; silence would be
//!   a worse failure than lateness.
//! - **It never allocates.** [`kessel_audio`] guarantees that for `render`; the
//!   queue is a fixed array and the stereo path writes straight into the
//!   device's buffer.
//! - **A full queue drops events**, counted, rather than making the game thread
//!   wait for a speaker.
//!
//! Events are applied at the start of the callback block that finds them, not
//! at a sample computed from the game's frame counter. The two clocks drift —
//! a game running slightly slow would otherwise accumulate a growing offset —
//! so realtime trades sample-accurate placement for a fixed latency of one
//! device buffer. `kessel render-audio` keeps the sample-accurate placement,
//! which is what makes *it* reproducible.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use kessel_audio::{AudioEngine, AudioEvent, SoundBank, SynthConfig};

/// Events the queue can hold. One frame emits a handful at most, so this is
/// several seconds of backlog — far past the point where dropping is right.
const RING_CAPACITY: usize = 256;

/// A single-producer, single-consumer queue of [`AudioEvent`].
///
/// Each slot is an `AtomicU64` holding an *encoded* event, which is what makes
/// this whole type safe code: a queue of `AudioEvent` values would need
/// `UnsafeCell` and hand-written synchronization, and an `unsafe` block in the
/// one place nobody can attach a debugger to is a poor trade for a 7-byte enum
/// that fits in a `u64` with room to spare.
struct Ring {
    slots: [AtomicU64; RING_CAPACITY],
    /// Next slot the consumer will read.
    head: AtomicUsize,
    /// Next slot the producer will write.
    tail: AtomicUsize,
    /// Pushes the queue refused. Whether a refusal *lost* an event depends on
    /// the caller: the game thread does not retry, so for it a rejection is a
    /// dropped sound — but a caller that retries loses nothing and still
    /// counts here.
    rejected: AtomicU64,
}

impl Ring {
    fn new() -> Self {
        Ring {
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            rejected: AtomicU64::new(0),
        }
    }

    /// Producer side. Returns false if the queue was full.
    fn push(&self, ev: AudioEvent) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= RING_CAPACITY {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.slots[tail % RING_CAPACITY].store(encode(ev), Ordering::Relaxed);
        // Release: the slot's contents must be visible before the consumer can
        // see the index that exposes them.
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Consumer side.
    fn pop(&self) -> Option<AudioEvent> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let bits = self.slots[head % RING_CAPACITY].load(Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        decode(bits)
    }

    fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }
}

/// Pack an event into a `u64`: an 8-bit tag and up to six bytes of payload.
fn encode(ev: AudioEvent) -> u64 {
    let (tag, a, b, c, d): (u64, u64, u64, u64, u64) = match ev {
        AudioEvent::PlaySfx { id } => (1, id as u64, 0, 0, 0),
        AudioEvent::PlayMusic { id } => (2, id as u64, 0, 0, 0),
        AudioEvent::StopMusic => (3, 0, 0, 0, 0),
        AudioEvent::Play {
            inst,
            note,
            vel,
            frames,
        } => (4, inst as u64, note as u64, vel as u64, frames as u64),
        AudioEvent::NoteOn {
            chan,
            inst,
            note,
            vel,
        } => (5, chan as u64, inst as u64, note as u64, vel as u64),
        AudioEvent::NoteOff { chan } => (6, chan as u64, 0, 0, 0),
        AudioEvent::Panic => (7, 0, 0, 0, 0),
    };
    tag | (a << 8) | (b << 24) | (c << 32) | (d << 40)
}

/// Inverse of [`encode`]. `None` for a tag that isn't one — an empty slot reads
/// as zero, and a queue that mistook that for an event would fire a note.
fn decode(bits: u64) -> Option<AudioEvent> {
    let a = ((bits >> 8) & 0xffff) as u16;
    let b = ((bits >> 24) & 0xff) as u8;
    let c = ((bits >> 32) & 0xff) as u8;
    let d = ((bits >> 40) & 0xffff) as u16;
    Some(match bits & 0xff {
        1 => AudioEvent::PlaySfx { id: a },
        2 => AudioEvent::PlayMusic { id: a },
        3 => AudioEvent::StopMusic,
        4 => AudioEvent::Play {
            inst: a as u8,
            note: b,
            vel: c,
            frames: d,
        },
        5 => AudioEvent::NoteOn {
            chan: a as u8,
            inst: b,
            note: c,
            vel: d as u8,
        },
        6 => AudioEvent::NoteOff { chan: a as u8 },
        7 => AudioEvent::Panic,
        _ => return None,
    })
}

/// A live output stream. Dropping it stops the audio.
pub struct AudioHost {
    // Held for its lifetime: dropping a cpal stream closes the device.
    _stream: cpal::Stream,
    ring: Arc<Ring>,
    sample_rate: u32,
}

impl AudioHost {
    /// Hand an event to the synth. Never blocks; a full queue drops it.
    pub fn send(&self, ev: AudioEvent) {
        self.ring.push(ev);
    }

    /// Sounds lost because the queue was full — worth printing on exit rather
    /// than losing silently. Exact here because [`send`](Self::send) never
    /// retries.
    pub fn dropped(&self) -> u64 {
        self.ring.rejected()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// The body of the audio callback: drain the queue, render, spread to the
/// device's channel count.
///
/// A free function rather than a closure body so it can be tested without a
/// sound card. The channel spreading is the reason that matters — like the
/// window's `blit`, a wrong stride here produces a plausible-sounding mistake
/// (one channel silent, or a mono device at half level) rather than a crash.
///
/// Allocation-free by construction: `scratch` is sized by the caller and long
/// blocks are rendered in chunks that fit it.
fn render_block(
    engine: &mut AudioEngine,
    ring: &Ring,
    out: &mut [f32],
    channels: usize,
    scratch: &mut [f32],
) {
    while let Some(ev) = ring.pop() {
        engine.handle(ev);
    }
    if channels == 2 {
        // The common path: the device buffer *is* an interleaved stereo block,
        // so render straight into it.
        engine.render(out);
        return;
    }
    if channels == 0 {
        return;
    }
    let per_chunk = (scratch.len() / 2) * channels;
    for block in out.chunks_mut(per_chunk) {
        let frames = block.len() / channels;
        let stereo = &mut scratch[..frames * 2];
        engine.render(stereo);
        for (i, frame) in block.chunks_mut(channels).enumerate() {
            let (l, r) = (stereo[i * 2], stereo[i * 2 + 1]);
            if frame.len() == 1 {
                // Mono: average, don't pick a side — a hard-panned sound would
                // otherwise vanish.
                frame[0] = (l + r) * 0.5;
            } else {
                frame[0] = l;
                frame[1] = r;
                // Surround: leave the rest silent rather than inventing a
                // centre channel from a stereo mix.
                for s in &mut frame[2..] {
                    *s = 0.0;
                }
            }
        }
    }
}

/// Open the default output device and start rendering `bank`.
///
/// Errors are returned rather than fatal: `kessel run` prints them and plays on
/// in silence. A machine with no sound card should still show the game.
pub fn start(bank: SoundBank) -> Result<AudioHost, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no output audio device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("no output config: {e}"))?;

    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    if format != cpal::SampleFormat::F32 {
        // Every platform this ships on offers f32 by default. Converting from
        // i16/u16 is easy but untested surface, so say so instead of guessing.
        return Err(format!(
            "output format {format:?} is not supported (need f32)"
        ));
    }

    // The engine runs at the *device's* rate, not a preferred one: resampling
    // would be a second signal path to get right for no gain.
    let mut engine = AudioEngine::new(SynthConfig {
        sample_rate,
        ..SynthConfig::default()
    });
    engine.set_bank(bank);

    // The device's default buffer, not a smaller one we ask for. At the sizes
    // hosts choose (256–512 frames, 5–11 ms) the latency is already under one
    // 60 Hz frame, so forcing it lower would trade a real underrun risk on a
    // loaded machine for a delay nobody can hear against a 16.7 ms frame.
    let ring = Arc::new(Ring::new());
    let consumer = Arc::clone(&ring);

    // Scratch for devices that are not stereo. Sized once, here, so the
    // callback never allocates; oversized blocks are rendered in chunks.
    const SCRATCH_FRAMES: usize = 4096;
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * 2];

    let stream = device
        .build_output_stream(
            &supported.config(),
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                render_block(&mut engine, &consumer, out, channels, &mut scratch);
            },
            |e| eprintln!("kessel: audio stream error: {e}"),
            None,
        )
        .map_err(|e| format!("could not open the audio stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("could not start audio: {e}"))?;

    Ok(AudioHost {
        _stream: stream,
        ring,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLES: [AudioEvent; 7] = [
        AudioEvent::PlaySfx { id: 0xBEEF },
        AudioEvent::PlayMusic { id: 3 },
        AudioEvent::StopMusic,
        AudioEvent::Play {
            inst: 200,
            note: 127,
            vel: 255,
            frames: 0xFFFF,
        },
        AudioEvent::NoteOn {
            chan: 7,
            inst: 3,
            note: 60,
            vel: 200,
        },
        AudioEvent::NoteOff { chan: 5 },
        AudioEvent::Panic,
    ];

    /// An engine holding one loud, instant, sustaining instrument.
    fn test_engine() -> AudioEngine {
        let (bank, errors) = kessel_audio::bank::parse(
            r#"
            instrument tone { wave = square  attack = 0  decay = 0  sustain = 255  volume = 255 }
            sfx beep { inst = tone  speed = 30  notes = "69" }
            "#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let mut e = AudioEngine::new(SynthConfig::default());
        e.set_bank(bank);
        e
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }

    #[test]
    fn the_callback_turns_queued_events_into_sound() {
        // Everything the audio thread does, minus the device: an event pushed
        // from the game side comes back as samples.
        let ring = Ring::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 512 * 2];
        let mut out = vec![0.0f32; 1024 * 2];

        render_block(&mut engine, &ring, &mut out, 2, &mut scratch);
        assert_eq!(peak(&out), 0.0, "silent until something is queued");

        ring.push(AudioEvent::PlaySfx { id: 0 });
        render_block(&mut engine, &ring, &mut out, 2, &mut scratch);
        assert!(
            peak(&out) > 0.1,
            "queued event never sounded: {}",
            peak(&out)
        );
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn panic_from_the_queue_silences_the_stream() {
        let ring = Ring::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 512 * 2];
        let mut out = vec![0.0f32; 1024 * 2];
        ring.push(AudioEvent::PlaySfx { id: 0 });
        render_block(&mut engine, &ring, &mut out, 2, &mut scratch);
        assert!(peak(&out) > 0.1);

        ring.push(AudioEvent::Panic);
        render_block(&mut engine, &ring, &mut out, 2, &mut scratch);
        assert_eq!(peak(&out), 0.0, "a reload left the old game ringing");
    }

    #[test]
    fn a_mono_device_gets_both_channels_averaged() {
        // Not one side: a hard-panned sound would vanish on a mono device.
        let ring = Ring::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 512 * 2];
        ring.push(AudioEvent::PlaySfx { id: 0 });
        let mut mono = vec![0.0f32; 900];
        render_block(&mut engine, &ring, &mut mono, 1, &mut scratch);
        assert!(peak(&mono) > 0.1, "mono device was silent");
        assert!(mono.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_surround_device_gets_the_mix_on_the_first_two_channels() {
        let ring = Ring::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 512 * 2];
        ring.push(AudioEvent::PlaySfx { id: 0 });
        const CH: usize = 6;
        let mut out = vec![0.0f32; 600 * CH];
        render_block(&mut engine, &ring, &mut out, CH, &mut scratch);

        let front: f32 = out
            .chunks(CH)
            .fold(0.0f32, |a, f| a.max(f[0].abs()).max(f[1].abs()));
        let rest: f32 = out
            .chunks(CH)
            .fold(0.0f32, |a, f| f[2..].iter().fold(a, |m, s| m.max(s.abs())));
        assert!(front > 0.1, "front channels were silent");
        assert_eq!(rest, 0.0, "the mix leaked into the surround channels");
    }

    #[test]
    fn a_block_longer_than_the_scratch_still_renders_whole() {
        // The chunking path: an oversized callback buffer must be filled, not
        // truncated, and without allocating a bigger scratch.
        let ring = Ring::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 64 * 2]; // deliberately tiny
        ring.push(AudioEvent::PlaySfx { id: 0 });
        let mut out = vec![0.0f32; 1000];
        render_block(&mut engine, &ring, &mut out, 1, &mut scratch);
        // The tail of the buffer is as alive as the head.
        assert!(peak(&out[..100]) > 0.1);
        assert!(peak(&out[900..]) > 0.1, "the block was left half-rendered");
    }

    #[test]
    fn every_event_survives_the_encoding() {
        // The queue's correctness rests on this: a field that got truncated
        // would play a different note, not fail.
        for ev in SAMPLES {
            assert_eq!(decode(encode(ev)), Some(ev), "{ev:?}");
        }
    }

    #[test]
    fn an_empty_slot_is_not_an_event() {
        assert_eq!(decode(0), None);
        assert_eq!(decode(0xff), None);
    }

    #[test]
    fn the_ring_is_first_in_first_out() {
        let r = Ring::new();
        for ev in SAMPLES {
            assert!(r.push(ev));
        }
        for ev in SAMPLES {
            assert_eq!(r.pop(), Some(ev));
        }
        assert_eq!(r.pop(), None);
    }

    #[test]
    fn a_full_ring_drops_and_counts() {
        let r = Ring::new();
        for _ in 0..RING_CAPACITY {
            assert!(r.push(AudioEvent::Panic));
        }
        assert!(!r.push(AudioEvent::Panic), "accepted more than capacity");
        assert_eq!(r.rejected(), 1);
        // And it still works afterwards: dropping is not a wedge.
        assert_eq!(r.pop(), Some(AudioEvent::Panic));
        assert!(r.push(AudioEvent::StopMusic));
    }

    #[test]
    fn the_ring_wraps_without_losing_order() {
        let r = Ring::new();
        // Several laps, so the modulo indexing and the wrapping counters both
        // get exercised.
        for i in 0..RING_CAPACITY * 5 {
            assert!(r.push(AudioEvent::PlaySfx { id: i as u16 }));
            assert_eq!(r.pop(), Some(AudioEvent::PlaySfx { id: i as u16 }));
        }
        assert_eq!(r.rejected(), 0);
    }

    #[test]
    fn a_producer_and_a_consumer_agree_across_threads() {
        // Not a proof of the memory ordering, but it does catch an index that
        // is wrong under contention, which a single-threaded test cannot.
        let ring = Arc::new(Ring::new());
        let producer = Arc::clone(&ring);
        const N: u16 = 10_000;
        let sender = std::thread::spawn(move || {
            let mut sent = 0u16;
            while sent < N {
                if producer.push(AudioEvent::PlaySfx { id: sent }) {
                    sent += 1;
                } else {
                    std::thread::yield_now();
                }
            }
        });

        let mut expect = 0u16;
        while expect < N {
            match ring.pop() {
                Some(AudioEvent::PlaySfx { id }) => {
                    assert_eq!(id, expect, "events arrived out of order");
                    expect += 1;
                }
                Some(other) => panic!("unexpected event {other:?}"),
                None => std::thread::yield_now(),
            }
        }
        sender.join().unwrap();
        // Nothing is asserted about `rejected` here: this producer *retries*
        // when the queue is full, so refusals are backpressure rather than
        // loss. Every event arrived, in order, which is the property.
    }
}
