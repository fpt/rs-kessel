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

screen { mode = Extended240 }

controls {
  dpad  = false
  touch = "edit"
  a     = "play / stop"
  b     = "bar 1 / 2"
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
  volume = 165
}
instrument kick_gh {
  wave = sine
  attack = 0  decay = 70  sustain = 0  release = 24
  pitch_env = 30  pitch_decay = 40
  filter = lpf  cutoff = 100  resonance = 20
  volume = 82
}

-- The ghost is the point of this game: dark, clipped, and gone before the ear
-- decides what it was. Every difference from `sn_norm` matters — dropping the
-- cutoff alone gives a muffled snare, dropping the decay alone gives a tick.
instrument sn_ghost {
  wave = noise
  attack = 0  decay = 34  sustain = 0  release = 18
  filter = lpf  cutoff = 95  resonance = 70
  volume = 62  reverb = 18
}
instrument sn_norm {
  wave = noise
  attack = 0  decay = 105  sustain = 0  release = 60
  filter = lpf  cutoff = 168  resonance = 50
  distortion = 30
  volume = 132  reverb = 55
}
instrument sn_accent {
  wave = noise
  attack = 0  decay = 150  sustain = 0  release = 95
  filter = lpf  cutoff = 218  resonance = 40
  distortion = 50
  volume = 158  reverb = 72
}

instrument hh_ghost {
  wave = noise
  attack = 0  decay = 15  sustain = 0  release = 8
  filter = hpf  cutoff = 200
  volume = 40
}
instrument hh_norm {
  wave = noise
  attack = 0  decay = 26  sustain = 0  release = 12
  filter = hpf  cutoff = 212
  volume = 86
}
instrument hh_accent {
  wave = noise
  attack = 0  decay = 40  sustain = 0  release = 20
  filter = hpf  cutoff = 226
  volume = 120
}
instrument ohh {
  wave = noise
  attack = 0  decay = 200  sustain = 30  release = 150
  filter = hpf  cutoff = 196
  volume = 92  reverb = 45
}

instrument rim {
  wave = square
  attack = 0  decay = 28  sustain = 0  release = 14
  pitch_env = 20  pitch_decay = 12
  filter = hpf  cutoff = 155
  volume = 98
}
instrument clap {
  wave = noise
  attack = 2  decay = 88  sustain = 0  release = 70
  filter = lpf  cutoff = 190  resonance = 95
  volume = 110  reverb = 95
}

-- Sub: a sine so far under the filter that it is felt rather than heard, held
-- for as long as the gate the sequencer computes.
instrument sub {
  wave = sine
  attack = 6  decay = 220  sustain = 235  release = 90
  filter = lpf  cutoff = 48
  volume = 82
}

-- The Reese, in eight cutoffs.
--
-- There is no LFO in this synth — see the end of `docs/SYNTH.md` for why — and
-- a patch cannot be edited once the ROM loads. So the wobble is not modulation,
-- it is a **lane**: each step names which of these eight it sounds, and the
-- sequencer draws the sweep.
-- That is better than an LFO here anyway, because a drawn wobble locks to the
-- grid instead of drifting against it.
--
-- Detune comes from the chorus send, which is the only thing in the machine
-- that puts a voice slightly beside itself.
instrument rs_0 {
  wave = saw
  attack = 4  decay = 240  sustain = 220  release = 80
  filter = lpf  cutoff = 26  resonance = 108
  chorus = 150  distortion = 35  volume = 58
}
instrument rs_1 {
  wave = saw
  attack = 4  decay = 240  sustain = 220  release = 80
  filter = lpf  cutoff = 48  resonance = 108
  chorus = 150  distortion = 35  volume = 58
}
instrument rs_2 {
  wave = saw
  attack = 4  decay = 240  sustain = 220  release = 80
  filter = lpf  cutoff = 72  resonance = 108
  chorus = 150  distortion = 35  volume = 58
}
instrument rs_3 {
  wave = saw
  attack = 4  decay = 240  sustain = 220  release = 80
  filter = lpf  cutoff = 98  resonance = 108
  chorus = 150  distortion = 35  volume = 58
}
instrument rs_4 {
  wave = saw
  attack = 4  decay = 240  sustain = 220  release = 80
  filter = lpf  cutoff = 126  resonance = 108
  chorus = 150  distortion = 35  volume = 58
}
instrument rs_5 {
  wave = saw
  attack = 4  decay = 240  sustain = 220  release = 80
  filter = lpf  cutoff = 156  resonance = 105
  chorus = 150  distortion = 35  volume = 58
}
instrument rs_6 {
  wave = saw
  attack = 4  decay = 240  sustain = 220  release = 80
  filter = lpf  cutoff = 190  resonance = 102
  chorus = 150  distortion = 35  volume = 56
}
instrument rs_7 {
  wave = saw
  attack = 4  decay = 240  sustain = 220  release = 80
  filter = lpf  cutoff = 226  resonance = 100
  chorus = 150  distortion = 35  volume = 54
}

