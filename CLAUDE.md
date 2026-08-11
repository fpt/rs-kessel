# Kessel - Developer Guide

## Overview

A tiny fantasy console, shipped three ways from one VM:

- `kessel mcp` — an **MCP stdio server** serving the `vm_*` tools, so any
  MCP-capable agent can drive the write → assemble → run → observe → debug loop.
- `kessel run <file>` — a **window** (winit + softbuffer) for a human to play
  the result.
- **The Android app** (`android/`) — a plain-Kotlin player for the games bundled
  in `games/`, over the same VM compiled to a `.so`.

Kessel does no LLM inference and hosts no agent. It used to be a macOS/Windows
voice assistant that embedded the VM; that frontend and its ACP client were
removed, leaving only the console.

- **Rust**: workspace in `crates/`, four members — `vm` (`kessel-vm`),
  `audio` (`kessel-audio`), `cli` (`kessel`), and `ffi` (`kessel-ffi`)
- **Platforms**: macOS, Windows, Linux, Android. `kessel mcp` is headless-safe.

## Architecture

```
Agent (Claude Code, codex, …)
  │  MCP over stdio (line-delimited JSON-RPC 2.0)
  ▼
kessel mcp ── crates/cli/src/mcp/ ── VmToolSet ──┐
                                                 ├─► VmConsole (crates/vm)
kessel run  ── crates/cli/src/play.rs ── VmPlayer ┤      │
                  winit window, 60 Hz tick        │      ▼
                  softbuffer CPU blit             │  indexed framebuffer
                                                  │  + sound event log
Android app ── android/ (Kotlin/Compose)          │
  └─ JNI ── crates/ffi ── C ABI ── VmPlayer ──────┘
             60 Hz game thread, direct ByteBuffer
```

### Video: two sizes, one colour model

The console has two screens — `Classic128` (128×128) and `Extended240`
(240×240) — and **only the size differs**. Both are an 8-bit palette-index
framebuffer over one 256-entry palette, with the same ports and the same 4bpp
sprite sheet. A second mode that also changed the colour model would fork the
blitter, the PNG encoder, and every host's upload path for nothing.

A ROM picks its screen with `screen { mode = Extended240 }`, parsed like
`controls` and carried as ROM metadata rather than in the ROM bytes. The mode is
fixed when the ROM loads: `Vm::load_rom` takes it, because the reset vector
draws and must draw at the size the game asked for.

Three things follow, and none of them should be re-litigated:

- **Sprites stay 4bpp.** Nibble `n` under bank `b` (screen port `0x1e`) draws as
  `b*16 + n`, so bank 0 is the identity and every existing sprite keeps its
  colours. Going 8bpp would have doubled the sheet and broken the
  one-char-per-pixel sprite syntax across the whole corpus — for reach that
  banks already provide.
- **Nibble 0 is transparent in every bank.** Otherwise a bank switch would give
  sprites a solid background.
- **The palette commits on the *index* write** (`0x01`), not blue. That is the
  order a stack machine yields for free: `pal(i,r,g,b)` pushes `i` first, so `b`
  pops first and `i` last.

`dim` is runtime state on `Devices`, not a constant. Anything that sizes a
buffer must read it **after** the ROM loads — `kessel_player_screen_dim(p)`,
`KesselVm.screenDim()`. Reading it earlier silently yields 128 and tears a
240×240 game across the buffer.

The load-bearing rule: **`kessel-vm` is host-free.** It does no I/O beyond the
files under its working directory, synthesizes no audio, and touches no GPU.
Drawing is a software rasterizer into an indexed framebuffer; sound is an event
log. Every backend (a window, an audio device, a future GPU upscaler) lives in
`crates/cli` or further out. This is what keeps the machine deterministic and
snapshotable — which is the entire point of the agent loop, since
`vm_snapshot`/`vm_restore` and frame-exact replay depend on it.

