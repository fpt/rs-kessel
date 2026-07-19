-- sokoban.lua — the classic warehouse puzzle: push every box ($) onto a target
-- (goal) tile. You can push a single box but never pull one, and never push two
-- at once, so a box shoved into a corner is stuck. Grid movement, one tile per
-- key press (btnp), so a held key doesn't skate across the board.
--
--   kessel --play games/sokoban.lua
--
-- Arrows move / push. Boxes turn green when parked on a goal; clear them all to
-- win. (A/Z restarts once you've won or wedged yourself.)

-- Tiles double as the board's logical cells: the sprite NAME is its tile id, so
-- `mget(x,y) == wall` reads the board directly. Declaration order sets the ids
-- (floor 0, wall 1, target 2, box 3, boxt 4, player 5).
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

tilemap board(8, 8)

local px = 3          -- player tile position
local py = 4
local moves = 0
local won = 0

local OX = 32         -- screen offset that centres the 8x8 board (128 = 16 tiles)
local OY = 32

function init()
  -- Frame the arena in walls, floor inside.
  for y = 0, 7 do
    for x = 0, 7 do
      if x == 0 or x == 7 or y == 0 or y == 7 then
        mset(x, y, wall)
      else
        mset(x, y, floor)
      end
    end
  end
  -- Two goals with a box just below each; the player nudges them up.
  mset(2, 2, target)  mset(5, 2, target)
  mset(2, 3, box)     mset(5, 3, box)
  px = 3  py = 4
  moves = 0
  won = 0
end

-- Resolve a step by (dx,dy): walk into floor/target, push a single box if the
-- cell beyond it is clear, or stay put against a wall / stuck box.
function try_move(dx: int, dy: int)
  local nx = px + dx
  local ny = py + dy
  local ncell = mget(nx, ny)

  if ncell == wall then return end

  if ncell == box or ncell == boxt then
    local bx = nx + dx
    local by = ny + dy
    local bcell = mget(bx, by)
    if bcell == floor or bcell == target then
      -- Box slides forward (green when it lands on a goal)...
      if bcell == target then mset(bx, by, boxt) else mset(bx, by, box) end
      -- ...and the cell it left reverts to goal or plain floor.
      if ncell == boxt then mset(nx, ny, target) else mset(nx, ny, floor) end
      px = nx  py = ny
      moves = moves + 1
      check_win()
    end
    return
  end

  -- Plain floor or an empty goal: just walk onto it.
  px = nx  py = ny
  moves = moves + 1
end

function check_win()
  local left = 0
  for y = 0, 7 do
    for x = 0, 7 do
      if mget(x, y) == box then left = left + 1 end   -- a box still off a goal
    end
  end
  if left == 0 then won = 1 end
end

function update()
  if won == 1 then
    if btnp(A) then init() end       -- play again
    return
  end

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
  map(0, 0, OX, OY, 8, 8)
  spr(player, OX + px * 8, OY + py * 8, 0)

  text("SOKOBAN", 34, 10, 7)
  text("MOVES", 34, 110, 6)
  number(moves, 78, 110, 6)
  if won == 1 then text("YOU WIN", 44, 60, 11) end

  entity(px, py, 1)      -- report the player for observation
end
