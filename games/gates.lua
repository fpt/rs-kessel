-- gates.lua — fly through the gates. Miss three and the run is over.
--
--   kessel run games/gates.lua
--
-- The corpus's Kessel3D example: `screen { mode = Kessel3D }` gives the 3D
-- GPU's 320x240 screen, `mesh` blocks declare the models, and `draw3d` queues
-- one each. The 2D calls still work and land on top of the scene, which is how
-- the score is drawn. docs/VM_3D.md is the reference.
--
-- The ship never moves forward. The world comes toward it: a gate is a z that
-- shrinks every frame, and passing one is the frame its z crosses zero. That
-- keeps every coordinate small — a run long enough to fly past z = 32767
-- would otherwise wrap and put the next gate behind you.

screen { mode = Kessel3D }

controls {
  dpad  = true      -- steer
  a     = "boost"
  b     = "restart"
  pause = START
}

-- A hazard stripe for the gates and a lit panel for the pylons. Textures are
-- sprites whose sides are powers of two; nibble 0 stays transparent, exactly
-- as it does for spr().
sprite hazard {
  aaaa0000aaaa0000
  aaa0000aaaa0000a
  aa0000aaaa0000aa
  a0000aaaa0000aaa
  0000aaaa0000aaaa
  000aaaa0000aaaa0
  00aaaa0000aaaa00
  0aaaa0000aaaa000
  aaaa0000aaaa0000
  aaa0000aaaa0000a
  aa0000aaaa0000aa
  a0000aaaa0000aaa
  0000aaaa0000aaaa
  000aaaa0000aaaa0
  00aaaa0000aaaa00
  0aaaa0000aaaa000
}

sprite panel {
  55555555
  5dddddd5
  5d6666d5
  5d6cc6d5
  5d6cc6d5
  5d6666d5
  5dddddd5
  55555555
  55555555
  5dddddd5
  5d6666d5
  5d6886d5
  5d6886d5
  5d6666d5
  5dddddd5
  55555555
}

-- Nose along +z. Faces run counter-clockwise seen from outside.
mesh ship {
  v   0  0  16
  v -12  0  -8
  v  12  0  -8
  v   0  7  -6
  v   0 -3  -8
  color 12
  f 1 2 4
  color 13
  f 1 4 3
  color 5
  f 1 3 5
  f 1 5 2
  color 5
  f 2 5 3 4
}

-- A square frame around a 48x48 opening, facing the ship (-z).
mesh gate {
  v -32 -32 0
  v  32 -32 0
  v  32 -24 0
  v -32 -24 0
  v -32  24 0
  v  32  24 0
  v  32  32 0
  v -32  32 0
  v -24 -24 0
  v -24  24 0
  v  24 -24 0
  v  24  24 0
  double
  tex hazard
  f 1 2 3 4
  f 5 6 7 8
  f 4 9 10 5
  f 11 3 6 12
}

-- A tall box, four textured sides and no top.
mesh pylon {
  v -4  0 -4
  v  4  0 -4
  v  4 64 -4
  v -4 64 -4
  v -4  0  4
  v  4  0  4
  v  4 64  4
  v -4 64  4
  tex panel
  f 1 2 3 4
  f 2 6 7 3
  f 6 5 8 7
  f 5 1 4 8
}

mesh ground {
  v -600 0 -200
  v  600 0 -200
  v  600 0 1400
  v -600 0 1400
  color 3
  f 1 2 3 4
}

record Gate { x: int, y: int, z: int }
record Pylon { x: int, z: int }

local GATES = 6
local SPACING = 220
local PYLONS = 8
local PYLON_GAP = 160
local HALF = 22          -- how far off-centre the ship may be and still pass
local REACH = 60         -- how far the ship may steer from the middle

local gates: array(6, Gate)
local pylons: array(8, Pylon)
local sx: int = 0        -- the ship, in world units
local sy: int = 0
local vx: int = 0
local speed = 4
local lives = 3
local score = 0
local best = 0
local over = 0
local flash = 0          -- frames of red after a miss

signal score
signal lives
signal speed

function init()
  lens3d(70, 8)
  light3d(0 - 3, 6, 0 - 4, 170, 160, 140)
  ambient3d(70, 70, 90)
  fog3d(250, 1300, 1)
  restart()
end

