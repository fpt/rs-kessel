-- dnb.lua — a drum'n'bass step machine.
--
--   kessel run games/dnb.lua
--
-- Two bars of 16ths at 174 BPM, eight tracks, punched in with a finger. What
-- this exists to make fast is not turning steps on and off — it is the part of
-- a breakbeat that takes the time: **how weak a hit is, how late it lands, and
-- how many of it there are.** So a step is not a bit. It carries a level, a
-- micro-timing nudge and a retrigger count, and all three are one byte.
--
-- Three decisions hold the rest up.
--
-- **A ghost note is a different patch, not a quieter one.** Velocity scales
-- amplitude and nothing else, so `snare` at velocity 50 is a small snare and
-- not a ghost — a real one is darker and shorter as well as weaker. Each drum
-- therefore declares its levels as separate instruments and `fire` picks one.
-- That is piano.lua's rule (a player-facing knob is a *choice between patches
-- declared up front*) applied to velocity, and the bank is metadata beside the
-- ROM, so twenty patches cost the game nothing.
--
-- **The step clock is a 1/64-frame accumulator.** 174 BPM is 5.172 frames per
-- 16th and no integer frame count reaches it: 5 is 180 BPM and 6 is 150. So
-- `step_len = 57600 / bpm` in 64ths of a frame, and the frame adds 64 to an
-- accumulator. 57600 is the largest numerator that survives a u16 — the
-- 1/256-frame version is 230400 and overflows.
--
-- **Every hit is a countdown in a queue, never an immediate `play`.** One
-- mechanism then covers ghosts, flams, rolls and swing: a roll is three
-- entries at spread delays, a nudge is a different starting count. The whole
-- pattern is scheduled one frame late so that "early" has somewhere to go —
-- 16.7 ms of uniform lateness nobody can hear, in exchange for a micro-timing
-- lane that reaches both directions.
--
-- The floor is the frame: 16.7 ms, about 19% of a 16th at this tempo. Nothing
-- finer is reachable, because `play` is stamped on the game's clock. (The one
-- sample-accurate thing here is a compile-time `track`, which a sequencer the
-- player edits at runtime cannot be.)

screen { mode = Landscape320 }

controls {
  dpad  = false
  touch = "edit"
  a     = "play / stop"
  b     = "next bar"
  pause = START
}

-- A small dark plate, not a hall: this is a machine sitting in a room.
fx {
  reverb_size = 150
  reverb_damping = 125
  chorus_rate = 34
  chorus_depth = 150
}

-- ---------------------------------------------------------------------------
-- The kit. Three levels where a level is audible as a *timbre* (snare, hat),
-- two where it is not (kick), one where the track has only one voice.

instrument kick {
  wave = sine
  attack = 0  decay = 125  sustain = 0  release = 40
  pitch_env = 42  pitch_decay = 55
  filter = lpf  cutoff = 125  resonance = 30
  distortion = 45
  volume = 232
}
instrument kick_gh {
  wave = sine
  attack = 0  decay = 70  sustain = 0  release = 24
  pitch_env = 30  pitch_decay = 40
  filter = lpf  cutoff = 100  resonance = 20
  volume = 116
}

-- The ghost is the point of this game: dark, clipped, and gone before the ear
-- decides what it was. Every difference from `sn_norm` matters — dropping the
-- cutoff alone gives a muffled snare, dropping the decay alone gives a tick.
instrument sn_ghost {
  wave = noise
  attack = 0  decay = 34  sustain = 0  release = 18
  filter = lpf  cutoff = 95  resonance = 70
  volume = 88  reverb = 18
}
instrument sn_norm {
  wave = noise
  attack = 0  decay = 105  sustain = 0  release = 60
  filter = lpf  cutoff = 168  resonance = 50
  distortion = 30
  volume = 186  reverb = 55
}
instrument sn_accent {
  wave = noise
  attack = 0  decay = 150  sustain = 0  release = 95
  filter = lpf  cutoff = 218  resonance = 40
  distortion = 50
  volume = 222  reverb = 72
}

instrument hh_ghost {
  wave = noise
  attack = 0  decay = 15  sustain = 0  release = 8
  filter = hpf  cutoff = 200
  volume = 57
}
instrument hh_norm {
  wave = noise
  attack = 0  decay = 26  sustain = 0  release = 12
  filter = hpf  cutoff = 212
  volume = 121
}
instrument hh_accent {
  wave = noise
  attack = 0  decay = 40  sustain = 0  release = 20
  filter = hpf  cutoff = 226
  volume = 169
}
instrument ohh {
  wave = noise
  attack = 0  decay = 200  sustain = 30  release = 150
  filter = hpf  cutoff = 196
  volume = 130  reverb = 45
}

instrument rim {
  wave = square
  attack = 0  decay = 28  sustain = 0  release = 14
  pitch_env = 20  pitch_decay = 12
  filter = hpf  cutoff = 155
  volume = 138
}
instrument clap {
  wave = noise
  attack = 2  decay = 88  sustain = 0  release = 70
  filter = lpf  cutoff = 190  resonance = 95
  volume = 155  reverb = 95
}

-- ---------------------------------------------------------------------------
-- Layout. 240 across: a 32 px name column and sixteen 13 px steps.

local DIM = 320

local HDR_Y = 0
local HDR_H = 22

local LBL_W = 32
-- The screen is 320 wide — the landscape one — and the width goes into the
-- step cells rather than into more steps. A bar is sixteen steps and the bar
-- buttons page by a bar, so showing two at once would quietly redefine what
-- "bar" means to `clear_bar` and to the playhead cap. Wider cells instead:
-- 18 px against 13 is a 38% bigger target for the finger that is doing all
-- the work here, and GRID_X + VIS*CELL_W is exactly 320.
local CELL_W = 18
local ROW_H = 22
local GRID_X = 32
local GRID_Y = 46
local VIS = 16                 -- steps on screen; the other bar is a toggle

-- The screen is split by *what a control is about*, not by how much room each
-- needed. Above the grid is the loop: which bar, whether playback stays in it,
-- and clearing one. Below the grid is the note: the level the next tap paints,
-- and the three attributes of the step under the finger.
--
-- The level row used to sit at the top, between the header and the bar
-- buttons, which put the two halves of one question — what level does the next
-- tap paint, what level is this step — at opposite ends of the screen with a
-- grid between them. They are one row now, and one control: see `set_pen`.
--
-- Everything a finger can hit is a flat box lit to neutral, and the grid
-- between the two bands is the only thing allowed to be dark.
--
-- The loop row, left to right: four bar buttons, the loop toggle, clear. The
-- bar buttons are first because they are the ones hit most often, and narrow
-- because a digit needs no room.
local BTN_Y = 24
local BTN_H = 18
local BAR_BTN_W = 40
local LOOP_X = 170
local LOOP_W = 70
local CLR_X = 248
local CLR_W = 64

