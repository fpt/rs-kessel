# Kessel Fantasy-Console VM

A tiny 16-bit stack VM (Uxn-inspired) that lets a model **write a small game,
assemble it, run it, observe the result, and debug it** — and the game is
playable by a human. Lives in `crates/vm/` (the `kessel-vm` crate). Pure Rust,
deterministic, snapshotable.

The `vm_*` tools reach an agent over MCP: `kessel mcp` serves them on stdio, so
any MCP-capable agent can drive the loop. `kessel run <file>` opens the same
console in a window for a human.

This document owns the machine — the instruction set, memory, the device map, the
luax language, and the agent loop. Each device surface a game actually draws on,
reads or plays through has its own reference:

| | |
|---|---|
| [**VM_GRAPHICS.md**](VM_GRAPHICS.md) | screens, palette, sprites, tilemap, drawing builtins |
| [**VM_CONTROLS.md**](VM_CONTROLS.md) | buttons, the analog stick, touch, gestures, `controls { }` |
| [**VM_AUDIO.md**](VM_AUDIO.md) | instruments, `sfx`, music, and reading the render report |
| [**SYNTH.md**](SYNTH.md) | how the synth itself is built (`kessel-audio`) |

## Machine

- 16-bit stack machine. Data stack + return stack, 256 `u16` cells each.
- **Video**: a square, 8-bit palette-index framebuffer plus one 256-entry RGB
  palette, in one of two sizes — 128×128 or 240×240, fixed when the ROM loads.
  Only the size differs between them; the framebuffer lives outside the 64 KiB
  address space, so the wider screen costs a game no RAM. See
  [**VM_GRAPHICS.md**](VM_GRAPHICS.md).
- Flat 64 KiB memory (`u16` addresses — no out-of-range accesses).
- ROMs load at `0x0100`; the **reset vector** runs once (init).
- Each frame calls the installed **frame vector**; it runs until `RET` (to the
  top), `HALT`, or the per-frame cap (200,000 instructions).
- Runtime errors (div-by-zero, stack under/overflow, illegal opcode) are
  *trapped*: they set a `fault` string and halt the machine — never a crash.

## Instruction set (34 opcodes)

Immediates: `LIT8` reads 1 following byte, `LIT16` reads 2 (big-endian). In
assembly you rarely write these directly — use `#ff` / `#1234` / decimal.

```
NOP HALT LIT8 LIT16
DUP DROP SWAP OVER ROT
ADD SUB MUL DIV MOD           ( wrapping u16; DIV/MOD by 0 -> fault )
AND OR XOR SHL SHR
EQ NE LT GT                   ( push 1/0; unsigned )
LOAD8 LOAD16 STORE8 STORE16
JMP JZ JNZ CALL RET
DEI DEO                       ( device in / out )
HALT
```

Stack effects worth memorizing (top of stack is the **rightmost**):

```
SUB      ( a b -- a-b )
LT       ( a b -- a<b )
STORE16  ( val addr -- )        LOAD16  ( addr -- val )
JZ       ( cond addr -- )       jump to addr if cond == 0
JNZ      ( cond addr -- )       jump to addr if cond != 0
CALL     ( addr -- )            RET ( -- )
DEI      ( port -- val )        DEO ( val port -- )
```

## Assembly syntax

```
ADD SUB DEO       bare mnemonic
#ff  #1234        hex literal push (LIT8 / LIT16)
42   0x20         decimal / hex literal (LIT8 if <256, else LIT16)
@name             define a label at the current address
name              reference a label -> pushes its 16-bit address
.byte 1 2 3       raw bytes
.word 0x1234      one raw 16-bit word
.res 2            reserve N zero bytes (RAM variables)
( ... )           block comment      ; ... line comment
```

Referencing a label pushes its **address**. For a variable, define it with
`@player-x .res 2` and use `player-x LOAD16` / `player-x STORE16`.

## Devices (via `DEI`/`DEO`)

Port byte = `(device << 4) | register`. This is the map; the three device
documents own the detail.

