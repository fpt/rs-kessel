-- sokoban.lua — the classic warehouse puzzle: push every box onto a goal. You
-- can push a single box but never pull one, and never push two at once, so a box
-- shoved into a corner is stuck.
--
--   kessel run games/sokoban.lua
--
-- Arrows move / push, one tile per press. A restarts the stage, or advances
-- after a clear. Boxes turn green when parked.
--
-- ## The levels
--
-- Twelve stages from **Microban**, by David W. Skinner, who released the set for
-- free use: <http://users.bentonrea.com/~sasquatch/sokoban/>. They are the
-- standard set for exactly this situation — small enough to fit one screen
-- without scrolling, and graded from "one push" to genuinely hard.
--
-- Every stage is guarded by `crates/vm/tests/sokoban_levels.rs`, which reads the
-- `data` blocks below and, for each, checks the room is sealed, the boxes and
-- goals balance, and a breadth-first search actually solves it. A puzzle nobody
-- has solved is not a level.
--
-- ## The level format
--
-- Each stage is a `data` block: a 12x12 grid of bytes, one character per cell,
-- in the same alphabet a `sprite` uses.
--
--   .  outside the room      3  box
--   1  floor                 4  goal
--   2  wall                  5  box already on a goal
--                            6  the player
--                            7  the player on a goal
--
-- `.` is not floor. It is the space *outside* the walls, and the guard checks
-- the player can never reach it — a room that leaks is a level with no bottom.
-- Stages are smaller than the grid and are centred on the screen at load time
-- from their own bounding box, so the padding never shows.

controls {
  dpad  = true      -- move / push
  a     = "restart / next"
  pause = START
}

-- Tiles double as the board's logical cells: the sprite NAME is its tile id, so
-- `mget(x,y) == wall` reads the board directly. Declaration order sets the ids
-- (void 0, floor 1, wall 2, target 3, box 4, boxt 5, player 6).
--
-- `void` is first so that id 0 — what an unwritten map cell holds — is the
-- outside, and it is drawn as nothing at all: the level's silhouette is the
-- black around it.
sprite void {
  ........
  ........
  ........
  ........
  ........
  ........
  ........
  ........
}
sprite floor {
  11111111
  11111111
  11111111
  11111111
  11111111
  11111111
  11111111
  11111111
}
sprite wall {
  66666666
  65555556
  65555556
  65555556
  65555556
  65555556
  65555556
  66666666
}
sprite target {
  11111111
  11111111
  111aa111
  11a11a11
  11a11a11
  111aa111
  11111111
  11111111
}
sprite box {
  44444444
  49999994
  49444494
  49444494
  49444494
  49444494
  49999994
  44444444
}
sprite boxt {
  bbbbbbbb
  b444444b
  b4bbbb4b
  b4bbbb4b
  b4bbbb4b
  b4bbbb4b
  b444444b
  bbbbbbbb
}
sprite player {
  ..7777..
  .777777.
  .7e77e7.
  .777777.
  ..8888..
  .888888.
  .8....8.
  ........
}

tilemap board(12, 12)

data stage1 {
  2222........
  2142........
  211222......
  256112......
  211312......
  211222......
  2222........
  ............
  ............
  ............
  ............
  ............
}

data stage2 {
  222222......
  211112......
  212612......
  213512......
  214512......
  211112......
  222222......
  ............
  ............
  ............
  ............
  ............
}

data stage3 {
  ..2222......
  222112222...
  211111312...
  212112312...
  214142612...
  222222222...
  ............
  ............
  ............
  ............
  ............
  ............
}

data stage4 {
  22222222....
  21111112....
  21455362....
  21111112....
  22222112....
  ....2222....
  ............
  ............
  ............
  ............
  ............
  ............
}

data stage5 {
  .2222222....
  .2111112....
  .2143412....
  22136312....
  21143412....
  21111112....
  22222222....
  ............
  ............
  ............
  ............
  ............
}

data stage6 {
  222222.22222
  211112221112
  213311111262
  213124441112
  211122222222
  22222.......
  ............
  ............
  ............
  ............
  ............
  ............
}

data stage7 {
  2222222.....
  2111112.....
  2143412.....
  2134312.....
  2143412.....
  2134312.....
  2116112.....
  2222222.....
  ............
  ............
  ............
  ............
}

