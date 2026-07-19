-- snake.lua — classic snake on a 16x16 grid (8px cells). Arrows steer; eat the
-- red food to grow; hitting a wall or yourself ends the run (press A to restart).
--
--   kessel --play games/snake.lua

sprite seg {
  .bbbbbb.
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  .bbbbbb.
}

sprite food {
  ..8888..
  .888888.
  88888888
  88888888
  88888888
  88888888
  .888888.
  ..8888..
}

record Cell { x, y }

local body: array(64, Cell)   -- grid coords; body[0] is the head
local len = 3
local dx = 1                  -- heading; -1 is stored as (0-1) = 65535 (word)
local dy = 0
local fx = 12                 -- food cell
local fy = 8
local tick = 0
local dead = 0

function reset_game()
  body[0].x = 8  body[0].y = 8
  body[1].x = 7  body[1].y = 8
  body[2].x = 6  body[2].y = 8
  len = 3
  dx = 1  dy = 0
  fx = 12  fy = 8
  tick = 0
  dead = 0
end

function init()
  reset_game()
end

-- read arrows into (dx,dy), refusing an immediate 180° reversal
function steer()
  if btn(LEFT)  and dx ~= 1       then dx = 0 - 1  dy = 0 end
  if btn(RIGHT) and dx ~= (0 - 1) then dx = 1      dy = 0 end
  if btn(UP)    and dy ~= 1       then dx = 0  dy = 0 - 1 end
  if btn(DOWN)  and dy ~= (0 - 1) then dx = 0  dy = 1 end
end

function update()
  if dead == 1 then
    if btn(A) then reset_game() end
    return
  end

  steer()
  tick = tick + 1
  if tick % 6 ~= 0 then return end     -- step every 6 frames

  local nx = body[0].x + dx
  local ny = body[0].y + dy
  if nx >= 16 or ny >= 16 then dead = 1  return end   -- wall (also catches wrap)

  local i = 0
  while i < len do
    if body[i].x == nx and body[i].y == ny then dead = 1 end
    i = i + 1
  end
  if dead == 1 then return end

  local ate = 0
  if nx == fx and ny == fy then ate = 1 end
  if ate == 1 and len < 64 then len = len + 1 end

  -- shift the body toward the tail (descending, so we don't clobber)
  local j = len - 1
  while j >= 1 do
    body[j].x = body[j - 1].x
    body[j].y = body[j - 1].y
    j = j - 1
  end
  body[0].x = nx  body[0].y = ny

  if ate == 1 then
    fx = rnd(16)
    fy = rnd(16)
  end
end

function draw()
  cls(0)
  spr(food, fx * 8, fy * 8, 0)
  local i = 0
  while i < len do
    spr(seg, body[i].x * 8, body[i].y * 8, 0)
    i = i + 1
  end
  entity(body[0].x * 8, body[0].y * 8, 1)
end
