# Kessel Graphics

Everything the console puts on screen: the framebuffer and its two sizes, the
palette, sprites, the tilemap, and the drawing builtins. Split out of
[`VM.md`](VM.md), which owns the machine itself.

The load-bearing idea: **one colour model, everywhere.** Every drawing port takes
a palette *index*, never RGB — framebuffer, sprites, tilemap and text all speak
the same 8-bit colour. Only ports `0x01`–`0x04` deal in RGB, and only to rewrite
what an index means.

## Ports

| Port | Dir | Meaning |
|------|-----|---------|
| `0x02..0x04` | out | palette: stage r, g, b |
| `0x01` | out | palette index (0–255) — **commits** the staged colour |
| `0x10` | out | screen/vector — install the frame vector (address) |
| `0x11` `0x12` | out | screen x, y |
| `0x13` | out | screen colour (0–255 palette index) |
| `0x14` | out | draw pixel at (x,y) |
| `0x15` | out | draw 8×8 sprite from `mem[addr]` (32 bytes, 4bpp, hi-nibble = left) |
| `0x16` | out | clear screen to colour |
| `0x17` `0x18` | out | camera x, y |
| `0x19` | out | sprite flags — bit0 flip-x, bit1 flip-y |
| `0x1a` | out | blit sheet tile by id at screen x/y |
| `0x1b` | out | tileset base address (the sprite sheet) |
| `0x1c` | out | draw one 3×5 glyph (ASCII code) at screen x/y |
| `0x1d` | out | horizontal span: fill from screen x to x2(=val) at row y in colour (endpoints are signed, so a span past the left edge clips) |
| `0x1e` | out | sprite palette bank (0–15): a sprite nibble `n` draws as `bank*16 + n` |
| `0xa0`–`0xa3` | out | `sprn`: base id, w, h, then draw a `w×h` block at screen x/y |
| `0xb0` `0xb1` | out | scaled sprite: scale (8.8 fixed, 256 = 1.0) / blit-id |
| `0xc0` `0xc1` | in/out | trig: write angle (0..255 = a turn) → read sin / cos. Signed 8.8 fixed (-256..256) |
| `0x70`–`0x78` | out | tilemap: base, width, tx, ty, sx, sy, tw, th, then draw |

## Two sizes, one colour model

The console has two screens, and **only the size differs** — same ports, same
4bpp sprite sheet, same palette:

| mode | screen | framebuffer | selected by |
|------|--------|-------------|-------------|
| `Classic128` | 128×128 | 16 KiB | the default |
| `Extended240` | 240×240 | 56.25 KiB | `screen { mode = Extended240 }` |

The mode is fixed when the ROM loads and never changes under a running game. The
framebuffer lives outside the 64 KiB address space, so the wider screen costs a
game no RAM.

A second mode that also changed the *colour* model would have forked the blitter,
the PNG encoder and every host's upload path for nothing, so it doesn't exist.

## Colour

The palette is 256 entries and the default fills all of them:

| range | contents |
|-------|----------|
| `0–15` | the PICO-8 16, so existing art is unchanged |
| `16–231` | a 6×6×6 RGB cube — index = `16 + 36r + 6g + b` |
| `232–255` | a 24-step grey ramp |

Nothing is reserved: the console draws no UI of its own, so a host that wants a
pause menu draws it in native UI, outside the framebuffer.

The palette commits on the **index** write, not the blue write, because that is
the order a stack machine produces for free — `pal(i,r,g,b)` pushes `i` first, so
`b` pops first and `i` last.

- `pal(i,r,g,b)` — rewrite palette entry `i` (0–255). The framebuffer is
  untouched, so recolouring the screen costs one loop and no redraw: fades,
  damage flashes, day/night and palette cycling all fall out of this.
- `sprbank(n)` — draw subsequent sprites through bank `n` (0–15), so a tile's
  nibble `c` becomes colour `n*16 + c`. Bank 0 is the identity. One tile, up to
  sixteen colour schemes; nibble 0 stays transparent in every bank.
- `screen { mode = Extended240 }` — a 240×240 screen instead of 128×128. Declared
  like `controls`, read by the host when the ROM loads, fixed for the run.
  `games/spectrum.lua` demonstrates all three.

## Sprites

**Sprites stay 4bpp.** A tile is 32 bytes, one nibble per pixel, and nibble `0`
is transparent in every bank. Port `0x1e` selects a bank, so nibble `n` draws as
`bank*16 + n`: bank 0 is the identity (old art keeps its colours) and one tile can
wear sixteen colour schemes without a second copy. Widening sprites to 8bpp would
have doubled the sheet and broken the one-char-per-pixel sprite syntax for no
extra reach.

A `sprite NAME { … }` declaration is a block of pixel rows — each a
whitespace-free run where `.` = transparent and any other char is a palette nibble
`0-9a-f`. **The size comes from the body**: rows are the height, characters the
width, so 8 rows of 8 chars is one tile and 16 rows of 16 chars is a 2×2 sprite
the compiler slices for you. Declared sprites form a **sheet**; `NAME` is a
constant equal to the id of its *first* tile, and a multi-tile sprite occupies
that many consecutive ids.

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

A single tile is forgiving — short rows and fewer than eight of them pad
transparent. Bigger than that, the grid must be exact (every row the same length,
both dimensions multiples of 8): a miscounted row there would not pad one sprite,
it would shift every tile after it in the block and every id after that. Pointing
`spr` at a multi-tile sprite, or giving `sprn` a size that contradicts the
declaration, is a diagnostic rather than a wrong-looking game.