function restart()
  sx = 0  sy = 0  vx = 0
  speed = 4
  lives = 3
  score = 0
  over = 0
  flash = 0
  for i = 0, GATES - 1 do
    gates[i].z = 300 + i * SPACING
    place(gates[i])
  end
  for i = 0, PYLONS - 1 do
    pylons[i].z = i * PYLON_GAP
    pylons[i].x = 110
    if i % 2 == 1 then pylons[i].x = 0 - 110 end
  end
end

-- Somewhere the ship can reach, but not where it already is.
function place(g: Gate)
  g.x = rnd(2 * REACH - 20) - (REACH - 10)
  g.y = rnd(REACH) - REACH / 2
end

function update()
  if over == 1 then
    if btnp(B) or btnp(A) then restart() end
    return
  end
  if btnp(B) then restart() end

  -- Steering eases in and out, so the bank below has something to show.
  -- (`clamp`, not min/max: those compare unsigned, and these go negative.)
  if btn(LEFT) then vx = clamp(vx - 1, 0 - 4, 4) end
  if btn(RIGHT) then vx = clamp(vx + 1, 0 - 4, 4) end
  if not btn(LEFT) and not btn(RIGHT) then
    if vx > 0 then vx = vx - 1 end
    if vx < 0 then vx = vx + 1 end
  end
  sx = clamp(sx + vx, 0 - REACH, REACH)
  if btn(UP) then sy = clamp(sy + 2, 0 - REACH, REACH) end
  if btn(DOWN) then sy = clamp(sy - 2, 0 - REACH, REACH) end

  local step = speed
  if btn(A) then step = speed * 2 end

  for i = 0, GATES - 1 do
    local before: int = gates[i].z
    gates[i].z = gates[i].z - step
    if before > 0 and gates[i].z <= 0 then
      if abs_of(sx - gates[i].x) < HALF and abs_of(sy - gates[i].y) < HALF then
        score = score + 1
        if score % 5 == 0 then speed = min(speed + 1, 10) end
      else
        lives = lives - 1
        flash = 20
        if lives == 0 then
          over = 1
          if score > best then best = score end
        end
      end
    end
    if gates[i].z < 0 - 60 then
      gates[i].z = gates[i].z + GATES * SPACING
      place(gates[i])
    end
  end
  for i = 0, PYLONS - 1 do
    pylons[i].z = pylons[i].z - step
    if pylons[i].z < 0 - 100 then pylons[i].z = pylons[i].z + PYLONS * PYLON_GAP end
  end
  if flash > 0 then flash = flash - 1 end

  signal(score, score)
  signal(lives, lives)
  signal(speed, speed)
end

function abs_of(v: int)
  if v < 0 then return 0 - v end
  return v
end

function clamp(v: int, lo: int, hi: int)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

-- v / 2 rounded toward zero. `/` divides unsigned, so a negative needs its
-- sign taken off first.
function half(v: int)
  if v < 0 then return 0 - (0 - v) / 2 end
  return v / 2
end

function draw()
  cls(1)
  -- The eye trails the ship and follows it halfway, so steering shows as the
  -- ship moving on screen *and* the world swinging past.
  camera3d(half(sx), half(sy) + 70, 0 - 110, 0, 0 - 5000, 0)

  draw3d(ground, 0, 0 - 70, 0, 0, 0, 0)
  for i = 0, PYLONS - 1 do
    draw3d(pylon, pylons[i].x, 0 - 70, pylons[i].z, 0, 0, 0)
  end
  for i = 0, GATES - 1 do
    -- Each gate turns slowly about its axis, by its own amount.
    draw3d(gate, gates[i].x, gates[i].y, gates[i].z, 0, 0, gates[i].z * 40)
  end
  -- Bank into the turn.
  draw3d(ship, sx, sy, 0, 0, 0, vx * 1800)

  -- The HUD is ordinary 2D, drawn over the scene.
  text("SCORE", 6, 6, 7)
  number(score, 30, 6, 10)
  for i = 1, lives do
    rect(300 - i * 10, 6, 6, 6, 8)
  end
  if flash > 0 then rect(0, 0, 320, 2, 8) end
  if over == 1 then
    text("GAME OVER", 142, 110, 7)
    text("BEST", 146, 120, 7)
    number(best, 166, 120, 10)
    text("PRESS A", 146, 132, 6)
  end
end
