# Kessel Sound

Everything a game does to make noise: declaring instruments, sound effects and
music, triggering them, and checking the result without ears. Split out of
[`VM.md`](VM.md), which owns the machine itself. For how the synth is *built* —
voices, filters, the two clocks, engine internals — see [`SYNTH.md`](SYNTH.md).

The load-bearing idea: **the VM makes no sound.** It records what a game asked
for into the observation's `sound` array and a *host* renders it — `kessel run`
through cpal, the Android app through `AudioTrack`, `vm_render_audio` and
`kessel render-audio` to a file. That is what keeps `vm_snapshot`/`vm_restore`
and frame-exact replay meaningful: a machine that synthesized audio itself could
not be rewound.

## Ports

| Port | Dir | Meaning |
|------|-----|---------|
| `0x90` `0x91` `0x92` | out | `sfx(id)` / `music(id)` / music-stop |
| `0x93`–`0x96` | out | note: frames / velocity / note, then instrument commits a `play` |
| `0x97` `0x98` `0x99` | out | held note: instrument, then channel commits a `note_on`; `0x99` is `note_off` |

## Declaring sound

Instruments, effects and songs are declared in luax and ride alongside the ROM as
**metadata**, exactly like `controls { }` and `screen { }`. Ids are assigned in
declaration order and the name binds as a compile-time constant, the way `sprite`
does, so `sfx(boom)` compiles to `sfx(2)`.

```lua
instrument lead {
  wave = "square"          -- sine | triangle | saw | square | noise
  attack = 0  decay = 8  sustain = 100  release = 4   -- ms, ms, 0..255, ms
  pitch_env = 24  pitch_decay = 40      -- semitones, ms  (kick/laser/coin)
  filter = "lpf"  cutoff = 160  resonance = 40         -- 0..255
  lfo = "tri"  lfo_rate = 30  lfo_depth = 20  lfo_target = "cutoff"
  volume = 200  pan = 0                                -- 0..255, -128..127
  chorus = 15  reverb = 40  distortion = 0             -- sends, 0..255
}

sfx boom { inst = lead  speed = 3  notes = "48 - 43 . 36" }
```

In a `notes` string a number starts a note, `-` holds the previous one and `.`
rests; `speed` is frames per row. A note plus holds is **one long note**; a
repeated number **retriggers**. That is the difference between a drone and a
machine gun, and games want both.

Every parameter is a `u8`/`u16` — the byte world the VM already lives in, and the
range a model writes correctly without a units table. `cutoff` maps exponentially
(`0 → 80 Hz`, `128 → ~1.2 kHz`, `255 → 18 kHz`); conversion to Hz, Q and seconds
happens once at load time, never while rendering.

Drums come from noise, or from a sine with a `pitch_env` — there is no drum
machine.

**A name means one thing.** Sprites, instruments, effects and tracks share one
namespace, so `sprite coin` and `sfx coin` in the same game is a diagnostic. It
used to compile, with `sfx(coin)` resolving to the *sprite's* id and triggering
nothing at all — a game that assembled, ran, and was silent.

## Music is a `track`

Channels of rows, one channel per instrument, `tempo` frames per row, played with
`music(name)` and stopped with `music_stop()`.

```lua
track drive {
  tempo = 9
  vel = 150                      -- music sits under the sound effects
  loop = 1
  thud  = "33 . 33 . 40 . 33 ."  -- a key that is not tempo/vel/loop names an
  pulse = "57 60 64 60 57 60 63 60"  -- instrument: that is its channel
}

function init() music(drive) end -- loops until something stops it
```

Rows mean what an `sfx`'s do. A track **runs on the audio clock**, so a slow frame
drops a frame rather than stuttering the tune, while `sfx()` stays on the game's
clock because that timing is the game's. Music notes also yield their voices to
sound effects, so an explosion is never eaten by a bassline.

Start music in `init()` if you want it from the first frame — that trigger is
carried to frame 0 rather than dropped, which it used to be on every host.

## Notes decided at runtime

`sfx` and `track` cover sound a game knows in advance; for a pitch it decides as
it runs there are three more builtins:

```lua
play(piano, 67, 200, 40)      -- instrument, MIDI note, velocity, frames
note_on(0, organ, 60, 200)    -- channel, instrument, note, velocity
note_off(0)                   -- release that channel
```

