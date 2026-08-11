//! A lock-free queue of [`AudioEvent`], from a game thread to an audio callback.
//!
//! Every host that plays sound live needs exactly this and must not get it
//! wrong twice, so it lives here rather than in one of them: the desktop player
//! and the mobile FFI feed their synths through the same queue.
//!
//! Producers are whatever threads ask for sound; the consumer is the audio
//! callback. Nothing blocks, the consumer never allocates, and a full queue
//! drops the sound rather than making a game wait for a speaker.
//!
//! **Many producers, one consumer.** A single producer is all a game loop needs
//! and would be cheaper, but the C ABI this feeds promises that a handle is safe
//! to use from several threads at once — and a queue that quietly corrupts under
//! a second caller is a worse thing to own than a compare-exchange.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::event::AudioEvent;

/// Events the queue can hold. One frame emits a handful at most, so this is
/// several seconds of backlog — far past the point where dropping is right.
pub const QUEUE_CAPACITY: usize = 256;

/// A multi-producer, single-consumer queue of [`AudioEvent`].
///
/// Each slot is an `AtomicU64` holding an *encoded* event, which is what makes
/// this whole type safe code: a queue of `AudioEvent` values would need
/// `UnsafeCell` and hand-written synchronization, and an `unsafe` block in the
/// one place nobody can attach a debugger to is a poor trade for a 7-byte enum
/// that fits in a `u64` with room to spare.
pub struct EventQueue {
    slots: [AtomicU64; QUEUE_CAPACITY],
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

impl EventQueue {
    pub fn new() -> Self {
        EventQueue {
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            rejected: AtomicU64::new(0),
        }
    }

    /// Producer side. Returns false if the queue was full.
    ///
    /// Safe from any number of threads: the slot is *reserved* with a
    /// compare-exchange and only then written, so two producers can never take
    /// the same one.
    pub fn push(&self, ev: AudioEvent) -> bool {
        let mut tail = self.tail.load(Ordering::Relaxed);
        loop {
            let head = self.head.load(Ordering::Acquire);
            if tail.wrapping_sub(head) >= QUEUE_CAPACITY {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            match self.tail.compare_exchange_weak(
                tail,
                tail.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => tail = actual,
            }
        }
        // Release: the slot's contents must be visible before a consumer that
        // reads a non-empty slot can act on them.
        self.slots[tail % QUEUE_CAPACITY].store(encode(ev), Ordering::Release);
        true
    }

    /// Consumer side. One thread only.
    pub fn pop(&self) -> Option<AudioEvent> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        // A reserved-but-unwritten slot reads as zero, which `encode` never
        // produces. Treating it as "nothing yet" rather than advancing is what
        // keeps a producer that was interrupted between reserving and writing
        // from costing the event: the next call finds it.
        let bits = self.slots[head % QUEUE_CAPACITY].load(Ordering::Acquire);
        if bits == 0 {
            return None;
        }
        self.slots[head % QUEUE_CAPACITY].store(0, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        decode(bits)
    }

    /// Pushes the queue refused; see the field.
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }
}

/// Pack an event into a `u64`: an 8-bit tag and up to six bytes of payload.
pub(crate) fn encode(ev: AudioEvent) -> u64 {
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
pub(crate) fn decode(bits: u64) -> Option<AudioEvent> {
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

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    #[test]
    fn every_event_survives_the_encoding() {
        // The queue's correctness rests on this: a field that got truncated
        // would play a different note, not fail.
        for ev in SAMPLES {
            assert_eq!(decode(encode(ev)), Some(ev), "{ev:?}");
        }
    }

    #[test]
    fn an_encoded_event_is_never_zero() {
        // The consumer uses zero to mean "reserved but not yet written". If any
        // event encoded to zero it would be skipped forever.
        for ev in SAMPLES {
            assert_ne!(encode(ev), 0, "{ev:?} encodes to the empty marker");
        }
    }

    #[test]
    fn an_empty_slot_is_not_an_event() {
        assert_eq!(decode(0), None);
        assert_eq!(decode(0xff), None);
    }

    #[test]
    fn the_ring_is_first_in_first_out() {
        let r = EventQueue::new();
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
        let r = EventQueue::new();
        for _ in 0..QUEUE_CAPACITY {
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
        let r = EventQueue::new();
        // Several laps, so the modulo indexing and the wrapping counters both
        // get exercised.
        for i in 0..QUEUE_CAPACITY * 5 {
            assert!(r.push(AudioEvent::PlaySfx { id: i as u16 }));
            assert_eq!(r.pop(), Some(AudioEvent::PlaySfx { id: i as u16 }));
        }
        assert_eq!(r.rejected(), 0);
    }

    /// Several producers at once, which the C ABI allows.
    ///
    /// With a plain load/store on `tail` two producers take the same slot and
    /// one event overwrites the other — silently, and only under contention.
    #[test]
    fn many_producers_lose_nothing() {
        const PRODUCERS: usize = 4;
        const EACH: usize = 5_000;
        let q = Arc::new(EventQueue::new());

        let writers: Vec<_> = (0..PRODUCERS)
            .map(|p| {
                let q = Arc::clone(&q);
                std::thread::spawn(move || {
                    for _ in 0..EACH {
                        // Retry rather than drop, so the count at the end is
                        // exact and a lost event cannot hide as a rejection.
                        while !q.push(AudioEvent::NoteOff { chan: p as u8 }) {
                            std::thread::yield_now();
                        }
                    }
                })
            })
            .collect();

        let mut seen = [0usize; PRODUCERS];
        let mut total = 0;
        // Bounded, because the failure this is looking for is a *lost* event —
        // and waiting forever for one that will never arrive is a hung test
        // rather than a failing one.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while total < PRODUCERS * EACH && std::time::Instant::now() < deadline {
            match q.pop() {
                Some(AudioEvent::NoteOff { chan }) => {
                    seen[chan as usize] += 1;
                    total += 1;
                }
                Some(other) => panic!("corrupted event {other:?}"),
                None => std::thread::yield_now(),
            }
        }
        assert_eq!(
            seen, [EACH; PRODUCERS],
            "a producer's events went missing under contention"
        );
        for w in writers {
            w.join().unwrap();
        }
    }

    #[test]
    fn a_producer_and_a_consumer_agree_across_threads() {
        // Not a proof of the memory ordering, but it does catch an index that
        // is wrong under contention, which a single-threaded test cannot.
        let ring = Arc::new(EventQueue::new());
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
