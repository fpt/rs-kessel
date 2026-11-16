-- demo3d.lua — tang-verilog's 3D demo, on Kessel3D.
--
--   kessel run games/demo3d.lua
--
-- The four scenes of tang-verilog's G5 demo (tools/harness/demo3d.py there),
-- ten seconds each, round and round; A skips to the next:
--
--   logo    KESSEL in chrome (TANG in the original) flies in from the distance and sways; bands of
--           light run under it (additive)
--   tunnel  down a winding stone tunnel, fogged with depth, through rings of
--           light (additive)
--   room    a mirror ball bounces in a marble room; the floor reflects the
--           room (the reflection drawn upside down beneath it, the floor
--           averaged over it) and the ball casts a shadow (subtractive)
--   city    over a night city, windows lit, street lamps (additive)
--
-- That demo computes everything in Python floats on a PC. Here the parts that
-- never change — textures, the logo, buildings, the tunnel's pieces — are
-- built once by tools/demo3d_assets.py from the demo's own procedures, and
-- everything that moves is computed per frame in 16-bit fixed point, which is
-- what lets the same frame come out bit for bit on a PC and an RP2350.
--
-- Three things differ from the demo, each because the engine does it its own
-- way: chrome is a texture mapped by position rather than an environment map
-- (the engine has no per-frame uv generation); the tunnel is pieces of a
-- straight tube laid along the curve rather than a tube bent to it; and the
-- floor reflects the whole room, not just the ball. The demo draws without a
-- Z-buffer, so it can lay the floor, the upside-down ball and the floor again;
-- here every triangle is depth-tested, so what shows through the floor is
-- whatever is drawn beneath it — and that is the room, upside down.
--
-- 64 world units are one of the demo's units, and angles are 16-bit
-- (65536 a turn), so a demo rate of w radians a second is w * 10430 / 60 a
-- frame.

screen { mode = Kessel3D }

controls {
  a = "next scene"
}

#include "demo3d/textures.lua"
#include "demo3d/meshes.lua"

local SCENE_FRAMES = 600   -- ten seconds a scene

local scene = 0            -- 0 logo, 1 tunnel, 2 room, 3 city
local k = 0                -- frames into the scene

signal scene

-- The tunnel's centre line at depth z, set by `center`.
local cx: int = 0
local cy: int = 0

function init()
  demo_palette()
end

function update()
  if btnp(A) then next_scene() end
  signal(scene, scene)
end

function next_scene()
  k = 0
  scene = (scene + 1) % 4
end

-- ---- fixed point ----

function abs_of(v: int)
  if v < 0 then return 0 - v end
  return v
end

-- a / b for a positive b, rounded toward zero. `/` divides unsigned, so a
-- negative needs its sign taken off first.
function sdiv(a: int, b: int)
  if a < 0 then return 0 - (0 - a) / b end
  return a / b
end

-- sin of a 16-bit angle, signed 8.8. The device's sine is 256 steps a turn;
-- interpolating between two of them is what keeps a slow pan from stepping.
function sin16(a)
  local i = a >> 8
  local s0: int = sin(i)
  local s1: int = sin((i + 1) & 255)
  return s0 + sdiv((s1 - s0) * (a & 255), 256)
end

function cos16(a)
  return sin16(a + 16384)
end

-- atan of a ratio in 8.8, as a 16-bit angle. x - 0.28x^3 is within a degree
-- of the real thing out to 35 degrees, which is as steep as anything here
-- looks. (a*a overflows an int but not a word, and `>>` shifts the bits.)
function atan8(r: int)
  local a: int = abs_of(r)
  if a > 255 then a = 255 end
  local c = (((a * a) >> 8) * a) >> 8
  local t = a - ((c * 72) >> 8)
  local ang = (t * 163) >> 2
  if r < 0 then return 0 - ang end
  return ang
end

-- ---- logo ----

