-- lantern.lua — light is the mechanic. A cave lit only by what you carry:
-- your torch burns down as you walk, braziers refill it, and the wisps that
-- want it are visible only by their own red glow. Reach the stairs to descend.
--
--   kessel run games/lantern.lua
--
-- This is the corpus's **lighting** example. Everything here is drawn in flat
-- palette indices exactly as any other game draws them — the darkness is a
-- separate layer, resolved on the way to the screen. Read `docs/VM_GRAPHICS.md`
-- ("Light") for the model; the four calls it costs are all in `draw()` below.
--
-- Sprite order sets the tile ids: floor=0, wall=1, moss=2 (the rest are drawn
-- as free sprites, not tiles).

controls {
  dpad  = true      -- walk
  a     = "flare"
  pause = START
}

sprite floor {
  11111111
  11111211
  11111111
  12111111
  11111111
  11111121
  11111111
  11211111
}

sprite wall {
  55555555
  51111115
  51555115
  51555115
  51111115
  55511555
  55511555
  55555555
}

sprite moss {
  11111111
  13111311
  11311111
  11113111
  13111131
  11311111
  11111311
  11113111
}

sprite hero {
  ..7777..
  .7cccc7.
  7cc99cc7
  7cccccc7
  .7cccc7.
  ..c..c..
  ..7..7..
  .7....7.
}

sprite wisp {
  ...88...
  ..8228..
  .822228.
  88222288
  88222288
  .822228.
  ..8228..
  ...88...
}

sprite brazier {
  ...aa...
  ..a99a..
  ..a99a..
  ...aa...
  ..4444..
  ..4554..
  .455554.
  .455554.
}

sprite brazier_out {
  ........
  ........
  ........
  ........
  ..4444..
  ..4554..
  .455554.
  .455554.
}

sprite flare {
  ........
  ..cccc..
  .cc77cc.
  .c7777c.
  .c7777c.
  .cc77cc.
  ..cccc..
  ........
}

sprite stair {
  cccccccc
  c666666c
  cc6666cc
  .c6666c.
  .cc66cc.
  ..c66c..
  ..cccc..
  ........
}

tilemap cave(30, 30)

record Wisp  { x, y, dir, alive }
record Torch { x, y, lit }

-- Torch fuel, in frames. It is also the torch's *radius* (see `draw`), which is
-- the whole design: the number the player is managing is the number they can
-- see, so nobody has to read a fuel bar to know they are in trouble.
local FULL = 40
local LOW = 14

local hx = 16
local hy = 16
local face = 1          -- 1 right, 0 left; the flare inherits it
local fuel = FULL
local depth = 1
local dead = 0
local bitten = 0        -- frames until a wisp may drink again

local wisps: array(4, Wisp)
local fires: array(3, Torch)

local fx = 0            -- the one flare in flight
local fy = 0
local fdir = 1
local flife = 0

local sx = 216          -- the stairs down
local sy = 216
local flicker = 0

signal fuel_left
signal depth_now
signal wisps_alive
signal state            -- 0 exploring, 1 snuffed out

function build_cave()
  fset(wall, SOLID, 1)
  for y = 0, 29 do
    for x = 0, 29 do
      if x == 0 or y == 0 or x == 29 or y == 29 then
        mset(x, y, wall)
      elseif rnd(11) == 0 then
        mset(x, y, moss)
      else
        mset(x, y, floor)
      end
    end
  end
  -- Pillars. Fixed rather than random: a cave you cannot walk across is a
  -- softlock, and this game has no way to tell you that is what happened.
  local i = 0
  while i < 8 do
    local px = 3 + i * 3
    local py = 4 + ((i + depth) % 5) * 5
    mset(px, py, wall)
    mset(px, py + 1, wall)
    i = i + 1
  end
end

function place(n)
  depth = n
  build_cave()
  hx = 16  hy = 16  face = 1
  flife = 0
  dead = 0
  bitten = 0

  -- Tile columns that are not a multiple of 3, so a brazier never lands
  -- inside one of the pillars `build_cave` puts at x = 3, 6, 9, …
  fires[0].x = 176  fires[0].y = 40   fires[0].lit = 1
  fires[1].x = 40   fires[1].y = 168  fires[1].lit = 1
  fires[2].x = 128  fires[2].y = 120  fires[2].lit = 1

  local i = 0
  while i < 4 do
    wisps[i].x = 64 + (i % 2) * 112
    wisps[i].y = 72 + (i / 2) * 88
    wisps[i].dir = i % 4
    wisps[i].alive = 1
    i = i + 1
  end

  sx = 104  sy = 104
  if depth % 2 == 0 then sx = 16  sy = 104 end
end

function init()
  fuel = FULL
  place(1)
end

-- A wisp drifts, turns at a wall, and never chases: what makes them dangerous
-- is that you cannot see the floor between you and one, not that they aim.
function move_wisp(i)
  if wisps[i].alive == 0 then return end
  local d = wisps[i].dir
  local dx = 0
  local dy = 0
  if d == 0 then dx = 1 elseif d == 1 then dx = 0 - 1 elseif d == 2 then dy = 1 else dy = 0 - 1 end

  local nx = collide_x(wisps[i].x, wisps[i].y, 8, 8, dx, SOLID)
  local ny = collide_y(nx, wisps[i].y, 8, 8, dy, SOLID)
  if nx == wisps[i].x and ny == wisps[i].y then
    wisps[i].dir = rnd(4)
  else
    wisps[i].x = nx
    wisps[i].y = ny
    if rnd(90) == 0 then wisps[i].dir = rnd(4) end
  end
end

