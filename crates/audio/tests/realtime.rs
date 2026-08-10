//! The realtime contract: `Synth::render` runs on an audio callback thread.
//!
//! No allocation, no locks, no syscalls, no panics. Allocation is the one of
//! those that is easy to add by accident and impossible to notice — it does not
//! fail, it just occasionally takes a lock in the allocator while a device is
//! waiting for 5 ms of audio, and the user hears a click. So it gets a test
//! rather than a comment.
//!
//! This lives in an integration test so the counting allocator applies to its
//! own binary and nothing else, and so it can only reach the public API — the
//! same surface a host has.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use kessel_audio::{
    samples_per_frame, AudioEvent, FilterMode, Patch, Synth, SynthConfig, Waveform, MAX_VOICES,
};

thread_local! {
    /// Counting is off unless a test turns it on, so the harness's own
    /// allocations (and this thread's, outside the window under test) don't
    /// register.
    static ARMED: Cell<bool> = const { Cell::new(false) };
    /// Per *thread*, not global: the harness runs these tests concurrently, and
    /// a shared counter would charge one test's allocations to another. That
    /// mistake reads as a synth bug, which is the expensive kind to chase.
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

impl Counting {
    /// Charge one allocation to this thread, if it asked to be counted.
    ///
    /// `try_with`: during thread-local teardown the cell is gone, and an
    /// allocator that panics there would take the process with it.
    fn note(&self) {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        }
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        self.note();
        unsafe { System.alloc(l) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        self.note();
        unsafe { System.alloc_zeroed(l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        self.note();
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Run `f` with allocation counting on, and report how many it made.
fn count_allocs(f: impl FnOnce()) -> usize {
    ALLOCS.with(|n| n.set(0));
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    ALLOCS.with(Cell::get)
}

/// A bank that exercises every per-sample branch in the render path: a filter,
/// resonance, drive, panning, a pitch envelope, noise, and both effect sends.
///
/// The sends matter here specifically: they are what makes `render` walk its
/// buses and run the delay lines, which is the part most likely to be "fixed"
/// one day by allocating a buffer to fit the caller's block.
fn loaded_synth() -> Synth {
    let mut synth = Synth::new(SynthConfig::default());
    synth.set_fx(kessel_audio::bank::FxSettings::default());
    synth.set_instruments(&[
        Patch {
            wave: Waveform::Saw,
            filter: FilterMode::Lpf,
            cutoff: 120,
            resonance: 200,
            distortion: 90,
            pan: -80,
            sustain: 180,
            reverb: 200,
            chorus: 120,
            ..Patch::default()
        },
        Patch {
            wave: Waveform::Noise,
            attack_ms: 0,
            decay_ms: 40,
            sustain: 0,
            pitch_env: -24,
            filter: FilterMode::Hpf,
            cutoff: 60,
            pan: 90,
            reverb: 90,
            ..Patch::default()
        },
        Patch {
            wave: Waveform::Square,
            sustain: 200,
            chorus: 255,
            ..Patch::default()
        },
    ]);
    synth
}

#[test]
fn render_never_allocates() {
    let mut synth = loaded_synth();
    let mut block = vec![0.0f32; samples_per_frame(48_000) as usize * 2];

    // Warm up outside the counted window — the first render must not be
    // special, but if it were, this would hide it, so count it too below.
    let allocs = count_allocs(|| {
        // A million samples, with events landing throughout: notes starting,
        // voices being stolen, channels released, and a panic.
        let mut n = 0usize;
        let mut frame = 0u32;
        while n < 1_000_000 {
            match frame % 17 {
                0 => synth.handle(AudioEvent::Play {
                    inst: 0,
                    note: 40 + (frame % 40) as u8,
                    vel: 200,
                    frames: 30,
                }),
                3 => synth.handle(AudioEvent::Play {
                    inst: 1,
                    note: 60,
                    vel: 255,
                    frames: 4,
                }),
                7 => synth.handle(AudioEvent::NoteOn {
                    chan: (frame % 8) as u8,
                    inst: 2,
                    note: 55,
                    vel: 180,
                }),
                11 => synth.handle(AudioEvent::NoteOff {
                    chan: (frame % 8) as u8,
                }),
                13 => synth.handle(AudioEvent::PlaySfx { id: 1 }),
                _ => {}
            }
            if frame % 500 == 499 {
                synth.handle(AudioEvent::Panic);
            }
            synth.render(&mut block);
            n += block.len() / 2;
            frame += 1;
        }
    });

    assert_eq!(
        allocs, 0,
        "render allocated {allocs} times — it runs on an audio callback thread"
    );
}

#[test]
fn a_long_render_stays_finite_and_bounded() {
    // The other half of the contract: whatever the events are, the samples
    // handed to a device are real numbers inside full scale.
    let mut synth = loaded_synth();
    let mut block = vec![0.0f32; 1024];
    let mut worst = 0.0f32;

    for frame in 0..2_000u32 {
        // Every voice sounding, all the time, with the loudest patch.
        if frame % 2 == 0 {
            for i in 0..MAX_VOICES {
                synth.handle(AudioEvent::Play {
                    inst: 0,
                    note: 30 + i as u8 * 4,
                    vel: 255,
                    frames: 120,
                });
            }
        }
        synth.render(&mut block);
        for s in &block {
            assert!(s.is_finite(), "render produced {s} at frame {frame}");
            worst = worst.max(s.abs());
        }
    }
    assert!(worst <= 1.0, "render left full scale: {worst}");
    // And it did not solve the problem by going silent.
    assert!(worst > 0.5, "render was inaudible: {worst}");
}

#[test]
fn set_instruments_is_the_only_call_that_allocates() {
    // Stated as a contract in the crate docs, so it gets checked: a host may
    // call this at load time and must not call it from a callback.
    let mut synth = Synth::new(SynthConfig::default());
    let patches = vec![Patch::default(); 8];
    let allocs = count_allocs(|| synth.set_instruments(&patches));
    assert!(
        allocs > 0,
        "set_instruments stopped allocating — if the instrument table became \
         fixed-size, this test and the crate docs should say so"
    );
}
