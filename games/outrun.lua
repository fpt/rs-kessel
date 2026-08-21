-- outrun.lua — a pseudo-3D road racer in the Outrun/Pole-Position mould. The
-- road is drawn one horizontal scanline at a time (hline): from the near bottom
-- row up to the horizon, the centre bends by an accumulating curve (a parabola,
-- so far rows bend most) and the width shrinks with distance. The player's car
-- is a Lotus Esprit in four steering poses, the roadside scenery is four 16x16
-- sprites distance-scaled with spr_scaled, and a bobbing sun shows sin().
--
--   kessel run games/outrun.lua
--
-- Arrows steer / accelerate, A boosts, Down brakes. Hold a turn at speed and the
-- tail comes round and the tyres smoke. Drop a wheel off the tarmac and the car
-- bogs down in the dirt.
--
-- **The stage is a clock, not a cruise.** You start with twenty seconds, every
-- checkpoint buys fifteen more, and the goal is 1800 m out. That is the arcade
-- OutRun answer rather than a lap count or a finish line alone, and it is one
-- number doing three jobs: it is the reason to go fast, it is what the dirt
-- costs you, and it is a tension curve that steepens on its own as the gap to
-- the next checkpoint stops shrinking. Without it there was nothing to play —
-- `vm_playtest` could only report that every way of driving ended in the same
-- place, because there was no place to end.
--
-- The art is in `outrun/`, one file per set, because a 48x32 car in four poses
-- is 128 rows of pixels and the game is 200 lines of arithmetic — together in
-- one file, neither is findable. `screen` and `controls` stay here: they are the
-- ROM's identity and an include may not set them.
#include "outrun/car.lua"
#include "outrun/scenery.lua"
#include "outrun/smoke.lua"

-- Five things here are load-bearing and easy to undo by accident:
--
-- * The stripes live in *world* units (`w = z + travel`), and `travel` advances
--   by `speed / SCROLL` — a fraction of a world unit per frame. Scrolling the
--   pattern by a whole `speed` per frame is what made the first version strobe:
--   near the camera one row spans only `ZSCALE / DEPTH²` ≈ 1.2 world units, so a
--   jump of 48 moved the near stripes eighty rows in one frame. Anything faster
--   than a few rows per frame is not motion, it is noise.
--
-- * Stripes stop at `STRIPE_D` and the last rows fade through three haze bands
--   into the sky. A stripe is `STRIPE * d² / ZSCALE` rows tall, so past a point
--   it is thinner than a pixel and *cannot* be drawn without shimmering. Fog is
--   not decoration here, it is the anti-aliasing.
--
-- * `ZSCALE`, `STRIPE` and `SEG` are all in world units, so they only mean
--   anything relative to `ZSCALE / DEPTH²` — the world units one near row spans.
--   Shortening the road from 83 rows to 51 without scaling all three by
--   `(51/83)²` would have made every stripe forty rows long and left one tree
--   per screen.
--
-- * Scenery is a second pass over the rows, walked horizon → bottom, after the
--   whole road is down. The road loop runs bottom → horizon (each row needs the
--   row's own width), so an object drawn inside it is painted over by the grass
--   of the rows above it, and far objects would land on top of near ones.
--
-- * The car and the scenery draw through **sprite banks 1 and 2** so they keep
--   their own colours instead of sharing the base sixteen: a yellow car and
--   green scenery in one 15-colour set leaves about four yellows for the car.
--   That is why nothing on the road, the grass or the sky may use palette
--   indices 16-47 — those belong to the two banks. The smoke is the exception
--   and is bank 0 on purpose; `outrun/smoke.lua` says why.

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- left/right steer, up accelerate, down brake
  a = "boost"
  pause = START
}

-- The low sun (scaled up, bobs with sin()), drawn before the road so the tarmac
-- cuts it off at the horizon. One tile, and bank 0 — a yellow disc needs neither
-- more room nor more colours than the base sixteen.
sprite sun {
  ..aaaa..
  .aaaaaa.
  aaaaaaaa
  aaaaaaaa
  a99999aa
  .999999.
  .999999.
  ..9999..
}

-- The road is the bottom 40% of the screen, as it is in Out Runners: a high
-- horizon puts the vanishing point in the middle of the picture and leaves the
-- scenery nothing to stand in front of.
local HORIZON = 76          -- sky at this row and above, road below
local BOTTOM = 127
local DEPTH = 51            -- BOTTOM - HORIZON (rows of road)
local ROAD_HALF = 60        -- road half-width at the nearest row