Corollary: if you are tempted to put wgpu, cpal, or any device backend into
`crates/vm`, don't. Put it in the player.

### `crates/vm` — `kessel-vm`

| File | Purpose |
|------|---------|
| `src/lib.rs` | `VmConsole` — the machine plus the authoring workspace (sources, ROMs, snapshots) and the `Observation` record. Disk-backed when a root is set. |
| `src/audio.rs` | Offline render: run the game, render its sound, and report what happened in numbers — the agent has no ears. |
| `src/isa.rs` | The 34-opcode instruction set. |
| `src/vm.rs` | The stack machine: memory, stacks, fetch/execute, frame runner. |
| `src/device.rs` | Varvara-lite device layer — screen, gamepad, rng, storage, debug, console, sound (recorded only). |
| `src/assembler.rs` | Two-pass textual assembler → ROM + diagnostics. |
| `src/luax.rs` | The statically-typed Lua-ish front-end that compiles to assembly. |
| `src/png.rs` | Dependency-free PNG + base64 for framebuffer output. |
| `src/player.rs` | `VmPlayer` — a standalone handle for human play (load, tick, framebuffer_rgba). |
| `src/tool.rs` | The crate's own tool surface: `VmTool`, `ToolResult`, `ImageContent`, `VmToolError`. Deliberately not borrowed from any host framework. |
| `src/tools.rs` | The `vm_*` tools and `VmToolSet` (name-dispatch over the set). |

### `crates/cli` — `kessel`

| File | Purpose |
|------|---------|
| `src/main.rs` | Subcommand dispatch (`mcp`, `play`, help/version) and `--root` parsing. |
| `src/mcp/mod.rs` | The stdio read → dispatch → write loop. |
| `src/mcp/server.rs` | Method dispatch: `initialize`, `tools/list`, `tools/call`, `ping`. Pure function of request + VM state, so it tests without a process. |
| `src/mcp/wire.rs` | MCP / JSON-RPC wire types, including the `image` content block. |
| `src/render_audio.rs` | `kessel render-audio` — headless WAV render plus the report. No window, no audio device, so it works under `--no-default-features` and over ssh. |
| `src/audio.rs` | The cpal output stream for `kessel run`, and the lock-free queue the game thread feeds it. `render_block` is the callback body, factored out so it is testable without a sound card. |
| `src/play.rs` | winit window, 60 Hz tick, key→gamepad mapping, and `blit` (nearest-neighbour upscale to a `0RGB` CPU surface). `Source` picks local vs. attached. |
| `src/attach/session.rs` | Session files in the cache dir: publish, list, discover. Directory is a parameter, never read from env inside logic — the tests run in parallel. |
| `src/attach/protocol.rs` | The binary HELLO/TICK framing between `mcp` and an attached `play`. Each TICK response carries its own `dim`. |
| `src/attach/server.rs` | Loopback listener inside `kessel mcp`; each TICK locks the shared console. |
| `src/attach/client.rs` | `AttachClient` — background tick thread, latest-frame slot. |

### `crates/audio` — `kessel-audio`

The synth. Host-free in the same sense as `kessel-vm` — it opens no audio
device, spawns no thread, does no I/O, and **depends on nothing**, the VM
included. cpal, `AudioTrack`, and `AVAudioSourceNode` live in the hosts.

The zero-dependency rule is what makes the crate reusable as a standalone
instrument: a synth app links `Synth` without carrying a stack machine, an
assembler, and a sprite blitter. When the event type has to be shared,
`kessel-vm` depends on *this* crate, never the reverse.

`docs/AUDIO.md` is the full architecture; issue #64 tracks what is built.
Today: voices, filter, pan, drive, master limiter, the bank, `sfx`, `music`,
the note-level API (`play` / `note_on` / `note_off`),
the offline render (`vm_render_audio`, `kessel render-audio`), **sound on
`kessel run`** through cpal, and the shared chorus and reverb. No audio on
Android or when attached.

