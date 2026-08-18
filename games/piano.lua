-- piano.lua — a four-voice keyboard you play with your fingers (or the mouse,
-- which stands in as touch slot 0 on a machine with no touchscreen).
--
--   kessel run games/piano.lua
--
-- Ten white keys — C up to the E past the octave — and the seven black keys
-- between them. Touch reports up to four fingers, so chords work for real:
-- `note_on` puts each finger's note on its own channel, and a key sounds for
-- exactly as long as the finger holding it stays down. This is the reference
-- for the held-note API (`note_on`/`note_off`), the way `popn.lua` is the
-- reference for a button row with no directions.
--
-- Four modes across the top, each with its own panel:
--
--   PIANO    a struck string — one triangle patch, nothing to adjust
--   E.PIANO  a tine and a tremolo — one chorused sine, nothing to adjust
--   ORGAN    four drawbars you pull, summed into every key you press
--   SYNTH    a waveform and a filter cutoff you pick
--
-- **A patch is a compile-time declaration; a player-facing knob is not.** The
-- bank is built when the ROM loads and there is no port to edit one afterwards,
-- so nothing here mutates a patch. A knob is a *choice between patches* that
-- were all declared up front (SYNTH's sixteen), or a *choice of how to play*
-- one patch (ORGAN's drawbars). That is the whole idiom this game exists to
-- show, and the bank is metadata beside the ROM rather than bytes in the 64 KiB,
-- so declaring sixteen variants of one synth costs the game nothing.
--
-- The organ is the interesting half. A drawbar organ is additive: one key
-- sounds several sine partials at harmonic footages, each at its own level.
-- This one has four — 16', 8', 4' and 2 2/3' — and plays them as four real
-- `note_on`s on four channels, with the drawbar's level scaling that partial's
-- velocity. Four fingers times four partials is exactly `MAX_VOICES`, which is
-- why there are four drawbars and not the nine a Hammond has.
--
-- Drawbar changes land on the *next* key pressed, not on notes already
-- sounding: a voice's velocity is fixed when it starts, and re-triggering a
-- held chord to fake it would stutter every note in it.

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
  chorus_rate = 40
  chorus_depth = 120
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

-- A tine, not a string: a sine struck hard, most of its body gone by the time
-- the note settles, with chorus doing the work a real one's tremolo does.
instrument epiano {
  wave = sine
  attack = 2  decay = 240  sustain = 80  release = 260
  filter = lpf  cutoff = 190
  chorus = 140  reverb = 50
  volume = 160
}

-- One tonewheel. Near-instant on and off, flat while held, no filter — the
-- shape is entirely in how many of these a key stacks and at what levels.
-- Played four times per key at four footages, never once.
instrument organ_tone {
  wave = sine
  attack = 1  decay = 20  sustain = 255  release = 40
  filter = off
  reverb = 30
  volume = 110
}

-- The synth's sixteen. A player-facing knob cannot edit a patch — the bank is
-- fixed when the ROM loads — so every position of both knobs is declared here
-- and `note_on` is handed whichever one the panel currently selects. Four
-- waveforms across, four cutoffs down; the ids go in `synths` at init so the
-- lookup is an array index rather than a bet on declaration order.
--
-- Resonance is high and equal on all sixteen so that moving the cutoff is
-- audible as a *filter* rather than as a volume change.
instrument syn_tri_0 {
  wave = triangle
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 60  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_tri_1 {
  wave = triangle
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 100  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_tri_2 {
  wave = triangle
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 155  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_tri_3 {
  wave = triangle
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 225  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_saw_0 {
  wave = saw
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 60  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_saw_1 {
  wave = saw
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 100  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_saw_2 {
  wave = saw
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 155  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_saw_3 {
  wave = saw
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 225  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_sqr_0 {
  wave = square
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 60  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_sqr_1 {
  wave = square
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 100  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_sqr_2 {
  wave = square
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 155  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_sqr_3 {
  wave = square
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 225  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_sin_0 {
  wave = sine
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 60  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_sin_1 {
  wave = sine
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 100  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_sin_2 {
  wave = sine
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 155  resonance = 175
  reverb = 30  volume = 140
}
instrument syn_sin_3 {
  wave = sine
  attack = 6  decay = 190  sustain = 170  release = 190
  filter = lpf  cutoff = 225  resonance = 175
  reverb = 30  volume = 140
}

local DIM = 240

-- Panel geometry, top to bottom: a header carrying the octave buttons, the
-- mode row, the mode's own parameter panel, then the keybed filling the rest.
local HDR_H = 26
local OCT_Y = 2
local OCT_H = 22
local OCT_DN_X = 140
local OCT_UP_X = 190
local OCT_W = 46

local MODE_Y = 28
local MODE_H = 24
local MODE_W = 60          -- 4 * 60 = 240, drawn 2px narrower for a seam

local PAR_Y = 56
local PAR_H = 48

-- The drawbars' travel, below a strip that keeps their footage labels off the
-- fill. Drawing the label inside the bar puts it on orange at level 8 and on
-- the background at level 0, so it is unreadable at one end whatever colour it
-- is given.
local BAR_Y = 65
local BAR_H = 39

-- 240 / 10 divides exactly, so every white key is the same width with no
-- rounding remainder on the last one.
local NUM_WHITE = 10
local WHITE_W = 24
local KB_Y = 108
local KB_H = 132
local BLACK_W = 14
local BLACK_H = 78

local NONE = 255           -- key_at's "no key here" answer

-- Octave shift is stored as the note C plays, not as a signed count — every
-- value it can ever hold is positive, so the clamp is a plain unsigned
-- compare and nothing here needs `int`.
local MIN_BASE = 36        -- C2
local MAX_BASE = 84        -- C6

local MODE_PIANO = 0
local MODE_EPIANO = 1
local MODE_ORGAN = 2
local MODE_SYNTH = 3

-- Four partials per key at four footages: 16', 8', 4', 2 2/3'. Held as offsets
-- from `note - 12` rather than from `note`, so the lowest partial is a plain
-- addition — `note` can be as low as 36, and an unsigned `note - 12` on the
-- 16' bar would be the one place this game could wrap.
local NUM_BARS = 4
local bar_off: array(4, byte)

-- A slot's four channels are `slot * 4 + partial`, so four fingers times four
-- partials is channels 0-15 and never a collision. The modes that sound one
-- voice per key use partial 0 and leave the rest idle.
local PARTS = 4

local base_note = 60       -- C4, the octave a fresh boot starts on
local mode = 0

-- Per touch-slot state. `role` is what the finger grabbed when it landed, and
-- it does not change while the finger is down: a drag that starts on a drawbar
-- must keep pulling that drawbar even when it wanders over the keybed, and a
-- glissando that starts on a key must not start flipping modes.
local ROLE_NONE = 0
local ROLE_KEY = 1
local ROLE_BAR = 2
local role: array(4, byte)
local active: array(4, byte)
local held: array(4, byte)
local bar_of: array(4, byte)

-- Drawbar levels, 0-8, the range a Hammond's stops are labelled with.
local bars: array(4, byte)

-- Is channel `slot * PARTS + partial` currently sounding? The organ skips a
-- drawbar sitting at zero rather than starting a silent voice, so which
-- channels a key started is not something the mode can be asked for later —
-- and a drawbar pulled to zero *during* a note would give the wrong answer.
-- One flag per channel is the whole memory, and it keeps every `note_off` in
-- the event log paired with a `note_on` that really happened.
local live: array(16, byte)

-- The synth panel's two knobs, and the sixteen ids they choose between.
local syn_wave = 0
local syn_cut = 2
local synths: array(16, byte)

function init()
  clear(role)
  clear(active)
  clear(held)
  clear(bar_of)
  clear(live)
  base_note = 60
  mode = 0
  syn_wave = 0
  syn_cut = 2

  bar_off[0] = 0           -- 16'
  bar_off[1] = 12          -- 8'
  bar_off[2] = 24          -- 4'
  bar_off[3] = 31          -- 2 2/3'

  -- A drawbar registration with the fundamental up and a little brightness.
  bars[0] = 8
  bars[1] = 6
  bars[2] = 4
  bars[3] = 2

  synths[0] = syn_tri_0   synths[1] = syn_tri_1
  synths[2] = syn_tri_2   synths[3] = syn_tri_3
  synths[4] = syn_saw_0   synths[5] = syn_saw_1
  synths[6] = syn_saw_2   synths[7] = syn_saw_3
  synths[8] = syn_sqr_0   synths[9] = syn_sqr_1
  synths[10] = syn_sqr_2  synths[11] = syn_sqr_3
  synths[12] = syn_sin_0  synths[13] = syn_sin_1
  synths[14] = syn_sin_2  synths[15] = syn_sin_3
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

-- The instrument the current mode plays a single voice with. ORGAN never asks:
-- it stacks `organ_tone` itself.
function voice_inst()
  if mode == MODE_EPIANO then return epiano end
  if mode == MODE_SYNTH then return synths[syn_wave * 4 + syn_cut] end
  return piano
end

-- Sound note `n` for the finger in `slot`, at `vel`.
--
-- One place, because the organ's stacking is the only thing that differs
-- between modes and it must be undone exactly the way it was done — a mode
-- that started four partials and released one is a note that never stops.
function voice_on(slot, n, vel)
  if mode == MODE_ORGAN then
    for b = 0, NUM_BARS - 1 do
      if bars[b] > 0 then
        -- The drawbar's level scales this partial's velocity; that is what a
        -- drawbar physically does to its tonewheel's contribution.
        note_on(slot * PARTS + b, organ_tone, n - 12 + bar_off[b], vel * bars[b] / 8)
        live[slot * PARTS + b] = 1
      end
    end
  else
    note_on(slot * PARTS, voice_inst(), n, vel)
    live[slot * PARTS] = 1
  end
end

-- Release exactly the channels this slot started, and no others. Firing all
-- four unconditionally would work — a `note_off` on an idle channel does
-- nothing — but it would put three phantom releases in the sound log for every
-- single-voice note, and this game is the reference an agent reads that log
-- against.
function voice_off(slot)
  for b = 0, PARTS - 1 do
    if live[slot * PARTS + b] == 1 then
      note_off(slot * PARTS + b)
      live[slot * PARTS + b] = 0
    end
  end
end

-- Every note any finger is currently holding, off — used before an octave or
-- mode change so nothing keeps ringing at a pitch, or in a timbre, that the
-- panel no longer shows for it.
function release_all()
  for s = 0, 3 do
    if active[s] == 1 then
      voice_off(s)
      active[s] = 0
    end
  end
end

function set_mode(m)
  if m ~= mode then
    release_all()
    mode = m
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

-- Which drawbar column is x in, or NONE? Only meaningful in ORGAN.
function bar_at(x)
  local b = x / MODE_W
  if b >= NUM_BARS then return NONE end
  return b
end

-- Pull the drawbar to wherever the finger is: the top of the panel is 8, the
-- bottom is 0. Continuous rather than press-only, so a drawbar is dragged the
-- way a real one is pulled — and clamped at both ends, because the hit test
-- that started the drag never runs again once the finger leaves the panel.
--
-- Nine bands over the panel's height, not eight, then clamped. Dividing the
-- height into eight makes level 8 reachable on exactly one row of pixels while
-- level 0 gets six — there are *nine* stops from 0 to 8, and sizing the bands
-- to the gaps between them is what puts the top one out of reach.
function pull_bar(b, y)
  if y <= BAR_Y then
    bars[b] = 8
  elseif y >= BAR_Y + BAR_H then
    bars[b] = 0
  else
    local lv = (BAR_Y + BAR_H - y) * 9 / BAR_H
    if lv > 8 then lv = 8 end
    bars[b] = lv
  end
end

-- A touch that landed in the panels, not on a key. Returns the role the finger
-- takes for the rest of its life: a drawbar keeps receiving the drag, anything
-- else is a button that has already done its work.
function press_panel(slot, x, y)
  if y >= OCT_Y and y < OCT_Y + OCT_H then
    if x >= OCT_DN_X and x < OCT_DN_X + OCT_W then octave_down() end
    if x >= OCT_UP_X and x < OCT_UP_X + OCT_W then octave_up() end
    return ROLE_NONE
  end

  if y >= MODE_Y and y < MODE_Y + MODE_H then
    set_mode(x / MODE_W)
    return ROLE_NONE
  end

  if y >= PAR_Y and y < PAR_Y + PAR_H then
    if mode == MODE_ORGAN then
      local b = bar_at(x)
      if b == NONE then return ROLE_NONE end
      bar_of[slot] = b
      pull_bar(b, y)
      return ROLE_BAR
    end
    if mode == MODE_SYNTH then
      -- Waveform on the top half, cutoff on the bottom: two rows of four in
      -- the same panel the drawbars use, so the layout never jumps.
      local col = x / MODE_W
      if col > 3 then return ROLE_NONE end
      if y < PAR_Y + PAR_H / 2 then
        syn_wave = col
      else
        syn_cut = col
      end
      -- A held note keeps the patch it started with; the new one is what the
      -- next key press gets. Retriggering the chord to apply it would restart
      -- every note in it, which reads as a stutter rather than a filter sweep.
      return ROLE_NONE
    end
  end

  return ROLE_NONE
end

function update()
  if btnp(A) then octave_down() end
  if btnp(B) then octave_up() end

  for s = 0, 3 do
    if touch_pressed(s) then
      local px = touch_x(s)
      local py = touch_y(s)
      if py >= KB_Y then
        role[s] = ROLE_KEY
      else
        role[s] = press_panel(s, px, py)
      end
    end

    if touch_down(s) then
      local tx = touch_x(s)
      local ty = touch_y(s)

      if role[s] == ROLE_BAR then
        pull_bar(bar_of[s], ty)
      elseif role[s] == ROLE_KEY then
        local n = key_at(tx, ty)
        if n == NONE then
          -- Dragged off the keybed entirely: stop, but keep the role, so
          -- sliding back on plays again rather than falling through to the
          -- panel underneath.
          if active[s] == 1 then
            voice_off(s)
            active[s] = 0
          end
        elseif active[s] == 0 then
          voice_on(s, n, vel_for_y(ty))
          active[s] = 1
          held[s] = n
        elseif n ~= held[s] then
          -- The finger slid onto a different key: retune rather than
          -- restart, so a fast glissando still reads as one continuous drag.
          voice_off(s)
          voice_on(s, n, vel_for_y(ty))
          held[s] = n
        end
      end
      entity(tx, ty, s)
    else
      if active[s] == 1 then
        voice_off(s)
        active[s] = 0
      end
      role[s] = ROLE_NONE
    end
  end

  -- Reported for observation: an agent cannot see the panel and cannot hear
  -- the result, so the two knobs and the registration are numbers it can read.
  entity(mode, base_note, 10)
  entity(syn_wave, syn_cut, 11)
  entity(bars[0], bars[1], 12)
  entity(bars[2], bars[3], 13)
end

-- A filled box. `hline` is the only filled primitive the console has, so every
-- panel, key and bar in this game is a run of them.
function box(x0, y0, w, h, c)
  for y = y0, y0 + h - 1 do
    hline(x0, x0 + w - 1, y, c)
  end
end

function draw_header()
  box(OCT_DN_X, OCT_Y, OCT_W, OCT_H, 5)
  box(OCT_UP_X, OCT_Y, OCT_W, OCT_H, 5)
  text("DN", OCT_DN_X + 16, OCT_Y + 8, 7)
  text("UP", OCT_UP_X + 16, OCT_Y + 8, 7)

  text("OCT", 100, OCT_Y + 8, 6)
  number(base_note / 12 - 1, 124, OCT_Y + 8, 10)
  text("PIANO", 4, OCT_Y + 8, 7)
end

function draw_modes()
  for i = 0, 3 do
    local c = 1
    if i == mode then c = 12 end
    box(i * MODE_W, MODE_Y, MODE_W - 2, MODE_H, c)
  end
  -- `text` takes a literal, so the four names are four calls rather than a
  -- lookup — there is no string type to hold them in.
  text("PIANO", 8, MODE_Y + 9, 7)
  text("E.PNO", 68, MODE_Y + 9, 7)
  text("ORGAN", 128, MODE_Y + 9, 7)
  text("SYNTH", 188, MODE_Y + 9, 7)
end

-- The organ panel: four drawbars, filled from the bottom to their level, with
-- the footage each one sounds written at the top.
function draw_bars()
  for b = 0, NUM_BARS - 1 do
    local x0 = b * MODE_W + 4
    local w = MODE_W - 10
    box(x0, BAR_Y, w, BAR_H, 1)
    local fill = bars[b] * BAR_H / 8
    if fill > 0 then
      box(x0, BAR_Y + BAR_H - fill, w, fill, 9)
    end
    number(bars[b], x0 + w - 6, PAR_Y + 2, 10)
  end
  -- Footage, the name of the pitch each bar sounds. 2 2/3' has no room and no
  -- fraction glyphs, so it goes by its harmonic instead.
  text("16", 6, PAR_Y + 2, 6)
  text("8", 66, PAR_Y + 2, 6)
  text("4", 126, PAR_Y + 2, 6)
  text("3RD", 186, PAR_Y + 2, 6)
end

-- The synth panel: waveform across the top row, cutoff across the bottom.
function draw_synth()
  local h = PAR_H / 2 - 2
  for i = 0, 3 do
    local c = 1
    if i == syn_wave then c = 12 end
    box(i * MODE_W, PAR_Y, MODE_W - 2, h, c)

    local c2 = 1
    if i == syn_cut then c2 = 9 end
    box(i * MODE_W, PAR_Y + PAR_H / 2, MODE_W - 2, h, c2)
  end
  text("TRI", 12, PAR_Y + 6, 7)
  text("SAW", 72, PAR_Y + 6, 7)
  text("SQR", 132, PAR_Y + 6, 7)
  text("SIN", 192, PAR_Y + 6, 7)
  -- The cutoff row names its own positions rather than carrying a "CUT" label
  -- beside a 1-4: the label had to sit inside the first button, which read as
  -- that button being called CUT.
  text("DARK", 8, PAR_Y + PAR_H / 2 + 5, 7)
  text("WARM", 68, PAR_Y + PAR_H / 2 + 5, 7)
  text("BRT", 132, PAR_Y + PAR_H / 2 + 5, 7)
  text("OPEN", 188, PAR_Y + PAR_H / 2 + 5, 7)
end

function draw_panel()
  if mode == MODE_ORGAN then
    draw_bars()
  elseif mode == MODE_SYNTH then
    draw_synth()
  else
    box(0, PAR_Y, DIM, PAR_H, 1)
    if mode == MODE_PIANO then
      text("STRUCK STRING", 8, PAR_Y + 20, 6)
    else
      text("TINE AND CHORUS", 8, PAR_Y + 20, 6)
    end
  end
end

function draw_keys()
  -- White keys first. Each is drawn one px narrower than its slot, so the
  -- background shows through as a seam between keys with no separate divider
  -- pass needed.
  for i = 0, NUM_WHITE - 1 do
    local x0 = i * WHITE_W
    local c = 7
    if key_is_active(base_note + white_offset(i)) == 1 then c = 10 end
    box(x0 + 1, KB_Y, WHITE_W - 2, KB_H, c)
  end

  -- Black keys on top, shorter and narrower, centred on the boundary between
  -- the two white keys they sit above.
  for i = 0, NUM_WHITE - 2 do
    if has_black(i) == 1 then
      local bx = (i + 1) * WHITE_W - BLACK_W / 2
      local c = 0
      if key_is_active(base_note + black_offset(i)) == 1 then c = 9 end
      box(bx, KB_Y, BLACK_W, BLACK_H, c)
    end
  end
end

function draw()
  cls(1)
  draw_header()
  draw_modes()
  draw_panel()
  draw_keys()
end