-- The road in world units: row `y` is at world distance `ZSCALE / d`, so one
-- near row spans ~1.2 units and one row at the horizon spans thousands.
local ZSCALE = 3072
local STRIPE = 24           -- half-period of the stripes, in world units
local SEG = 72              -- one scenery slot every this many world units
local SCROLL = 10           -- world units scrolled per frame = speed / SCROLL
-- `travel` wraps here rather than at 65536 so `z + travel` never overflows.
-- 55296 is a whole number of stripe cycles (÷48 = 1152) and its slot count
-- (÷72 = 768) is a multiple of 256, the period of `slot_kind` — so neither the
-- stripes nor the scenery jump when it wraps. Another value breaks one of them.
local WRAP = 55296

local STRIPE_D = 16         -- nearer than this: stripes. Beyond: haze.
local SCENE_D = 6           -- nearest the horizon a scenery slot may land

local SPEED_MAX = 48
local DRIFT = 32            -- sub-pixel units per pixel of centrifugal drift
local OFF_TOP = 12          -- what the dirt lets you hold
local OFF_LIMIT = 48        -- |px| past this and a wheel is off the tarmac
local PX_MAX = 88           -- far enough out to be lost, near enough to see back

local LEAN_MAX = 24         -- full opposite lock, in the units `lean` counts
local CAR_W = 48            -- the car sprite, so the smoke knows where its
local CAR_H = 32            -- rear wheels are

-- Sunset sky, top to horizon; the last band is close to the far road so the
-- tarmac melts into it instead of ending on a line. Every colour below is a
-- default-palette index outside 16-47, which the two sprite banks own.
local SKY0 = 62   local SKY1 = 98   local SKY2 = 140  local SKY3 = 176
local SKY4 = 218  local SKY5 = 217  local SKY6 = 223  local SKY7 = 230

-- Two shades per surface near the camera, then three haze steps outward.
local GRASS_A = 70   local GRASS_B = 76
local GRASS_1 = 113  local GRASS_2 = 150  local GRASS_3 = 193
local ROAD_N = 59    local ROAD_1 = 102   local ROAD_2 = 145  local ROAD_3 = 188
local RUMB_A = 160   local RUMB_B = 231
local RUMB_1 = 174   local RUMB_2 = 217   local RUMB_3 = 224
local LINE_C = 231                        -- centre dashes

-- This game is still silent, deliberately: what outrun wants is an engine that
-- rises with `speed`, and a blip on a checkpoint would be a placeholder for it
-- rather than a start on it. `games_audio.rs` derives "must be audible" from
-- whether a source declares instruments, so half a sound design would also have
-- to fire inside that guard's 300 frames — and the first checkpoint is a
-- thousand frames out.
--
-- The race: distance in metres (8 world units each, so the odometer cannot
-- overflow a word over a whole stage), time in frames.
-- These numbers were picked with `vm_playtest`, not by feel. The first attempt
-- put the opening checkpoint 600 m out on a 40-second clock, and the report said
-- flatly that `checkpoints` and `state` never moved under any policy and that
-- `time_left` ended identical for all of them — the one thing that can add to
-- the clock was out of reach, so the clock was decoration. A first segment
-- nobody reaches is also just bad arcade design: the earliest feedback should
-- arrive well inside the opening timer.
local MPU = 8               -- world units per metre
local GOAL = 1800           -- metres to the finish
local CP_EVERY = 350        -- ...and to each checkpoint on the way
local CP_COUNT = 4          -- checkpoints at 350/700/1050/1400
local START_T = 1200        -- 20 s, and 350 m needs 10 s of clean driving
local CP_BONUS = 900        -- 15 s for reaching one, so the margin never grows

-- What this game reports for observation. `entity` is for the car's *place*;
-- everything below is a *number*, and every one of them is also something the
-- player needs on the HUD — the speedometer and the clock were missing for both
-- audiences at once, which is why they are one job and not two.
--
-- Every name here is also the name of the local it mirrors. That is the normal
-- case for a signal and the reason `signal(speed, speed)` resolves its first
-- argument at the call site instead of binding `speed` as a constant.
signal speed
signal dist                 -- metres travelled
signal to_next              -- metres to the next checkpoint, or to the goal
signal time_left            -- seconds on the clock
signal checkpoints
signal offroad
signal state                -- 0 driving, 1 out of time, 2 goal
signal drift: int           -- how far the tail is out; signed, so a left slide
                            -- reads as a negative and not as 65512

