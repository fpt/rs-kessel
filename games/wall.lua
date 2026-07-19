-- wall.lua — a platformer that shows off the phase-2 collision helpers and
-- edge input: tilemap collision RESOLUTION (collide_x/collide_y instead of a
-- hand-rolled solid() probe), contact predicates (touching_*), and jumps driven
-- by btnp so a held button can't auto-repeat.
--
--   kessel --play games/wall.lua
--
-- Arrows move. A (Z key) jumps — press again in mid-air for a double-jump, or
-- press while sliding down a wall to wall-jump off it.

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move
  a = "jump"        -- press again mid-air / against a wall for double / wall-jump
  pause = START
}

sprite hero {
  ..9999..
  .999999.
  99999999
  99.99.99
  99999999
  .999999.
  ..9999..
  .99..99.
}

sprite block {
  33333333
  31111113
  31111113
  31111113
  31111113
  31111113
  31111113
  33333333
}

tilemap level(16, 16)

record Player { x, y, vx: int, vy: int, jumps }
local p: Player
local W = 8            -- the player's collision box is one tile
local H = 8

function init()
  fset(block, SOLID, 1)

  -- floor along the bottom, plus two facing walls to bounce between and a ledge.
  for i = 0, 15 do mset(i, 15, block) end
  for j = 6, 14 do mset(3, j, block)  mset(12, j, block) end
  mset(7, 11, block)  mset(8, 11, block)

  p.x = 48  p.y = 40  p.vx = 0  p.vy = 0  p.jumps = 2
end

function update()
  -- Horizontal: resolve movement against the map (stops flush at walls).
  p.vx = 0
  if btn(LEFT)  then p.vx = 0 - 2 end
  if btn(RIGHT) then p.vx = 2 end
  p.x = collide_x(p.x, p.y, W, H, p.vx, SOLID)

  -- Vertical: gravity, then resolve. If the resolved y differs from the naive
  -- target we hit a floor or ceiling, so cancel the vertical velocity.
  p.vy = p.vy + 1
  if p.vy > 5 then p.vy = 5 end
  local ty = p.y + p.vy
  local ny = collide_y(p.x, p.y, W, H, p.vy, SOLID)
  if ny ~= ty then p.vy = 0 end
  p.y = ny

  -- Contact predicates drive the jump rules.
  local grounded = touching_floor(p.x, p.y, W, H, SOLID)
  local on_left  = touching_left(p.x, p.y, W, H, SOLID)
  local on_right = touching_right(p.x, p.y, W, H, SOLID)
  if grounded == 1 then p.jumps = 2 end

  -- btnp: fire once per press, never while held.
  if btnp(A) then
    if p.jumps > 0 then
      p.vy = 0 - 6            -- ground jump or mid-air double-jump
      p.jumps = p.jumps - 1
    elseif on_left == 1 then
      p.vy = 0 - 6            -- wall-jump off a wall on the left: launch right
      p.x = collide_x(p.x, p.y, W, H, 3, SOLID)
    elseif on_right == 1 then
      p.vy = 0 - 6            -- ...or off a wall on the right: launch left
      p.x = collide_x(p.x, p.y, W, H, 0 - 3, SOLID)
    end
  end
end

function draw()
  cls(1)
  map(0, 0, 0, 0, 16, 16)
  spr(hero, p.x, p.y, 0)
  entity(p.x, p.y, 1)
end
