-- tetris.lua — a compact but real Tetris on a 10x15 well. Left/Right move,
-- A/B (Z/X keys) rotate clockwise/counterclockwise, Down soft-drops. Full rows
-- clear. Top out and press A to restart.
--
--   kessel --play games/tetris.lua
--
-- Pieces are 4x4 bitmasks (bit b => cell row b/4, col b%4) rotated at runtime
-- around a stable, piece-specific origin.
-- The well is a tilemap: cell 0 = empty (sprite id 0, an opaque dark tile so the
-- well is visible), cells 1-7 = coloured blocks matching each piece kind.

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- left/right move, down soft-drops
  a = "rotate cw"
  b = "rotate ccw"
  pause = START
}

sprite empty {
  11111111
  11111111
  11111111
  11111111
  11111111
  11111111
  11111111
  11111111
}

sprite block_i {
  77777777
  76666666
  76666666
  76666666
  76666666
  76666666
  76666666
  66666666
}

sprite block_o {
  77777777
  7aaaaaaa
  7aaaaaaa
  7aaaaaaa
  7aaaaaaa
  7aaaaaaa
  7aaaaaaa
  aaaaaaaa
}

sprite block_t {
  77777777
  7ddddddd
  7ddddddd
  7ddddddd
  7ddddddd
  7ddddddd
  7ddddddd
  dddddddd
}

sprite block_s {
  77777777
  7bbbbbbb
  7bbbbbbb
  7bbbbbbb
  7bbbbbbb
  7bbbbbbb
  7bbbbbbb
  bbbbbbbb
}

sprite block_z {
  77777777
  78888888
  78888888
  78888888
  78888888
  78888888
  78888888
  88888888
}

sprite block_j {
  77777777
  7ccccccc
  7ccccccc
  7ccccccc
  7ccccccc
  7ccccccc
  7ccccccc
  cccccccc
}

sprite block_l {
  77777777
  79999999
  79999999
  79999999
  79999999
  79999999
  79999999
  99999999
}

tilemap well(10, 15)

local BW = 10
local BH = 15
local OX = 4         -- leave room for the score HUD to the right of the well
local OY = 4

local shape: array(7, word)   -- the 7 tetromino spawn masks
local cur_kind = 0            -- index into shape; also selects the rotation origin
local next_kind = 0           -- queued piece shown in the HUD
local cur = 0                 -- current piece mask
local px: int = 3             -- 4x4 box top-left in board cells
local py: int = 0
local gtick = 0
local mcd = 0                 -- move/rotate cooldown
local dead = 0
local score = 0
local lines = 0

-- Sprite declarations are ordered to match the shape array.
function piece_tile(kind)
  return kind + 1
end

-- Gravity starts gently, then gains one speed level per 500 points.
function drop_delay()
  -- level is non-negative (score/500), so unsigned min() clamps it safely.
  local level = min(score / 500, 7)
  return 36 - level * 4
end

-- 1 if the piece `mask` placed with its box at (ox,oy) hits a wall/floor/block
function collision(mask, ox, oy)
  local b = 0
  while b < 16 do
    if ((mask >> b) & 1) == 1 then
      local x = ox + b % 4
      local y = oy + b / 4
      if x < 0 or x >= BW then return 1 end
      if y >= BH then return 1 end
      if y >= 0 and mget(x, y) ~= 0 then return 1 end
    end
    b = b + 1
  end
  return 0
end

-- Rotation origins use coordinates doubled so cell and half-cell pivots are both
-- exact: I rotates around (1.5,1.5), O around (1.5,0.5), all others around (1,1).
function pivot_x2(kind)
  if kind == 0 or kind == 1 then return 3 end
  return 2
end

function pivot_y2(kind)
  if kind == 0 then return 3 end
  if kind == 1 then return 1 end
  return 2
end

-- Rotate each occupied cell 90 degrees clockwise around the piece's own origin:
-- translate to the origin, (x,y) -> (-y,x), then translate back.
function rotate_cw(mask, kind)
  local out = 0
  local ox2: int = pivot_x2(kind)
  local oy2: int = pivot_y2(kind)
  local b = 0
  while b < 16 do
    if ((mask >> b) & 1) == 1 then
      local x2: int = (b % 4) * 2
      local y2: int = (b / 4) * 2
      local nx: int = (ox2 - (y2 - oy2)) / 2
      local ny: int = (oy2 + (x2 - ox2)) / 2
      local nb = ny * 4 + nx
      out = out | (1 << nb)
    end
    b = b + 1
  end
  return out
end

-- Rotate 90 degrees counterclockwise using the inverse transform:
-- translate to the origin, (x,y) -> (y,-x), then translate back.
function rotate_ccw(mask, kind)
  local out = 0
  local ox2: int = pivot_x2(kind)
  local oy2: int = pivot_y2(kind)
  local b = 0
  while b < 16 do
    if ((mask >> b) & 1) == 1 then
      local x2: int = (b % 4) * 2
      local y2: int = (b / 4) * 2
      local nx: int = (ox2 + (y2 - oy2)) / 2
      local ny: int = (oy2 - (x2 - ox2)) / 2
      local nb = ny * 4 + nx
      out = out | (1 << nb)
    end
    b = b + 1
  end
  return out