local px: int = 0           -- road's lateral offset (car sits at screen centre)
local speed: int = 0        -- forward speed
local sub = 0               -- fractional world units not yet handed to `travel`
local travel = 0            -- world units travelled (scrolls the stripes)
local dsub = 0              -- fractional pixels not yet handed to the drift
local curve: int = 0        -- current curvature, in 1/8ths
local ctarget: int = 0      -- what it is easing towards
local cmag = 0              -- |curve|, and its sign — see the note in update()
local cneg = 0
local next_curve = 0        -- frames until a new target is picked
local lean: int = 0         -- how far the tail is out, -LEAN_MAX..LEAN_MAX
local pose = 0              -- which car sprite that works out as, 0..3
local offroad = 0
local shake = 0

local adv = 0               -- world units advanced this frame
local msub = 0              -- fractional metres not yet handed to `dist`
local dist = 0              -- metres travelled
local time_t: int = START_T -- frames left on the clock
local cps = 0               -- checkpoints passed
local state = 0             -- 0 driving, 1 out of time, 2 goal
local cp_flash = 0          -- frames left on the CHECKPOINT banner

function init()
  px = 0  speed = 0  sub = 0  travel = 0  dsub = 0
  curve = 0  ctarget = 0  cmag = 0  cneg = 0  next_curve = 0
  lean = 0  pose = 0  offroad = 0  shake = 0
  adv = 0  msub = 0  dist = 0  time_t = START_T  cps = 0
  state = 0  cp_flash = 0
  car_palette()
  scenery_palette()
end

-- How far there is left to drive before something happens. The goal once the
-- last checkpoint is behind you, the next checkpoint until then — one number,
-- because from the driver's seat they are the same question.
function target_m()
  if cps < CP_COUNT then return (cps + 1) * CP_EVERY end
  return GOAL
end

-- Scroll the world under the car. Split out because the run being over must not
-- freeze the picture: the car still rolls to a stop, and the road still moves
-- under it while it does.
function advance()
  sub = sub + speed
  adv = sub / SCROLL
  sub = sub % SCROLL
  travel = travel + adv
  if travel >= WRAP then travel = travel - WRAP end
end