data stage8 {
  ..222222....
  ..214462....
  ..213312....
  ..221222....
  ...212......
  ...212......
  222212......
  2111122.....
  2121112.....
  2111212.....
  2221112.....
  ..22222.....
}

data stage9 {
  22222.......
  241122......
  263312......
  221112......
  .22112......
  ..2242......
  ...222......
  ............
  ............
  ............
  ............
  ............
}

data stage10 {
  ......22222.
  ......24112.
  ......24212.
  22222224212.
  21613131312.
  21212121222.
  211111112...
  222222222...
  ............
  ............
  ............
  ............
}

data stage11 {
  ..222222....
  ..211112....
  ..2122622...
  222121312...
  214421312...
  211111112...
  211222222...
  2222........
  ............
  ............
  ............
  ............
}

data stage12 {
  22222.......
  211122......
  213112......
  221312222...
  .22264112...
  ..2114212...
  ..2111112...
  ..2222222...
  ............
  ............
  ............
  ............
}

local W = 12          -- the data grid; every stage is padded to it
local H = 12
local STAGES = 12
local BAR = 8         -- the HUD bar owns the top 8 px and nothing else
-- The board draws at 2x. A Microban stage is at most 12 tiles across, which is
-- 96 px of a 240-px screen drawn 1:1 — a postage stamp with a wide black
-- margin. The level data is transcribed and must not change to suit the
-- screen, so the *presentation* scales instead: `spr_scaled` at 512 (8.8
-- fixed, 2x) and a 16-px pitch everywhere a cell is placed.
local CELL = 16

local px = 0          -- player tile position, in grid coordinates
local py = 0
local moves = 0
local won = 0
local stage = 1

-- Where the stage's own bounding box starts, and where it lands on screen.
local bx = 0
local by = 0
local bw = 1
local bh = 1
local ox = 0
local oy = 0

signal stage_now
signal moves_now
signal left            -- boxes still off a goal
signal cleared

function stage_data(n)
  if n == 1 then return stage1 end
  if n == 2 then return stage2 end
  if n == 3 then return stage3 end
  if n == 4 then return stage4 end
  if n == 5 then return stage5 end
  if n == 6 then return stage6 end
  if n == 7 then return stage7 end
  if n == 8 then return stage8 end
  if n == 9 then return stage9 end
  if n == 10 then return stage10 end
  if n == 11 then return stage11 end
  return stage12
end

function load_stage(n)
  local src = stage_data(n)
  for y = 0, H - 1 do
    for x = 0, W - 1 do
      local c = peek(src + y * W + x)
      if c == 2 then
        mset(x, y, wall)
      elseif c == 3 then
        mset(x, y, box)
      elseif c == 4 then
        mset(x, y, target)
      elseif c == 5 then
        mset(x, y, boxt)
      elseif c == 6 then
        mset(x, y, floor)
        px = x  py = y
      elseif c == 7 then
        mset(x, y, target)
        px = x  py = y
      elseif c == 1 then
        mset(x, y, floor)
      else
        mset(x, y, void)
      end
    end
  end

  -- Centre the stage on what it actually occupies, not on the padded grid.
  local x0 = W - 1
  local y0 = H - 1
  local x1 = 0
  local y1 = 0
  for y = 0, H - 1 do
    for x = 0, W - 1 do
      if mget(x, y) ~= void then
        if x < x0 then x0 = x end
        if y < y0 then y0 = y end
        if x > x1 then x1 = x end
        if y > y1 then y1 = y end
      end
    end
  end
  bx = x0  by = y0
  bw = x1 - x0 + 1
  bh = y1 - y0 + 1
  ox = (240 - bw * CELL) / 2
  oy = BAR + (240 - BAR - bh * CELL) / 2

  stage = n
  moves = 0
  won = 0
end

function init()
  load_stage(1)
end