-- The mixer's legend takes the row the grid does not need, directly under the
-- loop row. Grid view has nothing there: the grid starts at 46 and the band
-- below the buttons is the grid's own top edge.
local MIX_Y = 46
local MIX_H = 18

-- The mixer is six vertical channel strips, not six horizontal rows. 240
-- divides by six exactly, so each is 40 px with no remainder on the last one —
-- and a strip is the shape a mixer *is*: the fader travels the way a hand
-- pushes it, and six of them side by side can be read as a balance in one look,
-- which six bars stacked in rows cannot.
--
-- It also buys the travel. A row gave a fader 128 px lying down inside a 22 px
-- row; a strip gives it 108 px standing up, and the eight stops are 13 px apart
-- instead of 16 px apart but *aligned across all six channels*, so the shape of
-- the mix is a skyline.
local STRIP_W = 53         -- 6 * 53 = 318 of the 320-px width
local NAME_Y = 68
local FDR_Y = 82
local FDR_H = 108
local FDR_IN_X = 4
local FDR_IN_W = 24
local MTR_IN_X = 32
local MTR_IN_W = 17
local MUTE_Y = 194
local SOLO_Y = 216
local MS_H = 18

-- The note band: a line naming the selected step, the level row, and the two
-- attribute boxes. Losing the level row from the top paid for all of it —
-- 22 px moved down, and the grid moved up to meet the buttons.
local DET_Y = 180
local DET_H = 60
local PEN_Y = 194              -- DET_Y + 14
local PEN_H = 20
local ATT_Y = 216              -- PEN_Y + PEN_H + 2
local ATT_H = 20

-- Six drum tracks and nothing else. The sub and the bleep lane are gone: a
-- kick sounds note 33 and the sub sounded 29 — four semitones apart in the same
-- octave — so every downbeat was two instruments fighting for the same air, and
-- the one that lost was always the kick. Nothing overlaps it now because
-- nothing else down there exists.
--
-- Losing two rows bought the remaining six a taller cell: 22 px instead of 17,
-- which is the whole grid becoming easier to hit.
local NTRK = 6
-- Four bars of sixteen. Two was the breakbeat's own unit and is still what a
-- groove is written in, but a pattern that *is* two bars can only ever repeat;
-- four is the shortest length with room for the bar that answers and the bar
-- that fills. The BAR row picks which one is on screen, and LOOP decides
-- whether playback stays in it.
local BAR_LEN = 16
local NBARS = 4
local NSTEP = 64               -- BAR_LEN * NBARS

local NONE = 255

-- Groove hints. A track carrying fewer than this many hits in the bar on screen
-- is treated as not written yet, and gets suggestions.
--
-- Three rather than zero, so a track with one exploratory hit still gets help —
-- and so the demo pattern's thinnest lanes show hints at boot instead of the
-- feature being invisible until something is cleared. It is also self-limiting:
-- keep writing and the hints go away on their own, which is why there is no
-- switch for them.
local HINT_MAX_HITS = 3
-- The score a suggestion has to reach to show.
--
-- Four was too low and the reason is worth keeping: a snare's role likes the
-- backbeat *and* both ghost positions, so ten of sixteen steps cleared the bar
-- and the row came out more green than not — a suggestion sheet with everything
-- on it is a blank one. Five drops the weakest tier and leaves each row reading
-- as an idea: for the snare, the two backbeats and the four 'a's.
--
-- A per-track relative band was tried alongside this and taken out again: with
-- role scores topping out at 9 and this floor at 5, `best - band` never rose
-- above the floor, so the mechanism could not fire. Narrowing the rim's and the
-- clap's roles did the rest of the work the band was supposed to do.
local HINT_MIN = 5

-- Rebuilt once a frame, not once per cell. The crowding term needs to know how
-- many tracks hit each step, and asking that inside the drawing loop is six
-- tracks times sixteen steps times six tracks — the same answer computed
-- thirty-six times.
local crowd: array(16, byte)
local hinting: array(6, byte)

local T_KICK = 0
local T_SNR = 1
local T_HAT = 2
local T_OHH = 3
local T_RIM = 4
local T_CLP = 5

-- A step is one byte: level in 0-1, extra hits in 2-3, micro-timing in 4-5.
-- One byte because the three are one decision — a hit that is weak, late and
-- doubled is a single thing a finger sets, and splitting them into parallel
-- arrays would triple every read in the scheduler for nothing.
local LVL_OFF = 0
local LVL_GHOST = 1
local LVL_NORM = 2
local LVL_ACC = 3

-- Micro is stored biased: 0 early, 1 on the grid, 2 late. It is also the
-- hit's starting countdown, which is why the whole pattern runs one frame
-- behind — "early" needs somewhere below the grid to sit, and the frame is
-- the finest thing this clock has.
local MIC_ON = 1

local pat: array(384, byte)    -- NTRK * NSTEP

-- The pending queue. Every hit lands here first, including the ones due this
-- frame, so that a roll and a nudge and a plain hit all take the same path.
local PQ = 16
local q_on: array(16, byte)
local q_del: array(16, byte)
local q_trk: array(16, byte)
local q_note: array(16, byte)
local q_vel: array(16, byte)
local q_lvl: array(16, byte)

local bpm = 174
local step_len = 331           -- 57600 / bpm, in 64ths of a frame
local acc = 0
local cur = 0
local playing = 0

-- The light layer's state. `flash` is per track and decays; `pump` is the
-- whole plate ducking on a kick, which is sidechain compression made visible.
local flash: array(6, byte)
local pump = 0

-- Per-track light colour, so a stack of hits on one step mixes additively the
-- way two coloured lamps do.
local lr: array(6, byte)
local lg: array(6, byte)
local lb: array(6, byte)

-- Per-track paint colour in the framebuffer. Everything is drawn bright and
-- then sunk by the ambient, so a lit cell reads as *emitting* rather than as
-- a lighter shade of the same paint.
local col: array(6, byte)

-- What a finger grabbed when it landed, held for the rest of its life. Same
-- rule as piano.lua: a drag that starts on a step keeps painting steps even
-- when it wanders over the pen row, and a tap on a button has already done
-- everything it is going to do.
local ROLE_NONE = 0
local ROLE_GRID = 1
local ROLE_BTN = 2
local ROLE_FADER = 3
local role: array(4, byte)
local fader_of: array(4, byte)

-- Per-track level, 0-8 — the range a mixer's stops are labelled with, and the
-- same nine as piano.lua's drawbars. It scales the velocity a hit is queued
-- with, so a fader move lands on the next hit rather than on the one already
-- decaying, and 8 is unity: a fresh boot mixes exactly as the kit was voiced.
local vol: array(6, byte)

-- Muted and soloed tracks are decided in `sched_track`, before anything is
-- queued. That is what makes a silenced track silent *and* dark: no queue entry
-- means no hit, which means no flash, which means the meter reads nothing. A
-- mute applied later — at `fire`, or by zeroing the velocity — would leave a lit
-- meter beside a silent track.
local mute: array(6, byte)

