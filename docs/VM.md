# Kessel Fantasy-Console VM

A tiny 16-bit stack VM (Uxn-inspired) that lets a model **write a small game,
assemble it, run it, observe the result, and debug it** — and the game is
playable by a human. Lives in `crates/vm/` (the `kessel-vm` crate). Pure Rust,
deterministic, snapshotable.

The `vm_*` tools reach an agent over MCP: `kessel mcp` serves them on stdio, so
any MCP-capable agent can drive the loop. `kessel run <file>` opens the same
console in a window for a human.

Two companion documents: [**VM_CONTROLS.md**](VM_CONTROLS.md) for input —
buttons, the analog stick, touch, gestures and the `controls { … }` metadata —
and [**AUDIO.md**](AUDIO.md) for the synth.

## Machine

- 16-bit stack machine. Data stack + return stack, 256 `u16` cells each.
- **Video**: a square, 8-bit palette-index framebuffer plus one 256-entry RGB
  palette. Two screen sizes, and *only* the size differs — same ports, same
  4bpp sprite sheet, same palette:

  | mode | screen | framebuffer | selected by |
  |------|--------|-------------|-------------|
  | `Classic128` | 128×128 | 16 KiB | the default |
  | `Extended240` | 240×240 | 56.25 KiB | `screen { mode = Extended240 }` |

  The mode is fixed when the ROM loads and never changes under a running game.
  The framebuffer lives outside the 64 KiB address space, so the wider screen
  costs a game no RAM.
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

Port byte = `(device << 4) | register`.

| Port | Dir | Meaning |
|------|-----|---------|
| `0x00` | out | system/halt (non-zero halts the machine) |
| `0x02..0x04` | out | palette: stage r, g, b |
| `0x01` | out | palette index (0–255) — **commits** the staged colour |
| `0x10` | out | screen/vector — install the frame vector (address) |
| `0x11` `0x12` | out | screen x, y |
| `0x13` | out | screen colour (0–255 palette index) |
| `0x14` | out | draw pixel at (x,y) |
| `0x15` | out | draw 8×8 sprite from `mem[addr]` (32 bytes, 4bpp, hi-nibble = left) |
| `0x16` | out | clear screen to colour |
| `0x1e` | out | sprite palette bank (0–15): a sprite nibble `n` draws as `bank*16 + n` |
| `0x1d` | out | horizontal span: fill from screen x to x2(=val) at row y in colour (endpoints are signed, so a span past the left edge clips) |
| `0xb0` `0xb1` | out | scaled sprite: scale (8.8 fixed, 256 = 1.0) / blit-id (scaled tile at screen x/y) |
| `0xc0` | in/out | trig: write angle (0..255 = a turn) → read sin; `0xc1` reads cos. Signed 8.8 fixed (-256..256) |
| `0x20`–`0x24` | in | gamepad buttons, edges, and the analog stick — see [VM_CONTROLS.md](VM_CONTROLS.md) |
| `0xd0`–`0xd7` | in/out | touch points and gestures — see [VM_CONTROLS.md](VM_CONTROLS.md) |
| `0x30` | in/out | rng: read next `u16` / write to set the seed |
| `0x80` | in  | frame counter (frames since power-on; wraps at 65536) |
| `0x90` `0x91` `0x92` | out | sound: sfx(id) / music(id) / music-stop |
| `0x93`–`0x96` | out | note: frames / velocity / note, then instrument commits a `play` |
| `0x97` `0x98` `0x99` | out | held note: instrument, then channel commits a `note_on`; `0x99` is `note_off` |
| `0x40` `0x41` `0x42` | out/in/out | storage addr / read / write (256 bytes) |
| `0x50` `0x51` `0x52` | out | debug entity: x, y, commit(tag) — reported in the observation |
| `0x60` | out | console: write a byte to the text buffer |

Input has its own document: [**VM_CONTROLS.md**](VM_CONTROLS.md) covers the
button bits, the analog stick, touch, gestures, the `controls { … }` metadata,
and what each host provides.