-- ---------------------------------------------------------------------------
-- Layout. 240 across: a 32 px name column and sixteen 13 px steps.

local DIM = 240

local HDR_Y = 0
local HDR_H = 22

local LBL_W = 32
local CELL_W = 13
local ROW_H = 17
local GRID_X = 32
local GRID_Y = 66
local VIS = 16                 -- steps on screen; the other bar is a toggle

-- The pen row and the button row fill the band between the header and the grid;
-- the step editor takes the strip below it. Everything a finger can hit is a
-- flat box lit to neutral, and the grid between them is the only thing allowed
-- to be dark.
local PEN_Y = 24
local PEN_H = 20
local BTN_Y = 46
local BTN_H = 18
local DET_Y = 206
local DET_H = 34

local NTRK = 8
local NSTEP = 32               -- two bars of 16ths — the breakbeat's own unit
local HALF = 16

local NONE = 255

local T_KICK = 0
local T_SNR = 1
local T_HAT = 2
local T_OHH = 3
local T_RIM = 4
local T_CLP = 5
local T_SUB = 6
local T_RSE = 7

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

local pat: array(256, byte)    -- NTRK * NSTEP
local bnote: array(64, byte)   -- SUB and RSE: MIDI note per step, 0 = rest
local wob: array(32, byte)     -- the Reese's cutoff lane, 0-7

-- The pending queue. Every hit lands here first, including the ones due this
-- frame, so that a roll and a nudge and a plain hit all take the same path.
local PQ = 16
local q_on: array(16, byte)
local q_del: array(16, byte)
local q_trk: array(16, byte)
local q_note: array(16, byte)
local q_vel: array(16, byte)
local q_lvl: array(16, byte)
local q_var: array(16, byte)   -- the Reese's patch index; unused elsewhere
local q_gate: array(16, byte)  -- how long a held note sounds; drums ignore it

-- Frames left on each melodic track's held note, and 0 for silent. The two
-- melodic tracks sound through `note_on` on a channel they own (their own
-- track index), never through `play`.
--
-- That is not a stylistic choice. `play` is fire-and-forget, so re-entering a
-- long sub before the last one ends *stacks* them, and a player tapping
-- play/stop stacks one every time. `note_on` on a channel replaces what is on
-- it, so the same tapping costs one voice however fast it is done.
local hold: array(2, byte)

local bpm = 174
local step_len = 331           -- 57600 / bpm, in 64ths of a frame
local acc = 0
local cur = 0
local playing = 0

-- The light layer's state. `flash` is per track and decays; `pump` is the
-- whole plate ducking on a kick, which is sidechain compression made visible.
local flash: array(8, byte)
local pump = 0

local reeses: array(8, byte)

