-- swarm.lua — dodge the sparks. Survive; they get faster. A is a new run.
--
--   kessel run games/swarm.lua
--
-- This is the corpus's `#include` example: everything about *moving* lives in
-- `lib/motion.lua`, and this file is the game. The rules are in that file's
-- header — read it before writing your own library.

#include "lib/motion.lua"

controls {
  dpad  = true      -- move
  a     = "restart"
  pause = START
}

local DIM = 128
local SIZE = 4            -- everything on screen is a 4x4 block
local EDGE = 2            -- how close to the wall a spark may get
local SPEED = 2           -- player pixels per frame
local SPARKS = 7
local RAMP = 240          -- frames between speed-ups

local player: Body
local sparks: array(7, Body)
local score = 0
local best = 0
local dead = 0
local ramp = 0

function init()
  best = 0
  restart()
end

-- Start a run: player in the middle, sparks spread around the four walls at one
-- pixel a frame.
--
-- On a wall rather than anywhere, because a random position in the arena can
-- land on the player, and a run that is over before the first frame is drawn
-- reads as a broken game rather than a hard one.
function restart()
  player.x = 62  player.y = 62
  player.vx = 0  player.vy = 0
  score = 0
  ramp = 0
  dead = 0

  for i = 0, SPARKS - 1 do
    local along = 8 + rnd(DIM - 24)
    local wall = i % 4
    if wall == 0 then
      sparks[i].x = along  sparks[i].y = EDGE
    elseif wall == 1 then
      sparks[i].x = along  sparks[i].y = DIM - SIZE - EDGE
    elseif wall == 2 then
      sparks[i].x = EDGE  sparks[i].y = along
    else
      sparks[i].x = DIM - SIZE - EDGE  sparks[i].y = along
    end
    -- rnd(2) is 0 or 1, so this is "left or right", never "stopped" — a spark
    -- that never moves is one the player can ignore.
    sparks[i].vx = 1
    sparks[i].vy = 1
    if rnd(2) == 0 then sparks[i].vx = 0 - 1 end
    if rnd(2) == 0 then sparks[i].vy = 0 - 1 end
  end
end

-- Push every spark one step further from a standstill, up to four pixels a
-- frame. Called on the ramp, so a long run gets harder rather than longer.
function speed_up()
  for i = 0, SPARKS - 1 do
    if sparks[i].vx > 0 and sparks[i].vx < 4 then sparks[i].vx = sparks[i].vx + 1 end
    if sparks[i].vx < 0 and sparks[i].vx > 0 - 4 then sparks[i].vx = sparks[i].vx - 1 end
    if sparks[i].vy > 0 and sparks[i].vy < 4 then sparks[i].vy = sparks[i].vy + 1 end
    if sparks[i].vy < 0 and sparks[i].vy > 0 - 4 then sparks[i].vy = sparks[i].vy - 1 end
  end
end

function update()
  if dead == 1 then
    if btnp(A) then restart() end
    return
  end

  -- `nudge` clamps, so holding a direction at the wall stays put instead of
  -- wrapping to the far side.
  if btn(LEFT) then player.x = nudge(player.x, 0 - SPEED, 0, DIM - SIZE) end
  if btn(RIGHT) then player.x = nudge(player.x, SPEED, 0, DIM - SIZE) end
  if btn(UP) then player.y = nudge(player.y, 0 - SPEED, 0, DIM - SIZE) end
  if btn(DOWN) then player.y = nudge(player.y, SPEED, 0, DIM - SIZE) end

  for i = 0, SPARKS - 1 do
    move(sparks[i])
    bounce_x(sparks[i], EDGE, DIM - SIZE - EDGE)
    bounce_y(sparks[i], EDGE, DIM - SIZE - EDGE)
    if hits(player.x, player.y, sparks[i].x, sparks[i].y, SIZE) then
      dead = 1
      if score > best then best = score end
    end
  end

  if dead == 0 then
    score = score + 1
    ramp = ramp + 1
    if ramp >= RAMP then
      ramp = 0
      speed_up()
    end
  end
end

function draw()
  cls(1)

  -- The arena, so the walls the sparks bounce off are visible.
  for x = 0, DIM - 1 do
    pset(x, 0, 13)
    pset(x, DIM - 1, 13)
  end
  for y = 0, DIM - 1 do
    pset(0, y, 13)
    pset(DIM - 1, y, 13)
  end

  for i = 0, SPARKS - 1 do
    block(sparks[i].x, sparks[i].y, SIZE, 8)
    entity(sparks[i].x, sparks[i].y, 2)
  end

  local color = 11
  if dead == 1 then color = 7 end
  block(player.x, player.y, SIZE, color)
  entity(player.x, player.y, 1)

  -- The score bar: one pixel per eight frames survived, capped at the width.
  local bar = min(score / 8, DIM - 8)
  for x = 0, bar do
    pset(4 + x, 4, 10)
  end

  -- 4 px per glyph, so these are centred by hand on a 128-wide screen.
  if dead == 1 then
    text("HIT", 58, 44, 8)
    text("PRESS A", 50, 54, 7)
  end
end