end

function lock_piece()
  local b = 0
  while b < 16 do
    if ((cur >> b) & 1) == 1 then
      local x = px + b % 4
      local y = py + b / 4
      if y >= 0 then mset(x, y, piece_tile(cur_kind)) end
    end
    b = b + 1
  end
end

-- clear any full rows, collapsing everything above them down
function clear_lines()
  local cleared = 0
  local y: int = BH - 1
  while y >= 0 do
    local full = 1
    local x = 0
    while x < BW do
      if mget(x, y) == 0 then full = 0 end
      x = x + 1
    end
    if full == 1 then
      local yy: int = y
      while yy > 0 do
        local xx = 0
        while xx < BW do
          mset(xx, yy, mget(xx, yy - 1))
          xx = xx + 1
        end
        yy = yy - 1
      end
      local xt = 0
      while xt < BW do mset(xt, 0, 0)  xt = xt + 1 end
      cleared = cleared + 1
      -- re-test this same row (do not decrement y)
    else
      y = y - 1
    end
  end
  return cleared
end

function spawn()
  cur_kind = next_kind
  cur = shape[cur_kind]
  next_kind = rnd(7)
  px = 3
  py = 0
  if collision(cur, px, py) == 1 then dead = 1 end
end

function clear_board()
  local y = 0
  while y < BH do
    local x = 0
    while x < BW do mset(x, y, 0)  x = x + 1 end
    y = y + 1
  end
end

function restart()
  clear_board()
  dead = 0
  gtick = 0
  mcd = 0
  score = 0
  lines = 0
  next_kind = rnd(7)
  spawn()
end

function init()
  shape[0] = 0x00f0    -- I
  shape[1] = 0x0066    -- O
  shape[2] = 0x0072    -- T
  shape[3] = 0x0036    -- S
  shape[4] = 0x0063    -- Z
  shape[5] = 0x0071    -- J
  shape[6] = 0x0074    -- L
  restart()
end

function update()
  if dead == 1 then
    if btnp(A) then restart() end
    return
  end

  if mcd > 0 then mcd = mcd - 1 end
  if mcd == 0 then
    local acted = 0
    if btn(LEFT)  and collision(cur, px - 1, py) == 0 then px = px - 1  acted = 1 end
    if btn(RIGHT) and collision(cur, px + 1, py) == 0 then px = px + 1  acted = 1 end
    if acted == 1 then mcd = 6 end
  end

  -- Rotation is edge-triggered: one button press produces one quarter-turn.
  if btnp(A) then
    local r = rotate_cw(cur, cur_kind)
    if collision(r, px, py) == 0 then cur = r end
  elseif btnp(B) then
    local r = rotate_ccw(cur, cur_kind)
    if collision(r, px, py) == 0 then cur = r end
  end

  local step = 0
  gtick = gtick + 1
  if gtick >= drop_delay() then gtick = 0  step = 1 end
  if btn(DOWN) and gtick % 4 == 0 then step = 1 end

  if step == 1 then
    if collision(cur, px, py + 1) == 0 then
      py = py + 1
    else
      lock_piece()
      local cleared = clear_lines()
      lines = lines + cleared
      if cleared == 1 then score = score + 100 end
      if cleared == 2 then score = score + 300 end
      if cleared == 3 then score = score + 500 end
      if cleared == 4 then score = score + 800 end
      spawn()
    end
  end
end

function draw()
  cls(0)
  map(0, 0, OX, OY, BW, BH)
  local b = 0
  while b < 16 do
    if ((cur >> b) & 1) == 1 then
      local cx = px + b % 4
      local cy = py + b / 4
      if cy >= 0 then spr(piece_tile(cur_kind), OX + cx * 8, OY + cy * 8, 0) end
    end
    b = b + 1
  end
  text("SCORE", 88, 8, 7)
  number(score, 88, 15, 10)
  text("LINES", 88, 28, 7)
  number(lines, 88, 35, 11)
  text("NEXT", 88, 48, 7)
  local next_mask = shape[next_kind]
  local preview_x = 92
  local preview_y = 58
  if next_kind == 0 then preview_x = 88  preview_y = 50 end
  if next_kind == 1 then preview_x = 88 end
  local n = 0
  while n < 16 do
    if ((next_mask >> n) & 1) == 1 then
      spr(piece_tile(next_kind), preview_x + (n % 4) * 8, preview_y + (n / 4) * 8, 0)
    end
    n = n + 1
  end
  if dead == 1 then
    text("GAME OVER", 88, 96, 8)
    text("PRESS A", 88, 104, 7)
  end
  entity(OX + px * 8, OY + py * 8, 1)
end