-- Per-track light colour, so a stack of hits on one step mixes additively the
-- way two coloured lamps do.
local lr: array(8, byte)
local lg: array(8, byte)
local lb: array(8, byte)

-- Per-track paint colour in the framebuffer. Everything is drawn bright and
-- then sunk by the ambient, so a lit cell reads as *emitting* rather than as
-- a lighter shade of the same paint.
local col: array(8, byte)

-- What a finger grabbed when it landed, held for the rest of its life. Same
-- rule as piano.lua: a drag that starts on a step keeps painting steps even
-- when it wanders over the pen row, and a tap on a button has already done
-- everything it is going to do.
local ROLE_NONE = 0
local ROLE_GRID = 1
local ROLE_BTN = 2
local role: array(4, byte)

-- The pen is the level a tap paints. Laying ghosts across a bar is then eight
-- taps at one setting rather than eight trips through a menu — which is the
-- single thing this screen exists to make fast.
local pen = 2                  -- LVL_NORM

-- Which bar is on screen. **It does not follow the playhead.** Auto-follow is
-- the obvious behaviour and it is wrong here: the page turns under an editing
-- finger every 2.8 seconds, so half of a two-bar pattern can only be edited in
-- the gaps. The BAR button shows a bright edge when the beat is on the other
-- page, which is the part following was for.
local page = 0

-- The step the editor panel acts on. Touching a step selects it, so the panel
-- works with one finger (tap the step, tap VEL) and with two (hold the step,
-- tap VEL and watch it change). One piece of state serves both.
local sel_trk = 1              -- SNR: the track this whole machine is about
local sel_step = 255           -- NONE until something is touched

signal step
signal hits
signal pen
signal track

function set_step(t, s, lvl, extra, mic)
  pat[t * NSTEP + s] = lvl | (extra << 2) | (mic << 4)
end

function init()
  clear(pat)
  clear(bnote)
  clear(wob)
  clear(q_on)
  clear(hold)
  clear(flash)
  acc = 0
  cur = 0
  playing = 1
  pump = 0
  bpm = 174
  step_len = 331

  reeses[0] = rs_0  reeses[1] = rs_1  reeses[2] = rs_2  reeses[3] = rs_3
  reeses[4] = rs_4  reeses[5] = rs_5  reeses[6] = rs_6  reeses[7] = rs_7

  lr[T_KICK] = 96  lg[T_KICK] = 34  lb[T_KICK] = 8
  lr[T_SNR]  = 74  lg[T_SNR]  = 86  lb[T_SNR]  = 96
  lr[T_HAT]  = 40  lg[T_HAT]  = 62  lb[T_HAT]  = 84
  lr[T_OHH]  = 26  lg[T_OHH]  = 74  lb[T_OHH]  = 92
  lr[T_RIM]  = 88  lg[T_RIM]  = 66  lb[T_RIM]  = 20
  lr[T_CLP]  = 88  lg[T_CLP]  = 48  lb[T_CLP]  = 82
  lr[T_SUB]  = 18  lg[T_SUB]  = 30  lb[T_SUB]  = 96
  lr[T_RSE]  = 84  lg[T_RSE]  = 20  lb[T_RSE]  = 96

  col[T_KICK] = 208
  col[T_SNR]  = 195
  col[T_HAT]  = 153
  col[T_OHH]  = 81
  col[T_RIM]  = 221
  col[T_CLP]  = 218
  col[T_SUB]  = 63
  col[T_RSE]  = 165

  demo_pattern()

  -- The clock only schedules on an *advance*, so step 0 has to be put in the
  -- queue by hand. Without this the loop's first downbeat is silent and every
  -- one after it plays: a bug that hides itself after 2.8 seconds.
  sched_step(0)
end