-- Solo *replaces* the mute picture while any of them is on, rather than
-- intersecting with it: with something soloed, a track sounds if and only if it
-- is soloed. The other rule — mute still wins over solo — means soloing a muted
-- track does nothing at all, and "listen to just this" has to always do exactly
-- that or it is not worth having.
local solo: array(6, byte)

-- The pen is the level a tap paints. Laying ghosts across a bar is then eight
-- taps at one setting rather than eight trips through a menu — which is the
-- single thing this screen exists to make fast.
local pen = 2                  -- LVL_NORM

-- Which bar is on screen. **It does not follow the playhead.** Auto-follow is
-- the wrong default: the page would turn under an editing finger every 1.4
-- seconds, so three quarters of the pattern could only be edited in the gaps.
--
-- It never follows, and the grid is only ever drawn in LOOP ONE. That pairing
-- is the whole design: **LOOP ONE writes a bar, LOOP ALL plays the four and
-- shows the mixer instead of the grid.** A version in between let the page
-- follow the playhead in LOOP ALL, and it was wrong in both directions at once
-- — the grid flipped bars every 1.4 seconds *and* it was a grid, which is the
-- one thing there is no point editing while four bars run past. Whereas a
-- mixer is exactly what the ear wants while a phrase plays, and its meters give
-- the light layer more to do than the grid ever did.
--
-- Held as the first step of the bar rather than as a bar number, because every
-- use of it is `page + i` and a number would mean multiplying at each one.
local page = 0

-- Does playback stay inside the bar on screen, or run all four?
--
-- Looping one bar is how a bar gets written — the groove comes round every 1.4
-- seconds instead of every 5.5, so a ghost that lands wrong is heard again
-- immediately. Looping all four is how the four are heard as a phrase, which is
-- the only way to tell whether the fill arrives in the right place. Both are
-- needed and neither is a mode: it is one bit, on a button beside the bars it
-- applies to.
local loop_all = 0

-- The step the editor panel acts on. Touching a step selects it, so the panel
-- works with one finger (tap the step, tap VEL) and with two (hold the step,
-- tap VEL and watch it change). One piece of state serves both.
local sel_trk = 1              -- SNR: the track this whole machine is about
local sel_step = 255           -- NONE until something is touched

signal step
signal hits
signal pen
signal track
signal bar
signal loop

function set_step(t, s, lvl, extra, mic)
  pat[t * NSTEP + s] = lvl | (extra << 2) | (mic << 4)
end

function init()
  clear(pat)
  clear(q_on)
  clear(flash)
  clear(mute)
  clear(solo)
  clear(fader_of)
  loop_all = 0
  for t = 0, NTRK - 1 do
    vol[t] = 8
  end
  acc = 0
  cur = 0
  playing = 1
  pump = 0
  bpm = 174
  step_len = 331

  lr[T_KICK] = 96  lg[T_KICK] = 34  lb[T_KICK] = 8
  lr[T_SNR]  = 74  lg[T_SNR]  = 86  lb[T_SNR]  = 96
  lr[T_HAT]  = 40  lg[T_HAT]  = 62  lb[T_HAT]  = 84
  lr[T_OHH]  = 26  lg[T_OHH]  = 74  lb[T_OHH]  = 92
  lr[T_RIM]  = 88  lg[T_RIM]  = 66  lb[T_RIM]  = 20
  lr[T_CLP]  = 88  lg[T_CLP]  = 48  lb[T_CLP]  = 82

  col[T_KICK] = 208
  col[T_SNR]  = 195
  col[T_HAT]  = 153
  col[T_OHH]  = 81
  col[T_RIM]  = 221
  col[T_CLP]  = 218

  demo_pattern()

  -- The clock only schedules on an *advance*, so step 0 has to be put in the
  -- queue by hand. Without this the loop's first downbeat is silent and every
  -- one after it plays: a bug that hides itself after 2.8 seconds.
  sched_step(0)
end

-- A two-step, the shape most drum'n'bass starts from: kick on the one, snare
-- on the two and the four, and the bar's whole character in what sits between
-- them. Bar two answers bar one rather than repeating it.
-- A kick, and nothing else.
--
-- The four bars start with one instrument in them on purpose. Every other track
-- is empty, so every other track is showing hints from the first frame: the
-- machine opens as a suggestion sheet with the pulse already laid down, which is
-- how a breakbeat gets written anyway. A demo pattern with all six tracks
-- filled in looks more impressive and teaches nothing — there is nothing left to
-- do to it, and the one feature that helps a person start is invisible.
--
-- The two-step, in other words: kick on the one and on the "and of three", and
-- the bars getting busier as they go so there is a shape to answer.
function demo_pattern()
  set_step(T_KICK, 0, LVL_ACC, 0, MIC_ON)
  set_step(T_KICK, 10, LVL_NORM, 0, MIC_ON)

  set_step(T_KICK, 16, LVL_ACC, 0, MIC_ON)
  set_step(T_KICK, 22, LVL_NORM, 0, MIC_ON)
  set_step(T_KICK, 27, LVL_GHOST, 0, MIC_ON)

  set_step(T_KICK, 32, LVL_ACC, 0, MIC_ON)
  set_step(T_KICK, 42, LVL_NORM, 0, MIC_ON)

  set_step(T_KICK, 48, LVL_ACC, 0, MIC_ON)
  set_step(T_KICK, 54, LVL_NORM, 0, MIC_ON)
  set_step(T_KICK, 58, LVL_NORM, 0, MIC_ON)
  set_step(T_KICK, 62, LVL_GHOST, 0, 2)
end

-- ---------------------------------------------------------------------------
-- The clock and the queue.

function any_solo()
  for t = 0, NTRK - 1 do
    if solo[t] == 1 then return 1 end
  end
  return 0
end

-- Does track `t` sound? One function, called from the scheduler and from the
-- mixer's drawing, so a strip that looks silenced always is.
--
-- Recomputed per track per step rather than cached: six comparisons every 5.2
-- frames is nothing, and a cached copy is a second place for the truth to live.
function audible(t)
  if any_solo() == 1 then return solo[t] end
  if mute[t] == 1 then return 0 end
  return 1
end

-- The three levels, as velocities. These are the master fader: every patch is
-- mixed against them, so a kit that pins the limiter is turned down here once
-- rather than in twenty declarations.
function vel_for(lvl)
  if lvl == LVL_GHOST then return 50 end
  if lvl == LVL_NORM then return 148 end
  return 205
end

function queue_push(del, t, n, v, lvl)
  for i = 0, PQ - 1 do
    if q_on[i] == 0 then
      q_on[i] = 1
      q_del[i] = del
      q_trk[i] = t
      q_note[i] = n
      q_vel[i] = v
      q_lvl[i] = lvl
      return
    end
  end
  -- Full: drop it. Sixteen in flight is four frames of every track at once,
  -- and a dropped hit is better than a stolen slot — the same rule the sound
  -- device applies to an out-of-range note.
