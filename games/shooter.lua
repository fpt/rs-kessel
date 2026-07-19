-- shooter.lua — a vertical space shooter. Arrows move, A (Z key) fires. Bullets
-- destroy the descending foes; each kill scores a point.
--
--   kessel --play games/shooter.lua

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move
  a = "fire"
  pause = START
}

sprite ship {
  ...cc...
  ...cc...
  ..cccc..
  .cccccc.
  cccccccc
  c.cccc.c
  c......c
  ........
}

sprite bull {
  ...aa...
  ...aa...
  ...aa...
  ...aa...
  ........
  ........
  ........
  ........
}

sprite foe {
  ........
  .8....8.
  .888888.
  88888888
  8.8888.8
  88888888
  .8.88.8.
  ........
}

record Obj { x, y, alive }

local bullets: array(6, Obj)
local foes: array(6, Obj)
local px: int = 60
local cd = 0        -- fire cooldown
local spawn = 0     -- foe spawn timer
local score = 0

function init()
  local i = 0
  while i < 6 do
    bullets[i].alive = 0
    foes[i].alive = 0
    i = i + 1
  end
  px = 60
  cd = 0
  spawn = 0
  score = 0
end

function fire()
  local i = 0
  while i < 6 do
    if bullets[i].alive == 0 then
      bullets[i].x = px + 3
      bullets[i].y = 112
      bullets[i].alive = 1
      return
    end
    i = i + 1
  end
end

function spawn_foe()
  local i = 0
  while i < 6 do
    if foes[i].alive == 0 then
      foes[i].x = rnd(120)
      foes[i].y = 0
      foes[i].alive = 1
      return
    end
    i = i + 1
  end
end

function update()
  if btn(LEFT)  then px = px - 2 end
  if btn(RIGHT) then px = px + 2 end
  if px < 0 then px = 0 end
  if px > 120 then px = 120 end

  if cd > 0 then cd = cd - 1 end
  if btn(A) and cd == 0 then fire()  cd = 8 end

  -- move bullets up
  local i = 0
  while i < 6 do
    if bullets[i].alive == 1 then
      if bullets[i].y < 3 then
        bullets[i].alive = 0
      else
        bullets[i].y = bullets[i].y - 3
      end
    end
    i = i + 1
  end

  -- spawn + advance foes
  spawn = spawn + 1
  if spawn % 30 == 0 then spawn_foe() end
  i = 0
  while i < 6 do
    if foes[i].alive == 1 then
      foes[i].y = foes[i].y + 1
      if foes[i].y >= 128 then foes[i].alive = 0 end
    end
    i = i + 1
  end

  -- bullet vs foe
  i = 0
  while i < 6 do
    if bullets[i].alive == 1 then
      local j = 0
      while j < 6 do
        if foes[j].alive == 1 and rect_overlap(bullets[i].x, bullets[i].y, 2, 4, foes[j].x, foes[j].y, 8, 8) then
          foes[j].alive = 0
          bullets[i].alive = 0
          score = score + 1
          break                 -- this bullet is spent: don't let it kill more foes
        end
        j = j + 1
      end
    end
    i = i + 1
  end
end

function draw()
  cls(0)
  spr(ship, px, 112, 0)
  local i = 0
  while i < 6 do
    if bullets[i].alive == 1 then spr(bull, bullets[i].x, bullets[i].y, 0) end
    if foes[i].alive == 1 then spr(foe, foes[i].x, foes[i].y, 0) end
    i = i + 1
  end
  text("SCORE", 2, 2, 7)          -- HUD: a label...
  number(score, 40, 2, 7)          -- ...and the live score
  entity(px, 112, 1)
end
