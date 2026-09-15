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
--   * `touch_dx`/`touch_dy` are the drag's **signed** displacement from where
--     the press landed, so the origin comes back for free as `touch_x(0) - dx`.
--     Both read 0 once the finger lifts.
--   * `stick_x`/`stick_y` are signed 8.8 fixed point, ±256 at full deflection —
--     the same scale, and the same `int` type, that `sin`/`cos` return. Which
--     means the same caveat: `/` is unsigned on this machine, so a deflection's
--     sign has to be branched on before its magnitude is divided. See `mag`.
--
-- The trap worth reading before you copy this file: **a release edge carries no
-- position.** `touch_released(i)` fires on a frame when that finger is already
-- gone, so `touch_x(i)` reads 0 and not the lift point. A game that wants to
-- know where a stroke ended has to latch it while the finger was still down —
-- `lastx`/`lasty` below.
--
-- Nothing crashes if you get this wrong, and nothing appears in the corner
-- either: a mark centred on (0,0) has its top-left offset wrap below zero and
-- clips away entirely, so the cap is simply never drawn. A bug you cannot see
-- is the reason this one has a test.
--
-- On a machine with no touchscreen the mouse is slot 0 and the arrow keys
-- deflect the stick, so every path here is reachable from a keyboard.

screen { mode = Square240 }

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

-- Where each finger was on the last frame it was still down.
--
-- `touch_released(i)` tells you a finger left, but **not where it left from**:
-- the position ports read the slot's current state, and on the release frame
-- that finger is no longer down. Both hosts say so in their own way — the
-- desktop window rebuilds its touch array from empty every frame, and Android's
-- `TouchTracker` skips a pointer that is not pressed — so `touch_x(i)` on the
-- frame `touch_released(i)` fires is 0, not the lift point.
--
-- So the release edge has to be paired with a position the game latched while
-- the finger was still down. That is this array.
local lastx: array(4, word)
local lasty: array(4, word)

function init()
  cx = 120
  cy = 120
  color = 8
  drawing = 0
  touched = 0
  for i = 0, SLOTS - 1 do
    lastx[i] = 0
    lasty[i] = 0
  end
end

-- A signed value's magnitude, as a plain unsigned number.
--
-- `/`, `<` and `number()` are all **unsigned** on this machine, so anything
-- signed — the stick, `sin`/`cos`, a drag delta — has to have its sign branched
-- on before its magnitude is used. This is the shape `outrun.lua` uses for
-- `sin()`, and it is the single most common way to get a signed reading wrong
-- here: feeding 0xFF00 straight to `/ 256` gives 255, and to `number()` gives
-- 65280.
function mag(v: int)
  if v < 0 then return 0 - v end
  return v
end

-- Deflection -> pixels of travel this frame, always non-negative.
function travel(v: int)
  return mag(v) * SPEED / 256
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

-- A filled square centred on (x, y) -- a single pixel is invisible on a 240×240
-- screen scaled to a phone. This was a loop of `hline` before the console grew
-- `rect`; the centring is the only part of it left.
--
-- The loop ran `r = 0, size`, one row more than it drew columns, so every brush
-- was a pixel taller than it was wide. Nobody sees that in a paint smear, which
-- is exactly why it survived -- `2 * d + 1` on both axes is the square it was
-- always meant to be.
function blob(x, y, size, c)
  local d = size / 2
  rect(x - d, y - d, 2 * d + 1, 2 * d + 1, c)
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
      -- Latch it for the release edge below, which cannot read a position of
      -- its own. See `lastx`/`lasty`.
      lastx[i] = touch_x(i)
      lasty[i] = touch_y(i)
    end
    -- A fresh press marks its landing spot, so a tap leaves something behind
    -- even if the finger never moves.
    if touch_pressed(i) then
      blob(touch_x(i), touch_y(i), 9, 7)
    end
    -- And a lift caps the stroke, from the latched spot rather than from
    -- `touch_x(i)` — which is 0 on this exact frame.
    --
    -- The slot is a finger's *identity*, held for that finger's whole life, so
    -- this caps the stroke the finger that actually left was drawing. A host
    -- that renumbered its fingers between frames would cap someone else's.
    if touch_released(i) then
      blob(lastx[i], lasty[i], 7, 7)
    end
  end

  if drawing == 1 then
    blob(cx, cy, 3, color)
  end

  -- The brush's own marker, drawn last so it is never buried under paint.
  blob(cx, cy, 1, 7)

  -- HUD on a cleared strip, so the readout never disappears into the painting.
  rect(0, 0, 240, 7, 0)
  text("FINGERS", 4, 1, 6)
  number(touched, 66, 1, 7)
  text("COLOUR", 100, 1, 6)
  number(color, 156, 1, color)

  -- How far slot 0 has dragged from where it landed.
  --
  -- `touch_dx`/`touch_dy` are **signed** and measure displacement from the
  -- press, not the origin itself — the origin is `touch_x(0) - dx`, handed back
  -- for free. They also read 0 once the finger lifts: a released finger has no
  -- displacement, so this is a live readout and not a record of the last drag.
  --
  -- Chebyshev distance because it needs no multiply, and `mag` first because
  -- `number` is unsigned and a leftward drag is negative.
  local dx: int = touch_dx(0)
  local dy: int = touch_dy(0)
  text("DRAG", 176, 1, 6)
  number(max(mag(dx), mag(dy)), 208, 1, 10)

  entity(cx, cy, 1)
end
