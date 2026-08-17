# Kessel Synth — architecture

What sits under sound port `0x9`: the `kessel-audio` crate. Written first as a
plan (#64) and kept as the record of what was built and why — where the two
diverged, the divergence is noted rather than quietly edited out, because the
reasons are the useful part.

For *writing* sound in a game — declaring instruments, effects and music, and
reading the render report — see [`VM_AUDIO.md`](VM_AUDIO.md). This document is
the machinery underneath it.

The shape in one line: **a host-free synth crate, driven by an event log the VM
emits, rendered by whoever owns an audio device.** The VM never produces a
sample; the synth never opens a device. That boundary is the same one that makes
`kessel-vm` snapshotable, and it is what lets the same crate be a game sound
engine and a standalone instrument.

```
kessel-vm ──AudioEvent──► kessel-audio ──f32 stereo──► cpal / AAudio / AVAudio
 (no DSP)   (plain data)   (no I/O, no device)          (cli / ffi / host app)
```

## Why a separate crate, and which way the dependency points

`crates/audio` (`kessel-audio`) is a new workspace member with **no dependency
on `kessel-vm`**. That is the whole "tiny synth" story: an instrument app links
`kessel-audio` alone and never carries a stack machine, an assembler, or a
sprite blitter.

The event type has to live somewhere both sides see. It lives in
`kessel-audio::event`, and `kessel-vm` depends on `kessel-audio` for it (and
re-exports it from `device.rs`, where `SoundEvent` is today). Alternatives were
worse: putting it in `kessel-vm` drags the VM into the synth app; a third
`-types` crate is a crate to maintain for one enum. `kessel-audio` is pure
math with no I/O and no `std::fs`, so the VM's host-free rule is untouched.

```
crates/vm     kessel-vm      ──depends on──► kessel-audio  (event type only)
crates/audio  kessel-audio   ──depends on──► nothing
crates/cli    kessel         ──depends on──► both + cpal
crates/ffi    kessel-ffi     ──depends on──► both
```

**The rule that mirrors "no wgpu in `crates/vm`": no cpal, no AAudio, no
`AVAudioEngine`, no thread spawning, and no allocation-in-render inside
`crates/audio`.** It is a function from (events, time) to samples.

## The two clocks

The VM runs on frames. The synth runs on samples. Keeping them separate is the
single most load-bearing decision here.

```
game frame N ──► events, tagged with a sample timestamp
                 │
                 ▼
           SPSC event queue (lock-free, fixed capacity)
                 │
audio callback ──┴─► render(out): split the block at each pending timestamp
```

- `SAMPLE_RATE` is a parameter (48 kHz everywhere in practice). At 60 fps,
  `SAMPLES_PER_FRAME = 800 @ 48 kHz`. The offline renderer pins exactly that;
  the realtime one does not care.
- **Game events are timestamped by the game clock** — frame N's events land at
  `N * SAMPLES_PER_FRAME`, so a jump sound is always the same distance behind
  the jump.
- **The music sequencer runs on the audio clock**, not the frame clock. A late
  frame must not stutter the BGM. Only the game's own triggers come from the
  frame clock. This is easy to get backwards and expensive to unwind later.
- The renderer never blocks on the producer. If the game thread stalls, voices
  keep decaying and music keeps playing — silence-on-underrun is worse.

## Event surface

`SoundEvent` generalizes; the existing three variants keep working unchanged.

```rust
pub enum AudioEvent {
    PlaySfx  { id: u16 },
    PlayMusic{ id: u16 },
    StopMusic,
    /// Fire-and-forget: instrument, MIDI note, velocity, length in frames.
    Play     { inst: u8, note: u8, vel: u8, frames: u16 },
    /// Held note on a game-owned channel (0..7), released by NoteOff.
    NoteOn   { chan: u8, inst: u8, note: u8, vel: u8 },
    NoteOff  { chan: u8 },
    /// Everything off, tails included. Emitted on reset/restore — see below.
    Panic,
}
```

Deliberately POD and `Copy`: it goes through a lock-free queue, the observation
JSON, the attach protocol, and the C ABI as a flat array, with no conversion
layer at any of those.

**Channel ≠ voice.** A channel is a game-owned slot the game names in
`note_off`; a voice is an engine resource that gets stolen. Returning a voice
handle through a device port would need a `DEI` read and would make the game's
correctness depend on the allocator. `play(...)` with a frame duration is the
primary API precisely because it needs no bookkeeping at all.

### Device ports (`0x9`)

Registers `0..2` stay exactly as they are. The new ones latch and **commit on
the register that pops last**, which is the argument the game wrote first —
the same rule as the palette's `pal(i,r,g,b)` committing on the index write.
For `play(inst, note, frames)` the stack yields `frames` first and `inst` last,
so `inst` commits.

| reg | write | meaning |
|-----|-------|---------|
| 0 | `sfx(id)` | commit — unchanged |
| 1 | `music(id)` | commit — unchanged |
| 2 | `music_stop()` | commit — unchanged |
| 3 | frames | latch |
| 4 | velocity | latch |
| 5 | note | latch |
| 6 | instrument | **commit `Play`** |
| 7 | instrument | latch |
| 8 | channel | **commit `NoteOn`** (with latched inst/note/vel) |
| 9 | channel | **commit `NoteOff`** |

luax gains `play(inst, note, vel, frames)`, `note_on(chan, inst, note, vel)`,
and `note_off(chan)` beside `sfx` / `music` / `music_stop`. `play` takes a
velocity like `note_on` rather than assuming full: a game that plays notes at
all is a game that wants dynamics.

The commit register is the one holding the call's **first** argument, since that
is what a stack machine hands back last — `play(inst, …)` commits on `inst`,
`note_on(chan, …)` on `chan`. The palette's `pal(i,r,g,b)` works exactly this
way, and following it means there is one rule rather than two.

## Where patches live: metadata, not ROM bytes

Instruments, SFX, and songs are declared in luax and ride alongside the ROM as
metadata — exactly like `controls { }` and `screen { }`, which are parsed by
the front-end and carried on `Compiled` rather than assembled into the 64 KiB
space. Ids are assigned in declaration order and the name binds as a
compile-time constant, the way `sprite` already works, so `sfx(boom)` compiles
to `sfx(2)`.

Putting patch tables in ROM memory instead would mean the host reads game RAM
to know what a sound is, and `sfx()` semantics would depend on whatever the
game last wrote there. Metadata keeps the VM ignorant of DSP and hands the
audio engine a finished `SoundBank` at load time.

```lua
fx {                        -- one chorus and one reverb for the whole mix
  reverb_size = 190  reverb_damping = 90
  chorus_rate = 50   chorus_depth = 140
}

instrument lead {
  wave = "square"          -- sine | triangle | saw | square | noise
  attack = 0  decay = 8  sustain = 100  release = 4   -- ms, ms, 0..255, ms
  pitch_env = 24  pitch_decay = 40      -- semitones, ms  (kick/laser/coin)
  filter = "lpf"  cutoff = 160  resonance = 40         -- 0..255
  lfo = "tri"  lfo_rate = 30  lfo_depth = 20  lfo_target = "cutoff"
  volume = 200  pan = 0                                -- 0..255, -128..127
  chorus = 15  reverb = 40  distortion = 0             -- sends, 0..255
}

sfx boom  { inst = lead  speed = 3  notes = "48 - 43 . 36" }
-- note number, `-` holds the previous note, `.` rests; `speed` is frames/row.
-- A note plus holds is one long note; a repeated number retriggers. That is
-- the difference between a drone and a machine gun, and games want both.

track intro {
  tempo = 7          -- frames per row
  vel = 150          -- music sits under the sound effects
  loop = 1
  bass = "36 - - - 43 - - -"    -- a key that is not reserved names an
  lead = "60 . 64 67 . 72 - -"  -- instrument, and gives that channel's rows
}
```

Every parameter is a `u8`/`u16` — the byte world the VM already lives in, and
the range an LLM writes correctly without a units table. Conversion to Hz, Q,
and seconds happens once at bank-compile time into a float `VoiceParams`.
`cutoff` maps exponentially: `0 → 80 Hz`, `128 → ~1.2 kHz`, `255 → 18 kHz`.
`resonance` maps to Q from Butterworth-flat to a strong peak, and `pan` is
constant-power normalized so that centre is unity — otherwise adding pan would
have quietly dropped every existing sound by 3 dB.

The same block syntax, parsed by the same code, is the standalone synth's patch
file format. That is free only if the parser lives in `kessel-audio` and luax
calls into it — so it does: `kessel-audio::bank::parse` owns the grammar for
these four blocks, `luax.rs` hands it the block text.

## Engine internals

```
Voice × 16
  osc (sine/tri/saw/square/noise)  ← pitch env + LFO
  ADSR
  biquad LPF/HPF
  gain / pan
  ├─ dry ───────────────────────────────────┐
  ├─ chorus send ─► Chorus (1 shared) ──────┤
  └─ reverb send ─► Reverb (1 shared) ──────┤
                                            ▼
                              Master: limiter → clamp
```

- **16 voices**, fixed array, no allocation. BGM wants 6–8, SFX 2–4, and
  releasing voices linger; 8 forces you to think about stealing constantly.
- **Chorus and reverb are shared sends, never per-voice.** Sixteen reverbs is
  sixteen times the cost for an effect nobody can localize anyway.
- **Reverb is Freeverb-shaped**: 4 parallel combs → 2 all-passes, parameters
  `room_size / damping / wet`. Not convolution. Not a parameter more.
- **Chorus** is one modulated short delay line, `rate / depth / wet`, with the
  L/R LFOs phase-offset. That is where the width comes from.
- **Distortion** is soft clip with a drive term, per voice, with an output gain
  that keeps the level fixed as the drive rises — so `distortion` adds crunch
  rather than volume.
- **The master is a limiter and nothing else.** The sketch above said soft clip
  *plus* limiter; measuring the saturator says otherwise — it is 6.8% down at
  0.5 and 19% down at 0.9, so on the master bus it would attenuate and colour
  every quiet mix to catch peaks the limiter has already caught. The limiter
  has instant attack and a 150 ms release, so nothing reaches the final clamp
  and a mix that never approaches the ceiling comes out bit-unchanged. Voices
  keep the soft clip, where that curve is the point.
- **Pitch envelope before LFO** in the priority order. Kick, laser, jump, coin,
  and explosion all come from noise/saw + pitch envelope + ADSR; an LFO adds
  vibrato and wobble but is not what makes game sounds work.
- **Noise uses a seeded xorshift**, never OS randomness, so an offline render is
  reproducible.
- Voice stealing: releasing voice → lowest priority → quietest envelope →
  oldest. Music allocates at lower priority than SFX, so a boom never gets eaten
  by a bassline.

Explicitly **not in v1**: FM, wavetables, arbitrary routing, per-voice reverb,
delay, EQ, compressor, bitcrusher, automation curves. Bitcrush (bit depth +
sample-rate reduction) is the first thing to add afterwards; it is two
parameters and it is the one retro effect this design cannot fake.

## Public API

```rust
// The instrument. Standalone-synth apps use only this.
pub struct Synth { /* fixed voices, fx, master; no alloc after new */ }
impl Synth {
    pub fn new(cfg: SynthConfig) -> Self;       // sample_rate, seed
    pub fn set_bank(&mut self, bank: SoundBank);
    pub fn handle(&mut self, ev: AudioEvent);   // apply now
    pub fn render(&mut self, out: &mut [f32]);  // interleaved stereo
}

// The console's engine: bank + sequencer + timestamped queue on top of Synth.
pub struct AudioEngine { /* ... */ }
impl AudioEngine {
    pub fn submit(&mut self, ev: AudioEvent, at_sample: u64);
    pub fn render(&mut self, out: &mut [f32]); // splits blocks at event times
}
```

`render` must be callable from an audio callback: no allocation, no locks, no
syscalls, no panics. That is a test, not a comment — a `#[test]` that renders a
million samples under an allocation-counting global allocator.

Realtime and offline share `render`. Offline calls it with exactly
`SAMPLES_PER_FRAME` per frame and gets a deterministic result for a given event
log; realtime calls it with whatever the device asks for and gets the same
audio modulo block boundaries. Bit-exactness across CPUs is *not* a goal for
DSP — only reproducibility of the event log, which the VM already guarantees.

## Hosts

| host | who owns the device | how it reaches the engine |
|------|--------------------|---------------------------|
| `kessel run` | `crates/cli`, cpal, feature `audio` (implies `play`) | frame loop drains the frame's events → `kessel_audio::EventQueue` → cpal callback renders |
| Android | Kotlin `AudioTrack` on its own thread | `kessel_player_audio_render(p, buf, frames)` fills a **direct `ByteBuffer`** of f32 — the same reason frames do, 60 Hz of heap traffic is not acceptable |
| iOS | `AVAudioSourceNode` | the same C ABI from the render block — not built; nothing is needed here but the header |
| `kessel mcp` | nobody | events recorded only |

The C ABI, matching the existing `kessel_player_*` shape (null-tolerant,
`catch_unwind` around anything that runs game or DSP code):

```c
bool     kessel_player_audio_enable(KesselPlayer*, uint32_t sample_rate);
uint32_t kessel_player_audio_render(KesselPlayer*, float* out, uint32_t frames);
uint64_t kessel_player_audio_dropped(KesselPlayer*);
```

`audio_render` is the **only** entry point that does not take the console's
lock, and that is the design rather than an optimization: an audio thread
waiting on a frame of game code is a click in everything. The queue between them
is multi-producer, because the ABI promises a handle is usable from several
threads and a queue that corrupts under a second caller is worse to own than a
compare-exchange.

Audio is opt-in per host, so a host that never enables it pays nothing and the
Android app can ship its next release without it.

## Reset, snapshot, and attach

Synth state is **not** part of a VM snapshot. `vm_snapshot`/`vm_restore` rewind
the machine, and the audio engine is downstream of the event log. But a rewind
with a two-second reverb tail hanging over it sounds broken, so:

**`vm_reset`, `vm_restore`, and ROM load emit `AudioEvent::Panic`** — all voices
off, sequencer stopped, delay lines and reverb cleared. This is the host's job
in `VmConsole`, one line at each of three call sites, and it is the kind of
thing that is invisible until someone attaches a player and wonders why the
music doubled.

## Agent loop

The agent cannot listen, so the observable is a render plus a summary — not an
MCP audio content block:

```
vm_render_audio(frames: u16, path?: string)
  → writes a .wav into the workspace root, returns:
    duration, peak, rms, clipped samples, voices used / stolen,
    and the event trace with frame numbers
```

That is enough to debug the common failures — nothing triggered, everything
clipping, a voice count that says stealing ate the melody — without a listener.
The CLI gets the same thing:

```bash
kessel render-audio games/tetris.lua --frames 600 -o preview.wav
```

Which also means `games/` has an audio guard beside
`crates/vm/tests/games_compile.rs`: every bank compiles, and 300 frames of
render produce no NaN and no sustained clipping.

## The standalone synth

Because `kessel-audio` links alone and its bank format is text, a "tiny synth"
is a small host over `Synth`: parse a patch file, map input to
`NoteOn`/`NoteOff`, call `render`. `kessel synth patch.ksnd` is a plausible CLI
subcommand (computer keyboard → notes) and an iOS/Android instrument app is the
same C ABI with no `KesselPlayer` at all — which is the argument for the two
API layers above: `Synth` for instruments, `AudioEngine` for the console.

This is also why the patch grammar must not depend on luax. If patches parse
only as part of a game source file, the synth app has to link the compiler.

Both halves of that still hold: `cargo tree -p kessel-audio` is one line, and
`bank::parse` is what reads a patch file — the luax front-end shares the field
setters rather than the other way round. Nobody has written the host, so the
claim is unproven in the way any unbuilt thing is; what is proven is that
nothing stands in its way.

## Build order

1. `crates/audio` skeleton: `AudioEvent`, `Synth`, oscillators, ADSR, pitch
   envelope, voice allocator. Test: render to WAV, eyeball a scope. *(Done.)*
2. Biquad filter, pan, per-voice drive, master limiter. Allocation-free render
   test. *(Done — the master lost its soft clip; see above.)*
3. `SoundBank` + patch grammar + `sfx`/`Play` handling. *(Done, except `track`
   — a grammar for songs with no sequencer to play them would be surface
   nothing calls, so it lands with step 7.)*
4. `kessel render-audio` and `vm_render_audio` — the whole loop is observable
   before any device is opened. *(Done.)*
5. cpal in `kessel run`. This is where latency and underrun get tuned.
   *(Done — the tuning decision was to keep the device's default buffer: at
   5–11 ms it is already under one 60 Hz frame, and forcing it smaller trades a
   real underrun risk for inaudible latency.)*
6. Chorus, reverb, sends. *(Done. The reverb needed an input gain the sketch
   did not mention — four parallel combs sum, so an un-scaled network returns
   ~200× at the largest room.)*
7. Sequencer, `track` blocks, `music`/`music_stop` on the audio clock. *(Done.
   Tracks gained a `vel`, because several channels sustaining is a much louder
   thing than one hit and music that arrives already fighting the limiter
   leaves no room for the explosion it is meant to sit under.)*
8. FFI + Android `AudioTrack`. *(Done.)*
9. Device ports 3–9 and the luax `play` / `note_on` / `note_off` builtins.
   *(Done, and moved **ahead** of step 8: a game that catches falling notes
   needs them, and the port table above had `note_on` committing on the
   instrument — the commit has to be the call's first argument, since that is
   what a stack machine hands back last.)*
10. A `games/` audio guard. *(Done — its first run found `games/platform.lua`
    triggering sound ids it never declared, and under that, a name collision:
    `sfx(coin)` was compiling to the id of `sprite coin`.)*

## What was deliberately left out

FM, wavetables, arbitrary routing, per-voice reverb, delay, EQ, compressor, and
automation curves — all still out, for the reasons above. **Bitcrusher** is the
first thing worth adding: two parameters, and the one retro effect this design
cannot fake.

Two things the plan promised that nobody has built yet, because nothing needs
them: `kessel synth` as a standalone instrument (the crate links alone and its
patch format is text, so this is a host and not a redesign), and iOS, which
needs nothing from this repo but `crates/ffi/include/kessel.h`.