function update()
  flicker = rnd(5)

  if dead == 1 then
    if btnp(A) then
      fuel = FULL
      place(1)
    end
    return
  end

  local dx = 0
  local dy = 0
  if btn(LEFT)  then dx = 0 - 1  face = 0 end
  if btn(RIGHT) then dx = 1      face = 1 end
  if btn(UP)    then dy = 0 - 1 end
  if btn(DOWN)  then dy = 1 end
  hx = collide_x(hx, hy, 8, 8, dx, SOLID)
  hy = collide_y(hx, hy, 8, 8, dy, SOLID)

  -- Fire a flare: it flies flat, lights what it passes, and snuffs a wisp.
  if btnp(A) and flife == 0 then
    fx = hx  fy = hy  fdir = face  flife = 40
  end
  if flife > 0 then
    local step = 3
    if fdir == 0 then step = 0 - 3 end
    local nfx = collide_x(fx, fy, 8, 8, step, SOLID)
    if nfx == fx then flife = 1 else fx = nfx end
    flife = flife - 1
    local i = 0
    while i < 4 do
      if wisps[i].alive == 1 and rect_overlap(fx, fy, 8, 8, wisps[i].x, wisps[i].y, 8, 8) then
        wisps[i].alive = 0
        flife = 0
      end
      i = i + 1
    end
  end

  -- Braziers refill the torch; each one gives once.
  local i = 0
  while i < 3 do
    if fires[i].lit == 1 and rect_overlap(hx, hy, 8, 8, fires[i].x, fires[i].y, 8, 8) then
      fires[i].lit = 0
      fuel = FULL
    end
    i = i + 1
  end

  -- Wisps drink the torch on contact, then have to let go for a moment. A
  -- per-frame drain empties a full torch in seven frames, which reads as a
  -- broken game rather than a hard one.
  if bitten > 0 then bitten = bitten - 1 end
  i = 0
  while i < 4 do
    move_wisp(i)
    if bitten == 0 and wisps[i].alive == 1 and rect_overlap(hx, hy, 8, 8, wisps[i].x, wisps[i].y, 8, 8) then
      if fuel > 8 then fuel = fuel - 8 else fuel = 0 end
      bitten = 30
    end
    i = i + 1
  end

  if fuel > 0 and frame_count() % 12 == 0 then fuel = fuel - 1 end
  if fuel == 0 then dead = 1 end

  if rect_overlap(hx, hy, 8, 8, sx, sy, 8, 8) then
    local carry = fuel
    place(depth + 1)
    fuel = carry
    if fuel < FULL then fuel = fuel + 8 end
  end
end

function draw()
  cls(0)
  map(0, 0, 0, 0, 30, 30)

  local i = 0
  while i < 3 do
    if fires[i].lit == 1 then
      spr(brazier, fires[i].x, fires[i].y, 0)
    else
      spr(brazier_out, fires[i].x, fires[i].y, 0)
    end
    i = i + 1
  end

  spr(stair, sx, sy, 0)

  i = 0
  while i < 4 do
    if wisps[i].alive == 1 then spr(wisp, wisps[i].x, wisps[i].y, 0) end
    i = i + 1
  end

  if flife > 0 then spr(flare, fx, fy, 0) end
  spr(hero, hx, hy, face)

  -- ---- the light layer ----------------------------------------------------
  -- Everything above drew flat palette indices and knows nothing about any of
  -- this. `ambient` floods the layer (it is the light layer's `cls`), the
  -- sources add on top of it, and `light_rect` sets a readable strip back to
  -- neutral for the HUD.

  ambient(5, 5, 9)                      -- cave dark, and cold

  -- Walls stop light. Declared after the flood (which clears them) and before
  -- any lamp, because a blocker only affects the lamps that come after it.
  for ty = 0, 29 do
    for tx = 0, 29 do
      if fget(mget(tx, ty), SOLID) then shadow_rect(tx * 8, ty * 8, 8, 8) end
    end
  end

  i = 0
  while i < 3 do
    if fires[i].lit == 1 then
      light(fires[i].x + 4, fires[i].y + 2, 30 + flicker, 60, 40, 14)
    end
    i = i + 1
  end

  light(sx + 4, sy + 4, 18, 10, 46, 22)  -- the stairs, a cool green beacon

  i = 0
  while i < 4 do
    if wisps[i].alive == 1 then
      -- Each wisp carries its own colour, which is the only reason you can see
      -- one coming: the floor around it never gets drawn any differently.
      light(wisps[i].x + 4, wisps[i].y + 4, 16, 46, 6, 10)
    end
    i = i + 1
  end

  if flife > 0 then light(fx + 4, fy + 4, 22, 12, 44, 60) end

  if dead == 0 then
    light(hx + 4, hy + 4, fuel + flicker, 58, 42, 22)
  end

  light_rect(0, 0, 240, 8, 64, 64, 64)   -- the HUD bar, at neutral

  rect(0, 0, 240, 8, 0)
  text("DEPTH", 2, 2, 6)
  number(depth, 26, 2, 7)
  text("TORCH", 46, 2, 6)
  local bar = fuel
  if bar > 40 then bar = 40 end
  local bc = 9
  if fuel < LOW then bc = 8 end
  rect(70, 3, bar, 4, bc)

  if dead == 1 then
    light_rect(60, 106, 120, 28, 64, 64, 64)
    rect(60, 106, 120, 28, 0)
    text("DARK", 112, 112, 8)
    text("A - AGAIN", 102, 122, 6)
  end

  signal(fuel_left, fuel)
  signal(depth_now, depth)
  local n = 0
  i = 0
  while i < 4 do
    if wisps[i].alive == 1 then n = n + 1 end
    i = i + 1
  end
  signal(wisps_alive, n)
  signal(state, dead)
  entity(hx, hy, 1)
end
