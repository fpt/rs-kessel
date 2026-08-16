-- piano.lua — a genuine keyboard you play with your fingers (or the mouse,
-- which stands in as touch slot 0 on a machine with no touchscreen).
--
--   kessel run games/piano.lua
--
-- Ten white keys — C up to the E past the octave, "an octave and a few extra
-- keys" — and the seven black keys between them. Touch reports up to four
-- fingers, so chords work for real: `note_on` puts each finger's note on its
-- own channel, and the channel it picks is that finger's own touch slot. A
-- key stays lit and sounding for exactly as long as the finger that pressed
-- it stays down — drag across the keys and it retunes like a real glissando,
-- lift and it releases. Nothing here times out or fires-and-forgets; this is
-- the reference for the held-note API (`note_on`/`note_off`), the way
-- `popn.lua` is the reference for a button row with no directions.
--
-- A and B shift the whole keyboard down/up an octave. A shift releases every
-- held note first — otherwise a key could keep ringing at a pitch the screen
-- no longer shows for it, which is exactly the kind of plausibly-wrong state
-- this console exists to avoid.

screen { mode = Extended240 }

controls {
  dpad  = false
  touch = "play"
  a     = "octave down"
  b     = "octave up"
  pause = START
}

-- Small and bright: this is an instrument, not a cathedral.
fx {
  reverb_size = 110
  reverb_damping = 110
}

-- attack/decay/sustain/release all matter here, unlike a fire-and-forget
-- `play()` — a finger held down sits in the sustain stage, and lifting it is
-- what triggers the release tail.
instrument piano {
  wave = triangle
  attack = 4  decay = 140  sustain = 130  release = 200
  filter = lpf  cutoff = 210
  reverb = 40
  volume = 150
}

local DIM = 240

-- Keybed geometry. 240 / 10 divides exactly, so every white key is the same
-- width with no rounding remainder on the last one.
local NUM_WHITE = 10
local WHITE_W = 24
local KB_Y = 70
local KB_H = 150
local BLACK_W = 14
local BLACK_H = 90

local NONE = 255          -- key_at's "no key here" answer

-- Octave shift is stored as the note C plays, not as a signed count — every
-- value it can ever hold is positive, so the clamp is a plain unsigned
-- compare and nothing here needs `int`.
local MIN_BASE = 36        -- C2
local MAX_BASE = 84        -- C6

local base_note = 60       -- C4, the octave a fresh boot starts on

-- Per touch-slot state: is this finger holding a key, and which note. The
-- slot itself doubles as the note's channel, so a finger's identity and its
-- sound's identity are the same number.
local active: array(4, byte)
local held: array(4, byte)

function init()
  clear(active)
  clear(held)
  base_note = 60
end

-- White key index -> pitch class within one octave (C D E F G A B).
function white_pitch_class(i)
  local p = i % 7
  if p == 0 then return 0 end
  if p == 1 then return 2 end
  if p == 2 then return 4 end
  if p == 3 then return 5 end
  if p == 4 then return 7 end
  if p == 5 then return 9 end
  return 11
end

-- White key index -> semitone offset from `base_note`, folding in a full
-- octave for every seven white keys crossed.
function white_offset(i)
  return white_pitch_class(i) + (i / 7) * 12
end

-- Does a black key sit between white key `i` and `i + 1`? Every white-key
-- boundary has one except E-F and B-C.
function has_black(i)
  local p = i % 7
  if p == 2 then return 0 end
  if p == 6 then return 0 end
  return 1
end

-- The black key between white keys `i` and `i + 1` is that white key's sharp.
function black_offset(i)
  return white_offset(i) + 1
end

-- Which note, if any, is under a touch at (x, y)? Black keys are checked
-- first — they are drawn on top and are only reachable near the top of the
-- keybed, so a touch there must win over the white key underneath it.
function key_at(x, y)
  if x >= DIM then return NONE end
  if y < KB_Y or y >= KB_Y + KB_H then return NONE end

  if y < KB_Y + BLACK_H then
    for i = 0, NUM_WHITE - 2 do
      if has_black(i) == 1 then
        local bx = (i + 1) * WHITE_W - BLACK_W / 2
        if x >= bx and x < bx + BLACK_W then
          return base_note + black_offset(i)
        end
      end
    end
  end

  return base_note + white_offset(x / WHITE_W)
end

-- Strike velocity from where on the key the finger landed: near the top is a
-- light touch, near the bottom is a hard one. A real piano reads how fast the
-- hammer moved; a touchscreen has no such thing to read, so position stands
-- in for it.
function vel_for_y(y)
  local rel = y - KB_Y
  return 90 + rel * 130 / KB_H
end

-- Every note any finger is currently holding, off — used before an octave
-- shift so nothing keeps ringing at a pitch the keybed no longer shows.
function release_all()
  for s = 0, 3 do
    if active[s] == 1 then
      note_off(s)
      active[s] = 0
    end
  end
end

function octave_down()
  if base_note > MIN_BASE then
    release_all()
    base_note = base_note - 12
  end
end

function octave_up()
  if base_note < MAX_BASE then
    release_all()
    base_note = base_note + 12
  end
end

-- Is `note` currently sounding under any finger? Only ever 0-4 fingers to
-- check, so a linear scan per key drawn is nothing.
function key_is_active(note)
  for s = 0, 3 do
    if active[s] == 1 and held[s] == note then return 1 end
  end
  return 0
end

function update()
  if btnp(A) then octave_down() end
  if btnp(B) then octave_up() end

  for s = 0, 3 do
    if touch_down(s) then
      local tx = touch_x(s)
      local ty = touch_y(s)
      local n = key_at(tx, ty)
      if n == NONE then
        if active[s] == 1 then
          note_off(s)
          active[s] = 0
        end
      elseif active[s] == 0 then
        note_on(s, piano, n, vel_for_y(ty))
        active[s] = 1
        held[s] = n
      elseif n ~= held[s] then
        -- The finger slid onto a different key: retune rather than
        -- restart, so a fast glissando still reads as one continuous drag.
        note_off(s)
        note_on(s, piano, n, vel_for_y(ty))
        held[s] = n
      end
      entity(tx, ty, s)
    elseif active[s] == 1 then
      note_off(s)
      active[s] = 0
    end
  end
end

function draw()
  cls(1)

  -- White keys first. Each is drawn one px narrower than its slot, so the
  -- background shows through as a seam between keys with no separate divider
  -- pass needed.
  for i = 0, NUM_WHITE - 1 do
    local x0 = i * WHITE_W
    local c = 7
    if key_is_active(base_note + white_offset(i)) == 1 then c = 10 end
    for y = KB_Y, KB_Y + KB_H - 1 do
      hline(x0 + 1, x0 + WHITE_W - 2, y, c)
    end
  end

  -- Black keys on top, shorter and narrower, centred on the boundary between
  -- the two white keys they sit above.
  for i = 0, NUM_WHITE - 2 do
    if has_black(i) == 1 then
      local bx = (i + 1) * WHITE_W - BLACK_W / 2
      local c = 0
      if key_is_active(base_note + black_offset(i)) == 1 then c = 9 end
      for y = KB_Y, KB_Y + BLACK_H - 1 do
        hline(bx, bx + BLACK_W - 1, y, c)
      end
    end
  end

  text("PIANO", 4, 4, 7)
  text("OCT", 180, 4, 6)
  number(base_note / 12 - 1, 206, 4, 10)
end