function update()
  if state > 0 then
    if btnp(A) then init()  return end
    -- Over, but not stopped dead: drag the car down and keep the road rolling.
    speed = speed - 1
    if speed < 0 then speed = 0 end
    advance()
    return
  end

  -- throttle / brake, with drag when coasting
  if btn(A) then speed = speed + 2
  elseif btn(UP) then speed = speed + 1
  elseif btn(DOWN) then speed = speed - 3
  else speed = speed - 1 end

  -- Two wheels in the dirt: scrub hard down to what the grass allows. `speed`
  -- is `int` precisely so this and the drag above can undershoot and be caught
  -- here — as a `word` the test would be unsigned, 0-1 would read as 65535, and
  -- braking to a stop would pin the car at top speed instead.
  if offroad == 1 then
    if speed > OFF_TOP then speed = speed - 3 end
  end
  if speed < 0 then speed = 0 end
  if speed > SPEED_MAX then speed = SPEED_MAX end

  -- Scroll in fractions of a world unit: `sub` carries what a frame could not
  -- spend, so a slow crawl still creeps instead of standing still.
  advance()

  -- The odometer runs off the same advance, in metres, and does **not** wrap:
  -- `travel` folds at WRAP so the stripes and scenery stay in phase, which makes
  -- it useless as a distance. A stage measured in the scrolling counter would
  -- reset the race every 55296 world units.
  msub = msub + adv
  dist = dist + msub / MPU
  msub = msub % MPU

  -- The clock. A checkpoint is the only thing that adds to it, and the gap to
  -- the next one is constant while the bonus is fixed — so the margin a good
  -- driver banks shrinks by itself as the stage goes on.
  time_t = time_t - 1
  if cps < CP_COUNT then
    if dist >= (cps + 1) * CP_EVERY then
      cps = cps + 1
      time_t = time_t + CP_BONUS
      cp_flash = 90
    end
  end
  if cp_flash > 0 then cp_flash = cp_flash - 1 end
  if dist >= GOAL then
    state = 2
  elseif time_t <= 0 then
    time_t = 0
    state = 1
  end

  -- Steering: faster the faster you are going, but the floor is 2 px/frame and
  -- not 1. In the dirt `speed` is pinned at OFF_TOP, and a floor that fell below
  -- the drift would leave a curve holding the car off the road with the wheel
  -- turned. Full lock at full speed crosses the tarmac in about thirty frames.
  local st = speed / 24 + 2
  if btn(LEFT)  then px = px + st end
  if btn(RIGHT) then px = px - st end

  -- How far round the tail has come. Eased over ~24 frames rather than set from
  -- the button, because `pose` picks one of four drawn angles off it: snapping
  -- would flick through all four in three frames every time the player taps.
  local want: int = 0
  if btn(LEFT)  then want = 0 - LEAN_MAX end
  if btn(RIGHT) then want = LEAN_MAX end
  -- A parked car does not drift, so cap what the steering can ask for by speed.
  local cap: int = speed / 2
  if want > cap then want = cap end
  if want < 0 - cap then want = 0 - cap end
  if lean < want then lean = lean + 1
  elseif lean > want then lean = lean - 1 end

  local mag = lean
  if lean < 0 then mag = 0 - lean end
  pose = 0
  if mag >= 20 then pose = 3
  elseif mag >= 13 then pose = 2
  elseif mag >= 6 then pose = 1 end

  -- Ease the curvature towards its target rather than snapping: a jump of a
  -- whole unit shifts the far rows dozens of pixels in one frame.
  if next_curve == 0 then
    ctarget = 0 - 24 + rnd(49)   -- (0-24) is int, so the result is signed
    next_curve = 90 + rnd(150)
  end
  next_curve = next_curve - 1
  if curve < ctarget then curve = curve + 1
  elseif curve > ctarget then curve = curve - 1 end

  -- Split the curvature into magnitude and sign once a frame. Everything
  -- downstream divides it, and `/` is unsigned — dividing a negative curve
  -- directly bends the road the wrong way by about 8000 pixels.
  cneg = 0
  cmag = curve
  if curve < 0 then cneg = 1  cmag = 0 - curve end

  -- Centrifugal drift: a curve pushes the car to the outside, harder at speed.
  -- Accumulated in 1/DRIFT pixels for the same reason `travel` is: a whole pixel
  -- per frame is 60 px/s of sideways slide, and rounding it down to zero instead
  -- means gentle curves do not push at all.
  dsub = dsub + cmag * speed / 24
  local drift = dsub / DRIFT
  dsub = dsub % DRIFT
  if cneg == 1 then px = px - drift else px = px + drift end

  if px > PX_MAX then px = PX_MAX end
  if px < 0 - PX_MAX then px = 0 - PX_MAX end

  offroad = 0
  if px > OFF_LIMIT then offroad = 1 end
  if px < 0 - OFF_LIMIT then offroad = 1 end

  shake = 0
  if offroad == 1 then
    if speed > 6 then shake = frame_count() % 2 end
  end
end

-- Where the road's centre is at row depth `d`. The bend is a parabola in the
-- distance `k`, so far rows swing most; `k*k/512` is positive, so the sign is
-- put back by hand from `cneg`.
function road_cx(k)
  local bend = cmag * (k * k / 128) / 8
  if cneg == 1 then return 64 + px - bend end
  return 64 + px + bend
end

-- Draw a 16x16 sprite scaled by `sc`, standing with its base at (`cx`, `base`) —
-- bottom centre, since that is where an object meets the ground.
--
-- `sprn` knows a sprite's declared size but does not scale, and `spr_scaled`
-- scales but takes one tile, so the four tiles are walked by hand here. That is
-- the raw `id + row*w + col` contract, the same row-major order the compiler
-- slices a declaration into. Note that `spr_scaled(tree, …)` alone would compile
-- and quietly draw the top-left quarter: it is not size-aware the way `spr` and
-- `sprn` now are.
function spr2(id, cx: int, base, sc)
  local sz = 8 * sc / 256
  local x: int = cx - sz
  local y = base - sz - sz
  spr_scaled(id, x, y, sc, 0)
  spr_scaled(id + 1, x + sz, y, sc, 0)
  spr_scaled(id + 2, x, y + sz, sc, 0)
  spr_scaled(id + 3, x + sz, y + sz, sc, 0)
end

