-- popn.lua — a six-key rhythm game with no directions at all.
--
--   kessel run games/popn.lua
--   kessel render-audio games/popn.lua --frames 400 -o popn.wav
--
-- Notes fall down six coloured lanes; hit the matching key as one crosses the
-- judgment bar. Nothing here means "up" or "left" — the four direction bits are
-- plain buttons, which is what the labels in `controls` below declare. A host
-- reading that metadata draws a row of six keys instead of a d-pad, so the pad
-- under your thumbs looks like the lanes on the screen.
--
-- On a keyboard the six keys are, left to right: ← ↓ ↑ → Z X.

screen { mode = Extended240 }

-- Six labelled buttons and no d-pad. `dpad = false` is required alongside the
-- labels: the same four bits cannot be a direction and a key at once, and the
-- compiler says so rather than picking one.
controls {
  dpad  = false
  left  = "red"
  down  = "green"
  up    = "blue"
  right = "yellow"
  a     = "white"
  b     = "pink"
  pause = START
}

fx {
  reverb_size = 120
  reverb_damping = 100
}

-- What a caught note sounds like. Bright and short, so a fast run reads as
-- separate hits rather than one smear.
instrument key {
  wave = square
  attack = 0  decay = 200  sustain = 30  release = 80
  filter = lpf  cutoff = 200
  reverb = 55
  volume = 130
}

instrument bass {
  wave = triangle
  attack = 0  decay = 180  sustain = 50  release = 70
  filter = lpf  cutoff = 110
  volume = 100
}

instrument hat {
  wave = noise
  attack = 0  decay = 30  sustain = 0
  filter = hpf  cutoff = 180
  volume = 45
}

-- A key pressed with nothing under it. Dull and low: a wrong answer should be
-- audible without being musical.
instrument clank {
  wave = noise
  attack = 0  decay = 110  sustain = 0
  pitch_env = -20  pitch_decay = 60
  filter = lpf  cutoff = 60
  volume = 95
}

sfx whiff { inst = clank  notes = "38" }

-- The backing runs on the audio clock, so a dropped frame never drags the beat.
track groove {
  tempo = 11
  vel = 125
  bass = "36 - - - 43 - - - 41 - - - 43 - - -"
  hat  = ". 70 . 70 . 70 . 70 . 70 . 70 . 70 . 70"
}

record Note { lane, y, alive }

local LANES = 6
local LANE_W = 40
local BAR_Y = 188         -- the judgment row
local WINDOW = 10         -- how many pixels either side of it still count
local FALL = 3
local BOTTOM = 232        -- past here a note is gone

local notes: array(12, Note)
local pattern: array(16, byte)
local step = 0
local timer = 0
local score = 0
local missed = 0
local combo = 0

-- Per-lane flash counters: how many frames of "you just hit this" are left.
-- One slot per lane, because six keys can be held at once and a single shared
-- counter would make a chord look like one press.
local flash: array(6, byte)

function init()
  clear(notes)
  clear(flash)
  step = 0
  timer = 0
  score = 0
  missed = 0
  combo = 0

  -- 9 is a rest. Read with `groove` above: this is a pentatonic run over the
  -- bassline, so any chart the player survives is consonant.
  pattern[0] = 0   pattern[1] = 9   pattern[2] = 2   pattern[3] = 4
  pattern[4] = 9   pattern[5] = 1   pattern[6] = 3   pattern[7] = 9
  pattern[8] = 5   pattern[9] = 4   pattern[10] = 9  pattern[11] = 2
  pattern[12] = 0  pattern[13] = 3  pattern[14] = 9  pattern[15] = 5

  music(groove)
end

-- Lane -> gamepad bit, in the order a host lays the keys out: the four
-- direction bits left to right, then the action buttons. The pad's row and the
-- screen's lanes have to agree, or the game is unplayable on a phone while
-- looking fine on a keyboard.
function lane_bit(l)
  if l == 0 then return LEFT end
  if l == 1 then return DOWN end
  if l == 2 then return UP end
  if l == 3 then return RIGHT end
  if l == 4 then return A end
  return B
end

