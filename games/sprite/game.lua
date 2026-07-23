-- sprite — move a hand-drawn sprite with the arrow keys. Shows off the
-- `sprite { … }` declaration and id-indexed `spr(id, x, y, flags)`.
--
--   kessel --play games/sprite/game.lua

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move the sprite
  pause = START
}

sprite face {
  .aaaaaa.
  a999999a
  a9a99a9a
  a999999a
  a9a99a9a
  a99aa99a
  a999999a
  .aaaaaa.
}

local x = 60
local y = 60

function update()
  if btn(LEFT) then x = x - 1 end
  if btn(RIGHT) then x = x + 1 end
  if btn(UP) then y = y - 1 end
  if btn(DOWN) then y = y + 1 end
end

function draw()
  cls(1)
  spr(face, x, y, 0)
  entity(x, y, 1)
end