-- A two-step, the shape most drum'n'bass starts from: kick on the one, snare
-- on the two and the four, and the bar's whole character in what sits between
-- them. Bar two answers bar one rather than repeating it.
function demo_pattern()
  -- Kick.
  set_step(T_KICK, 0, LVL_ACC, 0, MIC_ON)
  set_step(T_KICK, 10, LVL_NORM, 0, MIC_ON)
  set_step(T_KICK, 16, LVL_ACC, 0, MIC_ON)
  set_step(T_KICK, 22, LVL_NORM, 0, MIC_ON)
  set_step(T_KICK, 27, LVL_GHOST, 0, MIC_ON)

  -- Snare: two accents a bar, and the ghosts that make it swing. Every ghost
  -- here sits a frame late, which is the whole trick — on the grid they read
  -- as a machine, behind it they read as a hand.
  set_step(T_SNR, 4, LVL_ACC, 0, MIC_ON)
  set_step(T_SNR, 12, LVL_ACC, 0, MIC_ON)
  set_step(T_SNR, 20, LVL_ACC, 0, MIC_ON)
  set_step(T_SNR, 28, LVL_ACC, 0, MIC_ON)
  set_step(T_SNR, 3, LVL_GHOST, 0, 2)
  set_step(T_SNR, 7, LVL_GHOST, 0, 2)
  set_step(T_SNR, 11, LVL_GHOST, 1, 2)
  set_step(T_SNR, 14, LVL_GHOST, 0, 2)
  set_step(T_SNR, 19, LVL_GHOST, 0, 2)
  set_step(T_SNR, 23, LVL_GHOST, 0, 2)
  set_step(T_SNR, 26, LVL_NORM, 0, MIC_ON)
  set_step(T_SNR, 31, LVL_GHOST, 2, 2)

  -- Hats: the pulse, accented on the beat, doubled here and there.
  for s = 0, NSTEP - 1 do
    if s % 4 == 0 then
      set_step(T_HAT, s, LVL_NORM, 0, MIC_ON)
    elseif s % 2 == 0 then
      set_step(T_HAT, s, LVL_GHOST, 0, MIC_ON)
    end
  end
  set_step(T_HAT, 15, LVL_GHOST, 1, 2)
  set_step(T_HAT, 30, LVL_ACC, 1, MIC_ON)

  set_step(T_OHH, 6, LVL_NORM, 0, MIC_ON)
  set_step(T_OHH, 24, LVL_NORM, 0, MIC_ON)

  set_step(T_RIM, 9, LVL_GHOST, 0, 2)
  set_step(T_RIM, 18, LVL_NORM, 0, MIC_ON)
  set_step(T_CLP, 12, LVL_NORM, 0, 2)

  -- Sub: two long notes a bar, the second answering a tone down.
  set_step(T_SUB, 0, LVL_NORM, 0, MIC_ON)
  bnote[0] = 29
  set_step(T_SUB, 10, LVL_NORM, 0, MIC_ON)
  bnote[10] = 29
  set_step(T_SUB, 16, LVL_NORM, 0, MIC_ON)
  bnote[16] = 32
  set_step(T_SUB, 24, LVL_NORM, 0, MIC_ON)
  bnote[24] = 27

  -- Reese: one note a bar, **restruck on every 16th**, its cutoff drawn across
  -- the lane. This is the wobble, and it is a drawing rather than an
  -- oscillator.
  --
  -- The restriking is the whole mechanism, not an ornament. A patch is fixed
  -- when its voice starts, so a note held across the bar sounds whichever
  -- cutoff its own step named and the other thirty-one are a drawing nobody
  -- hears. Retriggering is what a sweep costs when nothing modulates a running
  -- voice — and it is why the lane is worth drawing at all.
  for s = 0, NSTEP - 1 do
    local w = s % 8
    if w > 4 then w = 8 - w end
    wob[s] = w + 1

    set_step(T_RSE, s, LVL_NORM, 0, MIC_ON)
    if s < HALF then
      bnote[NSTEP + s] = 41
    else
      bnote[NSTEP + s] = 44
    end
  end
end

