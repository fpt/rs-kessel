-- shooter.lua — a vertical shoot-em-up in the Raiden mould, on the 240x240
-- screen. One stage: out over open sea, in across the surf and the beach, over
-- fields and a supply road, and into a super-heavy tank that fills a quarter of
-- the screen. Arrows move, A fires, B drops a bomb. The vulcan starts as one
-- round up the centreline and grows to a five-way; from its second level the
-- fighter also throws steering missiles off the wingtips.
--
--   kessel run games/shooter.lua
--
-- The art is in `shooter/`, one file per set: a 32x32 fighter in four poses and
-- a 64x64 boss are 200 rows of pixels, and the game is 400 lines of arithmetic —
-- in one file neither is findable. `screen` and `controls` stay here, since they
-- are the ROM's identity and an include may not set them.
#include "shooter/ship.lua"
#include "shooter/foes.lua"
#include "shooter/boss.lua"
#include "shooter/fx.lua"

-- Five things here are load-bearing:
--
-- * **The terrain is one hline per screen row**, coloured by the *world* row
--   `wy = scroll - y` — the outrun road turned ninety degrees. A vertical
--   scroller's ground is bands, so a band table indexed by distance flown is the
--   whole renderer, and "the stage goes from sea to land" becomes one function
--   of `wy` rather than a level format.
--
-- * **The terrain has its own `pal` ramp at 200-213.** Three sprite banks own
--   palette indices 16-63, and that range is exactly the default cube's `r = 0`
--   plane — every dark saturated blue and green the sea and the fields need. So
--   the terrain names its own colours instead of fighting for the leftovers.
--
-- * **Ground enemies move at the scroll speed and air enemies do not.** A tank
--   is a thing at a world position; a jet flies. Giving them one shared `vy`
--   made the tanks slide over their own ground, which reads as ice.
--
-- * **`scroll` stops when the boss arrives.** The stage is a distance, the boss
--   is an event; if the ground kept moving the fight would drift into the sea.
--
-- * **Signed things are `int`, and hit points are signed things.** Anything that
--   can go left, or negative for one frame at a screen edge, has to be: `word`
--   comparisons are unsigned, so a bullet at x = -1 reads as 65535 and is never
--   cleaned up. Health is the same trap wearing a different hat — `hp = hp - 3`
--   on 2 hit points is 65535, and `hp <= 0` is then *false*. As words, the boss
--   was immortal whenever two rounds landed on the same frame, and a bomb could
--   not finish anything whose health it overshot.

screen { mode = Extended240 }

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move
  a = "vulcan"
  b = "bomb"
  pause = START
}

-- ---------------------------------------------------------------- sound ----
-- One chorus and one reverb for the whole mix; instruments choose how much of
-- themselves to send. A short, dark room: explosions should feel like sky, not
-- like a cathedral.
fx {
  reverb_size = 120
  reverb_damping = 180
}