-- Resolve a step by (dx,dy): walk onto floor/goal, push a single box if the cell
-- beyond it is clear, or stay put against a wall, the outside, or a stuck box.
function try_move(dx: int, dy: int)
  local nx = px + dx
  local ny = py + dy
  local ncell = mget(nx, ny)

  if ncell == wall or ncell == void then return end

  if ncell == box or ncell == boxt then
    local tx = nx + dx
    local ty = ny + dy
    local tcell = mget(tx, ty)
    if tcell == floor or tcell == target then
      -- Box slides forward (green when it lands on a goal)...
      if tcell == target then mset(tx, ty, boxt) else mset(tx, ty, box) end
      -- ...and the cell it left reverts to goal or plain floor.
      if ncell == boxt then mset(nx, ny, target) else mset(nx, ny, floor) end
      px = nx  py = ny
      moves = moves + 1
      check_win()
    end
    return
  end

  px = nx  py = ny
  moves = moves + 1
end

function boxes_left()
  local n = 0
  for y = 0, H - 1 do
    for x = 0, W - 1 do
      if mget(x, y) == box then n = n + 1 end   -- a box still off a goal
    end
  end
  return n
end

function check_win()
  if boxes_left() == 0 then won = 1 end
end

function update()
  if won == 1 then
    if btnp(A) then
      if stage < STAGES then load_stage(stage + 1) else load_stage(1) end
    end
    return
  end

  if btnp(A) then load_stage(stage)  return end

  local dx: int = 0
  local dy: int = 0
  if btnp(LEFT)  then dx = 0 - 1 end
  if btnp(RIGHT) then dx = 1 end
  if btnp(UP)    then dy = 0 - 1 end
  if btnp(DOWN)  then dy = 1 end
  if dx == 0 and dy == 0 then return end
  try_move(dx, dy)
end

function draw()
  cls(0)
  -- `map` draws the tilemap 1:1, so the board is walked a cell at a time and
  -- each tile blown up instead.
  for y = 0, bh - 1 do
    for x = 0, bw - 1 do
      spr_scaled(mget(bx + x, by + y), ox + x * CELL, oy + y * CELL, 512, 0)
    end
  end
  spr_scaled(player, ox + (px - bx) * CELL, oy + (py - by) * CELL, 512, 0)

  rect(0, 0, 240, BAR, 0)
  text("STAGE", 2, 2, 6)
  number(stage, 26, 2, 10)
  text("MOVES", 60, 2, 6)
  number(moves, 86, 2, 10)
  if won == 1 then
    rect(60, 106, 120, 28, 0)
    if stage < STAGES then
      text("STAGE CLEAR", 88, 112, 11)
    else
      text("ALL CLEAR", 96, 112, 11)
    end
    text("PRESS A", 106, 122, 7)
  end

  -- ---- light ---------------------------------------------------------------
  -- A puzzle has to stay readable, so this is mood rather than fog: the board
  -- sits just under neutral and the lights only pick out what matters. See
  -- docs/VM_GRAPHICS.md ("Light"); `games/lantern.lua` is the dark end of the
  -- same calls.

  ambient(33, 32, 41)                       -- a cold warehouse, one lamp lit

  -- Walls, crates and the outside all stop light — which is the one hint this
  -- puzzle gives for free: a box you have shoved into a corner casts into it,
  -- and the dead space behind it is the dead space you just made.
  for y = 0, bh - 1 do
    for x = 0, bw - 1 do
      local t = mget(bx + x, by + y)
      if t == wall or t == box or t == boxt or t == void then
        shadow_rect(ox + x * CELL, oy + y * CELL, CELL, CELL)
      end
    end
  end

  -- Goals glow amber and parked boxes green, which is the state the player is
  -- actually tracking. A box on a goal already changes sprite; the light is what
  -- makes the count readable at a glance instead of tile by tile.
  for y = 0, bh - 1 do
    for x = 0, bw - 1 do
      local t = mget(bx + x, by + y)
      if t == target then
        light(ox + x * CELL + 8, oy + y * CELL + 8, 20, 30, 15, 4)
      elseif t == boxt then
        light(ox + x * CELL + 8, oy + y * CELL + 8, 22, 5, 28, 14)
      end
    end
  end

  light(ox + (px - bx) * CELL + 8, oy + (py - by) * CELL + 8, 60, 30, 25, 13)

  light_rect(0, 0, 240, BAR, 64, 64, 64)
  if won == 1 then light_rect(60, 106, 120, 28, 64, 64, 64) end

  signal(stage_now, stage)
  signal(moves_now, moves)
  signal(left, boxes_left())
  signal(cleared, won)
  entity(px, py, stage)      -- report player position and active stage
end