-- ---------------------------------------------------------------------------
-- The clock and the queue.

-- The three levels, as velocities. These are the master fader: every patch is
-- mixed against them, so a kit that pins the limiter is turned down here once
-- rather than in twenty declarations.
function vel_for(lvl)
  if lvl == LVL_GHOST then return 50 end
  if lvl == LVL_NORM then return 148 end
  return 205
end

-- How many frames a melodic step should sound for: up to the next note on the
-- same track, so a lane of held notes needs no explicit lengths. Capped at
-- 250 because `play`'s frame count is a byte.
function gate_frames(t, s)
  for k = 1, NSTEP - 1 do
    local n = s + k
    if n >= NSTEP then n = n - NSTEP end
    if (pat[t * NSTEP + n] & 3) ~= 0 then
      local f = k * step_len / 64
      if f > 250 then return 250 end
      if f < 1 then return 1 end
      return f
    end
  end
  return 250
end

function queue_push(del, t, n, v, lvl, var, gate)
  for i = 0, PQ - 1 do
    if q_on[i] == 0 then
      q_on[i] = 1
      q_del[i] = del
      q_trk[i] = t
      q_note[i] = n
      q_vel[i] = v
      q_lvl[i] = lvl
      q_var[i] = var
      q_gate[i] = gate
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
  local b = pat[t * NSTEP + s]
  local lvl = b & 3
  if lvl == LVL_OFF then return end

  local extra = (b >> 2) & 3
  local mic = (b >> 4) & 3
  local v = vel_for(lvl)

  local n = 38
  local var = 0
  if t == T_KICK then n = 33 end
  if t == T_SNR then n = 50 end
  if t == T_HAT then n = 72 end
  if t == T_OHH then n = 72 end
  if t == T_RIM then n = 64 end
  if t == T_CLP then n = 60 end
  if t == T_SUB then n = bnote[s] end
  if t == T_RSE then
    n = bnote[NSTEP + s]
    var = wob[s]
    if var > 7 then var = 7 end
  end
  if n == 0 then return end

  -- A held track is never rolled: a sub retriggered three times inside one
  -- 16th is a fart, not a fill.
  if t == T_SUB or t == T_RSE then
    -- The gate is measured here, from the step being scheduled, not in `fire`
    -- from `cur` — by the time a nudged hit sounds the playhead has moved on,
    -- and a note whose length depends on when it happened to fire is a note
    -- that changes length when you nudge it.
    queue_push(mic, t, n, v, lvl, var, gate_frames(t, s))
    return
  end

  for k = 0, extra do
    local d = mic + k * step_len / ((extra + 1) * 64)
    local hv = v
    if k > 0 then hv = v * 3 / 4 end
    queue_push(d, t, n, hv, lvl, var, 0)
  end
end

function sched_step(s)
  for t = 0, NTRK - 1 do
    sched_track(t, s)
  end
end

-- Sound one hit. The level picks the patch, not just the velocity — that is
-- the whole reason this game declares three snares instead of one.
function fire(t, n, v, lvl, var, gate)
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
  elseif t == T_CLP then
    play(clap, n, v, 12)
  elseif t == T_SUB then
    note_on(T_SUB, sub, n, v)
    hold[0] = gate
  else
    note_on(T_RSE, reeses[var], n, v)
    hold[1] = gate
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
        fire(q_trk[i], q_note[i], q_vel[i], q_lvl[i], q_var[i], q_gate[i])
        q_on[i] = 0
        fired = fired + 1
      else
        q_del[i] = q_del[i] - 1
      end
    end
  end
  return fired
end

-- Count the held notes down and release them. The `note_off` goes on the
-- frame the counter *reaches* zero, never while it sits there, so an idle
-- channel is not released once a frame — the sound log this game exists to be
-- read from would otherwise be all releases.
function tick_holds()
  for k = 0, 1 do
    if hold[k] > 0 then
      hold[k] = hold[k] - 1
      if hold[k] == 0 then note_off(T_SUB + k) end
    end
  end
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

