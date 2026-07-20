# Kessel Fantasy-Console VM

A tiny 16-bit stack VM (Uxn-inspired) that lets the model **write a small game,
assemble it, run it, observe the result, and debug it** — and the game is
playable by a human. Lives in `crates/lib/src/vm/`. Pure Rust, deterministic,
snapshotable.

The `vm_*` tools are registered **only** in `agent_new` (the standalone `kessel`
app), so they are present in the mac/Windows voice app alongside
screenshot/STT/TTS but **absent** from `kessel-cli`/app-server.

## Machine

- 16-bit stack machine. Data stack + return stack, 256 `u16` cells each.
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
| `0x01..0x04` | out | palette: index, r, g, b (writing **b** commits the entry) |
| `0x10` | out | screen/vector — install the frame vector (address) |
| `0x11` `0x12` | out | screen x, y |
| `0x13` | out | screen colour (0–15; 0 = transparent for sprites) |
| `0x14` | out | draw pixel at (x,y) |
| `0x15` | out | draw 8×8 sprite from `mem[addr]` (32 bytes, 4bpp, hi-nibble = left) |
| `0x16` | out | clear screen to colour |
| `0x1d` | out | horizontal span: fill from screen x to x2(=val) at row y in colour (endpoints are signed, so a span past the left edge clips) |
| `0xb0` `0xb1` | out | scaled sprite: scale (8.8 fixed, 256 = 1.0) / blit-id (scaled tile at screen x/y) |
| `0xc0` | in/out | trig: write angle (0..255 = a turn) → read sin; `0xc1` reads cos. Signed 8.8 fixed (-256..256) |
| `0x20` | in  | gamepad buttons bitfield (held) |
| `0x21` `0x22` | in | gamepad edges: just-pressed / just-released this frame |
| `0x30` | in/out | rng: read next `u16` / write to set the seed |
| `0x80` | in  | frame counter (frames since power-on; wraps at 65536) |
| `0x90` `0x91` `0x92` | out | sound: sfx(id) / music(id) / music-stop (recorded, no audio yet) |
| `0x40` `0x41` `0x42` | out/in/out | storage addr / read / write (256 bytes) |
| `0x50` `0x51` `0x52` | out | debug entity: x, y, commit(tag) — reported in the observation |
| `0x60` | out | console: write a byte to the text buffer |

Gamepad bits: `LEFT 0x01 RIGHT 0x02 UP 0x04 DOWN 0x08 A 0x10 B 0x20 START 0x40
SELECT 0x80`. Screen is 128×128, 16-colour (default PICO-8 palette).

## The tools (agent-facing loop)

`vm_write_source(path, source)` → `vm_assemble(path)` → `vm_load_rom(path)` →
`vm_run_frame(buttons)` / `vm_run_cycles(n)` → `vm_inspect_memory`,
`vm_inspect_stacks`, `vm_get_framebuffer` (PNG) → `vm_snapshot`/`vm_restore`,
`vm_reset`.

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
fixed-length. Entry points (VM is vector-driven, no `main(){ loop … }`): `init`
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
- **Input & timing:** `btn(mask)→0/1` (held), `btnp(mask)→0/1` (pressed *this*
  frame — the rising edge), `btnr(mask)→0/1` (released this frame). Use `btnp`
  for jumps, menu steps and fire-on-press so the model doesn't have to track the
  previous frame's buttons by hand. `frame_count()→word` gives frames since
  power-on (wraps at 65536) for blink/timers/periodic spawns.
- **Arrays:** `len(arr)→word` is the array's declared length (a compile-time
  constant) — write `for i = 0, len(bullets)-1 do` so the loop follows the array
  size instead of a hand-written bound. `clear(x)` zeroes a record or whole array
  in place (`clear(bullets)` resets a pool; `clear(bullets[i])` one element) —
  cheaper and less error-prone than field-by-field reinitialization.
- **Sound:** `sfx(id)`, `music(id)`, `music_stop()` fire sound triggers. The VM
  is deterministic and headless, so nothing is synthesized yet — the triggers
  are recorded and surfaced in the observation's `sound` array (so the agent
  sees a sound "played"); host audio in the play windows is a follow-up. The
  luax API is final.
- **On-screen text:** `text("LITERAL", x, y, color)` draws a compile-time string
  in a built-in 3×5 font (uppercase `A-Z`, `0-9`, space, `: ! . -`; lowercase
  folds to upper), one glyph every 4 px — the argument must be a `"..."` literal,
  luax has no runtime strings. `number(n, x, y, color)` draws an integer in
  decimal. For scores, titles, and `GAME OVER` — reset `camera(0,0)` first if the
  world is scrolled. See the HUD in `games/shooter.lua`.