| Device | Ports | Meaning |
|--------|-------|---------|
| system | `0x00` | halt (non-zero halts the machine) |
| system | `0x01`–`0x04` | palette: stage r, g, b, then commit on the index — [graphics](VM_GRAPHICS.md) |
| screen | `0x10`–`0x1e` | frame vector, x/y, colour, pixel, sprite, cls, camera, flags, blit-id, tileset base, glyph, hspan, sprite bank — [graphics](VM_GRAPHICS.md) |
| gamepad | `0x20`–`0x24` | buttons, edges, and the analog stick — [controls](VM_CONTROLS.md) |
| rng | `0x30` | read next `u16` / write to set the seed |
| storage | `0x40` `0x41` `0x42` | addr / read / write (256 bytes) |
| debug | `0x50` `0x51` `0x52` | entity x, y, commit(tag) — reported in the observation |
| console | `0x60` | write a byte to the text buffer |
| tilemap | `0x70`–`0x78` | base, width, and the `map` draw parameters — [graphics](VM_GRAPHICS.md) |
| time | `0x80` | frame counter (frames since power-on; wraps at 65536) |
| sound | `0x90`–`0x99` | `sfx`/`music`, and the note-level ports — [audio](VM_AUDIO.md) |
| sprn | `0xa0`–`0xa3` | base id, w, h, then draw a `w×h` block — [graphics](VM_GRAPHICS.md) |
| scale | `0xb0` `0xb1` | scaled sprite: scale, then blit-id — [graphics](VM_GRAPHICS.md) |
| trig | `0xc0` `0xc1` | write an angle (0..255 = a turn) → read sin / cos, signed 8.8 fixed |
| touch | `0xd0`–`0xd7` | touch points and gestures — [controls](VM_CONTROLS.md) |

The **frame vector** (`0x10`) is how a game runs at all: install an address once
and the console calls it every frame. Everything else is optional.

Two rules hold across every device, and both are "do nothing" rather than
"do something wrong": an off-screen `pset` draws nothing, and an out-of-range
note argument plays nothing. There is no spare value to land on that would not
belong to somebody.

## The tools (agent-facing loop)

`vm_write_source(path, source)` → `vm_assemble(path)` → `vm_load_rom(path)` →
`vm_run_frame(buttons)` / `vm_run_frames(script)` / `vm_run_cycles(n)` →
`vm_inspect_memory`, `vm_inspect_stacks`, `vm_get_framebuffer` (PNG),
`vm_render_audio` (WAV + report) → `vm_snapshot`/`vm_restore`, `vm_reset`.

**Sources are actual files on disk.** `vm_write_source` writes `game.lua` into the
working directory and `vm_assemble` re-reads it fresh on every call. This means
the agent's *own* file-editing tools and `vm_assemble` operate on the same file:
for a small tweak, edit `game.lua` directly and just call `vm_assemble`; for a
first draft or rewrite, use `vm_write_source`. `VmPlayer` (`kessel run`) and the
test suites set no root and keep sources in memory, unchanged.

**The working directory is chosen per session, by naming it.** `kessel mcp` takes
no arguments: an agent points the console at a project by passing an **absolute**
path to `vm_write_source` (or `vm_assemble`, for a game already on disk), and its
parent directory becomes the working directory for the rest of the session, with
bare names resolving there from then on. One registered server, a different project
each conversation, no per-project config.

Until an agent names one, the working directory is the cwd when that looks like a
project someone chose, and `~/Documents/Kessel` when it doesn't — a desktop host
launched from Finder starts in `/` or its own bundle, which is no place to save a
game. `$KESSEL_ROOT` pins the starting directory for a host that can set env vars
but not a cwd.

Four things make that safe rather than just convenient, and none should be
loosened without a reason:

- **Adoption is opt-in per console** (`set_adoptable_roots`), and the list of
  allowed parent directories — the configured root and the user's home, never
  `/` — is the whole boundary. The *model* picks the directory now, while a host
  approves `vm_write_source` once by name and not once per path, so an unbounded
  version would turn one approval into a licence to write anywhere.
- **A relative path still means what it always did**, confined to the root with
  no `..` or absolute escapes. That is what leaves `VmPlayer`, the FFI hosts and
  every existing tool call untouched — for them adoption is simply off, and an
  absolute path stays the error it has always been.
- **The confinement is checked after resolving links, not just lexically.**
  Component checks cannot see a symlink and `fs::write` follows one, so
  `resolve_in_root` also requires the resolved path to stay under the resolved
  workspace — otherwise a link planted in the workspace turns a confined write
  (and read) into an arbitrary one.