-- One roadside object at row `y`: 0 tree, 1 palm, 2 building, 3 billboard.
-- `side` is -1 for the left verge, +1 for the right.
--
-- Both the height and the distance out are tied to the row's road half-width,
-- which is already that row's perspective factor — so an object keeps its size
-- relative to the road as it arrives, and a building set back from the verge
-- stays set back instead of drifting onto the tarmac. `dist` is measured to the
-- object's own edge (`half + sz`) rather than by a fixed multiple of `half`,
-- which is what stops a big near tree from overhanging the road.
function scene_at(y, cx: int, half, side: int, kind)
  if kind == 4 then return end
  local id = tree
  local k = 26
  if kind == 1 then id = palm  k = 30
  elseif kind == 2 then id = bldg  k = 34
  elseif kind == 3 then id = sign  k = 20 end
  local sc = half * k
  -- An object taller than its own base row would put its top row at a negative
  -- y, which wraps round to the bottom of the screen rather than clipping. Cap
  -- the scale: it only bites on the tallest objects in the last few rows, where
  -- they are most of the way off the screen anyway.
  if sc > y * 16 then sc = y * 16 end
  local sz = 8 * sc / 256
  local dist = half + sz
  if kind == 2 then dist = dist + half / 3 end
  spr2(id, cx + side * dist, y, sc)
end

-- What stands in scenery slot `n`: 0 tree, 1 palm, 2 building, 3 billboard,
-- 4 nothing. `n + n/16` rather than `n` alone so the roadside repeats every 256
-- slots instead of every 16.
--
-- A quarter of the slots are empty on purpose. Both verges are filled from the
-- same slot list, so without gaps every slot puts an object on each side and the
-- roadside becomes two unbroken walls with no sky between them.
function slot_kind(n)
  local h = (n + n / 16) % 16
  if h == 3 then return 2 end
  if h == 11 then return 2 end
  if h == 7 then return 3 end
  if h % 4 == 1 then return 1 end
  if h % 3 == 0 then return 4 end
  return 0
end

-- One puff at a rear wheel. The four frames are separate sprites at contiguous
-- ids, so the frame is arithmetic rather than a branch.
function puff_at(cx: int, n)
  spr2(puff0 + n * 4, cx, BOTTOM, 352)
end

-- A backing plate for the banners. The sky is a sunset and there is a 32-pixel
-- sun sitting right where a centred message wants to be, so text drawn straight
-- onto it is legible everywhere except over the sun — and the banner is the one
-- thing on the screen that has to read every time.
function plate()
  local y = 50
  while y < 72 do
    hline(28, 99, y, 0)
    y = y + 1
  end
end

