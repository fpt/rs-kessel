# Kessel

A tiny **fantasy console** for AI agents and humans.

Kessel gives a model a real machine to write games for: a 16-bit stack VM with a
128×128 screen, a gamepad, and a statically-typed Lua-ish language that compiles
to it. The model writes a game, assembles it, runs frames, looks at the screen,
and debugs — then you play the result in a window.

```bash
kessel mcp                      # serve the console to an agent over MCP
kessel run games/tetris.lua    # play a game yourself
```

## Why

Most "let an LLM make a game" setups hand the model a browser and hope. Kessel
hands it a machine small enough to reason about completely — 34 opcodes, a flat
64 KiB address space, a fixed palette — and a feedback loop that actually closes:
every frame returns an observation (screen hash, changed-pixel bbox, stack state,
faults, game-reported entities) and the screen itself comes back as a PNG.

The VM is **deterministic and snapshotable**. Drawing is a software rasterizer
into an indexed framebuffer; sound is an event log, not audio. That is what makes
`vm_snapshot`/`vm_restore` and frame-exact replays possible, and it means the
console runs anywhere — CI, a container, a headless box — with no GPU.

## Install

```bash
cargo install --path crates/cli     # installs `kessel`
```

Or build in place:

```bash
make build          # cargo build --release
make install        # -> ~/bin/kessel (override with PREFIX=/usr/local)
make test
```

For a server or container with no window system, drop the player:

```bash
cargo build --release --no-default-features   # `kessel mcp` only
```

## Use it with an agent

`kessel mcp` is an MCP stdio server. Register it with any MCP-capable agent:

```json
{
  "mcpServers": {
    "kessel": { "command": "kessel", "args": ["mcp", "--root", "/path/to/project"] }
  }
}
```

With Claude Code, that's:

```bash
claude mcp add kessel -- kessel mcp --root .
```

The console is rooted at `--root` (default: the current directory) and **the
filesystem is the source of truth**. `vm_write_source` writes a real `game.lua`
there and `vm_assemble` re-reads it on every call — so the agent can equally well
edit the file with its own editing tools and just call `vm_assemble`. No stale
in-memory copy to get out of sync.

### The tools

| Tool | What it does |
|------|--------------|
| `vm_write_source` | Write a `.lua` (luax) or `.asm` source file |
| `vm_assemble` | Compile + assemble it to a ROM, with line-numbered diagnostics |
| `vm_load_rom` | Load a ROM and run its reset vector |
| `vm_run_frames` | Play an input script for many frames — **the one to reach for** |
| `vm_run_frame` | Advance a single frame with buttons held |
| `vm_run_cycles` | Step N instructions (fine-grained debugging) |
| `vm_get_framebuffer` | The screen as a PNG, so the model can *see* it |
| `vm_inspect_memory` / `vm_inspect_stacks` | Hex dump / stack + pc + fault state |
| `vm_snapshot` / `vm_restore` | Save and rewind machine state |
| `vm_reset` | Reset the machine |

Prefer `vm_run_frames`: it plays a whole scenario in one call and reports the
frames run, any fault that stopped it early, every sound trigger, and how often
the screen changed — an MCP round trip per frame is pure overhead.

## Play

```bash
kessel run games/2048.lua      # arrows slide tiles, A starts a new game
kessel run games/tetris.lua    # L/R move, A rotates, Down soft-drops
kessel run games/platform.lua  # arrows move, A jumps and wall-jumps
kessel run games/outrun.lua    # pseudo-3D road racer
kessel run games/spectrum.lua  # 240x240, the 256-colour palette, sprite banks
```

Arrows or WASD for the d-pad, `Z`/`X` for A/B, Return for START, Shift for
SELECT. `R` reloads the file from disk so you can edit and re-run without
leaving the window; Esc quits.

### Play the game an agent is building

Run `kessel attach` while a `kessel mcp` session is going, and the window joins
it:

```bash
kessel attach                  # joins the running session
kessel attach ./my-game        # ...that one, if several are running
```

This is a genuinely shared session — the window drives the agent's own machine,
not a copy. That is the point, and it cuts both ways:

- The agent's `vm_snapshot` / `vm_restore` / `vm_reset` will rewind or wipe the
  game while you're holding a button.