-- A melodic step painted from the grid has no pitch of its own, and a step with
-- note 0 is a step the scheduler silently skips. So it inherits the last pitch
-- that track played — which is also the musically useful answer: tapping more
-- sub steps repeats the note you are already on rather than dropping a root
-- nobody asked for.
function default_note(t, s)
  for k = 1, NSTEP - 1 do
    local n = s + NSTEP - k
    if n >= NSTEP then n = n - NSTEP end
    local v = bnote[(t - T_SUB) * NSTEP + n]
    if v ~= 0 then return v end
  end
  if t == T_SUB then return 29 end
  return 41
end

-- Paint one step at the pen's level.
--
-- `toggle` is 1 for the press that starts a gesture and 0 for every step the
-- finger then drags across. A drag that could also clear would flicker a hat
-- run on and off as the finger crossed its own work, so a drag only ever sets —
-- the rule every paint program already uses, for the same reason.
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

  if t >= T_SUB then
    local bi = (t - T_SUB) * NSTEP + s
    if bnote[bi] == 0 then bnote[bi] = default_note(t, s) end
  end
end

-- Scatter ghosts across a track's empty off-16ths.
--
-- Drums only: a ghost is a hit played weakly, and a sub played weakly is just a
-- quiet sub. The rule is the one a hand follows — fill the "e" and the "a", skip
-- anything touching an accent so the accent keeps its space, and lay them all a
-- frame behind the grid, which is what makes the result read as a player rather
-- than a machine.
function ghost_fill(t)
  if t >= T_SUB then return end
  for s = 1, NSTEP - 1, 2 do
    if (pat[t * NSTEP + s] & 3) == LVL_OFF then
      local nx = s + 1
      if nx >= NSTEP then nx = 0 end
      local near = 0
      if (pat[t * NSTEP + s - 1] & 3) == LVL_ACC then near = 1 end
      if (pat[t * NSTEP + nx] & 3) == LVL_ACC then near = 1 end
      if near == 0 then
        pat[t * NSTEP + s] = LVL_GHOST | (2 << 4)
      end
    end
  end
end

-- Clear the bar on screen, not the whole track. Rewriting one bar to answer the
-- other is the common edit; losing both to a mis-tap is not an edit at all.
function clear_bar(t)
  for i = 0, VIS - 1 do
    pat[t * NSTEP + page + i] = 0
    if t >= T_SUB then bnote[(t - T_SUB) * NSTEP + page + i] = 0 end
  end
end

-- The editor panel's three knobs. Each cycles, because a cycling button is one
-- finger-sized target where three radio buttons are three small ones, and the
-- panel has room for three targets rather than nine.
--
-- The level cycles through the three *sounding* levels and never through off: a
-- step is turned off by tapping it in the grid, where the finger already is.
function cycle_vel()
  local i = sel_trk * NSTEP + sel_step
  local lvl = pat[i] & 3
  if lvl == LVL_ACC or lvl == LVL_OFF then
    lvl = LVL_GHOST
  else
    lvl = lvl + 1
  end
  pat[i] = (pat[i] & 252) | lvl
  if sel_trk >= T_SUB then
    local bi = (sel_trk - T_SUB) * NSTEP + sel_step
    if bnote[bi] == 0 then bnote[bi] = default_note(sel_trk, sel_step) end
  end
end

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

function toggle_play()
  if playing == 1 then
    playing = 0
    clear(q_on)
    note_off(T_SUB)
    note_off(T_RSE)
    clear(hold)
  else
    playing = 1
    acc = 0
    cur = 0
    sched_step(0)
  end
end