- **`..` is refused rather than resolved.** The prefix check is what confines an
  adopted root, and a component that climbs back out after it would make the
  check decorative.
- **A read of a path that doesn't exist does not move anything.** Adopting means
  dropping the built ROMs (they describe a different game), so a typo'd
  `vm_assemble` path would otherwise throw away the whole session's work — an
  error either way, with the damage only visible on the *next* call.

An adopted directory is also where that game's `#include`s resolve — they read
through the same working directory, so a project carries its own `lib/` with it.
And `kessel attach` finds a server by working directory, so moving the workspace
re-publishes the session file under the new one.

**Sound is checked by reading, not listening.** An agent has no ears, so
`vm_render_audio` returns a *report* — every trigger with its frame, levels, voices
started and stolen, and a named warning for each way audio goes wrong — and writes
the `.wav` as a by-product. Declaring instruments, effects and music, and reading
that report, are in [**VM_AUDIO.md**](VM_AUDIO.md).

**Prefer `vm_run_frames` over a loop of `vm_run_frame`.** One call plays a whole
scenario from an input script and returns the final observation plus a summary
(frames run, whether it stopped early on a fault/halt, every sound trigger, and
how many frames the screen changed on) — an MCP round trip per frame is pure
overhead:

```json
{"script": [{"buttons": ["RIGHT"], "frames": 30}, {"buttons": ["A"], "frames": 2}],
 "image": true}
```

It stops at the first fault or halt so the returned observation is the one
showing the failure, and caps at 1800 frames (30s of play).

`vm_run_frame` returns the observation record (screen hash + changed bbox for
"look at the screen", `vm.*` internals for white-box debugging, and
game-reported `entities` for black-box tasks):

```json
{ "frame": 2, "cycles": 130, "buttons": ["LEFT"],
  "framebuffer_hash": "…", "changed_pixels_bbox": [31,60,31,60],
  "console": "", "fault": null, "halted": false,
  "vm": { "pc": 65535, "data_stack": [], "return_stack_depth": 0 },
  "entities": [ {"tag": 1, "x": 31, "y": 60} ],
  "sound": [ {"kind": "sfx", "id": 3} ] }
```

## Example: move a pixel with LEFT / RIGHT

```
( reset: install the frame vector, put the player at x=32 )
on-frame #10 DEO
#20 player-x STORE16
RET

@on-frame
    ( LEFT held? decrement x )
    #20 DEI #01 AND  skip-left JZ
    player-x LOAD16 #01 SUB player-x STORE16
    @skip-left

    ( RIGHT held? increment x )
    #20 DEI #02 AND  skip-right JZ
    player-x LOAD16 #01 ADD player-x STORE16
    @skip-right

    ( draw the player pixel at (player-x, 60) in white )
    player-x LOAD16 #11 DEO
    60 #12 DEO
    #07 #13 DEO
    #00 #14 DEO

    ( report the player entity for observation )
    player-x LOAD16 #50 DEO
    60 #51 DEO
    #01 #52 DEO
    RET

@player-x .res 2
```

Note: the example wraps `@skip-left`/`@skip-right` as labels **after** the branch
so `JZ` skips the movement block — labels mark addresses, no jump is needed to
"fall through" into them.

## luax dialect (`.lua`)

A small, statically-typed **Lua-flavored** language that **compiles to the
assembler above** — the high-level way to write games. Models have strong Lua
priors (PICO-8/TIC-80/Löve), so a Lua surface lets them reuse that knowledge.
Give the source a `.lua` path and `vm_assemble` compiles it, then assembles.
Everything downstream (load, run, observe, play) is identical.

**Not** real Lua — a static subset: no `require`, metatables, coroutines,
closures, varargs, GC, or stdlib. Tables are compile-time **records**; arrays are
fixed-length. (For several files, `#include` — see below — not `require`, which
returns a module value this machine has nowhere to put.) Entry points (VM is vector-driven, no `main(){ loop … }`): `init`
runs once at reset; `update` then `draw` run each frame (or a single `frame`).
Locals/params use static slots — **no recursion**.