### Colour

Every drawing port takes a **palette index**, never RGB; only ports
`0x01..0x04` deal in RGB. That keeps one colour model across the framebuffer,
sprites, tilemaps and text.

The palette is 256 entries and the default fills all of them:

| range | contents |
|-------|----------|
| `0–15` | the PICO-8 16, so existing art is unchanged |
| `16–231` | a 6×6×6 RGB cube — index = `16 + 36r + 6g + b` |
| `232–255` | a 24-step grey ramp |

Nothing is reserved: the console draws no UI of its own, so a host that wants a
pause menu draws it in native UI, outside the framebuffer.

The palette commits on the **index** write, not the blue write, because that is
the order a stack machine produces for free — `pal(i,r,g,b)` pushes `i` first,
so `b` pops first and `i` last.

**Sprites stay 4bpp.** A tile is 32 bytes, one nibble per pixel, and nibble `0`
is transparent in every bank. Port `0x1e` selects a bank, so nibble `n` draws
as `bank*16 + n`: bank 0 is the identity (old art keeps its colours) and one
tile can wear sixteen colour schemes without a second copy. Widening sprites to
8bpp instead would have doubled the sheet and broken the one-char-per-pixel
sprite syntax for no extra reach.

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

**Sound is checked by reading, not listening.** `vm_render_audio` runs the game
(same input-script shape as `vm_run_frames`, and it advances the machine the
same way) and returns a report: every trigger with the frame it fired on, peak
and RMS, voices started and stolen, and a warning for each specific way audio
goes wrong — an `sfx` id with no declaration, an instrument the bank lacks, notes
dropped because too many were in flight, or triggers that fired into silence.
With a working directory set it also writes a `.wav` for a human. The same
render is available headless from the shell:

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

`music()` triggers are traced but silent until the sequencer lands; the report
says so rather than leaving you to wonder.

**Music is a `track`**: channels of rows, one channel per instrument, `tempo`
frames per row, played with `music(name)` and stopped with `music_stop()`.

```lua
track drive {
  tempo = 9
  vel = 150                      -- music sits under the sound effects
  thud  = "33 . 33 . 40 . 33 ."  -- a key that is not tempo/vel/loop names an
  pulse = "57 60 64 60 57 60 63 60"  -- instrument: that is its channel
}

function init() music(drive) end -- loops until something stops it
```

Rows mean what an `sfx`'s do — a number starts a note, `-` holds it, `.` rests.
A track **runs on the audio clock**, so a slow frame drops a frame rather than
stuttering the tune; `sfx()` stays on the game's clock, because that timing is
the game's. Music notes also yield their voices to sound effects, so an
explosion is never eaten by a bassline.

**Notes without a bank entry.** `sfx` and `track` cover sound a game knows in
advance; for a pitch it decides at runtime there are three more builtins:

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

An out-of-range argument makes the whole note a **no-op** — nothing is played
and nothing is disturbed. Neither wrapping nor clamping would do: every channel
`0`–`255` is one some part of the game may be holding a note on, so *any*
mapping of an invalid value onto the valid range steals someone else's note.
`vm_render_audio` reports the count, so the silence has an explanation.
`games/piano.lua` is the worked example for `note_on`/`note_off`: each finger
on the keybed holds a note on its own touch slot as a channel, and lifting
that finger releases it.

**Chorus and reverb are shared sends.** A patch says how much of itself to send
(`reverb = 40`, `chorus = 15`, both `0`–`255`); there is one chorus and one
reverb for the whole mix, and an `fx { }` block says what they sound like:

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
- **Sprites:** a `sprite NAME { … }` declaration gives an 8×8 tile; each row is a
  whitespace-free run of up to 8 chars — `.` = transparent, else a palette nibble
  `0-9a-f`. Declared sprites form a **sheet** (ids 0,1,2… in order); `NAME` is a
  constant = its id. Draw with `spr(id, x, y, flags)`.
  ```lua
  sprite ball {
    ..2222..
    .222222.
    22222222
    22222222
    .222222.
    ..2222..
  }
  function draw() spr(ball, x, y, 0) end   -- flags bit0=flip-x, bit1=flip-y
  ```