Draw one tile with `spr(id, x, y, flags)` and anything bigger with
`sprn(NAME, x, y, flags)`. `sprn` also has a raw form,
`sprn(id, x, y, w, h, flags)`, which walks a `w×h` block of contiguous ids
(`id + row*w + col`) — for a run the compiler cannot see, such as separately
declared quadrants or a strip of frames. A flip mirrors the cell layout as well as
each tile's pixels, so a flipped 2×2 character faces the other way rather than
scrambling.

## Tilemap

One `tilemap NAME(w, h)` declaration reserves a `w×h` grid of tile ids.
`mget(tx,ty)` / `mset(tx,ty,id)` read and write cells; `map(tx,ty,sx,sy,tw,th)`
draws a `tw×th` block of the grid (tiles from the sprite sheet) to screen
`(sx,sy)`. Per-tile flag bits: `fset(tile,flag,v)` / `fget(tile,flag)→0/1`;
`solid(px,py)→0/1` is `fget(mget(px/8,py/8), SOLID)` — the platformer collision
primitive. Flag constants: `SOLID` (0), `FLAG1..FLAG3`.

Map cells are **bytes**, so a sprite used as a map tile has to land below id 256.

### Collision helpers

Higher-level helpers, so a game doesn't re-derive corner-sampling and
snap-to-grid every time. All take a rect `x,y,w,h` and a tile `flag`:

- `map_rect_overlap(x,y,w,h,flag)→bool` — does the rect touch any tile with `flag`
  set? Scans every tile the rect covers (one sample per 8-px cell), so boxes
  larger than a tile don't miss an interior tile.
- `collide_x(x,y,w,h,dx,flag)→new_x` / `collide_y(x,y,w,h,dy,flag)→new_y` — move
  the box by a signed `dx`/`dy` and return the coordinate snapped flush against
  the first flagged tile in the way (or the full move if clear). The whole leading
  edge is scanned tile-by-tile, so a box taller or wider than a tile can't slip
  past a tile between its corners. Resolve one axis at a time:
  `nx = collide_x(x,y,w,h,vx,SOLID)` then `ny = collide_y(nx,y,w,h,vy,SOLID)`.
  Assumes the box starts in a clear cell and the per-step move is smaller than a
  tile (no tunneling across a full tile in one frame).
- `touching_left|right|floor|ceiling(x,y,w,h,flag)→bool` — is a flagged tile
  directly against that edge? (Grounded checks, wall-slides, ceiling bonks.)

Jump *feel* — coyote time, jump buffering, wall-slides, wall-jumps — stays in
luax; see `games/platform.lua`.

## Drawing builtins

`cls(c)`, `pset(x,y,c)`, `spr(id,x,y,flags)`, `sprn(…)` (above),
`sspr(addr,x,y,flags)` (blit a raw 32-byte tile at `addr`), `camera(x,y)`, and the
tilemap builtins above. `rect_overlap(ax,ay,aw,ah,bx,by,bw,bh)→bool` is here too,
since it is what sprites are usually tested with.

### On-screen text

`text("LITERAL", x, y, color)` draws a compile-time string in a built-in 3×5 font
(uppercase `A-Z`, `0-9`, space, `: ! . -`; lowercase folds to upper), one glyph
every 4 px — the argument must be a `"..."` literal, luax has no runtime strings.
`number(n, x, y, color)` draws an integer in decimal. For scores, titles and
`GAME OVER` — reset `camera(0,0)` first if the world is scrolled. See the HUD in
`games/shooter.lua`.

### Pseudo-3D and scaling

For racers and mode-7-ish effects:

- `hline(x1,x2,y,c)` — fill a horizontal span at row `y`. The endpoints are
  signed, so a span whose left edge runs off-screen clips cleanly. One span per
  scanline gives a perspective road or floor cheaply (see `games/outrun.lua`).
- `spr_scaled(id,x,y,scale,flags)` — nearest-neighbour scaled sheet tile; `scale`
  is 8.8 fixed (`256` = 1.0, `512` = 2×, `128` = ½×). For distance-scaled cars,
  trees and signs. Prefer angle-specific sprites over runtime rotation (there is
  no rotate builtin — it costs a lot for little).
- `sin(a)→int` / `cos(a)→int` — fixed-point trig with `a` in `0..255` for a full
  turn (`64` = 90°). The result is **signed** 8.8 fixed in `[-256,256]`
  (`256` = 1.0), so `if cos(a) < 0` works. Note `/` is **always unsigned**, so
  `cos(a)*speed/256` does *not* auto-handle a negative product — branch on the
  sign and divide the magnitude, e.g.
  `if s < 0 then d = 0 - ((0 - s) / 40) else d = s / 40 end` (see the bobbing sun
  in `outrun.lua`).

## No GPU, on purpose

Drawing is a software rasterizer into an indexed framebuffer, in `kessel-vm`,
which touches no graphics adapter. Presentation is a plain upscale-and-blit in
whichever host is running, so the pixels a player sees are exactly the buffer an
agent gets back from `vm_get_framebuffer` — and the binary still runs on a machine
with no usable GPU.

## Sample games

`spectrum` (240×240, the 256-colour palette, sprite banks), `outrun` (per-scanline
`hline` road, `spr_scaled` roadside trees, a `sin`-bobbed sun), `platform` (tile
collision, gravity, wall-jumps), `rogue` and `sokoban` (`tilemap` +
`fset`/`solid`, a board mutated with `mset`), `shooter` (sprite pools and a
`text`/`number` HUD), `2048` (a 16×16 `sprn` panel frame).
