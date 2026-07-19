-- platform.lua — a tiny tile platformer: a tilemap level, gravity, solid-tile
-- collision, and a jump. Arrows move, A (Z key) jumps.
--
--   kessel --play games/platform.lua

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move
  a = "jump"
  pause = START
}

-- Tile id 0 fills untouched tilemap cells, so it must be background rather than
-- the player sprite.
sprite sky {
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
}

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

tilemap level(32, 16)

local WORLD_TILES = 32
local CAMERA_MAX = 128

-- y4 and vy use quarter-pixel units for a smoother, less abrupt jump arc.
record Player { x, y, y4: int, vy: int, grounded }
local p: Player
local cam_x: int = 0

function init()
  fset(block, SOLID, 1)          -- the block tile is solid

  -- A two-screen stage with a floor, boundary walls, and rising platforms.
  local i = 0
  while i < WORLD_TILES do mset(i, 14, block)  i = i + 1 end
  for y = 0, 13 do mset(0, y, block)  mset(WORLD_TILES - 1, y, block) end
  mset(4, 11, block)  mset(5, 11, block)  mset(6, 11, block)
  mset(10, 9, block)  mset(11, 9, block)
  mset(16, 11, block)  mset(17, 11, block)  mset(18, 11, block)
  mset(22, 8, block)   mset(23, 8, block)   mset(24, 8, block)
  mset(28, 10, block)  mset(29, 10, block)

  p.x = 16  p.y = 96  p.y4 = 96 * 4  p.vy = 0  p.grounded = 0
  cam_x = 0
end

function update()
  local dx: int = 0
  if btn(LEFT) then dx = 0 - 1 end
  if btn(RIGHT) then dx = 1 end
  p.x = collide_x(p.x, p.y, 8, 8, dx, SOLID)

  if btnp(A) and p.grounded == 1 then p.vy = 0 - 15 end   -- jump impulse

  p.vy = p.vy + 1                   -- 0.25 px/frame squared gravity
  if p.vy > 12 then p.vy = 12 end   -- 3 px/frame terminal fall speed

  local next_y4: int = p.y4 + p.vy
  local target_y: int = next_y4 / 4
  local dy: int = target_y - p.y
  local ny = collide_y(p.x, p.y, 8, 8, dy, SOLID)
  p.y = ny
  if ny ~= target_y then
    p.vy = 0
    p.y4 = ny * 4
  else
    p.y4 = next_y4
  end

  p.grounded = 0
  if p.vy >= 0 and touching_floor(p.x, p.y, 8, 8, SOLID) then
    p.vy = 0
    p.y4 = p.y * 4
    p.grounded = 1
  end

  -- Keep the player near the horizontal centre, clamped at both stage edges.
  cam_x = p.x - 60
  if cam_x < 0 then cam_x = 0 end
  if cam_x > CAMERA_MAX then cam_x = CAMERA_MAX end
end

function draw()
  cls(12)
  camera(cam_x, 0)
  map(0, 0, 0, 0, WORLD_TILES, 16)
  spr(hero, p.x, p.y, 0)
  entity(p.x, p.y, 1)
end
