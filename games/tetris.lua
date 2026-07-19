-- tetris.lua — a compact but real Tetris on a 10x15 well. Left/Right move, A
-- (Z key) rotates, Down soft-drops. Full rows clear. Top out and press A to
-- restart.
--
--   kessel --play games/tetris.lua
--
-- Pieces are 4x4 bitmasks (bit b => cell row b/4, col b%4) rotated at runtime.
-- The well is a tilemap: cell 0 = empty (sprite id 0, an opaque dark tile so the
-- well is visible), cell 1 = a locked/active block (sprite id 1).

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- left/right move, down soft-drops
  a = "rotate"
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

sprite block {
  cccccccc
  c6666666
  c6666666
  c6666666
  c6666666
  c6666666
  c6666666
  66666666
}

tilemap well(10, 15)

local BW = 10
local BH = 15
local OX = 24        -- screen offset of the well (10*8 = 80 wide, centred)
local OY = 4

local shape: array(7, word)   -- the 7 tetromino spawn masks
local cur = 0                 -- current piece mask
local px: int = 3             -- 4x4 box top-left in board cells
local py: int = 0
local gtick = 0
local mcd = 0                 -- move/rotate cooldown
local dead = 0

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

-- rotate a 4x4 mask 90° clockwise: cell (r,c) -> (c, 3-r)
function rotate(mask)
  local out = 0
  local b = 0
  while b < 16 do
    if ((mask >> b) & 1) == 1 then
      local r = b / 4
      local c = b % 4
      local nb = c * 4 + (3 - r)
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
      if y >= 0 then mset(x, y, block) end
    end
    b = b + 1
  end
end

-- clear any full rows, collapsing everything above them down
function clear_lines()
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
      -- re-test this same row (do not decrement y)
    else
      y = y - 1
    end
  end
end

function spawn()
  cur = shape[rnd(7)]
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
    if btn(A) then restart() end
    return
  end

  if mcd > 0 then mcd = mcd - 1 end
  if mcd == 0 then
    local acted = 0
    if btn(LEFT)  and collision(cur, px - 1, py) == 0 then px = px - 1  acted = 1 end
    if btn(RIGHT) and collision(cur, px + 1, py) == 0 then px = px + 1  acted = 1 end
    if btn(A) then
      local r = rotate(cur)
      if collision(r, px, py) == 0 then cur = r  acted = 1 end
    end
    if acted == 1 then mcd = 6 end
  end

  local step = 0
  gtick = gtick + 1
  if gtick % 24 == 0 then step = 1 end
  if btn(DOWN) and gtick % 4 == 0 then step = 1 end

  if step == 1 then
    if collision(cur, px, py + 1) == 0 then
      py = py + 1
    else
      lock_piece()
      clear_lines()
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
      if cy >= 0 then spr(block, OX + cx * 8, OY + cy * 8, 0) end
    end
    b = b + 1
  end
  entity(OX + px * 8, OY + py * 8, 1)
end