end

-- Resolve one step of one track into queue entries. A retrigger spreads its
-- hits across the step's own length, so a roll stays a roll when the tempo
-- moves.
function sched_track(t, s)
  if audible(t) == 0 then return end

  local b = pat[t * NSTEP + s]
  local lvl = b & 3
  if lvl == LVL_OFF then return end

  local extra = (b >> 2) & 3
  local mic = (b >> 4) & 3
  local v = vel_for(lvl) * vol[t] / 8
  if v == 0 then return end

  local n = 38
  if t == T_KICK then n = 33 end
  if t == T_SNR then n = 50 end
  if t == T_HAT then n = 72 end
  if t == T_OHH then n = 72 end
  if t == T_RIM then n = 64 end
  if t == T_CLP then n = 60 end

  for k = 0, extra do
    local d = mic + k * step_len / ((extra + 1) * 64)
    local hv = v
    if k > 0 then hv = v * 3 / 4 end
    queue_push(d, t, n, hv, lvl)
  end
end

function sched_step(s)
  for t = 0, NTRK - 1 do
    sched_track(t, s)
  end
end

-- Sound one hit. The level picks the patch, not just the velocity — that is
-- the whole reason this game declares three snares instead of one.
function fire(t, n, v, lvl)
  if t == T_KICK then
    if lvl == LVL_GHOST then play(kick_gh, n, v, 14) else play(kick, n, v, 18) end
  elseif t == T_SNR then
    if lvl == LVL_GHOST then play(sn_ghost, n, v, 8)
    elseif lvl == LVL_NORM then play(sn_norm, n, v, 12)
    else play(sn_accent, n, v, 16) end
  elseif t == T_HAT then
    if lvl == LVL_GHOST then play(hh_ghost, n, v, 4)
    elseif lvl == LVL_NORM then play(hh_norm, n, v, 6)
    else play(hh_accent, n, v, 8) end
  elseif t == T_OHH then
    play(ohh, n, v, 22)
  elseif t == T_RIM then
    play(rim, n, v, 6)
  else
    play(clap, n, v, 12)
  end

  local f = v / 4 + 20
  if f > 63 then f = 63 end
  if f > flash[t] then flash[t] = f end
  if t == T_KICK then pump = 26 end
end

function drain_queue()
  local fired = 0
  for i = 0, PQ - 1 do
    if q_on[i] == 1 then
      if q_del[i] == 0 then
        fire(q_trk[i], q_note[i], q_vel[i], q_lvl[i])
        q_on[i] = 0
        fired = fired + 1
      else
        q_del[i] = q_del[i] - 1
      end
    end
  end
  return fired
end

function decay_lights()
  for t = 0, NTRK - 1 do
    if flash[t] > 6 then
      flash[t] = flash[t] * 3 / 4
    else
      flash[t] = 0
    end
  end
  if pump > 4 then pump = pump - 5 else pump = 0 end
end

-- ---------------------------------------------------------------------------
-- Editing.

-- Which track a y lands on, and which step an x lands on. Two functions rather
-- than one hit test because there are no tuples here, and because the label
-- column is a hit on a track with no step — that is how a track is selected.
function hit_track(y)
  if y < GRID_Y then return NONE end
  local t = (y - GRID_Y) / ROW_H
  if t >= NTRK then return NONE end
  return t
end

function hit_step(x)
  if x < GRID_X then return NONE end
  local i = (x - GRID_X) / CELL_W
  if i >= VIS then return NONE end
  return page + i
end

-- Apply the pen to one step.
--
-- `toggle` is 1 when the pen is being applied deliberately and 0 for every step
-- a finger drags across. A drag that could also clear would flicker a hat run on
-- and off as the finger crossed its own work, so a drag only ever sets — the
-- rule every paint program already uses, for the same reason.
--
-- Which press *counts* as deliberate is decided in `press_at`, not here: an
-- occupied step has to be selected before it can be altered.
--
-- A step that already exists keeps its roll and its nudge: repainting a hit to
-- accent it must not throw away the two attributes that took the longest to set.
function paint(t, s, toggle)
  local i = t * NSTEP + s
  local b = pat[i]
  local lvl = b & 3

  if toggle == 1 and lvl == pen then
    pat[i] = 0
    return
  end

  local keep = b & 252
  if lvl == LVL_OFF then keep = MIC_ON << 4 end
  pat[i] = pen | keep
end

-- Clear the bar on screen, not the whole track. Rewriting one bar to answer the
-- other is the common edit; losing both to a mis-tap is not an edit at all.
function clear_bar(t)
  for i = 0, VIS - 1 do
    pat[t * NSTEP + page + i] = 0
  end
end

-- Choose a level: the one the next tap paints, and the one the selected step
-- is. **One control, because it is one question asked about "next" and about
-- "this".**
--
-- These were two controls a screen apart — a GHOST/NORM/ACC row above the grid
-- that set the pen, and a VEL box below it that cycled the selected step — and
-- nothing on either said they were different questions. They read as the same
-- setting shown twice and disagreeing, which is exactly what they were not.
--
-- Radio rather than a cycle, now that the row is three targets wide. A cycle is
-- the right shape for ROLL and MIC, where the values are a short ordered walk
-- and there is no room for six boxes; it is the wrong one here, because a level
-- is a thing you aim at and a cycle costs up to three taps to arrive at the one
-- you wanted.
--
-- Selecting a step does *not* move the pen back. The bright box is always what
-- the next tap will paint, and a pen that followed the selection would repaint
-- the next empty step at whatever level was last inspected.
--
-- An off step is revived rather than skipped — the same as the cycle it
-- replaces. Clearing a step and wanting it back is one tap, and the panel can
-- only ever act on the step the grid already selected.
function set_pen(p)
  pen = p
  if sel_step ~= NONE then
    local i = sel_trk * NSTEP + sel_step
    pat[i] = (pat[i] & 252) | p
  end
end

-- The editor panel's two knobs. Each cycles, because a cycling button is one
-- finger-sized target where three radio buttons are three small ones, and a
-- roll and a nudge are each a short ordered walk rather than a thing to aim at.
function cycle_roll()
  local i = sel_trk * NSTEP + sel_step
  local extra = ((pat[i] >> 2) & 3) + 1
  if extra > 2 then extra = 0 end
  pat[i] = (pat[i] & 243) | (extra << 2)
end

function cycle_mic()
  local i = sel_trk * NSTEP + sel_step
  local mic = ((pat[i] >> 4) & 3) + 1
  if mic > 2 then mic = 0 end
  pat[i] = (pat[i] & 207) | (mic << 4)
end

-- The step a fresh start plays first, and the step the loop returns to. One
-- function, because "where does it begin" and "where does it come back to" are
-- the same question and answering it twice is how they drift apart.
function loop_start()
  return page
end

