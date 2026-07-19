-- platform.lua — a tiny tile platformer: a tilemap level, gravity, solid-tile
-- collision, and a jump. Arrows move, A (Z key) jumps.
--
--   kessel --play games/platform.lua

sprite hero {
  ..7777..
  .777777.
  77777777
  77.77.77
  77777777
  .777777.
  ..7777..
  .77..77.
}

sprite block {
  55555555
  54444445
  54444445
  54444445
  54444445
  54444445
  54444445
  55555555
}

tilemap level(16, 16)

record Player { x, y, vy: int, grounded }
local p: Player

function init()
  fset(block, SOLID, 1)          -- the block tile is solid

  -- floor along the bottom row, plus a couple of platforms
  local i = 0
  while i < 16 do mset(i, 14, block)  i = i + 1 end
  mset(4, 11, block)  mset(5, 11, block)  mset(6, 11, block)
  mset(10, 9, block)  mset(11, 9, block)

  p.x = 16  p.y = 96  p.vy = 0  p.grounded = 0
end

function update()
  if btn(LEFT) then p.x = p.x - 1 end
  if btn(RIGHT) then p.x = p.x + 1 end

  if btn(A) and p.grounded == 1 then p.vy = 0 - 6 end   -- jump

  p.vy = p.vy + 1                 -- gravity
  if p.vy > 4 then p.vy = 4 end   -- terminal fall speed (signed compare)

  local ny = p.y + p.vy
  if p.vy > 0 and solid(p.x + 4, ny + 8) then
    p.vy = 0                      -- landed
    p.grounded = 1
  else
    p.grounded = 0
    p.y = ny
  end
end

function draw()
  cls(12)
  map(0, 0, 0, 0, 16, 16)
  spr(hero, p.x, p.y, 0)
  entity(p.x, p.y, 1)
end
