-- piano.lua — a falling-note catcher. Notes drop down five lanes of a piano
-- roll; move left and right to catch them, and each catch PLAYS its note.
--
--   kessel run games/piano.lua
--   kessel render-audio games/piano.lua --frames 400 -o piano.wav
--
-- The lanes are a C major pentatonic scale, so any order you catch them in is
-- consonant — the game cannot make you play a wrong note, only a quiet one.

controls {
  dpad = true       -- left/right move between lanes
  pause = START
}

-- Shared room. Small and bright: this is an instrument, not a cathedral.
fx {
  reverb_size = 110
  reverb_damping = 110
}

-- The note you play by catching. A plucked triangle with a little room on it.
instrument piano {
  wave = triangle
  attack = 0  decay = 260  sustain = 0
  filter = lpf  cutoff = 190
  reverb = 70
  volume = 150
}

-- The bassline under it all, and a soft pulse on the off-beats.
instrument bass {
  wave = square
  attack = 0  decay = 150  sustain = 40  release = 60
  filter = lpf  cutoff = 120
  volume = 95
}

instrument tick {
  wave = noise
  attack = 0  decay = 40  sustain = 0
  pitch_env = -10  pitch_decay = 25
  volume = 55
}

-- A missed note: a dull thud, deliberately unmusical.
instrument thud {
  wave = noise
  attack = 0  decay = 130  sustain = 0
  pitch_env = -24  pitch_decay = 70
  filter = lpf  cutoff = 70
  volume = 110
}

sfx miss { inst = thud  notes = "40" }

-- Music runs on the audio clock, so the backing never stutters no matter what
-- the game is doing.
track groove {
  tempo = 12
  vel = 130
  bass = "36 - - 43 - - 41 - - 43 - -"
  tick = ". 60 . . 60 . . 60 . . 60 ."
}

-- The catcher is 16 wide, so it is two tiles — and `sprn` draws a block of
-- CONTIGUOUS ids, so these two must stay adjacent and stay in this order.
sprite catcher_l {
  ........
  ...7777.
  ..7bbbbb
  .7bbbbbb
  .7bbbbbb
  ..7bbbbb
  ...7777.
  ........
}

sprite catcher_r {
  ........
  7777....
  bbbbb7..
  bbbbbb7.
  bbbbbb7.
  bbbbb7..
  7777....
  ........
}

sprite notespr {
  ..aaaa..
  .aaaaaa.
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  .aaaaaa.
  ..aaaa..
}

record Note { lane, y, alive }

local LANES = 5
local LANE_W = 24
local LANE_X = 4          -- left edge of lane 0
local CATCH_Y = 104       -- top of the catcher's row
local FALL = 2            -- pixels per frame

local notes: array(8, Note)
local lane = 2            -- where the catcher is
local score = 0
local missed = 0
local step = 0            -- position in `pattern`
local timer = 0           -- frames until the next spawn
local flash = 0           -- frames left of the catch flash

-- The melody, as lane numbers. 9 is a rest. Read together with `groove` above:
-- the bass is in C, and these are the pentatonic degrees over it.
local pattern: array(16, byte)

function init()
  clear(notes)
  lane = 2
  score = 0
  missed = 0
  step = 0
  timer = 0
  flash = 0

  pattern[0] = 0   pattern[1] = 2   pattern[2] = 4   pattern[3] = 9
  pattern[4] = 3   pattern[5] = 9   pattern[6] = 1   pattern[7] = 9
  pattern[8] = 4   pattern[9] = 3   pattern[10] = 2  pattern[11] = 9
  pattern[12] = 0  pattern[13] = 1  pattern[14] = 9  pattern[15] = 9

  music(groove)
end

-- Lane -> MIDI note. C major pentatonic: C D E G A.
function pitch(l)
  if l == 0 then return 60 end
  if l == 1 then return 62 end
  if l == 2 then return 64 end
  if l == 3 then return 67 end
  return 69
end

function lane_x(l)
  return LANE_X + l * LANE_W
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

function update()
  -- Move, one lane per press: this is a rhythm game, not a slider.
  if btnp(LEFT) and lane > 0 then lane = lane - 1 end
  if btnp(RIGHT) and lane < LANES - 1 then lane = lane + 1 end

  -- Spawn on the beat, following the pattern.
  if timer == 0 then
    timer = 24
    if pattern[step] < LANES then spawn(pattern[step]) end
    step = step + 1
    if step == len(pattern) then step = 0 end
  end
  timer = timer - 1

  if flash > 0 then flash = flash - 1 end

  for i = 0, len(notes) - 1 do
    if notes[i].alive == 1 then
      notes[i].y = notes[i].y + FALL
      if notes[i].y >= CATCH_Y and notes[i].lane == lane then
        -- Caught. Velocity rises with a run, so a good streak gets louder.
        notes[i].alive = 0
        score = score + 1
        flash = 6
        play(piano, pitch(notes[i].lane), min(140 + score * 6, 255), 40)
      end
      if notes[i].y > 120 then
        notes[i].alive = 0
        missed = missed + 1
        sfx(miss)
      end
    end
  end
end

function draw()
  cls(1)

  -- The roll: a dotted divider down each lane edge, and a bar across the
  -- catch row. The active lane is brighter, so you can see where you are
  -- before the note gets there.
  for l = 0, LANES do
    local x = LANE_X + l * LANE_W - 2
    for y = 0, 25 do
      pset(x, y * 5, 5)
    end
  end
  for l = 0, LANES - 1 do
    local x = lane_x(l)
    local shade = 5
    if l == lane then shade = 12 end
    hline(x, x + LANE_W - 4, CATCH_Y + 10, shade)
  end

  for i = 0, len(notes) - 1 do
    if notes[i].alive == 1 then
      local nx = lane_x(notes[i].lane) + 8
      spr(notespr, nx, notes[i].y, 0)
      entity(nx, notes[i].y, 1)
    end
  end

  local cx = lane_x(lane) + 4
  sprn(catcher_l, cx, CATCH_Y, 2, 1, 0)
  if flash > 0 then
    hline(cx - 2, cx + 17, CATCH_Y - 3, 10)
  end
  entity(cx, CATCH_Y, 0)

  text("SCORE", 2, 2, 7)
  number(score, 40, 2, 7)
  text("MISS", 82, 2, 8)
  number(missed, 116, 2, 8)
end
