-- brick.lua — a Breakout / brick-breaker. Arrows move the paddle; bounce the
-- ball to clear every brick. Miss it and the ball respawns from the centre.
--
--   kessel --play games/brick.lua

sprite ballspr {
  ..7777..
  .777777.
  77777777
  77777777
  77777777
  77777777
  .777777.
  ..7777..
}

sprite pad {
  cccccccc
  cccccccc
  cccccccc
  ........
  ........
  ........
  ........
  ........
}

sprite brick {
  99999999
  9aaaaaa9
  9aaaaaa9
  99999999
  ........
  ........
  ........
  ........
}

local BCOLS = 12
local BROWS = 5
local BX0 = 8
local BY0 = 16
local PADY = 118
local PADW = 24     -- 3 tiles wide

local alive: array(60, byte)    -- BCOLS*BROWS brick flags
local padx: int = 52
local bx: int = 60              -- ball position (signed so it can go < 0 briefly)
local by: int = 90
local vx: int = 1
local vy: int = 0 - 1

function launch_ball()
  bx = 60  by = 90  vx = 1  vy = 0 - 1
end

function init()
  local i = 0
  while i < 60 do alive[i] = 1  i = i + 1 end
  padx = 52
  launch_ball()
end

function update()
  if btn(LEFT)  then padx = padx - 2 end
  if btn(RIGHT) then padx = padx + 2 end
  if padx < 0 then padx = 0 end
  if padx > 128 - PADW then padx = 128 - PADW end

  bx = bx + vx
  by = by + vy

  if bx <= 0 then bx = 0  vx = 0 - vx end
  if bx >= 120 then bx = 120  vx = 0 - vx end
  if by <= 0 then by = 0  vy = 0 - vy end

  -- paddle bounce
  if vy > 0 and by + 8 >= PADY and by + 4 <= PADY and bx + 4 >= padx and bx + 4 <= padx + PADW then
    vy = 0 - vy
    by = PADY - 8
  end

  -- brick collision (guard the region before dividing)
  if bx + 4 >= BX0 and by + 4 >= BY0 then
    local col = (bx + 4 - BX0) / 8
    local row = (by + 4 - BY0) / 8
    if col < BCOLS and row < BROWS then
      local idx = row * BCOLS + col
      if alive[idx] == 1 then
        alive[idx] = 0
        vy = 0 - vy
      end
    end
  end

  if by >= 128 then launch_ball() end
end

function draw()
  cls(1)
  local row = 0
  while row < BROWS do
    local col = 0
    while col < BCOLS do
      if alive[row * BCOLS + col] == 1 then
        spr(brick, BX0 + col * 8, BY0 + row * 8, 0)
      end
      col = col + 1
    end
    row = row + 1
  end

  spr(pad, padx, PADY, 0)
  spr(pad, padx + 8, PADY, 0)
  spr(pad, padx + 16, PADY, 0)

  spr(ballspr, bx, by, 0)
  entity(bx, by, 1)
end
