-- mover — move a 2x2 block with the arrow keys (Z/X/Return/Space are A/B/
-- Start/Select). Reports itself as entity tag 1 for observation.
--
--   kessel --play games/mover/game.lua

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move
  pause = START
}

record P { x, y }
local p: P

function init()
  p.x = 60  p.y = 60
end

function update()
  if btn(LEFT) then p.x = p.x - 1 end
  if btn(RIGHT) then p.x = p.x + 1 end
  if btn(UP) then p.y = p.y - 1 end
  if btn(DOWN) then p.y = p.y + 1 end
end

function draw()
  cls(0)
  pset(p.x, p.y, 11)             -- green 2x2 block
  pset(p.x + 1, p.y, 11)
  pset(p.x, p.y + 1, 11)
  pset(p.x + 1, p.y + 1, 11)
  entity(p.x, p.y, 1)
end
