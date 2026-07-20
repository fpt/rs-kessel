-- outrun.lua — a pseudo-3D road racer in the Outrun/Pole-Position mould. The
-- road is drawn one horizontal scanline at a time (hline): from the near bottom
-- row up to the horizon, the centre bends by an accumulating curve (a parabola,
-- so far rows bend most) and the width shrinks with distance. Roadside trees are
-- distance-scaled with spr_scaled, and a bobbing sun shows sin().
--
--   kessel --play games/outrun.lua
--
-- Arrows steer / accelerate, A boosts, Down brakes. Drift too far off the
-- tarmac and you scrub speed. There is no crash — it is an endless cruise.

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- left/right steer, up accelerate, down brake
  a = "boost"
  pause = START
}

sprite car {
  ..8888..
  .888888.
  88888888
  8.8888.8
  88888888
  .877778.
  .8.00.8.
  ........
}

-- A roadside tree: green canopy over a brown trunk. Scaled by distance.
sprite tree {
  ...bb...
  ..bbbb..
  .bbbbbb.
  bbbbbbbb
  .bbbbbb.
  ...44...
  ...44...
  ...44...
}

-- A low sun on the horizon (scaled up, bobs with sin()).
sprite sun {
  ..aaaa..
  .aaaaaa.
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  .aaaaaa.
  ..aaaa..
}

local HORIZON = 44          -- sky above this row, road below
local BOTTOM = 127
local DEPTH = 83            -- BOTTOM - HORIZON (rows of road)
local ROAD_HALF = 60        -- road half-width at the nearest row

-- palette (PICO-8): 3/11 grass, 5/6 tarmac, 8/7 rumble, 12 sky, 10 sun
local px: int = 0           -- player lateral offset from centre (signed)
local speed = 0             -- forward speed
local pos = 0               -- distance travelled (scrolls the stripes)
local curve: int = 0        -- current road curvature (signed)
local next_curve = 0        -- frames until the curve changes

function init()
  px = 0  speed = 0  pos = 0  curve = 0  next_curve = 0
end

function update()
  -- throttle / brake, with drag when coasting
  if btn(A) then speed = speed + 2
  elseif btn(UP) then speed = speed + 1
  elseif btn(DOWN) then speed = speed - 2
  else speed = speed - 1 end
  if speed < 0 then speed = 0 end
  if speed > 48 then speed = 48 end

  pos = pos + speed

  -- steering
  if btn(LEFT)  then px = px - 3 end
  if btn(RIGHT) then px = px + 3 end

  -- centrifugal drift: a curve pushes the car to the outside, harder at speed.
  -- speed/12 is unsigned (speed >= 0); the direction is picked by curve's sign,
  -- so no signed division is needed.
  local drift = speed / 12
  if curve > 0 then px = px + drift
  elseif curve < 0 then px = px - drift end

  -- off-road scrubs speed and the car can't wander to infinity
  if px < 0 - 74 then px = 0 - 74  speed = speed - 2 end
  if px > 74 then px = 74  speed = speed - 2 end
  if speed < 0 then speed = 0 end

  -- occasionally pick a new curve in -3..3
  if next_curve == 0 then
    curve = 0 - 3 + rnd(7)     -- (0-3) is int, so the result is signed -3..3
    next_curve = 60 + rnd(120)
  end
  next_curve = next_curve - 1
end

-- Draw a roadside tree at road-edge `side` (-1 left, +1 right) for the row `y`,
-- scaled by the row's road half-width so nearer trees loom larger.
function tree_at(y, cx: int, half, side: int)
  local scale = 128 + half * 6            -- 8.8 fixed: ~1.4x near, small far
  local size = 8 * scale / 256            -- on-screen pixel size (all positive)
  local ex: int = cx + side * (half + half / 2)
  spr_scaled(tree, ex - size / 2, y - size, scale, 0)
end

function draw()
  cls(12)                                  -- sky fills rows above the horizon

  -- a low sun that bobs with sin(); sign is handled by branching so the
  -- unsigned divide only ever sees a non-negative value.
  local a = frame_count() / 2
  local s: int = sin(a)
  local sun_y = 20
  if s > 0 then sun_y = 20 - s / 40 else sun_y = 20 + (0 - s) / 40 end
  spr_scaled(sun, 90, sun_y, 384, 0)       -- ~1.5x, drawn before the road

  -- Road: draw each scanline from the near bottom up to the horizon. The centre
  -- follows a parabola in the distance `k` (far rows bend most); `k*k/512` is a
  -- positive quantity, so multiplying by the signed `curve` keeps the sign right
  -- in 16 bits without ever dividing a negative (which would be unsigned).
  local y = BOTTOM
  while y > HORIZON do
    local d = y - HORIZON                   -- 1..DEPTH, larger nearer the camera
    local half = ROAD_HALF * d / DEPTH      -- widest at the bottom row
    local k = DEPTH - d                     -- 0 near, DEPTH-1 at the horizon
    local off: int = curve * (k * k / 512)  -- parabolic bend
    local cx: int = 64 + px + off
    local z = 4096 / d                      -- perspective depth (big = far)
    local shade = (z + pos) / 24            -- scrolls toward the camera

    local grass = 3  local road = 5  local rumble = 8
    if shade % 2 == 0 then grass = 11  road = 6  rumble = 7 end

    hline(0, 127, y, grass)                 -- grass first
    local lx: int = cx - half
    local rx: int = cx + half
    hline(lx, rx, y, road)                  -- tarmac
    local edge = half / 6 + 1
    hline(lx, lx + edge, y, rumble)         -- rumble strips at both edges
    hline(rx - edge, rx, y, rumble)

    -- a tree every so often down the right and left verges
    if shade % 8 == 0 then tree_at(y, cx, half, 1) end
    if shade % 8 == 4 then tree_at(y, cx, half, 0 - 1) end

    y = y - 1
  end

  -- player car, fixed near the bottom centre (the road moves under it)
  spr(car, 60, 116, 0)

  number(speed, 2, 2, 7)                    -- speed HUD
  entity(px, speed, 1)                      -- report for observation
end