function draw()
  -- sunset sky, in bands
  local y = 0
  while y <= HORIZON do
    local c = SKY0
    if y >= 67 then c = SKY7
    elseif y >= 57 then c = SKY6
    elseif y >= 48 then c = SKY5
    elseif y >= 38 then c = SKY4
    elseif y >= 27 then c = SKY3
    elseif y >= 17 then c = SKY2
    elseif y >= 9 then c = SKY1 end
    hline(0, 127, y, c)
    y = y + 1
  end

  -- The sun sits on the horizon and bobs with sin(); the sign is handled by
  -- branching so the unsigned divide only ever sees a non-negative value.
  local a = frame_count() / 2
  local s: int = sin(a)
  local sun_y = 48
  if s > 0 then sun_y = 48 - s / 80 else sun_y = 48 + (0 - s) / 80 end
  spr_scaled(sun, 78, sun_y, 1024, 0)      -- 4x, drawn before the road

  -- Road, one scanline at a time from the near bottom row up to the horizon.
  y = BOTTOM
  while y > HORIZON do
    local d = y - HORIZON                   -- 1..DEPTH, larger nearer the camera
    local half = ROAD_HALF * d / DEPTH      -- widest at the bottom row
    local cx: int = road_cx(DEPTH - d)
    local w = ZSCALE / d + travel           -- world position of this row

    -- Alternate the grass and the rumble strip; leave the tarmac alone, since a
    -- two-tone tarmac shimmers for the same motion cue the rumble already gives.
    local grass = GRASS_A
    local road = ROAD_N
    local rumble = RUMB_A
    local dash = (w / STRIPE) % 2
    if dash == 1 then grass = GRASS_B  rumble = RUMB_B end
    -- ...and past STRIPE_D the pattern is finer than a row, so fade instead.
    if d < 6 then grass = GRASS_3  road = ROAD_3  rumble = RUMB_3
    elseif d < 11 then grass = GRASS_2  road = ROAD_2  rumble = RUMB_2
    elseif d < STRIPE_D then grass = GRASS_1  road = ROAD_1  rumble = RUMB_1 end

    hline(0, 127, y, grass)                 -- grass first
    local lx: int = cx - half
    local rx: int = cx + half
    hline(lx, rx, y, road)                  -- tarmac
    local edge = half / 8 + 1
    hline(lx, lx + edge, y, rumble)         -- rumble strips at both edges
    hline(rx - edge, rx, y, rumble)
    if d >= STRIPE_D then
      if dash == 0 then
        local hw = half / 20
        hline(cx - hw, cx + hw, y, LINE_C)  -- centre dashes
      end
    end

    y = y - 1
  end

  -- Scenery, horizon → bottom so a near object covers a far one. A slot lands
  -- on the row where `w / SEG` steps down; near the horizon several steps fall
  -- on one row, and only the nearest of them is drawn (the rest would be a
  -- pixel tall).
  sprbank(2)
  local sy = HORIZON + SCENE_D
  local prev = (ZSCALE / SCENE_D + travel) / SEG
  while sy <= BOTTOM do
    local d = sy - HORIZON
    local seg = (ZSCALE / d + travel) / SEG
    if seg < prev then
      local half = ROAD_HALF * d / DEPTH
      local cx: int = road_cx(DEPTH - d)
      scene_at(sy, cx, half, 1, slot_kind(prev))
      scene_at(sy, cx, half, 0 - 1, slot_kind(prev * 3 + 5))
      prev = seg
    end
    sy = sy + 1
  end
  sprbank(0)

  -- Tyre smoke under the rear wheels, before the car so the car sits on top of
  -- it: smoke when the tail is out at speed, dust when a wheel is in the dirt.
  local smoking = 0
  if pose >= 2 then
    if speed >= 20 then smoking = 1 end
  end
  if offroad == 1 then
    if speed >= 10 then smoking = 1 end
  end
  if smoking == 1 then
    local n = frame_count() / 3 % 4
    -- The two wheels run out of phase, or the pair pulses as one cloud.
    puff_at(64 - 17, n)
    puff_at(64 + 17, (n + 2) % 4)
  end

  -- The player's car, fixed near the bottom centre (the road moves under it).
  -- Only the right-hand poses are drawn; a left-hand drift is the same sprite
  -- flipped, which `sprn` mirrors as a block rather than tile by tile.
  --
  -- The pose is a computed id, so this is the raw six-argument `sprn`: the short
  -- form reads the size off a declared sprite *name*, and there is no name here.
  -- CAR_W/CAR_H are the one place that size is written down.
  local id = car
  if pose == 1 then id = car1
  elseif pose == 2 then id = car2
  elseif pose == 3 then id = car3 end
  local flip = 0
  if lean < 0 then flip = 1 end
  sprbank(1)
  sprn(id, 64 - CAR_W / 2, BOTTOM + 1 - CAR_H + shake, CAR_W / 8, CAR_H / 8, flip)
  sprbank(0)

  -- The HUD. Glyphs advance 4 px, so all three readouts fit on one row of a
  -- 128-wide screen. `speed * 4` is a speedometer's worth of numbers rather than
  -- the engine's 0..48 — the *signal* below reports the raw value, because that
  -- is the one a person tuning the handling is looking at.
  text("SPD", 2, 2, 7)
  number(speed * 4, 18, 2, 7)
  text("TIME", 40, 2, 10)
  number((time_t + 59) / 60, 60, 2, 10)
  text("NEXT", 84, 2, 7)
  number(target_m() - dist, 104, 2, 7)

  if offroad == 1 then
    if frame_count() % 20 < 12 then text("OFF ROAD", 48, 88, 8) end
  end
  if cp_flash > 0 then
    if cp_flash % 20 < 12 then
      plate()
      text("CHECKPOINT", 44, 54, 10)
      text("+15 SEC", 50, 63, 7)
    end
  end
  if state == 1 then
    plate()
    text("TIME UP", 50, 54, 8)
    text("PRESS A", 50, 63, 7)
  end
  if state == 2 then
    plate()
    text("GOAL", 56, 54, 10)
    text("PRESS A", 50, 63, 7)
  end

  -- Reported for observation: the car's place, then the numbers. These used to
  -- be one call, `entity(px, speed, 1)`, which put a speed in a y coordinate and
  -- left every reader parsing a position that was not one.
  entity(px, speed, 1)
  signal(speed, speed)
  signal(dist, dist)
  signal(to_next, target_m() - dist)
  signal(time_left, (time_t + 59) / 60)
  signal(checkpoints, cps)
  signal(offroad, offroad)
  signal(state, state)
  signal(drift, lean)
end