function toggle_play()
  if playing == 1 then
    playing = 0
    clear(q_on)
  else
    playing = 1
    acc = 0
    cur = loop_start()
    sched_step(cur)
  end
end

-- Show bar `b` (0-3), and make it the bar that is happening.
--
-- One meaning for the button in both loop modes. In LOOP ONE the bar on screen
-- *is* the loop, so choosing it is choosing what repeats. In LOOP ALL it is a
-- seek: the playhead jumps to that bar's downbeat and carries on through the
-- rest. A button that only changed the view while all four ran would be dead
-- half the time, and "go to this bar" covers both.
function show_bar(b)
  page = b * BAR_LEN
  if loop_all == 1 and playing == 1 then
    cur = page
    acc = 0
    sched_step(cur)
  end
end

-- Pull a fader to wherever the finger is. Continuous rather than press-only, so
-- a level is dragged the way a real one is — and clamped at both ends, because
-- the hit test that started the drag never runs again once the finger leaves
-- the row.
--
-- Nine bands over the travel, then clamped. Dividing the height into eight
-- makes level 8 reachable on one row of pixels while 0 gets thirteen: there are
-- *nine* stops from 0 to 8, and sizing the bands to the gaps between them is
-- what puts the top one out of reach. The same trap piano.lua's drawbars have,
-- and this is the same fix.
function set_fader(t, y)
  if y <= FDR_Y then
    vol[t] = 8
  elseif y >= FDR_Y + FDR_H then
    vol[t] = 0
  else
    local lv = (FDR_Y + FDR_H - y) * 9 / FDR_H
    if lv > 8 then lv = 8 end
    vol[t] = lv
  end
end

-- What each track's role wants, before anything else is taken into account.
--
-- The numbers are a groove, written down: a snare's job is the backbeat, a
-- hat's is the pulse, an open hat's is the '&' of two, a rim's is the gaps a
-- kick and a snare leave, a clap's is the pickup into the next bar. `p` is the
-- position inside a beat — 0 the beat, 1 the 'e', 2 the '&', 3 the 'a' — and
-- `b` is which beat of the bar.
--
-- This is deliberately opinionated and deliberately small. A hint that tried to
-- be a general theory of rhythm would suggest everything, and a suggestion
-- sheet with every step on it is a blank one.
function role_score(t, s)
  local p = s % 4
  local b = (s % BAR_LEN) / 4

  if t == T_KICK then
    if p == 0 and b == 0 then return 9 end
    if s % BAR_LEN == 10 then return 8 end
    if p == 2 and b == 1 then return 5 end
    return 0
  end
  if t == T_SNR then
    if p == 0 and b == 1 then return 9 end
    if p == 0 and b == 3 then return 9 end
    if p == 3 then return 5 end
    if p == 1 then return 4 end
    return 0
  end
  if t == T_HAT then
    if p == 0 then return 8 end
    if p == 2 then return 6 end
    return 0
  end
  if t == T_OHH then
    if p == 2 and b == 1 then return 9 end
    if p == 2 and b == 3 then return 6 end
    return 0
  end
  if t == T_RIM then
    -- The syncopation against the backbeat, not every 'e' in the bar. The wider
    -- version scored eight of sixteen steps and read as static.
    if p == 1 and b == 1 then return 7 end
    if p == 1 and b == 3 then return 7 end
    if p == 3 and b == 0 then return 5 end
    return 0
  end
  if s % BAR_LEN == 15 then return 9 end
  if p == 0 and b == 2 then return 6 end
  return 0
end

-- What a track should play at step `i` of the bar on screen, 0 for "nothing to
-- suggest".
--
-- The crowding term is what makes this a suggestion rather than a template. A
-- step three instruments already hit is a step this one should leave alone
-- whatever its role says — which is the same rule that got the sub and the
-- bleep deleted, applied to the thing being written rather than to the kit.
function hint_score(t, i)
  local sc = role_score(t, page + i)
  if sc == 0 then return 0 end
  local n = crowd[i] * 3
  if n >= sc then return 0 end
  return sc - n
end

function build_hints()
  for i = 0, VIS - 1 do
    local n = 0
    for k = 0, NTRK - 1 do
      if (pat[k * NSTEP + page + i] & 3) ~= LVL_OFF then n = n + 1 end
    end
    crowd[i] = n
  end

  for t = 0, NTRK - 1 do
    local hits = 0
    for i = 0, VIS - 1 do
      if (pat[t * NSTEP + page + i] & 3) ~= LVL_OFF then hits = hits + 1 end
    end
    if hits < HINT_MAX_HITS then
      hinting[t] = 1
    else
      hinting[t] = 0
    end
  end
end

-- Which strip an x falls in. 240 / 6 divides exactly, so every strip is the
-- same width with nothing left over on the last one.
function hit_strip(x)
  local t = x / STRIP_W
  if t >= NTRK then return NONE end
  return t
end

-- Where a finger landed, and what role it takes for the rest of its life.
function press_at(i, x, y)
  if y < HDR_H then
    if x >= 220 then toggle_play() end
    return ROLE_BTN
  end

  if y < BTN_Y + BTN_H then
    if x < BAR_BTN_W * NBARS then
      show_bar(x / BAR_BTN_W)
    elseif x < CLR_X then
      if loop_all == 1 then loop_all = 0 else loop_all = 1 end
    else
      clear_bar(sel_trk)
    end
    return ROLE_BTN
  end

  -- Below the button row the two screens share no geometry, so the view is
  -- decided here and once. Testing the step panel's strip first is what an
  -- earlier version did, and it ate the whole SOLO row in mixer view: 202 is
  -- inside both the panel and the buttons.
  --
  -- The mixer's body: one strip per track, tested bottom-up because the two
  -- buttons sit under the fader and the fader is the fallback. The meter is not
  -- touchable — it reports, it does not take.
  if loop_all == 1 then
    -- The legend is a label, not a row of six things: a tap on the word MIXER
    -- must not select the kick.
    if y < MIX_Y + MIX_H then return ROLE_BTN end
    local st = hit_strip(x)
    if st == NONE then return ROLE_NONE end
    sel_trk = st
    if y >= SOLO_Y then
      if solo[st] == 1 then solo[st] = 0 else solo[st] = 1 end
      return ROLE_BTN
    end
    if y >= MUTE_Y then
      if mute[st] == 1 then mute[st] = 0 else mute[st] = 1 end
      return ROLE_BTN
    end
    if y >= FDR_Y then
      fader_of[i] = st
      set_fader(st, y)
      return ROLE_FADER
    end
    return ROLE_BTN
  end

  -- The note band. The level row is live whether or not a step is selected —
  -- there is always a next tap — while the two attribute boxes need one. The
  -- line naming the step is a label and takes nothing.
  if y >= DET_Y then
    if y >= ATT_Y then
      if sel_step == NONE then return ROLE_BTN end
      if x < 160 then cycle_roll() else cycle_mic() end
    elseif y >= PEN_Y then
      local c = x / 106
      if c > 2 then c = 2 end
      set_pen(c + 1)
    end
    return ROLE_BTN
  end

  local t = hit_track(y)
  if t == NONE then return ROLE_NONE end

  -- The name column selects the track the buttons act on.
  if x < GRID_X then
    sel_trk = t
    return ROLE_BTN
  end

  local st = hit_step(x)
  if st == NONE then return ROLE_NONE end

  -- An empty step takes the pen straight away — placing notes is the main verb
  -- and it must not cost two taps. An occupied one is *selected* by the first
  -- press and only altered by the second.
  --
  -- Reaching for a note to change its roll was the gesture that deleted it: the
  -- press applied the pen, and applying the pen to a step that already has it
  -- means clearing it. Nothing about that is wrong except that it happened on
  -- the way to somewhere else. Requiring the press to land on the *already
  -- selected* step keeps the whole rule and makes it deliberate — and the second
  -- press still applies the pen, so repainting a ghost as an accent is two taps
  -- rather than a special case.
  local occupied = 0
  if (pat[t * NSTEP + st] & 3) ~= LVL_OFF then occupied = 1 end
  local again = 0
  if t == sel_trk and st == sel_step then again = 1 end

  sel_trk = t
  sel_step = st
  if occupied == 0 or again == 1 then paint(t, st, 1) end
  return ROLE_GRID
