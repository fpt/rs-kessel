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
| `0x1f` | out | vertical span: fill from screen y to y2(=val) down column x, in colour (endpoints signed, same as the horizontal one) |
| `0x20` `0x21` `0x22` | out | light: stage r, g, b |
| `0x23` | out | light radius |
| `0x24` | out | light y (shared by the radial light and the box) |
| `0x25` | out | **draw** a radial light at x(=val) — adds |
| `0x26` | out | **flood** the light layer with r(=val) and the staged g/b — sets |
| `0x27` `0x28` | out | light box: h, w |
| `0x29` | out | **fill** the light box at x(=val) — sets |
| `0x2a` `0x2b` | out | shadow box: h, w |
| `0x2c` | out | **mark** the shadow box solid to light at x(=val) |
| `0xe0` `0xe1` `0xe2` | out | filled rect: h, w, then **draw** taking x (y and colour come from the screen page). Both axes signed, so a box off the top-left clips |
| `0xa0`–`0xa3` | out | `sprn`: base id, w, h, then draw a `w×h` block at screen x/y |
| `0xb0` `0xb1` | out | scaled sprite: scale (8.8 fixed, 256 = 1.0) / blit-id |
| `0xc0` `0xc1` | in/out | trig: write angle (0..255 = a turn) → read sin / cos. Signed 8.8 fixed (-256..256) |
| `0x70`–`0x78` | out | tilemap: base, width, tx, ty, sx, sy, tw, th, then draw |

## Two sizes, one colour model

The console has two screens, and **only the size differs** — same ports, same
4bpp sprite sheet, same palette:

| mode | screen | framebuffer | selected by |
|------|--------|-------------|-------------|
| `Square240` | 240×240 | 56.25 KiB | the default |
| `Portrait320` | 240 wide × 320 tall | 75 KiB | `screen { mode = Portrait320 }` |
| `Landscape320` | 320 wide × 240 tall | 75 KiB | `screen { mode = Landscape320 }` |

The short side is 240 on every screen, so art and gestures are the same fraction
of the picture whichever a game picks. `240x320` and `320x240` are accepted as
plain spellings of the two rectangular modes, and `Extended240` — the old name
of the square screen — still means `Square240`. The framebuffer is row-major
with the **width** as its stride.

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
- `screen { mode = Landscape320 }` — a 320×240 screen instead of 240×240. Declared
  like `controls`, read by the host when the ROM loads, fixed for the run.
  `games/spectrum.lua` demonstrates all three (its `screen` block picks the
  square).

## Light

A game can hand the console a **light layer**: one r/g/b light level per pixel,
applied on the way to the screen. `64` is neutral — a pixel at `(64,64,64)`
presents exactly its palette colour — `0` is black, and `255` is 4×.

The framebuffer is untouched by any of it. A game still draws flat palette
indices through the same blitter, the same tilemap and the same sprite banks;
only the expansion to RGBA reads the layer. So nothing upstream forks, a game's
own `peek` at what it drew still reads what it drew, and the window, Android and
the PNG an agent looks at all show one picture without a line of host code.

A ROM that never calls one of these three has no layer at all — nothing is
allocated and the pixels come out byte-for-byte as they always did.

- `ambient(r,g,b)` — flood the whole layer. This is the light layer's `cls`, and
  like `cls` it is the game's job to call: the layer persists across frames
  because the framebuffer does.
- `light(x,y,radius,r,g,b)` — a radial light at a **world** coordinate (the
  camera applies, exactly as it does to a sprite). Falloff is `1 - d²/r²`: a
  bright core easing to nothing at the rim.
- `light_rect(x,y,w,h,r,g,b)` — set a box of the layer. Origin and size, signed
  and clipped, the same four numbers `rect` takes.
- `shadow_rect(x,y,w,h)` — mark a box **solid to light**. No colour: it is not a
  thing that glows, it is a thing light stops at.

**Sources add, fills set.** A `light` is a lamp: it adds to whatever is already
there and saturates, so two torches overlap brighter and a red lamp beside a
blue one reads as magenta between them. `ambient` and `light_rect` are fills and
overwrite, exactly as `cls` and `rect` overwrite pixels. That is also the answer
to the HUD: a score at ambient 6 is white multiplied by `6/64`, and no
arrangement of round lights makes a strip of text readable without bleeding into
the room behind it — so set the strip back to neutral and draw on it.

Over neutral a light *brightens*. That headroom is the reason a coloured light
can tint what it touches instead of merely failing to darken it.

### Shadows

`shadow_rect` declares an obstacle, and every `light` after it is stopped by
that obstacle. A frame therefore reads **flood → walls → lamps**:

```lua
ambient(5, 5, 9)                    -- clears the layer AND its obstacles
for ty = 0, 15 do                   -- the walls, before any lamp
  for tx = 0, 15 do
    if fget(mget(tx, ty), SOLID) then shadow_rect(tx * 8, ty * 8, 8, 8) end
  end
end
light(hx + 4, hy + 4, 30, 56, 40, 20)
```

`ambient` clears the obstacles as well as the light, because they belong to one
frame: a wall left over from the previous frame is in the wrong place the moment
the world scrolls. A blocker only affects the lamps declared *after* it.

**A solid pixel is lit; what is behind it is not.** A wall facing a torch is the
one thing in the room the torch most needs to show you.