- **Statements:** `local`, assignment, `if/elseif/else … end`, `while … do … end`,
  `for i = a, b[, step] do … end` (ascending, positive literal step), `break`,
  `return`, calls.
- **Operators (Lua):** `+ - * / %`, `& | ~ << >>` (binary `~` is xor), `== ~= < <=
  > >=`, `and or not`, unary `-` `~` (bitwise not). Assignment is a statement.
- **Tilemap:** one `tilemap NAME(w, h)` declaration reserves a `w×h` grid of tile
  ids. `mget(tx,ty)` / `mset(tx,ty,id)` read/write cells; `map(tx,ty,sx,sy,tw,th)`
  draws a `tw×th` block of the grid (tiles from the sprite sheet) to screen
  `(sx,sy)`. Per-tile flag bits: `fset(tile,flag,v)` / `fget(tile,flag)→0/1`;
  `solid(px,py)→0/1` is `fget(mget(px/8,py/8), SOLID)` — the platformer collision
  primitive. Flag constants: `SOLID` (0), `FLAG1..FLAG3`.
- **Tilemap collision (phase 2):** higher-level helpers so the model doesn't
  re-derive corner-sampling and snap-to-grid every game (all take a rect
  `x,y,w,h` and a tile `flag`):
  - `map_rect_overlap(x,y,w,h,flag)→bool` — does the rect touch any tile with
    `flag` set? Scans every tile the rect covers (one sample per 8-px cell), so
    boxes larger than a tile don't miss an interior tile.
  - `collide_x(x,y,w,h,dx,flag)→new_x` / `collide_y(x,y,w,h,dy,flag)→new_y` —
    move the box by a signed `dx`/`dy` and return the coordinate snapped flush
    against the first flagged tile in the way (or the full move if clear). The
    whole leading edge is scanned tile-by-tile, so a box taller/wider than a tile
    can't slip past a tile between its corners. Resolve one axis at a time:
    `nx = collide_x(x,y,w,h,vx,SOLID)` then `ny = collide_y(nx,y,w,h,vy,SOLID)`.
    Assumes the box starts in a clear cell and the per-step move is smaller than a
    tile (no tunneling across a full tile in one frame).
  - `touching_left|right|floor|ceiling(x,y,w,h,flag)→bool` — is a flagged tile
    directly against that edge? (Grounded checks, wall-slides, ceiling bonks.)
  Jump *feel* (coyote time, jump buffering, wall-slides, and wall-jumps) stays in
  luax — see `games/platform.lua`.