end

function edit_touches()
  for i = 0, 3 do
    if touch_pressed(i) then
      role[i] = press_at(i, touch_x(i), touch_y(i))
    elseif touch_down(i) then
      if role[i] == ROLE_FADER then
        set_fader(fader_of[i], touch_y(i))
      elseif role[i] == ROLE_GRID then
        local t = hit_track(touch_y(i))
        local st = hit_step(touch_x(i))
        if t ~= NONE and st ~= NONE then
          if t ~= sel_trk or st ~= sel_step then
            sel_trk = t
            sel_step = st
            paint(t, st, 0)
          end
        end
      end
    else
      role[i] = ROLE_NONE
    end
  end
end

function update()
  if btnp(A) then toggle_play() end
  -- B steps through the bars, and does not clear. A bare button that destroys a
  -- bar of work is a bare button somebody leans on: clearing stays on the
  -- labelled box, where hitting it means having aimed at it.
  if btnp(B) then
    local b = page / BAR_LEN + 1
    if b >= NBARS then b = 0 end
    show_bar(b)
  end
  edit_touches()
  -- Rebuilt after the edits and before the drawing, so a hit painted this frame
  -- is already counted: the green under a finger goes out as the pad appears
  -- rather than a frame later.
  if loop_all == 0 then build_hints() end

  if playing == 1 then
    acc = acc + 64
    while acc >= step_len do
      acc = acc - step_len
      cur = cur + 1
      -- One bar or all four. The `cur < page` half matters: switching to LOOP
      -- ONE while the playhead is in some other bar has to pull it back in, and
      -- the wrap test alone never would.
      if loop_all == 1 then
        if cur >= NSTEP then cur = 0 end
      elseif cur >= page + BAR_LEN or cur < page then
        cur = page
      end
      sched_step(cur)
    end
  end

  -- Drained after scheduling, so a hit with no nudge sounds on the frame its
  -- step begins rather than the frame after it.
  local fired = drain_queue()
  decay_lights()

  signal(step, cur)
  signal(hits, fired)
  signal(pen, pen)
  signal(track, sel_trk)
  signal(bar, page / BAR_LEN)
  signal(loop, loop_all)
end

-- ---------------------------------------------------------------------------
-- Drawing, and the light layer over it.
--
-- Everything is painted in saturated colour and then sunk by an ambient of
-- about a third, so the grid sits dark and anything the light layer touches
-- reads as *emitting* rather than as a paler shade of paint. That is the only
-- reason a ghost hit and an accent are distinguishable across the room: they
-- differ in light, not in pigment.
--
-- Every band a finger can hit is a `light_rect` at neutral instead. A control
-- that dims with the room is a control you cannot find, and round lights over
-- a button row bleed into the grid behind it.

function pad_h(lvl)
  if lvl == LVL_GHOST then return 5 end
  if lvl == LVL_NORM then return 10 end
  return 16
end

-- One place that knows a track's name, because `text` takes a literal and the
-- alternative is the same eight-way branch written twice.
function track_name(t, x, y, c)
  if t == T_KICK then text("KCK", x, y, c)
  elseif t == T_SNR then text("SNR", x, y, c)
  elseif t == T_HAT then text("HAT", x, y, c)
  elseif t == T_OHH then text("OHH", x, y, c)
  elseif t == T_RIM then text("RIM", x, y, c)
  else text("CLP", x, y, c) end
end

function playhead_on_page()
  if cur < page then return 0 end
  if cur >= page + VIS then return 0 end
  return 1
end

function draw_header()
  rect(0, HDR_Y, DIM, HDR_H, 234)
  text("DNB", 4, 8, 250)
  text("BPM", 30, 8, 245)
  number(bpm, 54, 8, 255)
  text("STEP", 88, 8, 245)
  number(cur + 1, 116, 8, 255)

  -- The transport is a button, not a lamp. A is the same thing for a keyboard.
  if playing == 1 then
    rect(220, 2, 96, 18, 34)
    text("PLAYING", 254, 8, 255)
  else
    rect(220, 2, 96, 18, 238)
    text("STOPPED", 254, 8, 250)
  end
end

-- The level row, carrying both readings the two old controls carried between
-- them: the **bright** box is the pen, the level the next tap paints; the
-- **capped** box is the selected step, in its own track's colour.
--
-- Merging the controls without merging the readings is the whole point. They
-- differ exactly while a step of one level sits selected under a pen set to
-- another, which is a real and common state — inspecting an accent's roll with
-- the pen on ghost — and a row that showed only the pen would have quietly lost
-- what the panel used to say. Tap either and the two coincide, which is also
-- what says the button did both things.
function draw_pen()
  local sl = NONE
  if sel_step ~= NONE then sl = pat[sel_trk * NSTEP + sel_step] & 3 end

  for i = 0, 2 do
    local x0 = i * 106 + 2
    local c = 234
    if pen == i + 1 then c = 245 end
    rect(x0, PEN_Y, 103, PEN_H, c)
    if sl == i + 1 then
      rect(x0, PEN_Y + PEN_H - 3, 103, 3, col[sel_trk])
    end
  end
  text("GHOST", 43, PEN_Y + 7, 250)
  text("NORM", 151, PEN_Y + 7, 250)
  text("ACC", 259, PEN_Y + 7, 250)
end

