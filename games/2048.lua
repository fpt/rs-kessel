-- 2048.lua — slide matching numbered tiles together to reach 2048. Arrow keys
-- move every tile once per press, or **swipe** the board; A (Z key) starts a
-- new game.
--
--   kessel run games/2048.lua        (drag with the mouse to swipe)
--
-- The swipe reference. `swipe(slot)` reports a `LEFT`/`RIGHT`/`UP`/`DOWN` bit on
-- the one frame a finger passes the distance threshold — the same constants
-- `btnp` takes, so the two input paths below collapse into one `direction()`.
--
-- Note the `touch` declaration: the ports work regardless, but a host only
-- routes screen touches to a game that asks for them, so a swipe game that
-- forgets this line works on a keyboard and does nothing on a phone.

controls {
  dpad = true
  a = "new game"
  touch = "swipe to slide"
  pause = START
}


local cells: array(16, word)
local line: array(4, word)
local output: array(4, word)
local score = 0
local state = 0       -- 0 playing, 1 reached 2048, 2 no moves left
local changed = 0
local anim_timer = 0
local anim_dir = 0    -- 1 left, 2 right, 3 up, 4 down

local TILE = 48       -- board pitch: 4 tiles fill 192 of the 240-px screen
local OX = 24         -- (240 - 4*TILE) / 2
local OY = 32         -- under the title and score
local draw_ox: int = OX
local draw_oy: int = OY

function spawn_tile()
  local empty = 0
  local i = 0
  while i < 16 do
    if cells[i] == 0 then empty = empty + 1 end
    i = i + 1
  end
  if empty == 0 then return end

  local target = rnd(empty)
  i = 0
  while i < 16 do
    if cells[i] == 0 then
      if target == 0 then
        if rnd(10) == 0 then cells[i] = 4 else cells[i] = 2 end
        return
      end
      target = target - 1
    end
    i = i + 1
  end
end

-- Compact one directional line and merge each destination at most once.
function process_line()
  clear(output)
  local write = 0
  local last_merged = 0
  local i = 0
  while i < 4 do
    local value = line[i]
    if value ~= 0 then
      if write > 0 and output[write - 1] == value and last_merged ~= write then
        output[write - 1] = value * 2
        score = score + output[write - 1]
        last_merged = write
        if output[write - 1] == 2048 then state = 1 end
      else
        output[write] = value
        write = write + 1
      end
    end
    i = i + 1
  end
end

function put_cell(index, value)
  if cells[index] ~= value then
    cells[index] = value
    changed = 1
  end
end

function move_left()
  changed = 0
  for row = 0, 3 do
    for col = 0, 3 do line[col] = cells[row * 4 + col] end
    process_line()
    for col = 0, 3 do put_cell(row * 4 + col, output[col]) end
  end
end

function move_right()
  changed = 0
  for row = 0, 3 do
    for col = 0, 3 do line[col] = cells[row * 4 + (3 - col)] end
    process_line()
    for col = 0, 3 do put_cell(row * 4 + (3 - col), output[col]) end
  end
end

function move_up()
  changed = 0
  for col = 0, 3 do
    for row = 0, 3 do line[row] = cells[row * 4 + col] end
    process_line()
    for row = 0, 3 do put_cell(row * 4 + col, output[row]) end
  end
end

function move_down()
  changed = 0
  for col = 0, 3 do
    for row = 0, 3 do line[row] = cells[(3 - row) * 4 + col] end
    process_line()
    for row = 0, 3 do put_cell((3 - row) * 4 + col, output[row]) end
  end
end

function can_move()
  local i = 0
  while i < 16 do
    if cells[i] == 0 then return 1 end
    i = i + 1
  end
  for row = 0, 3 do
    for col = 0, 3 do
      local index = row * 4 + col
      if col < 3 and cells[index] == cells[index + 1] then return 1 end
      if row < 3 and cells[index] == cells[index + 4] then return 1 end
    end
  end
  return 0
end

function init()
  clear(cells)
  score = 0
  state = 0
  changed = 0
  anim_timer = 0
  anim_dir = 0
  spawn_tile()
  spawn_tile()