-- Lane -> palette index, matching the labels in `controls`.
function lane_color(l)
  if l == 0 then return 8 end    -- red
  if l == 1 then return 11 end   -- green
  if l == 2 then return 12 end   -- blue
  if l == 3 then return 10 end   -- yellow
  if l == 4 then return 7 end    -- white
  return 14                      -- pink
end

-- Lane -> MIDI note. C major pentatonic across an octave and a bit.
function lane_pitch(l)
  if l == 0 then return 60 end
  if l == 1 then return 62 end
  if l == 2 then return 64 end
  if l == 3 then return 67 end
  if l == 4 then return 69 end
  return 72
end

function lane_x(l)
  return l * LANE_W
end

function spawn(l)
  for i = 0, len(notes) - 1 do
    if notes[i].alive == 0 then
      notes[i].lane = l
      notes[i].y = 0
      notes[i].alive = 1
      return
    end
  end
end

-- Retire the lowest note in `l` that is inside the window, and report whether
-- there was one. A press with nothing under it is a whiff, and a press with a
-- note is worth exactly one note — holding a key must not clear a lane.
function judge(l)
  for i = 0, len(notes) - 1 do
    if notes[i].alive == 1 and notes[i].lane == l then
      if notes[i].y + WINDOW >= BAR_Y and notes[i].y <= BAR_Y + WINDOW then
        notes[i].alive = 0
        return 1
      end
    end
  end
  return 0
end

function update()
  for l = 0, LANES - 1 do
    if flash[l] > 0 then flash[l] = flash[l] - 1 end
    if btnp(lane_bit(l)) then
      if judge(l) == 1 then
        score = score + 1
        combo = combo + 1
        flash[l] = 8
        -- Velocity rises with the combo, so a clean run gets louder.
        play(key, lane_pitch(l), min(120 + combo * 5, 255), 30)
      else
        combo = 0
        sfx(whiff)
      end
    end
  end

  if timer == 0 then
    timer = 20
    if pattern[step] < LANES then spawn(pattern[step]) end
    step = step + 1
    if step == len(pattern) then step = 0 end
  end
  timer = timer - 1

  for i = 0, len(notes) - 1 do
    if notes[i].alive == 1 then
      notes[i].y = notes[i].y + FALL
      if notes[i].y > BOTTOM then
        notes[i].alive = 0
        missed = missed + 1
        combo = 0
      end
    end
  end
end

function draw()
  cls(0)

  -- The lanes, each in its own colour at low intensity, with the key at the
  -- bottom lighting up when it is hit.
  for l = 0, LANES - 1 do
    local x = lane_x(l)
    local c = lane_color(l)
    for y = 0, 5 do
      hline(x + 1, x + LANE_W - 2, 12 + y * 34, 1)
    end
    -- The key face. Bright while flashing, dim otherwise.
    local top = BAR_Y + WINDOW
    for y = 0, 22 do
      if flash[l] > 0 then
        hline(x + 2, x + LANE_W - 3, top + y, c)
      else
        hline(x + 2, x + LANE_W - 3, top + y, 5)
      end
    end
    -- A stripe of the lane's colour, so an unpressed key still says which is
    -- which.
    hline(x + 2, x + LANE_W - 3, top, c)
    hline(x + 2, x + LANE_W - 3, top + 22, c)
  end

  -- The judgment bar, across every lane.
  hline(0, 239, BAR_Y, 6)
  hline(0, 239, BAR_Y + WINDOW, 6)

  -- Falling notes: a solid block in the lane's colour.
  for i = 0, len(notes) - 1 do
    if notes[i].alive == 1 then
      local x = lane_x(notes[i].lane)
      local c = lane_color(notes[i].lane)
      for y = 0, 7 do
        hline(x + 4, x + LANE_W - 5, notes[i].y + y, c)
      end
      entity(x + LANE_W / 2, notes[i].y, 1)
    end
  end

  text("HIT", 4, 4, 6)
  number(score, 30, 4, 7)
  text("MISS", 84, 4, 6)
  number(missed, 122, 4, 8)
  text("COMBO", 168, 4, 6)
  number(combo, 216, 4, 10)
end