function draw_logo()
  lens3d(60, 8)
  fog3d(0, 0, 0)
  -- The eye at (0, 1.5, -26), looking at the origin.
  camera3d(0, 96, 0 - 1664, 0, 0 - 601, 0)
  ambient3d(128, 128, 128)
  light3d(0, 1, 0, 0, 0, 0)
  draw3d(sky_dusk, 0, 96, 0 - 1664, 0, 0 - 601, 0)

  -- Two seconds flying in from z = 60, spinning out of six radians.
  local f = 0
  if k < 120 then f = ((120 - k) * 32) / 15 end
  local h = f / 2
  local z = ((h * h) >> 6) * 15
  local y: int = sdiv(sin16(k * 226) * 38, 256)
  local yaw = sin16(k * 156) * 20 + f * 244
  local pitch = sin16(k * 122) * 6
  ambient3d(82, 77, 66)
  light3d(40, 80, 0 - 60, 90, 84, 72)
  draw3d(logo, 0, y, z, yaw, pitch, 0)

  -- Six bands flowing toward the eye, brightest in the middle distance.
  light3d(0, 1, 0, 0, 0, 0)
  local m = (k * 13) % 2304
  for i = 0, 5 do
    local bz: int = (i * 384 + 2304 - m) % 2304 - 384
    local a: int = 256 - abs_of(bz - 768) / 6
    if a < 0 then a = 0 end
    ambient3d((20 * a) >> 8, (45 * a) >> 8, (80 * a) >> 8)
    draw3d(band, 0, 0 - 384, bz, 0, 0, 0)
  end
end

-- ---- tunnel ----

-- The demo's tunnel_center: x = 3 sin(0.11 s) + 1.5 sin(0.23 s), y = 2 sin(0.07 s).
function center(z)
  cx = sdiv(sin16(z * 18) * 3, 4) + sdiv(sin16(z * 37) * 3, 8)
  cy = sdiv(sin16(z * 11), 2)
end

function draw_tunnel()
  local z0 = k * 8
  lens3d(75, 8)
  center(z0)
  local ex: int = cx
  local ey: int = cy
  center(z0 + 192)            -- look three rings ahead
  local yaw = atan8(sdiv((cx - ex) * 4, 3))
  local pitch = atan8(sdiv((cy - ey) * 4, 3))
  camera3d(ex, ey, z0, yaw, pitch, sin16(z0 * 16) * 12)

  -- Torchlight that flickers, darkening into the distance.
  local fl = 218 + sdiv(sin16(k * 3129) * 38, 256)
  ambient3d((180 * fl) >> 8, (171 * fl) >> 8, (153 * fl) >> 8)
  light3d(0, 0, 1, 0, 0, 0)
  fog3d(64, 640, 0)

  local first: int = z0 / 64 - 1
  for i = 0, 10 do
    local s: int = first + i
    local zs: int = s * 64
    center(zs + 64)
    local nx: int = cx
    local ny: int = cy
    center(zs)
    local syaw = atan8((nx - cx) * 4)
    local spitch = atan8((ny - cy) * 4)
    local ph = s & 3
    if ph == 0 then draw3d(seg0, cx, cy, zs, syaw, spitch, 0)
    elseif ph == 1 then draw3d(seg1, cx, cy, zs, syaw, spitch, 0)
    elseif ph == 2 then draw3d(seg2, cx, cy, zs, syaw, spitch, 0)
    else draw3d(seg3, cx, cy, zs, syaw, spitch, 0) end
  end

  -- Every sixth ring a ring of light, fading with distance rather than fog.
  fog3d(0, 0, 0)
  for i = 0, 10 do
    local s: int = first + i
    if s >= 0 and s % 6 == 0 then
      local zs: int = s * 64 + 32
      local a: int = 256 - sdiv((zs - z0) * 4, 11)
      if a < 0 then a = 0 end
      if a > 256 then a = 256 end
      ambient3d((15 * a) >> 8, (80 * a) >> 8, (110 * a) >> 8)
      center(zs + 32)
      local nx: int = cx
      local ny: int = cy
      center(zs)
      draw3d(halo, cx, cy, zs, atan8((nx - cx) * 8), atan8((ny - cy) * 8), 0)
    end
  end
end

-- ---- room ----