```lua
record Ball { x, y, vx, vy, color: byte }   -- fields default to `word`

local ball: Ball          -- top-level local = a global (persistent state)
local GRAVITY = 1         -- constant-initialized local also folds as a constant

function init() ball.x = 20  ball.y = 30  ball.vx = 1  ball.vy = 1  ball.color = 8 end

function move(b: Ball)    -- records pass by ADDRESS (mutable)
  b.x = b.x + b.vx
  if b.x >= 118 or b.x <= 2 then b.vx = 0 - b.vx end
end

function update() move(ball) end

function draw()
  cls(0)
  pset(ball.x, ball.y, ball.color)
  entity(ball.x, ball.y, 1)       -- report for observation
end
```

- **Types:** `word` (default, unsigned) / `byte` / `int` (16-bit signed) / `bool`;
  `record Name { field[: type], … }`; fixed arrays `array(N, T)` where `T` is a
  scalar **or a record** (`local es: array(16, Enemy)`), indexed `a[i]` /
  `a[i].field`.
- **Signed vs unsigned:** `word` comparisons are unsigned (fine for pixel coords /
  addresses); declare a value `int` when you need signed comparisons — e.g. a
  velocity `local vy: int` so `if vy < 0 then …` works. `int` arithmetic is
  identical to `word` (two's-complement wrapping); only `< <= > >=` differ.
  Comparing two operands is signed iff either is `int` (a unary `-x` counts as
  `int`).
- **Declarations:** `record`; top-level `local name[: T] [= const]` (a global);
  `function name(a[: T], …) … end`; `sprite NAME { <pixel rows> }` (see below).
  Records pass by address (functions mutate them); scalars pass by value.
- **Sprites, tilemaps, colour:** declared with `sprite NAME { <pixel rows> }` and
  `tilemap NAME(w, h)`; a sprite's size comes from its body, and its name is a
  constant equal to its first tile id. See [**VM_GRAPHICS.md**](VM_GRAPHICS.md).
- **Statements:** `local`, assignment, `if/elseif/else … end`, `while … do … end`,
  `for i = a, b[, step] do … end` (ascending, positive literal step), `break`,
  `return`, calls.
- **Operators (Lua):** `+ - * / %`, `& | ~ << >>` (binary `~` is xor), `== ~= < <=
  > >=`, `and or not`, unary `-` `~` (bitwise not). Assignment is a statement.
- **Builtins:** `entity(x,y,tag)` (report a game object, so it shows up in the
  observation an agent reads), `rnd(n)→0..n-1`, `peek/poke(addr[,v])` (8-bit) and
  `peek16/poke16`, `min(a,b)` / `max(a,b)`, and
  `rect_overlap(ax,ay,aw,ah,bx,by,bw,bh)→bool`.
- **Drawing:** `cls(c)`, `pset(x,y,c)`, sprites (`spr`/`sprn`), the `tilemap`
  declaration and its collision helpers, `camera`, the palette (`pal`/`sprbank`),
  `text`/`number`, and the pseudo-3D pieces (`hline`, `spr_scaled`, `sin`/`cos`).
  Full reference in [**VM_GRAPHICS.md**](VM_GRAPHICS.md).
- **Input:** `btn`/`btnp`/`btnr(mask)→0/1` (held / pressed this frame /
  released this frame), the analog stick, touch, and `swipe`. Full reference in
  [**VM_CONTROLS.md**](VM_CONTROLS.md). `frame_count()→word` gives frames since
  power-on (wraps at 65536) for blink/timers/periodic spawns.
- **Arrays:** `len(arr)→word` is the array's declared length (a compile-time
  constant) — write `for i = 0, len(bullets)-1 do` so the loop follows the array
  size instead of a hand-written bound. `clear(x)` zeroes a record or whole array
  in place (`clear(bullets)` resets a pool; `clear(bullets[i])` one element) —
  cheaper and less error-prone than field-by-field reinitialization.
- **Sound:** `sfx(id)`, `music(id)`, `music_stop()` for what a game declares in
  advance, and `play`/`note_on`/`note_off` for a pitch it decides at runtime.
  Declarations (`instrument`, `sfx`, `track`, `fx`) and the render report are in
  [**VM_AUDIO.md**](VM_AUDIO.md).
- **Button constants:** `LEFT RIGHT UP DOWN A B START SELECT` — also the values
  `swipe()` reports.