A box rather than a per-pixel mask taken from the art, because opacity from art
would mean deciding which palette indices are solid — a rule no palette can
answer for every game. What stops light in games like these is a wall, a crate
or a pillar, and a game already knows those bounds.

A ROM that declares nothing solid pays nothing: the lamp takes the plain radial
path with no ray walk at all.

```lua
function draw()
  cls(0)
  map(0, 0, 0, 0, 16, 16)
  spr(hero, hx, hy, 0)

  ambient(5, 5, 9)                      -- cave dark, and cold
  light(hx + 4, hy + 4, fuel, 58, 42, 22)  -- the torch, warm; radius = fuel
  light(wx + 4, wy + 4, 16, 46, 6, 10)     -- a wisp, its own red

  light_rect(0, 0, 128, 9, 64, 64, 64)  -- a readable HUD strip
  rect(0, 0, 128, 9, 0)
  text("DEPTH", 2, 2, 6)
end
```

`games/lantern.lua` is the worked example: a cave lit only by what the player
carries, where the torch's *radius is its fuel*, so the number being managed is
the number you can see.

### Why a layer and not alpha

This is not per-sprite alpha blending, and adding that would mean blending in
*index* space, where there is no answer: the blend of index 3 and index 12 is
whatever the palette happens to make it, and every game would need its own
mixing table. A light layer sidesteps that entirely — it resolves in RGB, after
the indices are gone, so it works with any palette a game invents and costs the
blitter nothing.

What it buys is what games actually reach for alpha to get: point lights, spot
lights, coloured glows on bullets and enemies, day/night, and a room going dark.
What it does not buy is a translucent *sprite*. For that the console already
has `pal` — recolouring an index is one loop and no redraw — and sprite banks.

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

`cls(c)`, `pset(x,y,c)`, `rect(x,y,w,h,c)`, `hline(x,x2,y,c)`, `vline(y,y2,x,c)`,
`ambient(r,g,b)`, `light(x,y,radius,r,g,b)`, `light_rect(x,y,w,h,r,g,b)` (above),
`spr(id,x,y,flags)`, `sprn(…)` (above),
`sspr(addr,x,y,flags)` (blit a raw 32-byte tile at `addr`), `camera(x,y)`, and the
tilemap builtins above. `rect_overlap(ax,ay,aw,ah,bx,by,bw,bh)→bool` is here too,
since it is what sprites are usually tested with.

`rect` takes an **origin and a size**, deliberately the same four numbers
`rect_overlap` takes — a game that tests a box and then draws it hands both the
same values, where corners-and-a-size mixed together is an off-by-one waiting
for the second reader. A zero `w` or `h` draws nothing, the answer
`rect_overlap` gives it too. Both axes are signed, so a box that has scrolled
off the top-left clips instead of wrapping.

It is also the difference between a filled box costing one device write and
costing one per row: a 32×32 box is ~35 cycles as `rect`, 1,071 as a loop of
`hline`, and 36,879 as a nested `pset` loop, against a 200,000-cycle frame. Most
of the corpus wrote one of the latter two before this existed — `piano`'s
`box`, `paint`'s `blob` and `lib/motion.lua`'s `block` were all the same missing
primitive.

### On-screen text

`text("LITERAL", x, y, color)` draws a compile-time string in a built-in 3×5 font
(uppercase `A-Z`, `0-9`, space, `: ! . -`; lowercase folds to upper), one glyph
every 4 px — the argument must be a `"..."` literal, luax has no runtime strings.
`number(n, x, y, color)` draws an integer in decimal. For scores, titles and
`GAME OVER` — reset `camera(0,0)` first if the world is scrolled. See the HUD in
`games/shooter.lua`.

### Pseudo-3D and scaling

For racers and mode-7-ish effects:

- `rect(x,y,w,h,c)` — a filled box, above. Reach for it before either span: a
  solid rectangle is what most HUD bars, panels and sprite-less things are, and
  drawing one a row at a time is the corpus's most common avoidable cost.
- `hline(x1,x2,y,c)` — fill a horizontal span at row `y`. The endpoints are
  signed, so a span whose left edge runs off-screen clips cleanly. One span per
  scanline gives a perspective road or floor cheaply (see `games/outrun.lua`).
- `vline(y,y2,x,c)` — `hline`'s mirror, and the one primitive a row-at-a-time
  renderer cannot fake: a boundary that moves with **x**. A tilted horizon is
  exactly that. `games/outrun.lua` draws its sky and grass a column at a time for
  this reason, and its road a row at a time for the opposite one.
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
`hline` road sheared into a bank, a `vline` horizon tilted with it,
`spr_scaled` roadside trees, a `sin`-bobbed sun), `platform` (tile
collision, gravity, wall-jumps), `rogue` (`tilemap` + `fset`/`solid`), `sokoban` (twelve Microban puzzles as
`data` grids, centred from their own bounding box), `shooter` (sprite pools, three
sprite banks plus a `pal` ramp of its own for the terrain, and a `text`/`number`
HUD), `2048` (a 16×16 `sprn` panel frame), `lantern` (the light layer: a dark cave, a
torch whose radius is its fuel, coloured glows on the things hunting you, walls
that cast, and a `light_rect` HUD). `rogue` and `sokoban` light the same three
ways at two very different depths — a dungeon you can only half see, and a
puzzle that stays fully readable while its crates cast into the corners they are
stuck in.