| File | Purpose |
|------|---------|
| `src/lib.rs` | `Synth` — the voice pool, event handling, and voice stealing. `SynthStats` counts what a render did, since "nothing played" is hard to hear. |
| `src/event.rs` | `AudioEvent` — the wire between a frame clock and a sample clock. Plain `Copy` data, because it crosses a lock-free queue, the observation record, and the C ABI. |
| `src/patch.rs` | `Patch` (authored: `u8`/`u16`, the VM's byte world) and `VoiceParams` (compiled: float rates at a known sample rate). The conversion happens at load time, never in `render`. |
| `src/voice.rs` | One voice: oscillator, ADSR, pitch envelope, seeded xorshift noise, filter, drive, pan. |
| `src/bank.rs` | `SoundBank`, the `instrument`/`sfx` grammar, and the field setters both front-ends call. |
| `src/engine.rs` | `AudioEngine` — bank + a timestamped queue over `Synth`. Splits each render block at pending event times. |
| `src/filter.rs` | 2-pole biquad LPF/HPF and the byte→Hz mapping. Games write `cutoff = 160`, never a frequency. |
| `src/master.rs` | `soft_clip` (a per-voice *distortion*, 6.8% down at 0.5 — not a safety net) and the master `Limiter`. |
| `src/fx.rs` | The two shared send effects: a Freeverb-shaped `Reverb` (4 combs → 2 all-passes) and a `Chorus` (one modulated delay, the two sides a quarter cycle apart). |
| `src/sequencer.rs` | The music player. Decides which notes fall due and when the next row is; `AudioEngine` splits its render there and starts them. |
| `src/wav.rs` | Dependency-free 16-bit PCM WAV, for offline render and previews. |
| `examples/preview.rs` | Renders the waveforms and a kick/snare/laser/coin to WAV. The tests check frequency and lifetime; only an ear checks whether a kick sounds like one. |

`Synth::render` runs on an audio callback thread: **no allocation, no locks, no
syscalls, no panics**. `set_instruments` is the only call in the crate that
allocates, and it is load-time. `tests/realtime.rs` enforces this with a
counting global allocator; it lives in an integration test so the allocator
applies to its own binary, and its counter is thread-*local* because the harness
runs tests concurrently and a shared counter charges one test's allocations to
another.

Three details that look like oversights and are not: a voice does **not** reset
its noise RNG on `start` (restarting it makes repeated hits identical, which is
the machine-gun artifact); a patch that sustains at zero goes idle when its
decay finishes rather than waiting for a release that a fire-and-forget note
never sends; and the master applies **no** soft clip, because a saturator on the
master bus colours every quiet mix to catch peaks the limiter already caught.

**The sequencer runs on the audio clock; sound effects run on the game's.** A
row falls due after so many *samples*, so a dropped frame drops a frame instead
of stuttering the music — while `sfx()` is the game's own timing and stays
frame-timestamped. That split is the reason `AudioEngine::render` splits its
block at *both* the next queued event and the next row.

Music notes are allocated at `Priority::Music`, below sound effects, so an
explosion can take a voice from the bassline rather than the other way round.
`music_stop()` releases every music-priority voice and leaves effects alone.

**The device's sound log is `Vec<AudioEvent>`**, not a narrower type of its own.
The note ports need every field of a `Play`, and a second vocabulary for the
same thing meant three places converting between them — the observation, the
tools, and the player. One description (`audio::event_json`) now serves the
observation record and `vm_run_frames`, so an agent cannot see two spellings of
one frame.

**A name means one thing.** Sprites, instruments, effects and tracks bind their
names into one namespace and `gen_expr` resolves sprites first — so a game with
both `sprite coin` and `sfx coin` compiled `sfx(coin)` to the *sprite's* id and
triggered nothing. It assembled, ran, and was silent. `games/platform.lua`
shipped that way until `games_audio.rs` counted it. Declaring a name twice
across kinds is now a diagnostic, phrased by `bank::name_conflict` so both
front-ends say the same thing.

**An out-of-range note argument emits nothing**, and is counted in
`Devices::sound_dropped`. Truncating puts `note_on(256, …)` on channel 0 and
clamping puts it on 255 — and since every channel `0..=255` is one a game may
be holding a note on, *both* steal someone else's note. There is no spare value
to land on, so the only answer that cannot corrupt state is to do nothing. Same
rule as an off-screen `pset`, which this device has always ignored. The count
reaches the render report, so the silence is explainable rather than mysterious.

**`init()`'s sound has to be carried to frame 0.** The reset vector runs outside
any frame and the device log is cleared at the start of the next one, so
`music()` in `init()` — the obvious way to write it — was silently dropped by
every host until `VmConsole::take_reset_sound` existed.

**Chorus and reverb are shared sends, never per voice.** A patch says how much
of itself to send (`reverb = 40`); one unit of each processes the summed bus and
returns it to the mix. Sixteen reverbs would cost sixteen times as much for an
effect nobody can localize — a room is a property of the room. The `fx { }`
block sets what those two units sound like, and there is exactly one of it.

`Reverb` scales its input by `INPUT_GAIN` (0.06). That is load-bearing, not
taste: four combs run in *parallel* and sum, so an un-scaled network has a gain
of about `4 / (1 - feedback)` — two hundred at the largest room — and a send
returning a hundred times what went in would duck the whole game through the
master limiter. The test that caught this measures the return against the input
rather than checking for `NaN`.

The voice chain is oscillator → filter → drive → envelope → pan. The envelope
sits after the filter so resonant ringing fades with the note, and the drive
sits before it so how dirty a patch sounds doesn't depend on how hard it was
played. A voice writes into the dry mix and both send buses in **one pass** —
rendering to a scratch and scaling it into each bus afterwards is the obvious
shape and costs two extra passes for a multiply the loop already has the value
for.

`Synth::render` walks the caller's block in fixed `CHUNK_FRAMES` pieces so the
send buses can be sized once. The effects are stateful, so splitting changes
nothing — which is also what keeps the output independent of the device's buffer
size.

**The grammar's *meaning* lives in `bank.rs`; its tokenization does not.** A
patch file and a game source say the same thing because both call
`set_instrument_field`/`set_sfx_field`. `luax` lexes the blocks with its own
lexer because the luax lexer carries no byte spans, and adding them so `parse`
could be handed a block's text would touch every rule in the compiler. Two
tokenizers over one definition of meaning is the cheaper half of that trade —
but the setters are the part that must never be duplicated.

**The offline render's report is the deliverable, not the WAV.** The agent
cannot listen, so `AudioSummary::report` names each failure a person would
otherwise diagnose by ear: no triggers at all, an id with no declaration, an
instrument the bank lacks, notes dropped from a full queue, triggers that fired
into silence, and a limiter that had to pull the mix down. A WAV alone would be
an artefact nobody in the loop can read.

`AudioTrace::frame` is the **console's** frame counter, the same number
`vm_run_frames` prints for the same trigger — not the render's own offset. Two
numbering schemes for one event is how "it sounded wrong" becomes an hour.

`VmConsole::audio_epoch()` changes on reset, restore, and ROM load. A host with
a live synth turns a change into `AudioEvent::Panic`; the VM signals rather than
emits because it does not know a synth exists and is not going to start.
`kessel run` checks it each tick, which is what keeps a reload from leaving the
previous game's notes ringing over the new one.

**Realtime places events at the callback block that finds them**, not at a sample
derived from the game's frame counter — the two clocks drift, and a game running
slightly slow would accumulate a growing offset. So `kessel run` trades
sample-accurate placement for a fixed one-buffer latency, while
`kessel render-audio` keeps it, which is what makes the offline render
reproducible. The realtime path also **reloads by restarting the stream**:
swapping a bank under a live callback would need a lock in the one place that
must not have one, to handle a keypress.

`AudioEngine::submit` expands a `PlaySfx` into one queued `Play` per note at
trigger time rather than walking a cursor each frame; `Panic` then cancels the
rest of an effect for free. It cancels **from its own timestamp forward** — a
panic scheduled for frame 10 must not delete the sounds of frames 0–9 that are
still queued.

### `crates/ffi` — `kessel-ffi`

The console's play surface for hosts that are not Rust. `VmPlayer` only: load,
tick, read pixels, read the ROM's control metadata. The authoring half
(`vm_assemble`, `vm_snapshot`, …) is deliberately absent until an in-app editor
needs it — widening this later is additive, but shipping bindings nothing calls
means carrying and testing them for free.

| File | Purpose |
|------|---------|
| `src/lib.rs` | The C ABI (`kessel_player_*`). Every entry point tolerates a null handle; strings are owned and freed by `kessel_string_free`. |
| `src/android.rs` | JNI entry points, `cfg(target_os = "android")` only, bound by name to `dev.kessel.vm.KesselNative`. |
| `include/kessel.h` | The C header — what an iOS target compiles against. |

The C ABI is the portable layer and the JNI module is a wrapper over it, not a
parallel implementation. That is what makes "iOS later" a build-system problem
rather than a second port: Swift calls the header directly.

Sound is **opt-in**: `kessel_player_audio_enable` gives a player a synth, and a
host that never calls it has no engine, no delay lines, and nothing in `tick`
beyond a push onto an empty queue.

`kessel_player_audio_render` is the only entry point that does **not** take the
console's lock. That is the whole design: the audio thread must never wait on a
frame of game code, because a late buffer is a click in everything rather than a
dropped frame in one thing. The two sides meet through a lock-free
`kessel_audio::EventQueue` — the same queue the desktop player uses, which is
why it lives in the synth crate and not in either host.

It also never *waits* for the lock it does use (the engine's own): a contended
synth renders silence rather than a late buffer.

Two things the JNI layer owes the JVM, both load-bearing:

- **Frames go through a direct `ByteBuffer`.** Returning a `byte[]` would put
  64 KiB on the Java heap sixty times a second. `framebuffer_rgba_into` exists
  in `kessel-vm` for exactly this.
- **Panics stop at the boundary.** Unwinding into the JVM is UB, so the
  functions that run game code or DSP — `playerLoad`, `playerTick`,
  `playerAudioRender` — sit inside `catch_unwind`. The audio one matters most:
  it runs on a thread the platform will kill the process over.
- **`playerTick` goes through the C ABI, not `player.tick`.** The C entry point
  is what collects the frame's sound into the queue; calling the inner player
  directly is how Android ends up silent while every other host plays.

### Attaching (`kessel attach`)

`kessel attach` joins a running `kessel mcp` and drives **the agent's own
`VmConsole`** — one machine, one timeline, two drivers. This is deliberate and
was chosen with the consequences understood:

- The agent's `vm_snapshot`/`vm_restore`/`vm_reset` rewind the game under the
  player; `vm_run_frames` advances it in bursts.
- The player's inputs land in the agent's observations, so a run with someone
  attached is not reproducible.

Do not "fix" these — they are what sharing one machine means. `kessel run <file>`
is the independent-timeline path.

Two invariants hold it together:

- **The server never ticks on its own.** With no player attached, the machine
  advances only through tool calls, so reproducibility is untouched by the mere
  existence of this feature.
- **The client ticks off the UI thread.** The agent can hold the console mutex
  for a long time (`vm_run_frames(1800)` is one call); a tick issued from the
  event loop would freeze the whole window, not just the game. The worker blocks,
  the UI redraws the last frame it got.

Transport is loopback TCP (not a Unix socket) so the same path works on Windows,
carrying a small binary protocol (not JSON) because it's a 60 Hz framebuffer
stream. It binds `127.0.0.1` only — `bind_addr()` is a separate function with a
test, because that's a security property rather than an incidental literal.

### The Android app (`android/`)

Run-only: pick a game from the bundled library, play it. The editor, cloud
upload, and LLM coding loop are later releases — the FFI is scoped so they are
additions rather than a rewrite.

| File | Purpose |
|------|---------|
| `vm/KesselNative.kt` | The raw `external fun` declarations. Names bind to symbols in `crates/ffi/src/android.rs` — renaming this class or its package breaks them at *runtime*, not build time. |
| `vm/KesselVm.kt` | The safe handle: owns the pointer's lifetime, one lock so `close` cannot race a `tick`. |
| `vm/Controls.kt` | Parses the ROM's control metadata, so the pad shows only the buttons that do something. |
| `game/GameCatalog.kt` | The library, read from `assets/`. |
| `game/GameEngine.kt` | The 60 Hz thread. Draws to a `Surface`; publishes only pause/halt to Compose. |
| `game/AudioPlayer.kt` | The audio thread: an `AudioTrack` in `ENCODING_PCM_FLOAT`, fed from a direct `ByteBuffer` the native synth renders into. `write` blocks, which is what clocks the loop — there is no timer. |
| `game/Blit.kt` | `destRect` — integer upscale + letterbox, matching `blit` in `play.rs`. Pure, so it is testable off-device. |
| `ui/GameSurface.kt` | The `SurfaceView` the engine draws into. |
| `ui/Gamepad.kt` | Geometry-driven touch pad — one `pointerInput` hit-tests every pointer, which is what makes multi-touch and d-pad diagonals work. |

**Frames never enter Compose.** The screen is a `SurfaceView`, not an `Image`,
and this is the one place the app deliberately isn't Compose. A producer thread
and a compositor need an ownership handoff; `lockCanvas` /
`unlockCanvasAndPost` is one, and "publish alternating bitmaps and assume the
reader is done with the older one" is not — that was the first version, and
nothing enforced the assumption. It also keeps 60 frames a second from meaning
60 recompositions a second.

**`KesselVm.renderAudio` is the one method that is not `@Synchronized`**, and it
has to be: that lock is held for a whole frame of game code, and an audio
callback waiting on it is an audible gap in everything. The price is that the
caller owns the ordering — `GameEngine.stop()` joins the audio thread *before*
the console is closed, and a **confirmed dead** thread is the only thing making
that safe. `AudioPlayer.stop()` therefore returns whether it got one, and
`close()` skips `vm.close()` when it did not: leaking one console beats freeing
one a native render call is still reading.

For the same reason the **render thread owns the `AudioTrack`** and releases it
in a `finally`, rather than `stop()` releasing it after a timed join. The only
thread that calls `write` is the one that frees it, so an early join cannot pull
the track out from under a call in flight — a native crash rather than an
exception.

`destRect` is a pure function over plain ints rather than `android.graphics.Rect`
because that class is a throwing stub on the unit-test classpath, and geometry
that draws a wrong-but-plausible picture is exactly what needs a test.

Three decisions worth not re-litigating:

- **`games/` is the assets directory**, via `assets.srcDir` in
  `app/build.gradle.kts` — not a copy. A copy would fork the corpus that
  `crates/vm/tests/games_compile.rs` guards, and the fork would rot.
- **The Rust is always built `--release`**, even into a debug APK. A debug
  `kessel-vm` is roughly an order of magnitude slower, and a machine that must
  finish a frame in 16 ms cannot afford it.
- **The game loop is not on the main thread**, for the reason `kessel attach`
  isn't either: a frame runs arbitrary game code, and a slow one should drop a
  frame rather than jank the app.

R8 cannot see JNI's by-name binding, so `proguard-rules.pro` keeps
`dev.kessel.vm.KesselNative`. Without it a release build fails at first call
with `UnsatisfiedLinkError` and debug builds stay fine — the worst shape of bug.

### Key patterns

- **Tools are dynamically dispatched.** `Vec<Box<dyn VmTool>>` with hand-written
  JSON schemas, wrapped by `VmToolSet`. This is why the MCP layer is hand-rolled
  rather than using `rmcp`: that SDK's value is its `#[tool]` macros over typed
  Rust fns, which buys nothing here, and it would drag in tokio.
- **Tool failure ≠ protocol error.** A program that won't compile, faults, or
  halts is a *successful* `tools/call` whose text reports the failure — the model
  is meant to read it and debug. `VmToolError` (unknown tool, bad argument) comes
  back as `isError: true`. JSON-RPC errors are reserved for protocol faults.
- **The filesystem is the source of truth** when a root is set. `vm_write_source`
  writes through to disk and `vm_assemble` re-reads on every call, so the agent's
  own file-editing tools and the VM never diverge. In-memory sources exist only
  for `VmPlayer` and tests.
- **stdout is the MCP channel.** Every diagnostic in `kessel mcp` goes to stderr.
- **The `play` and `audio` features are default-on but removable.**
  `--no-default-features` drops winit/softbuffer/cpal for a headless
  `kessel mcp`. `audio` **implies `play`** — the window is the only thing that
  plays sound — but stays a separate feature because the reverse is a real
  machine: a screen with no sound card wants `play` alone, and
  `kessel render-audio` needs neither.
- Prefer `vm_run_frames` over looping `vm_run_frame`: an MCP round trip per frame
  is pure overhead. It stops at the first fault/halt and caps at 1800 frames.

## Build & Run

```bash
cd crates && cargo build --release
cd crates && cargo test
cd crates && cargo build --release --no-default-features   # headless

./crates/target/release/kessel mcp --root /path/to/project
./crates/target/release/kessel run games/tetris.lua

cd crates && cargo run -p kessel-audio --example preview   # → target/audio-preview/*.wav
```

`make install` builds release and copies `kessel` into `$PREFIX/bin` (default
`~/bin`). The binary is self-contained — no dylib, no separate backend process.

## Project Structure

```
kessel/
├── crates/vm/          kessel-vm: the console (host-free)
├── crates/audio/       kessel-audio: the synth (host-free, VM-free)
├── crates/cli/         kessel: `mcp` + `play`
├── games/              sample games / luax reference corpus
├── docs/VM.md          machine, ISA, devices, luax, agent loop
└── docs/AUDIO.md       synth architecture, event surface, build order
```

## Testing notes

- `crates/vm/tests/games_audio.rs` guards what every game in `games/` *sounds*
  like: no trigger naming a declaration that does not exist, no note on a
  missing instrument, nothing non-finite, nothing past full scale, and no mix
  that keeps the master limiter engaged. The games that declare instruments must
  also be audible, and the ones that don't must be silent — otherwise the whole
  suite passes happily over silence.
- `crates/vm/tests/games_compile.rs` guards every file in `games/`: each must
  compile with no diagnostics and survive 300 frames under both idle and rotating
  button input without faulting. Sources are `include_str!`'d, so renaming a game
  breaks the build rather than silently skipping it.
- `crates/cli/src/mcp/server.rs` has a full write → assemble → load → run test
  over the MCP surface — the thing that actually has to work for a real host.
- `blit` in `play.rs` is tested separately (channel order, integer upscale,
  letterboxing, undersized window) because a wrong stride there produces a
  plausible-but-wrong picture rather than a crash.

## Troubleshooting

**`kessel run` reports diagnostics and exits**: the game didn't compile. That's
deliberate — a blank window would be worse. Fix the source and re-run, or keep
the window open and press `R` to reload.

**An agent can't see the VM tools**: check the agent spawned `kessel mcp` (not
bare `kessel`) and that stdout isn't being polluted — the protocol lives there.