- **Builtins:** `cls(c)`, `pset(x,y,c)`, `spr(id,x,y,flags)` (draw sheet tile
  `id`; flags bit0/1 = flip x/y), `sprn(id,x,y,w,h,flags)` (draw a `w×h` block of
  contiguous sheet tiles — id at col/row = `id + row*w + col` — for 16×16+
  players/bosses/UI panels; flip applies per tile, the block isn't mirrored),
  `sspr(addr,x,y,flags)` (blit a raw 32-byte tile at `addr`), `camera(x,y)`, `entity(x,y,tag)`, `btn(mask)→0/1`, `rnd(n)→0..n-1`,
  `peek/poke(addr[,v])` (8-bit) + `peek16/poke16`, `min(a,b)` `max(a,b)`,
  `rect_overlap(ax,ay,aw,ah,bx,by,bw,bh)→bool`, and the tilemap builtins above.
- **Colour:**
  - `pal(i,r,g,b)` — rewrite palette entry `i` (0–255). The framebuffer is
    untouched, so recolouring the screen costs one loop and no redraw: fades,
    damage flashes, day/night, and palette cycling all fall out of this.
  - `sprbank(n)` — draw subsequent sprites through bank `n` (0–15), so a tile's
    nibble `c` becomes colour `n*16 + c`. Bank 0 is the identity. One tile, up
    to sixteen colour schemes; nibble 0 stays transparent in every bank.
  - `screen { mode = Extended240 }` — a 240×240 screen instead of 128×128.
    Declared like `controls`, read by the host when the ROM loads, fixed for the
    run. `games/spectrum.lua` demonstrates all three.
- **Pseudo-3D / scaling (racers, mode-7-ish effects):**
  - `hline(x1,x2,y,c)` — fill a horizontal span at row `y`. The endpoints are
    signed, so a span whose left edge runs off-screen clips cleanly. Drawing one
    span per scanline gives a perspective road/floor cheaply (see
    `games/outrun.lua`).
  - `spr_scaled(id,x,y,scale,flags)` — nearest-neighbour scaled sheet tile;
    `scale` is 8.8 fixed (`256` = 1.0, `512` = 2×, `128` = ½×). For
    distance-scaled cars, trees and signs. Prefer angle-specific sprites over
    runtime rotation (there is no rotate builtin — it costs a lot for little).
  - `sin(a)→int` / `cos(a)→int` — fixed-point trig with `a` in `0..255` for a
    full turn (`64` = 90°). The result is **signed** 8.8 fixed in `[-256,256]`
    (`256` = 1.0), so `if cos(a) < 0` works. Note `/` is **always unsigned**, so
    `cos(a)*speed/256` does *not* auto-handle a negative product — branch on the
    sign and divide the magnitude, e.g.
    `if s < 0 then d = 0 - ((0 - s) / 40) else d = s / 40 end` (see the bobbing
    sun in `outrun.lua`).
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
  advance, and `play(inst, note, vel, frames)` / `note_on(chan, inst, note, vel)`
  / `note_off(chan)` for a pitch it decides at runtime. The VM itself stays
  deterministic and headless: it records what was asked for into the
  observation's `sound` array, and a *host* renders it — `kessel run` through
  cpal, the Android app through `AudioTrack`, and `vm_render_audio` /
  `kessel render-audio` to a file. See **Sound** above for the declarations
  (`instrument`, `sfx`, `track`, `fx`).
- **On-screen text:** `text("LITERAL", x, y, color)` draws a compile-time string
  in a built-in 3×5 font (uppercase `A-Z`, `0-9`, space, `: ! . -`; lowercase
  folds to upper), one glyph every 4 px — the argument must be a `"..."` literal,
  luax has no runtime strings. `number(n, x, y, color)` draws an integer in
  decimal. For scores, titles, and `GAME OVER` — reset `camera(0,0)` first if the
  world is scrolled. See the HUD in `games/shooter.lua`.
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

`kessel run` renders a ROM in a native window, so the games a model authors are
**human-playable**:

```bash
kessel run games/2048.lua      # 2048 — arrows slide tiles, A starts a new game
kessel run games/bounce.lua    # a self-animating demo
kessel run games/mover.lua     # arrows move; Z/X = A/B; Return/Space = Start/Select
kessel run games/snake.lua     # grid snake — arrows steer, eat food, A restarts
kessel run games/brick.lua     # Breakout — arrows move the paddle
kessel run games/shooter.lua   # vertical shooter — arrows move, A fires
kessel run games/tetris.lua    # Tetris — L/R move, A rotates, Down soft-drops
kessel run games/rogue.lua     # top-down action — arrows move, A swings a sword
kessel run games/platform.lua  # tile platformer — arrows move, A jumps/wall-jumps
kessel run games/sokoban.lua   # box-pushing puzzle — grid moves (btnp), mset-mutated board
kessel run games/outrun.lua    # pseudo-3D road racer — arrows steer/accelerate, A boosts
```

The `games/` set doubles as worked luax examples spanning the builtins:
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