- **Controls metadata:** an optional top-level `controls { … }` block records the
  game's input layout as ROM metadata, so a host UI can label and lay out a pad
  without guessing. Irrelevant to VM execution. See
  [**VM_CONTROLS.md**](VM_CONTROLS.md).
- **Several files:** `#include "lib/motion.lua"` — see below.
- Comments: `--` line, `--[[ … ]]` block.

### Splitting a game across files (`#include`)

```lua
#include "lib/motion.lua"     -- top level, quoted, with the extension
```

The named file's declarations are spliced in **at the directive**, into the same
flat namespace — records, functions, globals, sprites, instruments, `sfx`,
tracks. There is no module value and nothing to bind: writing Lua's
`require` is a diagnostic that says so, because `require` returns a table and
this machine has no runtime tables to return one into. (PICO-8 spells its
equivalent the same way, for the same reason.)

The rules, all of which are diagnostics rather than surprises:

- **A file is included at most once**, however many files ask for it — so two of
  your sources may both include the same library without declaring anything
  twice.
- **A cycle is an error**, reported with its chain (`a.lua → b.lua → a.lua`), as
  is nesting more than 16 deep.
- **`screen` and `controls` belong to the game's own file.** They are the ROM's
  identity; a library that quietly changed your screen size would be a long
  afternoon.
- **Diagnostics name their file** — `util.lua line 12: unknown variable 'nope'`.

Where the named file is looked up is the *host's* answer, since the VM has no
opinion about directories:

| Host | `#include "x.lua"` finds |
|------|--------------------------|
| `kessel mcp` (`vm_assemble`) | `x.lua` in the working directory, which it cannot escape |
| `kessel run games/swarm.lua` | `games/x.lua` — the game's own directory |
| Android | a source the app handed over from `assets/` before loading |

In `games/`, shared sources live in `games/lib/` and are included by that path:
`games/swarm.lua` is the worked example, and `games/lib/motion.lua` the library
it uses.

### Tutorial snippets

Worked examples the model can adapt (this is what helps most):

```lua
-- input: move a block
function update()
  if btn(LEFT)  then p.x = p.x - 1 end
  if btn(RIGHT) then p.x = p.x + 1 end
end

-- entity list: update an array of records
record Enemy { x, y, alive }
local es: array(8, Enemy)
function update()
  for i = 0, 7 do
    if es[i].alive == 1 then es[i].x = es[i].x + 1 end
  end
end

-- simple state switch
local state = 0            -- 0 title, 1 play
function update()
  if state == 0 and btn(START) then state = 1 end
end

-- tilemap + collision: draw a level and stop the player at solid tiles
tilemap level(16, 16)
function init()
  fset(1, SOLID, 1)                 -- tile id 1 is solid
  for x = 0, 15 do mset(x, 14, 1) end  -- a floor row
end
function draw() map(0, 0, 0, 0, 16, 16) end
function update()
  local vy: int = p.vy + 1          -- gravity
  if vy > 0 and solid(p.x + 4, p.y + 8) then vy = 0 end
  p.vy = vy
end
```

**Full worked example:** `games/platform.lua` is a ~70-line tile platformer —
sprites, a `tilemap` level, gravity, `solid()` collision, and a jump — the kind
of complete example to adapt.

## Playing a game (`kessel run`)

`kessel run games/tetris.lua` renders a ROM in a native window, so the games a
model authors are **human-playable**. The README lists the bundled set with their
controls; what matters here is that they double as worked luax examples spanning
the builtins:
`2048` (array transforms + edge-triggered grid input, and the **swipe**
reference — `swipe()` and `btnp` folded into one `direction()`), `snake` (record arrays +
grid movement), `brick` (signed `int` velocity + AABB brick hits + a
`len`-bounded pool init), `shooter` (entity pools driven by `len` +
`clear`-reset pools + `rect_overlap`), `tetris` (bitmask pieces, runtime
rotation, a `tilemap` well + line clears, `min`-clamped difficulty), `rogue`
(`tilemap` + `fset`/`solid` collision + simple enemy AI + `min`-capped healing),
`platform` (tile collision, gravity, wall-jumps, collectibles, and enemies), and
`sokoban` (grid puzzle — `btnp` step input, a board held in the `tilemap` and
mutated with `mset`, `text`/`number` HUD), and `outrun` (a pseudo-3D road racer
— per-scanline `hline` road with a parabolic curve, `spr_scaled` roadside trees,
and a `sin`-bobbed sun), `popn` (a six-key rhythm game with **no directions** —
the four direction bits declared as labelled keys, so a host draws a button row),
and `paint` (the two analog surfaces — `touch_*` fingers in console pixels and a
`stick_x`/`stick_y` brush, with the branch-on-the-sign idiom the unsigned divide
forces).