- `vm_run_frames` advances the machine in bursts you didn't ask for.
- Your button presses land in the agent's observations, so a run with someone
  attached is **not reproducible**.

The window title tells you what's happening (attached / paused / no ROM loaded /
session ended). If you'd rather have your own timeline, use `kessel run game.lua` — that shares
nothing.

Discovery uses a small session file in your cache directory naming a loopback
port; the server binds `127.0.0.1` only and never advances the machine on its
own, so with no player attached an agent's runs are exactly as reproducible as
before.

### On Android

`android/` is a plain-Kotlin player for the games in `games/`, running the same
Rust console compiled to a `.so`. Pick a game, play it — no MCP, no agent, no
editor yet.

```bash
make android-deps      # once: the Rust Android targets and cargo-ndk
make android-install   # build and push to a connected device or emulator
```

Gradle drives `cargo-ndk` itself, so there is no separate Rust step, and the APK
takes its assets straight from `games/` rather than a copy. Needs a JDK 17+ and
the NDK version named in `android/app/build.gradle.kts`; ABIs are `arm64-v8a`,
`armeabi-v7a`, and `x86_64` (the last so the emulator works).

The on-screen pad is built from each ROM's own `controls { … }` block, so it
shows only the buttons that game actually reads, captioned with what they do —
`tetris.lua` gets a d-pad plus A "rotate cw" and B "rotate ccw"; `bounce.lua`
declares `dpad = false` and gets no pad at all.

An editor, cloud upload, and an in-app LLM coding loop are later releases. The
FFI is scoped to the play surface for now (`crates/ffi`), so those are additions
rather than a rewrite — and because the portable layer is a C ABI with a header,
an iOS app is a build-system problem rather than a second port.

### Screens and colour

The console is an 8-bit palette-index framebuffer over a 256-entry palette, in
one of two square sizes:

```lua
screen { mode = Extended240 }   -- 240×240; omit for the 128×128 default
```

Only the size differs between modes — same ports, same 4bpp sprites, same
palette. Colour is always a palette index; only `pal` deals in RGB:

```lua
pal(7, 255, 0, 77)   -- rewrite entry 7; the framebuffer is untouched
sprbank(3)           -- draw sprites through bank 3: nibble n -> 3*16 + n
```

Because `pal` recolours without redrawing, a fade, a damage flash, a day/night
cycle, or palette cycling is one loop over the palette. Because sprites stay
4bpp with a bank offset, one tile can wear sixteen colour schemes and every
existing sprite keeps working unchanged. The default palette fills all 256
entries: the PICO-8 16 at `0–15`, a 6×6×6 colour cube at `16–231`, and a grey
ramp at `232–255`. `games/spectrum.lua` demonstrates all of it.

The `games/` directory doubles as worked luax examples covering the whole
language — sprite declarations, tilemaps and `solid()` collision, entity pools,
edge-triggered input, fixed-point trig. Point a model at them.

## Write a game

```lua
sprite hero {
  ..7777..
  .777777.
  77777777
  77.77.77
  77777777
  .777777.
  ..7777..
  .77..77.
}

local x = 60

function update()
  if btn(LEFT)  then x = x - 1 end
  if btn(RIGHT) then x = x + 1 end
end

function draw()
  cls(0)
  spr(hero, x, 60, 0)
  entity(x, 60, 1)   -- reported back in the observation
end
```

luax is Lua-*flavored*, not Lua: statically typed, no tables, closures, or
recursion. It exists because models have strong Lua priors from PICO-8 and
TIC-80, and reusing those priors beats teaching a new syntax. See
**[docs/VM.md](docs/VM.md)** for the full language, instruction set, and device
map.

## Layout

```
crates/vm/     kessel-vm — the console: ISA, VM, assembler, luax, PNG, vm_* tools.
               Host-free: no I/O beyond the working directory, no audio, no GPU.
crates/cli/    kessel — the binary. `mcp` (stdio server) and `play` (winit window).
crates/ffi/    kessel-ffi — the C ABI and JNI bindings, for hosts that aren't Rust.
android/       The Android app: plain Kotlin + Compose over the same VM.
games/         Sample games, and the luax reference corpus.
docs/VM.md     Machine, instruction set, devices, luax, and the agent loop.
```

## License

MIT OR Apache-2.0. See [LICENSE.txt](LICENSE.txt).
