-- bounce.lua — a self-animating demo (no input): a 3x3 block bouncing around.
-- Good for verifying the play window renders and animates.
--
--   kessel --play games/bounce.lua

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = false      -- self-animating demo, no input
  pause = START
}

record Ball { x, y, vx, vy }
local b: Ball

function init()
  b.x = 20  b.y = 30  b.vx = 1  b.vy = 1
end

function update()
  b.x = b.x + b.vx
  b.y = b.y + b.vy
  -- vx/vy are u16; "0 - v" wraps to the negative step, so the block reverses.
  if b.x >= 118 then b.vx = 0 - b.vx end
  if b.x <= 2 then b.vx = 0 - b.vx end
  if b.y >= 118 then b.vy = 0 - b.vy end
  if b.y <= 2 then b.vy = 0 - b.vy end
end

function draw()
  cls(1)                         -- dark-blue background
  for dy = 0, 2 do
    for dx = 0, 2 do
      pset(b.x + dx, b.y + dy, 8) -- a 3x3 red block
    end
  end
end