function draw_buttons()
  for b = 0, NBARS - 1 do
    local x0 = b * BAR_BTN_W
    local c = 234
    if b * BAR_LEN == page then c = 245 end
    rect(x0, BTN_Y, BAR_BTN_W - 2, BTN_H, c)

    -- A bright cap on the bar the beat is *in*, whichever bar is on screen.
    -- This is the one thing following the playhead was for, and four buttons
    -- say it better than the single edge marker two bars needed: it names the
    -- bar rather than only reporting that it is elsewhere.
    if playing == 1 and cur >= b * BAR_LEN then
      if cur < b * BAR_LEN + BAR_LEN then
        rect(x0, BTN_Y, BAR_BTN_W - 2, 2, 255)
      end
    end
  end
  text("B1", 15, BTN_Y + 7, 252)
  text("B2", 55, BTN_Y + 7, 252)
  text("B3", 95, BTN_Y + 7, 252)
  text("B4", 135, BTN_Y + 7, 252)

  -- The loop toggle carries its state in its colour as well as its word: a
  -- two-state button whose only difference is the text is a button you read.
  if loop_all == 1 then
    rect(LOOP_X, BTN_Y, LOOP_W, BTN_H, 245)
    text("LOOP ALL", LOOP_X + 19, BTN_Y + 7, 252)
  else
    rect(LOOP_X, BTN_Y, LOOP_W, BTN_H, 234)
    text("LOOP 1", LOOP_X + 23, BTN_Y + 7, 252)
  end

  rect(CLR_X, BTN_Y, CLR_W, BTN_H, 234)
  text("CLR BAR", CLR_X + 18, BTN_Y + 7, 250)
end

function draw_labels()
  for t = 0, NTRK - 1 do
    local y0 = GRID_Y + t * ROW_H
    if t == sel_trk then
      -- The two buttons above act on this track and nothing else says so.
      rect(0, y0, LBL_W - 2, ROW_H - 2, 237)
    end
    track_name(t, 6, y0 + 8, col[t])
  end
end

function draw_grid()
  for t = 0, NTRK - 1 do
    local y0 = GRID_Y + t * ROW_H
    for i = 0, VIS - 1 do
      local st = page + i
      local x0 = GRID_X + i * CELL_W

      -- Every fourth step is a beat and sits brighter, so the eye finds the
      -- downbeat without counting.
      local floor_c = 233
      if i % 4 == 0 then floor_c = 236 end
      rect(x0, y0, CELL_W - 1, ROW_H - 2, floor_c)

      local b = pat[t * NSTEP + st]
      local lvl = b & 3

      -- A suggestion, drawn as the *outline* of the pad it is suggesting: an
      -- empty box exactly where a NORM hit would sit. Green, and hollow, so it
      -- cannot be mistaken for something already written — every real hit on
      -- this screen is a filled block in its track's colour, and nothing else
      -- is an outline.
      if lvl == LVL_OFF and hinting[t] == 1 then
        local sc = hint_score(t, i)
        if sc >= HINT_MIN then
          local hh = pad_h(LVL_NORM)
          local hy = y0 + ROW_H - 3 - hh
          local hc = 71
          if sc >= 7 then hc = 83 end
          rect(x0 + 1, hy, CELL_W - 4, 1, hc)
          rect(x0 + 1, hy + hh - 1, CELL_W - 4, 1, hc)
          rect(x0 + 1, hy, 1, hh, hc)
          rect(x0 + CELL_W - 4, hy, 1, hh, hc)
        end
      end

      if lvl ~= LVL_OFF then
        local h = pad_h(lvl)
        local mic = (b >> 4) & 3
        -- Micro-timing is drawn as a physical offset, because that is what it
        -- is: the same hit, standing slightly off its line.
        local dx = 1
        if mic == 0 then dx = 0 end
        if mic == 2 then dx = 3 end
        rect(x0 + dx, y0 + ROW_H - 3 - h, CELL_W - 4, h, col[t])

        -- A roll's extra hits, as ticks along the top of the cell.
        local extra = (b >> 2) & 3
        for k = 1, extra do
          rect(x0 + 1 + k * 3, y0 + 1, 2, 2, 255)
        end
      end

      -- The selected step, bracketed rather than filled, so what the editor
      -- panel is about to change stays visible while it changes.
      if t == sel_trk and st == sel_step then
        rect(x0, y0, 1, ROW_H - 2, 255)
        rect(x0 + CELL_W - 2, y0, 1, ROW_H - 2, 255)
      end
    end
  end

  if playhead_on_page() == 1 then
    local px = GRID_X + (cur - page) * CELL_W
    rect(px, GRID_Y - 2, CELL_W - 1, 2, 255)
    rect(px, GRID_Y + NTRK * ROW_H, CELL_W - 1, 2, 255)
  end
end

-- The note band: everything that is about one hit, in the order a hand reaches
-- for it. The level first, because it is the one control that is also live with
-- nothing selected, then the two attributes of the step under the finger.
--
-- Two cycling boxes rather than six radio buttons, and each is half the width
-- the three used to share: a roll and a nudge are short ordered walks, so a
-- thumb walks them, and the targets doubled by dropping the box that the level
-- row now is.
function draw_detail()
  rect(0, DET_Y, DIM, DET_H, 234)
  draw_pen()

  if sel_step == NONE then
    text("TAP A STEP TO EDIT IT", 6, DET_Y + 4, 240)
    text("GREEN IS A GROOVE HINT", 6, ATT_Y + 7, 71)
    return
  end

  text("STEP", 4, DET_Y + 4, 245)
  number(sel_step + 1, 30, DET_Y + 4, 255)
  track_name(sel_trk, 58, DET_Y + 4, col[sel_trk])

  local b = pat[sel_trk * NSTEP + sel_step]
  local extra = (b >> 2) & 3
  local mic = (b >> 4) & 3

  rect(2, ATT_Y, 157, ATT_H, 237)
  if extra == 0 then
    text("ROLL 1", 68, ATT_Y + 7, 248)
  elseif extra == 1 then
    text("ROLL 2", 68, ATT_Y + 7, 252)
  else
    text("ROLL 3", 68, ATT_Y + 7, 255)
  end

  rect(161, ATT_Y, 157, ATT_H, 237)
  if mic == 0 then
    text("MIC EARLY", 221, ATT_Y + 7, 250)
  elseif mic == 1 then
    text("MIC ON", 227, ATT_Y + 7, 248)
  else
    text("MIC LATE", 223, ATT_Y + 7, 253)
  end
end

-- What the two bars in every strip are. Said once above them rather than six
-- times down the side — there is no room in a strip this narrow for a column
-- heading, and the answer is the same for all of them.
--
-- It sits in the row grid view gives to the top of the grid, so the strips
-- below it start where they always did and only this label moved.
function draw_mix_head()
  rect(0, MIX_Y, DIM, MIX_H, 234)
  text("MIXER", 6, MIX_Y + 7, 252)
  rect(82, MIX_Y + 5, 8, 8, 208)
  text("LEVEL", 94, MIX_Y + 7, 245)
  rect(176, MIX_Y + 5, 8, 8, 252)
  text("HIT", 188, MIX_Y + 7, 245)