end

-- This frame's move, from either input, as a direction bit — or 0.
--
-- One function because a swipe and a `btnp` are the same event here: both are
-- edges that fire once, and both report the same four constants. A board that
-- tracked them separately would need two "did I already move this frame" flags.
--
-- Only slot 0 is read. A second finger on a puzzle board is a stray thumb, not
-- a second move.
function direction()
  local s = swipe(0)
  if s ~= 0 then return s end
  if btnp(LEFT) then return LEFT end
  if btnp(RIGHT) then return RIGHT end
  if btnp(UP) then return UP end
  if btnp(DOWN) then return DOWN end
  return 0
end

function update()
  if anim_timer > 0 then anim_timer = anim_timer - 1 end
  if btnp(A) then init()  return end
  if state ~= 0 then return end

  local dir = direction()
  local acted = 0
  if dir == LEFT then move_left()  anim_dir = 1  acted = 1
  elseif dir == RIGHT then move_right()  anim_dir = 2  acted = 1
  elseif dir == UP then move_up()  anim_dir = 3  acted = 1
  elseif dir == DOWN then move_down()  anim_dir = 4  acted = 1 end

  if acted == 1 then
    anim_timer = 4
    if changed == 1 and state == 0 then spawn_tile() end
    if state == 0 and can_move() == 0 then state = 2 end
  end
end

-- The tile's fill colour. This was eleven solid-colour 8x8 sprites; at a
-- 48-px tile they would each need redrawing, and a `rect` says the same thing
-- at any size.
function fill_color(value)
  if value == 2 then return 6 end
  if value == 4 then return 15 end
  if value == 8 then return 9 end
  if value == 16 then return 10 end
  if value == 32 then return 8 end
  if value == 64 then return 14 end
  if value == 128 then return 11 end
  if value == 256 then return 3 end
  if value == 512 then return 12 end
  if value == 1024 then return 13 end
  return 7
end

function number_color(value)
  if value <= 16 or value == 2048 then return 0 end
  return 7
end

function draw_tile(index)
  local x = draw_ox + (index % 4) * TILE
  local y = draw_oy + (index / 4) * TILE
  rect(x, y, TILE, TILE, 6)                       -- the frame
  rect(x + 1, y + 1, TILE - 2, TILE - 2, 5)       -- the well

  local value = cells[index]
  if value ~= 0 then
    rect(x + 8, y + 8, TILE - 16, TILE - 16, fill_color(value))
    -- 4 px a glyph, so this centres the number over its tile.
    local nx = x + 16
    if value < 10 then nx = x + 22
    elseif value < 100 then nx = x + 20
    elseif value < 1000 then nx = x + 18 end
    number(value, nx, y + 21, number_color(value))
    entity(OX + (index % 4) * TILE, OY + (index / 4) * TILE, value)
  end
end

function draw()
  cls(1)
  text("2048", 112, 6, 7)
  text("SCORE", 80, 18, 6)
  number(score, 124, 18, 10)

  -- Soft four-frame nudge: move toward the swipe, then ease back to rest.
  local amount: int = 0
  if anim_timer == 4 then amount = 2 end
  if anim_timer == 3 then amount = 1 end
  draw_ox = OX
  draw_oy = OY
  if anim_dir == 1 then draw_ox = OX - amount end
  if anim_dir == 2 then draw_ox = OX + amount end
  if anim_dir == 3 then draw_oy = OY - amount end
  if anim_dir == 4 then draw_oy = OY + amount end
  for i = 0, 15 do draw_tile(i) end

  -- The board fills the screen now, so the end-of-run message sits *over* it
  -- on its own backing rather than in a margin that no longer exists.
  if state == 1 then
    rect(48, 104, 144, 32, 0)
    text("YOU WIN", 106, 112, 11)
    text("PRESS A", 106, 124, 7)
  elseif state == 2 then
    rect(48, 104, 144, 32, 0)
    text("GAME OVER", 102, 112, 8)
    text("PRESS A", 106, 124, 7)
  end
  entity(score, state, 30)
  entity(anim_dir, anim_timer, 31)
end
