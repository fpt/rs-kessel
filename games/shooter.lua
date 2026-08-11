-- shooter.lua — a vertical space shooter. Arrows move, A (Z key) fires. Bullets
-- destroy the descending foes; each kill scores a point. Colliding with a foe
-- ends the run; press A to restart.
--
--   kessel run games/shooter.lua

-- Sound. Instruments are synth patches and each `sfx` is a short line of notes
-- on one of them: a note number starts a note, `-` holds it, `.` rests, and
-- `speed` is frames per row. Check what a game actually plays with
-- `kessel render-audio games/shooter.lua --buttons A`.
-- One chorus and one reverb for the whole mix; instruments choose how much of
-- themselves to send. A short, dark room: explosions should feel like space,
-- not like a cathedral.
fx {
  reverb_size = 120
  reverb_damping = 180
}

instrument zap {
  wave = saw
  attack = 0  decay = 90  sustain = 0
  pitch_env = 30  pitch_decay = 60   -- the downward sweep that makes it a laser
  filter = lpf  cutoff = 200
  volume = 150
  reverb = 30                        -- a touch of space, not a wash
}

instrument boom {
  wave = noise
  attack = 0  decay = 220  sustain = 0
  pitch_env = -18  pitch_decay = 120 -- pitch falling = something big broke
  filter = lpf  cutoff = 140
  volume = 200
  reverb = 90                        -- the big one gets the room
}

-- Music. A `track` is channels of rows, one channel per instrument, `tempo`
-- frames per row. It plays on the AUDIO clock, so a dropped frame drops a
-- frame instead of stuttering the tune.
instrument pulse {
  wave = square
  attack = 0  decay = 90  sustain = 60  release = 40
  filter = lpf  cutoff = 150
  volume = 90
}

instrument thud {
  wave = triangle
  attack = 0  decay = 140  sustain = 0
  pitch_env = 12  pitch_decay = 60
  volume = 110
}

track drive {
  tempo = 9
  vel = 150                          -- music sits under the sound effects
  thud  = "33 . 33 . 40 . 33 ."
  pulse = "57 60 64 60 57 60 63 60"
}

sfx shoot   { inst = zap   speed = 1  notes = "84" }
sfx explode { inst = boom  speed = 3  notes = "48 - -" }
sfx gameover {
  inst = zap
  speed = 6
  notes = "60 55 51 46 - -"          -- a falling arpeggio, not a chord
}

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move
  a = "fire / restart"
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
local dead = 0

function init()
  clear(bullets)                    -- zero both pools in one call (alive -> 0)
  clear(foes)
  px = 60
  cd = 0
  spawn = 0
  score = 0
  dead = 0
  music(drive)                       -- loops until something stops it
end

function fire()
  -- len(bullets) follows the array's declared size, so resizing the pool needs
  -- no edits here (or in any other loop below).
  for i = 0, len(bullets) - 1 do
    if bullets[i].alive == 0 then
      -- The bullet sprite's visible pixels are columns 3-4, matching the ship's
      -- centreline when both sprites share the same x origin.
      bullets[i].x = px
      bullets[i].y = 111
      bullets[i].alive = 1
      sfx(shoot)
      return
    end
  end
end

function spawn_foe()
  for i = 0, len(foes) - 1 do
    if foes[i].alive == 0 then
      foes[i].x = rnd(120)
      foes[i].y = 0
      foes[i].alive = 1
      return
    end
  end
end

function update()
  if dead == 1 then
    if btnp(A) then init() end
    return
  end

  if btn(LEFT)  then px = px - 2 end
  if btn(RIGHT) then px = px + 2 end
  -- px is a signed int (goes negative at the left edge), so clamp with signed
  -- comparisons — min/max use unsigned LT/GT and would wrap a negative px.
  if px < 0 then px = 0 end
  if px > 120 then px = 120 end

  if cd > 0 then cd = cd - 1 end
  if btn(A) and cd == 0 then fire()  cd = 8 end

  -- move bullets up
  for i = 0, len(bullets) - 1 do
    if bullets[i].alive == 1 then
      if bullets[i].y < 3 then
        bullets[i].alive = 0
      else
        bullets[i].y = bullets[i].y - 3
      end
    end
  end

  -- spawn + advance foes
  spawn = spawn + 1
  if spawn % 30 == 0 then spawn_foe() end
  for i = 0, len(foes) - 1 do
    if foes[i].alive == 1 then
      foes[i].y = foes[i].y + 1
      if foes[i].y >= 128 then foes[i].alive = 0 end
    end
  end

  -- Use an inset ship hitbox so transparent wing corners do not feel unfair.
  for i = 0, len(foes) - 1 do
    if foes[i].alive == 1 and rect_overlap(px + 1, 113, 6, 6, foes[i].x, foes[i].y, 8, 8) then
      dead = 1
      music_stop()                   -- the tune gets out of the way
      sfx(gameover)
      return
    end
  end

  -- bullet vs foe
  for i = 0, len(bullets) - 1 do
    if bullets[i].alive == 1 then
      for j = 0, len(foes) - 1 do
        if foes[j].alive == 1 and rect_overlap(bullets[i].x + 3, bullets[i].y, 2, 4, foes[j].x, foes[j].y, 8, 8) then
          foes[j].alive = 0
          bullets[i].alive = 0
          score = score + 1
          sfx(explode)
          break                 -- this bullet is spent: don't let it kill more foes
        end
      end
    end
  end
end

function draw()
  cls(0)
  if dead == 0 then spr(ship, px, 112, 0) end
  -- Each pool is walked by its own len(), so the two arrays stay independent.
  for i = 0, len(bullets) - 1 do
    if bullets[i].alive == 1 then spr(bull, bullets[i].x, bullets[i].y, 0) end
  end
  for i = 0, len(foes) - 1 do
    if foes[i].alive == 1 then spr(foe, foes[i].x, foes[i].y, 0) end
  end
  text("SCORE", 2, 2, 7)          -- HUD: a label...
  number(score, 40, 2, 7)          -- ...and the live score
  if dead == 1 then
    text("GAME OVER", 46, 54, 8)
    text("PRESS A", 50, 64, 7)
  end
  entity(px, 112, dead + 1)
end