end

function draw_mixer()
  for t = 0, NTRK - 1 do
    local x0 = t * STRIP_W
    local on = audible(t)

    -- The name, and the fader, go grey together when the track cannot be heard
    -- — whether that is its own mute or somebody else's solo. One `audible`
    -- answers both, so a strip never looks live while it is silent.
    local nc = col[t]
    if on == 0 then nc = 236 end
    track_name(t, x0 + 8, NAME_Y, nc)

    rect(x0 + FDR_IN_X, FDR_Y, FDR_IN_W, FDR_H, 233)
    -- Nine notches, drawn under the fill: the ones still showing are the travel
    -- left. Aligned across all six strips, so the mix reads as a skyline.
    for k = 0, 8 do
      rect(x0 + FDR_IN_X, FDR_Y + k * (FDR_H - 1) / 8, FDR_IN_W, 1, 236)
    end
    local fc = col[t]
    if on == 0 then fc = 237 end
    local fh = vol[t] * FDR_H / 8
    if fh > 0 then
      rect(x0 + FDR_IN_X, FDR_Y + FDR_H - fh, FDR_IN_W, fh, fc)
    end

    -- The meter is `flash`, the same decaying number the light layer uses. One
    -- source, so what the meter says and what the strip glows can never
    -- disagree. White rather than the track's colour, because a fader at 8 and
    -- a meter at full are three pixels apart and two bars of one colour that
    -- close read as one bar. The *light* keeps the track's colour.
    rect(x0 + MTR_IN_X, FDR_Y, MTR_IN_W, FDR_H, 233)
    local mh = flash[t] * FDR_H / 63
    if mh > 0 then
      rect(x0 + MTR_IN_X, FDR_Y + FDR_H - mh, MTR_IN_W, mh, 252)
    end

    -- Orange for mute, yellow for solo — the two colours every console in the
    -- world uses for them, which is worth more here than anything this palette
    -- could invent.
    local mc = 234
    if mute[t] == 1 then mc = 208 end
    rect(x0 + 1, MUTE_Y, STRIP_W - 3, MS_H, mc)
    text("MUTE", x0 + 18, MUTE_Y + 7, 252)

    local sc = 234
    if solo[t] == 1 then sc = 226 end
    rect(x0 + 1, SOLO_Y, STRIP_W - 3, MS_H, sc)
    text("SOLO", x0 + 18, SOLO_Y + 7, 252)
  end
end

function draw()
  cls(232)
  draw_header()
  draw_buttons()
  if loop_all == 1 then
    draw_mix_head()
    draw_mixer()
  else
    draw_labels()
    draw_grid()
    draw_detail()
  end

  -- The plate, dark and cold, ducking on every kick. A fill rather than a lamp,
  -- so it also clears the previous frame's light and its obstacles.
  ambient(21 - pump / 3, 18 - pump / 4, 38 - pump / 4)

  -- Every control surface, flat at neutral. Text on these stays readable at an
  -- ambient a third of it, which no arrangement of round lights achieves
  -- without bleeding into the grid.
  light_rect(0, HDR_Y, DIM, HDR_H, 64, 64, 64)
  light_rect(0, BTN_Y, DIM, BTN_H, 58, 58, 62)
  if loop_all == 1 then
    -- The legend and the names, and the two button rows. The fader band between
    -- them is lit lower than neutral on purpose: a meter has to be able to rise
    -- *above* its surroundings, and it cannot do that over a strip already
    -- at 64.
    light_rect(0, MIX_Y, DIM, FDR_Y - MIX_Y, 58, 58, 62)
    light_rect(0, FDR_Y, DIM, FDR_H, 40, 40, 46)
    light_rect(0, MUTE_Y, DIM, DIM - MUTE_Y, 58, 58, 62)
    -- An engaged mute or solo is lit past neutral, so the button that is doing
    -- something is the brightest thing in its strip. Drawn after the band, and
    -- `light_rect` sets rather than adds, so it replaces it cleanly.
    for t = 0, NTRK - 1 do
      local x0 = t * STRIP_W
      if mute[t] == 1 then
        light_rect(x0 + 1, MUTE_Y, STRIP_W - 3, MS_H, 100, 74, 56)
      end
      if solo[t] == 1 then
        light_rect(x0 + 1, SOLO_Y, STRIP_W - 3, MS_H, 100, 96, 58)
      end
    end
  else
    light_rect(0, GRID_Y, LBL_W, NTRK * ROW_H, 58, 58, 62)
    light_rect(0, DET_Y, DIM, DET_H, 58, 58, 62)
  end

  if loop_all == 1 then
    -- The lamp rides the *top* of each meter, so a hit reads as a flare thrown
    -- up and falling back rather than as a bar that merely gets taller. Same
    -- `flash` and same colours as the grid's playhead flashes — only the place
    -- moves.
    for t = 0, NTRK - 1 do
      local f = flash[t]
      if f > 0 then
        local mh = f * FDR_H / 63
        light(t * STRIP_W + MTR_IN_X + MTR_IN_W / 2, FDR_Y + FDR_H - mh,
              12 + f / 3, lr[t] * f / 64, lg[t] * f / 64, lb[t] * f / 64)
      end
    end
  elseif playhead_on_page() == 1 then
    -- Two lamps rather than one, a quarter and three quarters down. A single
    -- lamp wide enough to reach both the kick row and the clap row is also
    -- wide enough to wash out the four steps either side of it; two narrow
    -- ones add in the middle and stay a column.
    local px = GRID_X + (cur - page) * CELL_W + CELL_W / 2
    light(px, GRID_Y + NTRK * ROW_H / 4, 40, 72, 66, 96)
    light(px, GRID_Y + NTRK * ROW_H * 3 / 4, 40, 72, 66, 96)

    -- Each track's own hit, in its own colour, fading over about six frames.
    -- Intensity carries the velocity, so a ghost is a dim smudge and an accent
    -- burns — the pattern's dynamics, visible without listening.
    for t = 0, NTRK - 1 do
      local f = flash[t]
      if f > 0 then
        local ry = GRID_Y + t * ROW_H + ROW_H / 2
        light(px, ry, 12 + f / 3, lr[t] * f / 64, lg[t] * f / 64, lb[t] * f / 64)
      end
    end
  end

  -- The selected step glows a little, so the bracket is findable at a glance on
  -- a screen this dark. Grid view only: there is no step under a fader.
  if loop_all == 0 and sel_step ~= NONE then
    if sel_step >= page and sel_step < page + VIS then
      light(GRID_X + (sel_step - page) * CELL_W + CELL_W / 2,
            GRID_Y + sel_trk * ROW_H + ROW_H / 2, 9, 30, 30, 38)
    end
  end

  -- No `shadow_rect` anywhere: a flat panel has no occluders, and declaring one
  -- would buy the whole O(r squared) shadow walk for every lamp above in
  -- exchange for nothing.
end