instrument zap {
  wave = saw
  attack = 0  decay = 90  sustain = 0
  pitch_env = 30  pitch_decay = 60   -- the downward sweep that makes it a laser
  filter = lpf  cutoff = 200
  volume = 130
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

-- The bomb: the same falling noise as an explosion but an octave down and four
-- times as long, so the screen-clear sounds like a different event and not like
-- eight enemies dying at once (which is also what is happening).
instrument sub {
  wave = sine
  attack = 0  decay = 255  sustain = 0
  pitch_env = -40  pitch_decay = 200
  filter = lpf  cutoff = 90
  volume = 220
  reverb = 120
}

instrument blip {
  wave = square
  attack = 0  decay = 60  sustain = 0
  filter = lpf  cutoff = 210
  volume = 140
}

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

-- Music. A `track` is channels of rows, one channel per instrument, `tempo`
-- frames per row. It plays on the AUDIO clock, so a dropped frame drops a frame
-- instead of stuttering the tune.
track drive {
  tempo = 9
  vel = 150                          -- music sits under the sound effects
  thud  = "33 . 33 . 40 . 33 ."
  pulse = "57 60 64 60 57 60 63 60"
}

-- The boss gets its own: same tempo, minor, and the bass on every row rather
-- than every other one.
track march {
  tempo = 8
  vel = 160
  thud  = "31 31 31 31 38 38 31 31"
  pulse = "55 58 62 58 55 58 61 63"
}

sfx shoot   { inst = zap   speed = 1  notes = "84" }
sfx explode { inst = boom  speed = 3  notes = "48 - -" }
sfx bigboom { inst = boom  speed = 5  notes = "36 - - - -" }
sfx bombhit { inst = sub   speed = 8  notes = "30 - - - - -" }
sfx pickup  { inst = blip  speed = 2  notes = "72 79 84" }
sfx tick    { inst = blip  speed = 1  notes = "60" }
sfx warn    { inst = zap   speed = 6  notes = "72 60 72 60" }
sfx gameover {
  inst = zap
  speed = 6
  notes = "60 55 51 46 - -"          -- a falling arpeggio, not a chord
}

-- ------------------------------------------------------------ constants ----
local DIM = 240
local RIGHT_EDGE = 208               -- DIM - 32, the ship's right limit

-- How far into the stage each thing happens, in world rows.
local SEA_END = 900
local SURF_END = 980
local BEACH_END = 1220
local GRASS_AT = 1300
local BOSS_AT = 2200

local SCROLL_SPEED = 2

-- The terrain ramp. Indices 200-213 because 16-63 belong to the three sprite
-- banks; see the note at the top.
local C_DEEP = 200   local C_SEA = 201   local C_SHALLOW = 202
local C_WAVE = 203   local C_FOAM = 204  local C_WET = 205
local C_SAND = 206   local C_DUNE = 207  local C_GRASS_D = 208
local C_GRASS = 209  local C_GRASS_L = 210  local C_FOREST = 211
local C_ROAD = 212   local C_KERB = 213

local SHIP_HIT = 8                   -- the fighter's hitbox, inset hard: the
local SHIP_HIT_OFF = 12              -- wings are decoration, the fuselage is you

local BOSS_HP = 220
local BOSS_W = 64

-- ---------------------------------------------------------------- state ----
record Shot { x: int, y: int, vx: int, vy: int, kind, alive }
record Foe  { x: int, y: int, vx: int, kind, hp: int, t, ground, alive }
record Pup  { x: int, y: int, kind, alive }
record Boom { x: int, y: int, t, big, alive }

local pshot: array(16, Shot)
local eshot: array(28, Shot)
local foes: array(10, Foe)
local pups: array(4, Pup)
local booms: array(10, Boom)

local px: int = 104
local py: int = 170
local lean: int = 0        -- which way the fighter is banking, -1/0/1
local cd = 0               -- vulcan cooldown
local mcd = 0              -- missile cooldown, much longer
local power = 1            -- vulcan level, 1..4
local bombs = 3
local lives: int = 3
local invuln = 0           -- frames of grace after a respawn
local score = 0
local flash = 0            -- bomb screen-flash timer

local scroll = 239         -- world row at the top of the screen
local spawn = 0
local wave = 0             -- animates the surf

local state = 0            -- 0 playing, 1 game over, 2 stage clear
local warn_t = 0

local bx: int = 88         -- the boss
local by: int = 0-64
local bhp: int = 0
local bt = 0
local bactive = 0

function init()
  clear(pshot)  clear(eshot)  clear(foes)  clear(pups)  clear(booms)
  px = 104  py = 170  lean = 0
  cd = 0  mcd = 0  power = 1  bombs = 3  lives = 3  invuln = 90
  score = 0  flash = 0
  scroll = 239  spawn = 0  wave = 0
  state = 0  warn_t = 0
  bx = 88  by = 0 - 64  bhp = 0  bt = 0  bactive = 0

  ship_palette()
  foe_palette()
  boss_palette()

  -- The terrain's own ramp. Sea through surf to sand to grass, plus the road.
  pal(C_DEEP, 12, 26, 58)
  pal(C_SEA, 20, 46, 96)
  pal(C_SHALLOW, 30, 78, 140)
  pal(C_WAVE, 60, 130, 190)
  pal(C_FOAM, 210, 235, 245)
  pal(C_WET, 170, 148, 104)
  pal(C_SAND, 222, 200, 146)
  pal(C_DUNE, 240, 226, 180)
  pal(C_GRASS_D, 74, 106, 48)
  pal(C_GRASS, 96, 134, 58)
  pal(C_GRASS_L, 124, 162, 76)
  pal(C_FOREST, 52, 78, 36)
  pal(C_ROAD, 108, 106, 100)
  pal(C_KERB, 148, 146, 138)

  music(drive)                       -- loops until something stops it
end

-- ----------------------------------------------------------- the pools -----
-- `kind` is 0 for a vulcan round and 1 for a missile. One pool and one field
-- rather than a second array: they move, collide and expire identically, and
-- everything that differs — sprite, damage, whether it steers — is a branch on
-- one byte at the three places that care.
function add_shot(x: int, y: int, vx: int, vy: int, kind)
  for i = 0, len(pshot) - 1 do
    if pshot[i].alive == 0 then
      pshot[i].x = x  pshot[i].y = y
      pshot[i].vx = vx  pshot[i].vy = vy
      pshot[i].kind = kind  pshot[i].alive = 1
      return
    end
  end
end

function add_eshot(x: int, y: int, vx: int, vy: int)
  for i = 0, len(eshot) - 1 do
    if eshot[i].alive == 0 then
      eshot[i].x = x  eshot[i].y = y
      eshot[i].vx = vx  eshot[i].vy = vy
      eshot[i].kind = 0  eshot[i].alive = 1
      return
    end
  end
end

function add_boom(x: int, y: int, big)
  for i = 0, len(booms) - 1 do
    if booms[i].alive == 0 then
      booms[i].x = x  booms[i].y = y
      booms[i].t = 0  booms[i].big = big  booms[i].alive = 1
      return
    end
  end
end

function add_pup(x: int, y: int, kind)
  for i = 0, len(pups) - 1 do
    if pups[i].alive == 0 then
      pups[i].x = x  pups[i].y = y  pups[i].kind = kind  pups[i].alive = 1
      return
    end
  end
end

function add_foe(x: int, y: int, kind, hp, ground)
  for i = 0, len(foes) - 1 do
    if foes[i].alive == 0 then
      foes[i].x = x  foes[i].y = y  foes[i].vx = 0
      foes[i].kind = kind  foes[i].hp = hp  foes[i].t = 0
      foes[i].ground = ground  foes[i].alive = 1
      return
    end
  end
end

-- ------------------------------------------------------------- the ship ----
-- The vulcan. Level 1 is a single round up the centreline; each level adds a
-- pair, and the outer pairs fan out. Spelt out rather than computed because
-- four hand-placed patterns is what a shooter's feel actually is.
function fire()
  local cx: int = px + 12
  add_shot(cx, py, 0, 0 - 7, 0)
  if power >= 2 then
    add_shot(cx - 9, py + 4, 0, 0 - 7, 0)
    add_shot(cx + 9, py + 4, 0, 0 - 7, 0)
  end
  if power >= 3 then
    add_shot(cx - 12, py + 8, 0 - 2, 0 - 6, 0)
    add_shot(cx + 12, py + 8, 2, 0 - 6, 0)
  end
  if power >= 4 then
    add_shot(cx - 14, py + 10, 0 - 4, 0 - 5, 0)
    add_shot(cx + 14, py + 10, 4, 0 - 5, 0)
  end
  sfx(shoot)
end

-- Missiles, from vulcan level 2 up. They come off the wingtips angled outwards
-- and then steer back in, which is what makes them feel like a second weapon
-- rather than more vulcan: the stream in front of you is yours to aim, and the
-- missiles are what covers the thing you did not line up on.
--
-- On their own cooldown, three times the vulcan's. A missile that fired as often
-- as the gun would make the gun pointless, and the frame is loud enough already.
function fire_missiles()
  local cx: int = px + 12
  add_shot(cx - 14, py + 10, 0 - 2, 0 - 5, 1)
  add_shot(cx + 14, py + 10, 2, 0 - 5, 1)
  if power >= 4 then
    add_shot(cx - 6, py + 12, 0 - 1, 0 - 5, 1)
    add_shot(cx + 6, py + 12, 1, 0 - 5, 1)
  end
end

-- A bomb clears every enemy shot on the screen and hurts everything on it. The
-- shots go first: the point of a bomb is the half second of safety, and an
-- enemy that dies but leaves its bullets behind has not given you that.
function bomb()
  bombs = bombs - 1
  flash = 24
  sfx(bombhit)
  for i = 0, len(eshot) - 1 do
    eshot[i].alive = 0
  end
  for i = 0, len(foes) - 1 do
    if foes[i].alive == 1 then
      foes[i].hp = foes[i].hp - 12
      add_boom(foes[i].x, foes[i].y, 0)
      if foes[i].hp <= 0 then
        foes[i].alive = 0
        score = score + 10
      end
    end
  end
  if bactive == 1 then
    bhp = bhp - 30
    add_boom(bx + 16, by + 16, 1)
  end
end

function kill_player()
  add_boom(px, py, 1)
  sfx(bigboom)
  lives = lives - 1
  power = 1
  if lives <= 0 then
    state = 1
    music_stop()                     -- the tune gets out of the way
    sfx(gameover)
  else
    px = 104  py = 170  invuln = 120
    if bombs < 2 then bombs = 2 end
  end
end

function hit_player()
  if invuln > 0 then return end
  kill_player()
end

-- ---------------------------------------------------------- the enemies ----
-- A coarse five-way aim: near enough to make the player move, cheap enough to
-- do per shot. A real unit vector needs a signed divide, and `/` is unsigned.
function aim_x(fx: int)
  local dx: int = px + 12 - fx
  if dx > 40 then return 3 end
  if dx > 12 then return 2 end
  if dx > 3 then return 1 end
  if dx < 0 - 40 then return 0 - 3 end
  if dx < 0 - 12 then return 0 - 2 end
  if dx < 0 - 3 then return 0 - 1 end
  return 0
end

function spawn_wave()
  local frontier = scroll
  if frontier < SEA_END then
    -- Open sea: jets in from the top, the odd patrol boat riding the swell.
    if rnd(3) == 0 then
      add_foe(rnd(200) + 8, 0 - 16, 1, 6, 1)          -- boat
    else
      add_foe(rnd(200) + 8, 0 - 16, 0, 3, 0)          -- jet
    end
  elseif frontier < BEACH_END then
    -- The beach: emplacements dug into the sand, jets overhead.
    if rnd(2) == 0 then
      add_foe(rnd(200) + 8, 0 - 16, 2, 10, 1)         -- gun
    else
      add_foe(rnd(200) + 8, 0 - 16, 0, 3, 0)          -- jet
    end
  else
    -- Inland: armour.
    local r = rnd(4)
    if r == 0 then add_foe(rnd(200) + 8, 0 - 16, 0, 3, 0)
    elseif r == 1 then add_foe(rnd(200) + 8, 0 - 16, 2, 10, 1)
    else add_foe(rnd(200) + 8, 0 - 16, 3, 8, 1) end   -- tank
  end
end

function foe_shoot(i)
  local fx: int = foes[i].x + 8
  local fy: int = foes[i].y + 8
  local k = foes[i].kind
  if k == 2 then
    -- An emplacement cannot move, so it gets the spread.
    add_eshot(fx, fy, 0 - 2, 2)
    add_eshot(fx, fy, 0, 3)
    add_eshot(fx, fy, 2, 2)
  else
    add_eshot(fx, fy, aim_x(fx), 3)
  end
end

-- ------------------------------------------------------------- updating ----
function update_player()
  local mx: int = 0
  if btn(LEFT)  then mx = 0 - 3 end
  if btn(RIGHT) then mx = 3 end
  px = px + mx
  if btn(UP)   then py = py - 3 end
  if btn(DOWN) then py = py + 3 end
  if px < 0 then px = 0 end
  if px > RIGHT_EDGE then px = RIGHT_EDGE end
  if py < 0 then py = 0 end
  if py > 200 then py = 200 end

  -- The bank pose eases rather than snapping: at three frames from level to
  -- full lock the fighter flickers between two sprites on every tap.
  if mx > 0 then
    if lean < 6 then lean = lean + 2 end
  elseif mx < 0 then
    if lean > 0 - 6 then lean = lean - 2 end
  else
    if lean > 0 then lean = lean - 1 end
    if lean < 0 then lean = lean + 1 end
  end

  if cd > 0 then cd = cd - 1 end
  if mcd > 0 then mcd = mcd - 1 end
  if btn(A) and cd == 0 then fire()  cd = 7 end
  if btn(A) and mcd == 0 and power >= 2 then fire_missiles()  mcd = 22 end
  if btnp(B) and bombs > 0 then bomb() end
  if invuln > 0 then invuln = invuln - 1 end
end

-- Which way a missile at `mx` should lean to find something. Nearest *in x*
-- only, and only among enemies still above it: a missile that turned round to
-- chase what it had already passed would spiral, and there is no facing to draw
-- it with anyway.
function steer(mx: int, my: int)
  local best = 999
  local dir: int = 0
  for j = 0, len(foes) - 1 do
    if foes[j].alive == 1 and foes[j].y < my then
      local d: int = foes[j].x + 8 - mx
      local m = d
      if d < 0 then m = 0 - d end
      if m < best then
        best = m
        dir = 0
        if d > 2 then dir = 1 end
        if d < 0 - 2 then dir = 0 - 1 end
      end
    end
  end
  if bactive == 1 and by + 58 < my then
    local d: int = bx + 26 - mx
    if d > 2 then dir = 1
    elseif d < 0 - 2 then dir = 0 - 1
    else dir = 0 end
  end
  return dir
end

function update_shots()
  for i = 0, len(pshot) - 1 do
    if pshot[i].alive == 1 then
      if pshot[i].kind == 1 then
        -- One pixel of turn per frame, clamped: enough to curve onto a target a
        -- third of the screen away, slow enough that the arc is visible.
        local dir: int = steer(pshot[i].x + 4, pshot[i].y)
        pshot[i].vx = pshot[i].vx + dir
        if pshot[i].vx > 4 then pshot[i].vx = 4 end
        if pshot[i].vx < 0 - 4 then pshot[i].vx = 0 - 4 end
      end
      pshot[i].x = pshot[i].x + pshot[i].vx
      pshot[i].y = pshot[i].y + pshot[i].vy
      if pshot[i].y < 0 - 8 then pshot[i].alive = 0 end
      if pshot[i].x < 0 - 8 then pshot[i].alive = 0 end
      if pshot[i].x > DIM then pshot[i].alive = 0 end
    end
  end
  for i = 0, len(eshot) - 1 do
    if eshot[i].alive == 1 then
      eshot[i].x = eshot[i].x + eshot[i].vx
      eshot[i].y = eshot[i].y + eshot[i].vy
      if eshot[i].y > DIM then eshot[i].alive = 0 end
      if eshot[i].x < 0 - 8 then eshot[i].alive = 0 end
      if eshot[i].x > DIM then eshot[i].alive = 0 end
      if eshot[i].alive == 1 and invuln == 0 then
        if rect_overlap(px + SHIP_HIT_OFF, py + SHIP_HIT_OFF, SHIP_HIT, SHIP_HIT,
                        eshot[i].x, eshot[i].y, 6, 6) then
          eshot[i].alive = 0
          hit_player()
          return
        end
      end
    end
  end
end

function update_foes()
  for i = 0, len(foes) - 1 do
    if foes[i].alive == 1 then
      foes[i].t = foes[i].t + 1
      if foes[i].ground == 1 then
        -- Anchored to the ground, so it moves at exactly the scroll speed.
        foes[i].y = foes[i].y + SCROLL_SPEED
      else
        foes[i].y = foes[i].y + 2
        -- A slow weave, from the same fixed-point trig the road racer uses.
        local s: int = sin(foes[i].t * 2)
        if s > 0 then foes[i].x = foes[i].x + s / 160
        else foes[i].x = foes[i].x - (0 - s) / 160 end
      end
      if foes[i].y > DIM then foes[i].alive = 0 end
      if foes[i].t % 70 == 30 then foe_shoot(i) end

      if foes[i].alive == 1 and invuln == 0 then
        if rect_overlap(px + SHIP_HIT_OFF, py + SHIP_HIT_OFF, SHIP_HIT, SHIP_HIT,
                        foes[i].x, foes[i].y, 16, 16) then
          hit_player()
          return
        end
      end
    end
  end
end

-- Every eighth kill leaves a pickup, alternating power and bombs, so a player
-- who keeps shooting keeps upgrading without the drop feeling random.
local kills = 0

function foe_died(i)
  add_boom(foes[i].x, foes[i].y, 0)
  sfx(explode)
  score = score + 10
  foes[i].alive = 0
  kills = kills + 1
  if kills % 8 == 0 then
    local kind = 0
    if kills % 16 == 0 then kind = 1 end
    add_pup(foes[i].x, foes[i].y, kind)
  end
end

-- A missile is worth three rounds. It also fires a third as often, so the two
-- weapons carry about the same damage per second and the missiles are aim
-- assistance rather than a straight upgrade that retires the gun.
function shot_damage(i)
  if pshot[i].kind == 1 then return 9 end
  return 3
end

function update_hits()
  for i = 0, len(pshot) - 1 do
    if pshot[i].alive == 1 then
      for j = 0, len(foes) - 1 do
        if foes[j].alive == 1 and rect_overlap(pshot[i].x, pshot[i].y, 8, 8,
                                               foes[j].x, foes[j].y, 16, 16) then
          pshot[i].alive = 0
          foes[j].hp = foes[j].hp - shot_damage(i)
          if foes[j].hp <= 0 then foe_died(j) end
          break                 -- this round is spent: don't let it kill more
        end
      end
    end
    if pshot[i].alive == 1 and bactive == 1 then
      if rect_overlap(pshot[i].x, pshot[i].y, 8, 8, bx + 6, by + 6, 52, 52) then
        pshot[i].alive = 0
        bhp = bhp - shot_damage(i)
        if bt % 4 == 0 then sfx(tick) end
      end
    end
  end
end

function update_pups()
  for i = 0, len(pups) - 1 do
    if pups[i].alive == 1 then
      pups[i].y = pups[i].y + 1
      if pups[i].y > DIM then pups[i].alive = 0 end
      if rect_overlap(px + 4, py + 4, 24, 24, pups[i].x, pups[i].y, 16, 16) then
        pups[i].alive = 0
        sfx(pickup)
        score = score + 50
        if pups[i].kind == 0 then
          if power < 4 then power = power + 1 end
        else
          if bombs < 6 then bombs = bombs + 1 end
        end
      end
    end
  end
end

function update_booms()
  for i = 0, len(booms) - 1 do
    if booms[i].alive == 1 then
      booms[i].t = booms[i].t + 1
      if booms[i].t >= 16 then booms[i].alive = 0 end
    end
  end
end

-- The boss: in from the top, then a slow sweep left and right. The cannon fires
-- a fan the player has to move through; the secondaries pick at where they are.
function update_boss()
  bt = bt + 1
  if by < 24 then
    by = by + 1
    return
  end
  local s: int = sin(bt)
  if s > 0 then bx = 88 + s / 4 else bx = 88 - (0 - s) / 4 end

  if bt % 100 == 0 then
    add_eshot(bx + 30, by + 56, 0 - 4, 3)
    add_eshot(bx + 30, by + 56, 0 - 2, 4)
    add_eshot(bx + 30, by + 56, 0, 4)
    add_eshot(bx + 30, by + 56, 2, 4)
    add_eshot(bx + 30, by + 56, 4, 3)
  end
  if bt % 46 == 0 then
    add_eshot(bx + 8, by + 44, aim_x(bx + 8), 3)
    add_eshot(bx + 52, by + 44, aim_x(bx + 52), 3)
  end
  if bt % 23 == 0 then add_boom(bx + rnd(48), by + rnd(48), 0) end

  if invuln == 0 then
    if rect_overlap(px + SHIP_HIT_OFF, py + SHIP_HIT_OFF, SHIP_HIT, SHIP_HIT,
                    bx + 6, by + 6, 52, 52) then
      hit_player()
    end
  end

  if bhp <= 0 then
    bactive = 0
    state = 2
    score = score + 1000
    music_stop()
    sfx(bigboom)
    for i = 0, 5 do
      add_boom(bx + rnd(48), by + rnd(48), 1)
    end
  end
end

function update()
  if state > 0 then
    if btnp(A) then init() end
    update_booms()
    return
  end

  update_player()
  update_shots()
  update_foes()
  update_hits()
  update_pups()
  update_booms()

  if flash > 0 then flash = flash - 1 end
  wave = wave + 1

  if bactive == 1 then
    update_boss()
    return
  end

  -- Still flying the stage: scroll, spawn, and watch for the boss line.
  scroll = scroll + SCROLL_SPEED
  spawn = spawn + 1
  if spawn % 40 == 0 then spawn_wave() end

  if scroll > BOSS_AT - 300 and warn_t == 0 then
    warn_t = 1
    sfx(warn)
  end
  if warn_t > 0 then warn_t = warn_t + 1 end

  if scroll >= BOSS_AT then
    bactive = 1
    bhp = BOSS_HP
    bt = 0
    by = 0 - 64
    -- The siren has done its job. It is drawn while `bactive == 0`, so leaving
    -- it running means it comes back the moment the boss dies and flashes
    -- WARNING over STAGE CLEAR.
    warn_t = 0
    music(march)
  end
end

-- ------------------------------------------------------------- drawing -----
-- One hline per screen row, coloured by the world row it is showing. Bands only:
-- a vertical scroller's ground has no perspective to get wrong, so the whole
-- renderer is "which band is `wy` in".
function draw_terrain()
  local y = 0
  while y < DIM do
    local wy = scroll - y
    local c = C_SEA
    if wy < SEA_END then
      -- Open water is flat. Banding it — even in two close blues — reads as a
      -- striped rug, because a full-width hline is a horizon line and the eye
      -- takes every one of them as one. Texture comes from the whitecaps below
      -- instead: short dashes at scattered x, which read as water and cost two
      -- hlines on one row in forty.
      c = C_DEEP
      if wy > SEA_END - 90 then c = C_SEA end
      if wy > SEA_END - 40 then c = C_SHALLOW end
    elseif wy < SURF_END then
      -- Breaking water: alternating bands that crawl, so the surf looks alive
      -- rather than like a striped rug.
      if (wy + wave / 4) % 10 < 5 then c = C_FOAM else c = C_WAVE end
    elseif wy < BEACH_END then
      c = C_SAND
      if wy < SURF_END + 40 then c = C_WET end
      if wy % 40 < 2 then c = C_DUNE end
    else
      c = C_GRASS
      -- Fields: blocks of rows in three greens, so the ground reads as farmland
      -- and not as a lawn.
      local block = wy / 46 % 3
      if block == 1 then c = C_GRASS_D
      elseif block == 2 then c = C_GRASS_L end
      if wy < GRASS_AT then c = C_DUNE end
    end
    hline(0, DIM - 1, y, c)

    -- Whitecaps. `wave` crawls independently of `scroll`, so the sea still moves
    -- when the boss stops the stage.
    if wy < SEA_END - 40 and (wy + wave / 5) % 37 < 2 then
      local a = wy * 37 % 190
      hline(a, a + 13, y, C_WAVE)
      local b = wy * 91 % 190
      hline(b, b + 8, y, C_SHALLOW)
    end

    -- The supply road, once the fields start, with a dashed centre line.
    if wy >= GRASS_AT then
      hline(96, 143, y, C_ROAD)
      if wy % 24 < 10 then hline(118, 121, y, C_KERB) end
    end
    y = y + 1
  end
end

-- Scenery at fixed world positions, one slot every 64 rows down each edge, so
-- the ground has something the eye can track. Kept out of the middle: it is
-- decoration, and a tree the player mistakes for an enemy is worse than none.
function draw_props()
  local s = 0
  if scroll > DIM then s = (scroll - DIM) / 64 end
  local top = scroll / 64
  for i = s, top do
    local wy = i * 64
    if wy <= scroll then
      local y = scroll - wy
      if y < DIM then
        local id = tree
        if wy < BEACH_END then id = rock end
        -- Nothing stands in open water, and an empty sea is the point of the
        -- first stretch. Rocks start where the ground comes up to meet it.
        if wy >= SEA_END - 60 then
          -- A computed id, so the raw six-argument form: the short one reads the
          -- size off a declared sprite *name*, and there is no name here.
          sprn(id, i * 37 % 26, y, 2, 2, 0)
          sprn(id, DIM - 18 - (i * 53 % 22), y, 2, 2, 0)
        end
      end
    end
  end
end

function draw_hud()
  text("SCORE", 4, 4, 7)
  number(score, 52, 4, 7)
  text("PWR", 172, 4, 7)
  number(power, 204, 4, 7)
  -- Bombs as icons, the way an arcade cabinet shows them: a number is a stat,
  -- a row of pods is how many more times you are allowed to panic.
  --
  -- A `while` and not `for i = 0, bombs - 1`. `bombs` is a word, so at zero
  -- bombs that limit is 65535, and the loop draws sixty-five thousand pods and
  -- blows the frame's instruction cap. The counter is fine; the *bound* is the
  -- trap, and it only springs once you have spent your last bomb.
  local i = 0
  while i < bombs do
    sprn(pup_b, 4 + i * 18, 220, 0)
    i = i + 1
  end
  text("SHIPS", 172, 222, 7)
  number(lives, 216, 222, 7)

  if bactive == 1 then
    -- Boss health, drawn as a bar because a number would not read at a glance.
    local w = bhp * 232 / BOSS_HP
    hline(4, 235, 18, 5)
    if bhp > 0 then hline(4, 4 + w, 18, 8) end
    hline(4, 235, 19, 5)
    if bhp > 0 then hline(4, 4 + w, 19, 8) end
  end
end

function draw()
  draw_terrain()
  draw_props()

  for i = 0, len(pups) - 1 do
    if pups[i].alive == 1 then
      local id = pup_p
      if pups[i].kind == 1 then id = pup_b end
      sprn(id, pups[i].x, pups[i].y, 2, 2, 0)
    end
  end

  -- Everything from here to the matching sprbank(0) draws through the enemy
  -- bank. Forgetting the switch does not fail: the tiles come out in the base
  -- sixteen, which is a boat painted like a fire engine.
  sprbank(2)
  for i = 0, len(foes) - 1 do
    if foes[i].alive == 1 then
      -- The art was drawn barrel-up, the way a sprite sheet is laid out; on
      -- screen a gun that is shooting at you points down. flag bit 1 is flip-y,
      -- and `sprn` mirrors the whole block, so the barrel swaps ends rather than
      -- each tile flipping in place. The jet already faces the camera and the
      -- boat is just sailing, so those two are drawn as they were drawn.
      local id = jet
      local f = 0
      if foes[i].kind == 1 then id = boat
      elseif foes[i].kind == 2 then id = gun  f = 2
      elseif foes[i].kind == 3 then id = tank  f = 2 end
      sprn(id, foes[i].x, foes[i].y, 2, 2, f)
    end
  end
  sprbank(0)

  if bactive == 1 then
    sprbank(3)
    sprn(boss, bx, by, 2)      -- barrel down, at the player
    sprbank(0)
  end

  for i = 0, len(eshot) - 1 do
    if eshot[i].alive == 1 then spr(pellet, eshot[i].x, eshot[i].y, 0) end
  end
  for i = 0, len(pshot) - 1 do
    if pshot[i].alive == 1 then
      local id = shot
      if pshot[i].kind == 1 then id = missile end
      spr(id, pshot[i].x, pshot[i].y, 0)
    end
  end

  -- The fighter. Only the right-hand bank is drawn; banking left is the same
  -- sprite flipped, which `sprn` mirrors as a block and not tile by tile.
  -- Blink while the respawn grace lasts, so "you cannot be hit" is visible.
  if state == 0 then
    local show = 1
    if invuln > 0 and frame_count() % 8 < 4 then show = 0 end
    if show == 1 then
      sprbank(1)
      if lean >= 4 then sprn(ship_bank, px, py, 0)
      elseif lean <= 0 - 4 then sprn(ship_bank, px, py, 1)
      else sprn(ship, px, py, 0) end
      sprbank(0)
    end
  end

  for i = 0, len(booms) - 1 do
    if booms[i].alive == 1 then
      local f = booms[i].t / 4
      if booms[i].big == 1 then
        spr_scaled(boom0 + f * 9, booms[i].x - 8, booms[i].y - 8, 512, 0)
      end
      sprn(boom0 + f * 9, booms[i].x - 4, booms[i].y - 4, 3, 3, 0)
    end
  end

  -- A bomb whites out the sky for a few frames. Cheap, and it is what sells the
  -- screen-clear as an event rather than as eight enemies quietly vanishing.
  if flash > 0 then
    local y = flash % 3
    while y < DIM do
      hline(0, DIM - 1, y, 7)
      y = y + 3
    end
  end

  draw_hud()

  if warn_t > 0 and bactive == 0 and warn_t % 24 < 14 then
    text("WARNING", 88, 100, 8)
  end
  if state == 1 then
    text("GAME OVER", 84, 110, 8)
    text("PRESS A", 92, 124, 7)
  end
  if state == 2 then
    text("STAGE CLEAR", 76, 110, 10)
    text("PRESS A", 92, 124, 7)
  end
  -- Reported for observation: where the player is and what state the run is in,
  -- the two upgrade counters (an agent cannot read a HUD), and the boss while it
  -- is on the screen.
  entity(px, py, state + 1)
  entity(power, bombs, 8)
  if bactive == 1 then entity(bx, by, 9) end
end
