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

- **Rust**: workspace in `crates/`, three members — `vm` (`kessel-vm`),
  `cli` (`kessel`), and `ffi` (`kessel-ffi`)
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
| `src/play.rs` | winit window, 60 Hz tick, key→gamepad mapping, and `blit` (nearest-neighbour upscale to a `0RGB` CPU surface). `Source` picks local vs. attached. |
| `src/attach/session.rs` | Session files in the cache dir: publish, list, discover. Directory is a parameter, never read from env inside logic — the tests run in parallel. |
| `src/attach/protocol.rs` | The binary HELLO/TICK framing between `mcp` and an attached `play`. Each TICK response carries its own `dim`. |
| `src/attach/server.rs` | Loopback listener inside `kessel mcp`; each TICK locks the shared console. |
| `src/attach/client.rs` | `AttachClient` — background tick thread, latest-frame slot. |

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

Two things the JNI layer owes the JVM, both load-bearing:

- **Frames go through a direct `ByteBuffer`.** Returning a `byte[]` would put
  64 KiB on the Java heap sixty times a second. `framebuffer_rgba_into` exists
  in `kessel-vm` for exactly this.
- **Panics stop at the boundary.** Unwinding into the JVM is UB, so the two
  functions that run game code — `playerLoad`, `playerTick` — sit inside
  `catch_unwind`.

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
- **The `play` feature is default-on but removable.** `--no-default-features`
  drops winit/softbuffer for a headless `kessel mcp`.
- Prefer `vm_run_frames` over looping `vm_run_frame`: an MCP round trip per frame
  is pure overhead. It stops at the first fault/halt and caps at 1800 frames.

## Build & Run

```bash
cd crates && cargo build --release
cd crates && cargo test
cd crates && cargo build --release --no-default-features   # headless

./crates/target/release/kessel mcp --root /path/to/project
./crates/target/release/kessel run games/tetris.lua
```

`make install` builds release and copies `kessel` into `$PREFIX/bin` (default
`~/bin`). The binary is self-contained — no dylib, no separate backend process.

## Project Structure

```
kessel/
├── crates/vm/          kessel-vm: the console (host-free)
├── crates/cli/         kessel: `mcp` + `play`
├── games/              sample games / luax reference corpus
└── docs/VM.md          machine, ISA, devices, luax, agent loop
```

## Testing notes

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