> Note on `min`/`max`: they compile to the VM's **unsigned** `LT`/`GT`, so only
> clamp values that stay non-negative with them (e.g. a score-derived level).
> For a signed `int` that can go negative (a velocity or an off-screen
> coordinate), keep explicit `if x < 0` comparisons — see `shooter`'s player
> clamp — since `min`/`max` would treat the wrapped negative as a huge number.

### Controls

Arrows or WASD for the d-pad (which also deflect the analog stick), `Z`/`X` for
A/B, Return for START, Shift for SELECT, and a mouse drag is touch slot 0. Full
key table and what each host provides: [**VM_CONTROLS.md**](VM_CONTROLS.md).

`R` recompiles the source and restarts the game, so you can keep the window open
while editing; Esc quits. Pressing the ROM's **pause** button (from its
`controls` metadata, default `START` = Return) freezes the game.

### Attaching to an agent's session

`kessel attach` joins a running `kessel mcp` and drives the
agent's own console, so you can play the work in progress:

```bash
kessel attach                  # joins the running session
kessel attach ./my-game        # ...that one, if several are running
```

You share one machine and one timeline with the agent. Its
`vm_snapshot`/`vm_restore`/`vm_reset` rewind the game under you, `vm_run_frames`
advances it in bursts, and your button presses appear in its observations — a run
with a player attached is not reproducible. `R` does nothing when attached: the
agent owns what's loaded. Pass a file for an independent timeline.

The server publishes a session file (cache dir) naming a loopback port, binds
`127.0.0.1` only, and never advances the machine on its own — with no player
attached, agent runs are exactly as deterministic as before.

### How it works

`kessel run` loads a `.lua`/`.asm` file into a `VmPlayer` (`crates/vm/src/player.rs`),
opens a window with `winit`, and on a 60 Hz tick calls `tick(buttons)` +
`framebuffer_rgba()`, blitting the framebuffer (128×128 or 240×240, whichever
the ROM asked for) scaled up with nearest-neighbour into a `softbuffer` CPU
surface (`crates/cli/src/play.rs`).

**Sound** comes out of the same tick: each frame's `sfx()` triggers go through a
lock-free queue to a `cpal` output stream, where `kessel-audio` renders them
(`crates/cli/src/audio.rs`). The audio thread never blocks on the game and never
allocates, so a slow frame drops a frame rather than clicking; a full queue drops
the sound and says how many on exit. Pressing `R` restarts the stream, because
the reloaded game may declare different instruments.

The realtime path applies an event at the start of the callback block that finds
it, which costs one device buffer of latency (5–11 ms, under one frame) and
avoids the drift you would get by mapping the game's frame counter onto the audio
clock. `kessel render-audio` is the sample-accurate, reproducible path — use that
one when you need to *check* a sound rather than hear it.

There is no sound when **attached**: `kessel attach` drives the agent's console,
and its sound events belong to that process.

There is deliberately **no GPU** in the path. The console rasterizes into its own
palette-indexed framebuffer, so presentation is a plain upscale-and-blit; keeping
it on the CPU means the pixels you see are exactly the buffer an agent gets back
from `vm_get_framebuffer`, and it keeps the binary runnable on a machine with no
usable graphics adapter. The player is behind a default-on `play` feature —
`cargo build --no-default-features` yields a headless binary with `kessel mcp`
only, for servers and containers.

The same boundary holds for **sound**, and for the same reason: the VM records
what a game asked for and never produces a sample, so every host renders the one
event log. That is what keeps `vm_snapshot` / `vm_restore` and frame-exact replay
meaningful — a machine that synthesized audio itself could not be rewound. The
synth lives in `kessel-audio`, which opens no device either; only `play.rs` here
knows what a sound card is.