-- Where a finger landed, and what role it takes for the rest of its life.
function press_at(x, y)
  if y < HDR_H then
    if x >= 160 then toggle_play() end
    return ROLE_BTN
  end

  if y < PEN_Y + PEN_H then
    local c = x / 80
    if c > 2 then c = 2 end
    pen = c + 1
    return ROLE_BTN
  end

  if y < BTN_Y + BTN_H then
    if x < 72 then
      if page == 0 then page = HALF else page = 0 end
    elseif x < 156 then
      ghost_fill(sel_trk)
    else
      clear_bar(sel_trk)
    end
    return ROLE_BTN
  end

  if y >= DET_Y then
    if sel_step == NONE then return ROLE_BTN end
    if x < 78 then
      cycle_vel()
    elseif x < 158 then
      cycle_roll()
    else
      cycle_mic()
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
  sel_trk = t
  sel_step = st
  paint(t, st, 1)
  return ROLE_GRID
end

function edit_touches()
  for i = 0, 3 do
    if touch_pressed(i) then
      role[i] = press_at(touch_x(i), touch_y(i))
    elseif touch_down(i) then
      if role[i] == ROLE_GRID then
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
  -- B pages, it does not clear. A bare button that destroys a bar of work is a
  -- bare button somebody leans on: clearing stays on the labelled box, where
  -- hitting it means having aimed at it.
  if btnp(B) then
    if page == 0 then page = HALF else page = 0 end
  end
  edit_touches()

  if playing == 1 then
    acc = acc + 64
    while acc >= step_len do
      acc = acc - step_len
      cur = cur + 1
      if cur >= NSTEP then cur = 0 end
      sched_step(cur)
    end
  end

  -- Drained after scheduling, so a hit with no nudge sounds on the frame its
  -- step begins rather than the frame after it.
  local fired = drain_queue()
  tick_holds()
  decay_lights()

  signal(step, cur)
  signal(hits, fired)
  signal(pen, pen)
  signal(track, sel_trk)
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
  if lvl == LVL_GHOST then return 4 end
  if lvl == LVL_NORM then return 8 end
  return 12
end

-- One place that knows a track's name, because `text` takes a literal and the
-- alternative is the same eight-way branch written twice.
function track_name(t, x, y, c)
  if t == T_KICK then text("KCK", x, y, c)
  elseif t == T_SNR then text("SNR", x, y, c)
  elseif t == T_HAT then text("HAT", x, y, c)
  elseif t == T_OHH then text("OHH", x, y, c)
  elseif t == T_RIM then text("RIM", x, y, c)
  elseif t == T_CLP then text("CLP", x, y, c)
  elseif t == T_SUB then text("SUB", x, y, c)
  else text("RSE", x, y, c) end
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
    rect(160, 2, 78, 18, 34)
    text("PLAYING", 184, 8, 255)
  else
    rect(160, 2, 78, 18, 238)
    text("STOPPED", 184, 8, 250)
  end
end

-- The pen: the level a tap paints. Selected is the bright one.
function draw_pen()
  for i = 0, 2 do
    local c = 234
    if pen == i + 1 then c = 245 end
    rect(i * 80, PEN_Y, 78, PEN_H, c)
  end
  text("GHOST", 28, PEN_Y + 8, 250)
  text("NORM", 111, PEN_Y + 8, 250)
  text("ACC", 193, PEN_Y + 8, 250)
end

function draw_buttons()
  rect(0, BTN_Y, 70, BTN_H, 234)
  if page == 0 then
    text("BAR 1", 25, BTN_Y + 7, 252)
  else
    text("BAR 2", 25, BTN_Y + 7, 252)
  end
  -- The beat is on the bar you are not looking at: the one thing following the
  -- playhead was for, kept without letting the page move under a finger.
  if playhead_on_page() == 0 then
    rect(64, BTN_Y + 2, 4, BTN_H - 4, 255)
  end

  rect(74, BTN_Y, 80, BTN_H, 234)
  text("GHOST", 104, BTN_Y + 7, 250)
  rect(158, BTN_Y, 80, BTN_H, 234)
  text("CLR BAR", 184, BTN_Y + 7, 250)