function draw_room()
  local a = k * 61
  lens3d(70, 8)
  fog3d(0, 0, 0)
  -- Round the room at radius 9.5, looking at (0, 1.2, 0).
  local ex: int = sdiv(sin16(a) * 19, 8)
  local ez: int = 0 - sdiv(cos16(a) * 19, 8)
  camera3d(ex, 205, ez, 0 - a, 0 - 2164, 0)

  -- Everything above the floor is drawn twice: as itself, and turned upside
  -- down beneath it. The floor is averaged over the second, which is the
  -- reflection.
  ambient3d(128, 128, 128)
  light3d(0, 1, 0, 0, 0, 0)
  draw3d(ceiling, 0, 0, 0, 0, 0, 0)
  draw3d(ceiling, 0, 0, 0, 0, 0, 32768)
  draw3d(skirt, 0, 0, 0, 0, 0, 0)
  ambient3d(105, 105, 105)
  light3d(30, 100, 20, 45, 45, 45)
  draw3d(walls, 0, 0, 0, 0, 0, 0)
  draw3d(walls, 0, 0, 0, 0, 0, 32768)

  local b: int = abs_of(sin16(k * 382))
  local bx: int = sdiv(sin16(k * 139) * 35, 64)
  local by: int = 70 + ((141 * b) >> 8)
  local bz: int = sdiv(cos16(k * 104) * 35, 64)
  ambient3d(90, 90, 96)
  light3d(30, 100, 20, 75, 75, 80)
  draw3d(ball, bx, by, bz, k * 100, 0, 0)
  draw3d(ball, bx, 0 - by, bz, k * 100, 0, 32768)

  -- Half of what shows is the reflection, so the marble is lit up to 2x.
  ambient3d(255, 255, 255)
  light3d(0, 1, 0, 0, 0, 0)
  draw3d(floor, 0, 0, 0, 0, 0, 0)

  -- The shadow shrinks and fades as the ball rises.
  local sh = 256 - b / 2
  ambient3d((35 * sh) >> 8, (35 * sh) >> 8, (30 * sh) >> 8)
  local r = (b * 3 + 128) / 256
  if r == 0 then draw3d(shadow0, bx, 0, bz, 0, 0, 0)
  elseif r == 1 then draw3d(shadow1, bx, 0, bz, 0, 0, 0)
  elseif r == 2 then draw3d(shadow2, bx, 0, bz, 0, 0, 0)
  else draw3d(shadow3, bx, 0, bz, 0, 0, 0) end
end

-- ---- city ----

function tower(t, x: int, z: int)
  if t == 1 then draw3d(tower1, x, 0, z, 0, 0, 0)
  elseif t == 2 then draw3d(tower2, x, 0, z, 0, 0, 0)
  elseif t == 3 then draw3d(tower3, x, 0, z, 0, 0, 0)
  elseif t == 4 then draw3d(tower4, x, 0, z, 0, 0, 0)
  elseif t == 5 then draw3d(tower5, x, 0, z, 0, 0, 0)
  elseif t == 6 then draw3d(tower6, x, 0, z, 0, 0, 0)
  elseif t == 7 then draw3d(tower7, x, 0, z, 0, 0, 0)
  else draw3d(tower8, x, 0, z, 0, 0, 0) end
end

function draw_city()
  lens3d(62, 8)
  -- Forward at three units a second, weaving, looking 16 ahead at y = 4.
  local ex: int = 384 + sdiv(sin16(k * 87) * 3, 8)
  local ey: int = 608 + sdiv(sin16(k * 52) * 3, 8)
  local ez: int = (k * 16) / 5
  local tx: int = 384 + sdiv(sin16(k * 87 + 8344), 2)
  local yaw = atan8(sdiv(tx - ex, 4))
  local pitch = atan8(sdiv(256 - ey, 4))
  local roll = sin16(k * 87) * 2
  camera3d(ex, ey, ez, yaw, pitch, roll)

  fog3d(0, 0, 0)
  ambient3d(128, 128, 128)
  light3d(0, 1, 0, 0, 0, 0)
  draw3d(sky_night, ex, ey, ez, yaw, pitch, roll)

  fog3d(384, 1216, 0)
  local j0: int = ez / 128 - 1
  ambient3d(230, 230, 230)
  for i = 0, 10 do
    draw3d(road, 0, 0, (j0 + i) * 128, 0, 0, 0)
  end

  ambient3d(142, 142, 150)
  light3d(50, 80, 0 - 30, 66, 66, 70)
  for i = 0, 10 do
    local j: int = j0 + i
    local row = ((j + 32) % 32) * 9
    for c = 0, 8 do
      local t = peek(city + row + c)
      if t > 0 then tower(t, c * 128 - 64, j * 128 + 64) end
    end
  end

  ambient3d(128, 128, 128)
  light3d(0, 1, 0, 0, 0, 0)
  for i = 0, 8 do
    local z: int = (j0 + i) * 128 + 64
    draw3d(lamp, 13, 0, z, 0, 0, 0)
    draw3d(lamp, 371, 0, z, 0, 0, 0)
  end
end

function draw()
  cls(0)
  if scene == 0 then draw_logo()
  elseif scene == 1 then draw_tunnel()
  elseif scene == 2 then draw_room()
  else draw_city() end

  k = k + 1
  if k >= SCENE_FRAMES then next_scene() end
end
