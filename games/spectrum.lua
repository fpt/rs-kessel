-- spectrum.lua — the 240×240 screen, the 256-colour palette, and sprite banks.
--
-- A reference for the three video features rather than a game: the wide screen
-- gives room for a HUD beside the play field, `pal` rewrites palette entries
-- live, and `sprbank` draws one 4bpp sprite in sixteen colour schemes.
--
--   kessel run games/spectrum.lua

screen { mode = Extended240 }

controls {
  dpad = true       -- left/right pick a bank, up/down fade
  a = "cycle hue"
  pause = START
}

-- One tile, drawn sixteen times below in sixteen banks. Every pixel is a
-- nibble 1-15; nibble 0 (the `.`) stays transparent in every bank, which is
-- what keeps the holes holes when the bank changes.
sprite gem {
  ...11...
  ..1221..
  .122221.
  12222221
  .122221.
  ..1221..
  ...11...
  ........
}

local bank = 1
local fade = 0     -- 0 = full brightness, 15 = black
local hue = 0
local t = 0

function update()
  t = t + 1
  if btnp(LEFT) then bank = bank - 1 end
  if btnp(RIGHT) then bank = bank + 1 end
  if bank < 0 then bank = 15 end
  if bank > 15 then bank = 0 end

  if btnp(UP) then fade = fade - 1 end
  if btnp(DOWN) then fade = fade + 1 end
  if fade < 0 then fade = 0 end
  if fade > 15 then fade = 15 end

  if btnp(A) then hue = hue + 1 end
  if hue > 5 then hue = 0 end
end

-- Paint banks 1..15 out of the 6x6x6 colour cube that fills indices 16..231 by
-- default, so each bank is a two-tone ramp of one hue. Bank 0 keeps the stock
-- 16 colours, which is why every existing game still looks like itself.
function set_banks()
  local b = 1
  while b < 16 do
    local lo = b % 6
    local hi = (b + hue) % 6
    -- nibble 1 and 2 are the only ones the gem uses.
    pal(b * 16 + 1, 40 * hi, 20 * lo, 255 - 12 * b)
    pal(b * 16 + 2, 16 * hi, 40 * lo, 128 - 6 * b)
    b = b + 1
  end
end

-- A fade is a palette walk, not a redraw: the framebuffer is untouched.
function apply_fade()
  local k = 15 - fade
  local i = 1
  while i < 16 do
    pal(i, 17 * i * k / 15, 12 * i * k / 15, 20 * i * k / 15)
    i = i + 1
  end
end

function draw()
  cls(0)
  set_banks()
  apply_fade()

  text("ONE SPRITE, 16 BANKS", 8, 12, 6)

  -- The same tile in every bank, 8 across and 2 down.
  local i = 0
  while i < 16 do
    sprbank(i)
    spr(gem, 8 + (i % 8) * 16, 24 + (i / 8) * 18, 0)
    i = i + 1
  end

  -- The selected bank, drawn large. spr_scaled takes 8.8 fixed point.
  sprbank(bank)
  spr_scaled(gem, 8, 66, 1536, 0)

  -- The default 6x6x6 colour cube that fills indices 16..231, as a solid
  -- 24x9 grid of 8x8 swatches. None of this is reachable with 16 colours.
  local c = 0
  while c < 216 do
    local sx = 8 + (c % 24) * 9
    local sy = 150 + (c / 24) * 9
    local r = 0
    while r < 8 do
      hline(sx, sx + 7, sy + r, 16 + c)
      r = r + 1
    end
    c = c + 1
  end
  text("DEFAULT 216-COLOUR CUBE", 8, 138, 6)

  -- HUD down the right-hand side — the room the wide screen buys.
  sprbank(0)
  text("SPECTRUM", 140, 12, 7)
  text("BANK", 140, 30, 6)
  number(bank, 190, 30, 10)
  text("FADE", 140, 42, 6)
  number(fade, 190, 42, 10)
  text("HUE", 140, 54, 6)
  number(hue, 190, 54, 10)
  text("240X240", 140, 72, 12)

  entity(bank, fade, 1)
end