- **Button constants:** `LEFT RIGHT UP DOWN A B START SELECT`.
- **Controls metadata:** an optional top-level `controls { … }` block records the
  game's input layout **as ROM metadata** — a host UI (on-screen buttons, help
  text, a smartphone virtual pad) reads it instead of guessing from source
  comments. It is **irrelevant to VM execution**; the machine only ever sees the
  raw gamepad bitfield.
  ```lua
  controls {
    dpad = true       -- is the movement pad used
    a = "jump"        -- action labels for the A / B / Start / Select buttons
    b = "dash"
    pause = START     -- which physical button pauses (default START)
  }
  ```
  Keys: `dpad` (bool), `a`/`b`/`start`/`select` (a `"..."` label), and `pause` (a
  button name). Entries are separated by whitespace (commas optional). Every game
  has a **pause** binding by default (`START`) even with no block, so the host
  always has a pause control to offer — the play window freezes/resumes the game
  on that button and shows "PAUSED" in the title. `VmPlayer.controls_json()`
  hands the whole layout to the host as JSON.
- Comments: `--` line, `--[[ … ]]` block.

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

## Playing a game (`kessel --play`)

The standalone `kessel` app can render a ROM in a native window, so the games the
model authors are **human-playable**:

```bash
kessel --play games/2048.lua      # 2048 — arrows slide tiles, A starts a new game
kessel --play games/bounce.lua    # a self-animating demo
kessel --play games/mover.lua     # arrows move; Z/X = A/B; Return/Space = Start/Select
kessel --play games/snake.lua     # grid snake — arrows steer, eat food, A restarts
kessel --play games/brick.lua     # Breakout — arrows move the paddle
kessel --play games/shooter.lua   # vertical shooter — arrows move, A fires
kessel --play games/tetris.lua    # Tetris — L/R move, A rotates, Down soft-drops
kessel --play games/rogue.lua     # top-down action — arrows move, A swings a sword
kessel --play games/platform.lua  # tile platformer — arrows move, A jumps/wall-jumps
kessel --play games/sokoban.lua   # box-pushing puzzle — grid moves (btnp), mset-mutated board
kessel --play games/outrun.lua    # pseudo-3D road racer — arrows steer/accelerate, A boosts
```

The `games/` set doubles as worked luax examples spanning the builtins:
`2048` (array transforms + edge-triggered grid input), `snake` (record arrays +
grid movement), `brick` (signed `int` velocity + AABB brick hits + a
`len`-bounded pool init), `shooter` (entity pools driven by `len` +
`clear`-reset pools + `rect_overlap`), `tetris` (bitmask pieces, runtime
rotation, a `tilemap` well + line clears, `min`-clamped difficulty), `rogue`
(`tilemap` + `fset`/`solid` collision + simple enemy AI + `min`-capped healing),
`platform` (tile collision, gravity, wall-jumps, collectibles, and enemies), and
`sokoban` (grid puzzle — `btnp` step input, a board held in the `tilemap` and
mutated with `mset`, `text`/`number` HUD), and `outrun` (a pseudo-3D road racer
— per-scanline `hline` road with a parabolic curve, `spr_scaled` roadside trees,
and a `sin`-bobbed sun).

> Note on `min`/`max`: they compile to the VM's **unsigned** `LT`/`GT`, so only
> clamp values that stay non-negative with them (e.g. a score-derived level).
> For a signed `int` that can go negative (a velocity or an off-screen
> coordinate), keep explicit `if x < 0` comparisons — see `shooter`'s player
> clamp — since `min`/`max` would treat the wrapped negative as a huge number.

`--play` needs no model or API key. It loads a `.lua`/`.asm` file into a standalone
`VmPlayer` (`lib/src/vm/player.rs`, exported over UniFFI), opens an AppKit window,
and on a 60 Hz timer calls `tick(buttons)` + `framebuffer_rgba()`, blitting the
128×128 framebuffer scaled up with nearest-neighbour. The keyboard maps to the
gamepad. Pressing the ROM's **pause** button (from its `controls` metadata,
default `START` = Return) freezes the game and the title shows "PAUSED"; `tick`
handles this in the player, so both host windows get it for free. `games/` holds sample ROMs.

Under the hood the render loop is just: expand the palette-indexed framebuffer to
RGBA (`Devices::framebuffer_rgba`), hand it to a `CGImage`, and draw it with
interpolation off.

**Windows** mirrors this: `kessel --play <file>` in the C# frontend
(`win/KesselCli/PlayWindow.cs`) opens a WinForms window backed by the same
`VmPlayer`, blitting the framebuffer into a `Bitmap` (RGBA→BGRA) drawn with
`InterpolationMode.NearestNeighbor` on a 60 Hz timer, same keyboard mapping.