`play` is fire-and-forget and needs no bookkeeping. `note_on` holds until you
release it, on a channel **you** own — any value `0`–`255`, and not a voice,
because voices get stolen and a game must always be able to stop the note it
started. A channel is just a label the synth matches on, so using an entity's
index as its channel works.

An out-of-range argument makes the whole note a **no-op** — nothing is played and
nothing is disturbed. Neither wrapping nor clamping would do: every channel
`0`–`255` is one some part of the game may be holding a note on, so *any* mapping
of an invalid value onto the valid range steals someone else's note.
`vm_render_audio` reports the count, so the silence has an explanation.

`games/piano.lua` is the worked example: each finger on the keybed holds a note on
its own touch slot as a channel, and lifting that finger releases it.

**Retriggering a channel replaces the note on it** rather than stacking one on
top, which is what makes a channel usable as a continuously *changing* voice and
not only a held one. `games/outrun.lua`'s engine is one channel re-sounded as the
car accelerates — there is no pitch-bend port, so a rising engine can only be
stepped, and a `note_on` on the channel it is already using is the step.

Step on a change in the *value*, not on the bucket it falls in. outrun's first
version compared buckets, and a car scrubbing speed in the dirt sat on a bucket
edge and alternated between two notes every other frame — audible as a machine
gun, and visible in the render report as pages of `note_on`. Anchoring the test
on the value the note was last sounded at is a dead band, and a dead band cannot
chatter.

## Chorus and reverb are shared sends

A patch says how much of itself to send (`reverb = 40`, `chorus = 15`, both
`0`–`255`); there is one chorus and one reverb for the whole mix, and an `fx { }`
block says what they sound like:

```lua
fx {
  reverb_size = 190      -- how long it rings
  reverb_damping = 90    -- how fast the treble dies (high = soft room)
  chorus_rate = 50  chorus_depth = 140
}
```

One of each, not one per instrument — a room is a property of the room, and
sixteen of them would cost sixteen times as much for a difference nobody can
localize.

## Checking it by reading, not listening

An agent has no ears, so the render **report** is the deliverable and the `.wav`
is a by-product. `vm_render_audio` runs the game (same input-script shape as
`vm_run_frames`, advancing the machine the same way) and returns every trigger
with the frame it fired on, peak and RMS, voices started and stolen, and a warning
for each specific way audio goes wrong — an `sfx` id with no declaration, an
instrument the bank lacks, notes dropped because too many were in flight, or
triggers that fired into silence.

The same render is available headless from the shell:

```bash
kessel render-audio games/shooter.lua --frames 200 --buttons A -o shooter.wav
```

```text
rendered 200 frames (3.83s at 48000 Hz)
level: peak 0.359, rms 0.067
voices: 25 started, 0 stolen
25 triggers:
  frame 1     sfx shoot
  frame 9     sfx shoot
  ...
```

Trigger frames are the **console's** frame numbers — the same ones `vm_run_frames`
prints for the same trigger, not the render's own offset. Two numbering schemes
for one event is how "it sounded wrong" becomes an hour.

## What each host does

| Host | Sound |
|------|-------|
| `kessel run` | a cpal output stream; `R` restarts it, since a reloaded game may declare different instruments |
| Android | an `AudioTrack` fed by the native synth |
| `kessel render-audio` / `vm_render_audio` | offline, sample-accurate, reproducible |
| `kessel attach` | **none** — it drives the agent's console, and those events belong to that process |

Realtime places an event at the start of the callback block that finds it: one
device buffer of latency (5–11 ms, under a frame) in exchange for never drifting
against the game's frame counter. The offline render keeps sample-accurate
placement, which is what makes it reproducible — use it when you need to *check* a
sound rather than hear it.

## Sample games

`shooter` (six instruments, effects fired from gameplay, and two `track`s —
the stage tune and the one that replaces it when the boss arrives), `popn`
(a rhythm game — a `track` plus the effect its keys trigger), `platform` (three
instruments and their effects, no music), `piano` (`note_on`/`note_off` per
touch slot, one instrument, nothing declared in advance), and `outrun` (one held
note *retuned* as the car accelerates — the other way to use a channel).