end

function draw_labels()
  for t = 0, NTRK - 1 do
    local y0 = GRID_Y + t * ROW_H
    if t == sel_trk then
      -- The two buttons above act on this track and nothing else says so.
      rect(0, y0, LBL_W - 2, ROW_H - 2, 237)
    end
    track_name(t, 6, y0 + 6, col[t])
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

-- The step editor. Three cycling buttons rather than nine radio buttons: the
-- strip has room for three finger-sized targets, and every one of these is a
-- short cycle a thumb can walk.
function draw_detail()
  rect(0, DET_Y, DIM, DET_H, 234)

  if sel_step == NONE then
    text("TAP A STEP TO EDIT IT", 6, DET_Y + 12, 240)
    return
  end

  text("STEP", 4, DET_Y + 4, 245)
  number(sel_step + 1, 30, DET_Y + 4, 255)
  track_name(sel_trk, 58, DET_Y + 4, col[sel_trk])

  local by = DET_Y + 14
  local b = pat[sel_trk * NSTEP + sel_step]
  local lvl = b & 3
  local extra = (b >> 2) & 3
  local mic = (b >> 4) & 3

  rect(2, by, 74, 18, 237)
  if lvl == LVL_OFF then
    text("VEL OFF", 25, by + 6, 241)
  elseif lvl == LVL_GHOST then
    text("VEL GHST", 23, by + 6, 250)
  elseif lvl == LVL_NORM then
    text("VEL NORM", 23, by + 6, 253)
  else
    text("VEL ACC", 25, by + 6, 255)
  end

  rect(80, by, 76, 18, 237)
  if extra == 0 then
    text("ROLL 1", 106, by + 6, 248)
  elseif extra == 1 then
    text("ROLL 2", 106, by + 6, 252)
  else
    text("ROLL 3", 106, by + 6, 255)
  end

  rect(160, by, 78, 18, 237)
  if mic == 0 then
    text("MIC EARLY", 181, by + 6, 250)
  elseif mic == 1 then
    text("MIC ON", 187, by + 6, 248)
  else
    text("MIC LATE", 183, by + 6, 253)
  end
end

function draw()
  cls(232)
  draw_header()
  draw_pen()
  draw_buttons()
  draw_labels()
  draw_grid()
  draw_detail()

  -- The plate, dark and cold, ducking on every kick. A fill rather than a lamp,
  -- so it also clears the previous frame's light and its obstacles.
  ambient(21 - pump / 3, 18 - pump / 4, 38 - pump / 4)

  -- Every control surface, flat at neutral. Text on these stays readable at an
  -- ambient a third of it, which no arrangement of round lights achieves
  -- without bleeding into the grid.
  light_rect(0, HDR_Y, DIM, HDR_H, 64, 64, 64)
  light_rect(0, PEN_Y, DIM, BTN_Y + BTN_H - PEN_Y, 58, 58, 62)
  light_rect(0, GRID_Y, LBL_W, NTRK * ROW_H, 58, 58, 62)
  light_rect(0, DET_Y, DIM, DET_H, 58, 58, 62)

  if playhead_on_page() == 1 then
    -- Two lamps rather than one, a quarter and three quarters down. A single
    -- lamp wide enough to reach both the kick row and the Reese row is also
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

  -- The selected step glows a little, so the bracket is findable at a glance
  -- on a screen this dark.
  if sel_step ~= NONE and sel_step >= page and sel_step < page + VIS then
    light(GRID_X + (sel_step - page) * CELL_W + CELL_W / 2,
          GRID_Y + sel_trk * ROW_H + ROW_H / 2, 9, 30, 30, 38)
  end

  -- No `shadow_rect` anywhere: a flat panel has no occluders, and declaring one
  -- would buy the whole O(r squared) shadow walk for every lamp above in
  -- exchange for nothing.
end
