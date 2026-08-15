-- paint.lua — draw on the screen with your fingers, steer the brush with the
-- stick.
--
--   kessel run games/paint.lua        (drag with the mouse; arrows nudge)
--
-- The console's two analog surfaces, in one program:
--
--   * `touch_*` reports up to four fingers in **console pixels** — the host has
--     already undone its own letterboxing and upscale, so the numbers here are
--     the numbers `pset` draws with. Slot 0 is the first finger down, and a
--     finger keeps its slot until it lifts, which is what makes
--     `touch_pressed`/`touch_released` mean anything.
--   * `stick_x`/`stick_y` are signed 8.8 fixed point, ±256 at full deflection —
--     the same scale, and the same `int` type, that `sin`/`cos` return. Which
--     means the same caveat: `/` is unsigned on this machine, so a deflection's
--     sign has to be branched on before its magnitude is divided. See `travel`.
--
-- On a machine with no touchscreen the mouse is slot 0 and the arrow keys
-- deflect the stick, so every path here is reachable from a keyboard.

screen { mode = Extended240 }

controls {
  dpad  = false
  stick = "move brush"
  touch = "draw"
  a     = "clear"
  b     = "cycle colour"
  pause = START
}

local DIM = 240
local SLOTS = 4           -- the console reports at most four fingers
local SPEED = 4           -- pixels per frame at full deflection

local cx = 120
local cy = 120
local color = 8
local drawing = 0         -- whether the stick brush is laying down paint
local touched = 0         -- fingers seen this frame, for the HUD

function init()
  cx = 120
  cy = 120
  color = 8
  drawing = 0
  touched = 0
end

-- Deflection -> pixels of travel this frame, always non-negative.
--
-- `/` is **unsigned** on this machine, so a negative deflection has to have its
-- magnitude taken before the divide — the same branch-on-the-sign shape
-- `outrun.lua` uses for `sin()`. Feeding 0xFF00 straight to `/ 256` would give
-- 255 pixels of travel instead of one.
function travel(v: int)
  if v < 0 then return (0 - v) * SPEED / 256 end
  return v * SPEED / 256
end

-- Move `p` along one axis by `v`'s deflection, clamped to the screen. Clamping
-- rather than wrapping: a brush that reappears on the far edge would look like
-- a bug in the console rather than the edge of the canvas.
function step(p, v: int)
  local d = travel(v)
  if v < 0 then
    if p > d then return p - d end
    return 0
  end
  return min(p + d, DIM - 1)
end

function update()
  -- The stick moves the brush; B cycles its colour; A wipes the canvas.
  local sx: int = stick_x()
  local sy: int = stick_y()
  cx = step(cx, sx)
  cy = step(cy, sy)

  if btnp(B) then
    color = color + 1
    if color > 15 then color = 8 end
  end

  -- The brush lays down paint only while the stick is actually deflected, so a
  -- parked cursor does not burn a hole in the canvas.
  drawing = 0
  if sx ~= 0 or sy ~= 0 then drawing = 1 end

  touched = touch_count()
end

-- A filled square, since the console has no rect primitive and a single pixel
-- is invisible on a 240×240 screen scaled to a phone.
function blob(x, y, size, c)
  for r = 0, size do
    hline(x - size / 2, x + size / 2, y - size / 2 + r, c)
  end
end

function draw()
  -- No `cls`: the framebuffer *is* the canvas. Everything below adds to what
  -- is already there, which is what makes this a painting rather than a
  -- one-frame drawing — and why A has to clear it explicitly.
  if btn(A) then cls(1) end

  -- Every finger down paints in the current colour. `touch_x`/`touch_y` are
  -- already console pixels, so no unprojection happens here.
  for i = 0, SLOTS - 1 do
    if touch_down(i) then
      blob(touch_x(i), touch_y(i), 5, color)
    end
    -- A fresh press marks its landing spot, so a tap leaves something behind
    -- even if the finger never moves.
    if touch_pressed(i) then
      blob(touch_x(i), touch_y(i), 9, 7)
    end
  end

  if drawing == 1 then
    blob(cx, cy, 3, color)
  end

  -- The brush's own marker, drawn last so it is never buried under paint.
  blob(cx, cy, 1, 7)

  -- HUD on a cleared strip, so the readout never disappears into the painting.
  hline(0, 239, 0, 0)
  hline(0, 239, 1, 0)
  hline(0, 239, 2, 0)
  hline(0, 239, 3, 0)
  hline(0, 239, 4, 0)
  hline(0, 239, 5, 0)
  hline(0, 239, 6, 0)
  text("FINGERS", 4, 1, 6)
  number(touched, 66, 1, 7)
  text("COLOUR", 100, 1, 6)
  number(color, 156, 1, color)
  entity(cx, cy, 1)
end
