-- 2048.lua — slide matching numbered tiles together to reach 2048. Arrow keys
-- move every tile once per press; A (Z key) starts a new game.
--
--   kessel --play games/2048.lua

controls {
  dpad = true
  a = "new game"
  pause = START
}

-- Four contiguous sprites form a reusable 16x16 tile frame via sprn(...,2,2).
sprite panel_tl {
  66666666
  65555555
  65555555
  65555555
  65555555
  65555555
  65555555
  65555555
}
sprite panel_tr {
  66666666
  55555556
  55555556
  55555556
  55555556
  55555556
  55555556
  55555556
}
sprite panel_bl {
  65555555
  65555555
  65555555
  65555555
  65555555
  65555555
  65555555
  66666666
}
sprite panel_br {
  55555556
  55555556
  55555556
  55555556
  55555556
  55555556
  55555556
  66666666
}

sprite fill_2 {
  66666666
  66666666
  66666666
  66666666
  66666666
  66666666
  66666666
  66666666
}
sprite fill_4 {
  ffffffff
  ffffffff
  ffffffff
  ffffffff
  ffffffff
  ffffffff
  ffffffff
  ffffffff
}
sprite fill_8 {
  99999999
  99999999
  99999999
  99999999
  99999999
  99999999
  99999999
  99999999
}
sprite fill_16 {
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
  aaaaaaaa
}
sprite fill_32 {
  88888888
  88888888
  88888888
  88888888
  88888888
  88888888
  88888888
  88888888
}
sprite fill_64 {
  eeeeeeee
  eeeeeeee
  eeeeeeee
  eeeeeeee
  eeeeeeee
  eeeeeeee
  eeeeeeee
  eeeeeeee
}
sprite fill_128 {
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
  bbbbbbbb
}
sprite fill_256 {
  33333333
  33333333
  33333333
  33333333
  33333333
  33333333
  33333333
  33333333
}
sprite fill_512 {
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
}
sprite fill_1024 {
  dddddddd
  dddddddd
  dddddddd
  dddddddd
  dddddddd
  dddddddd
  dddddddd
  dddddddd
}
sprite fill_2048 {
  77777777
  77777777
  77777777
  77777777
  77777777
  77777777
  77777777
  77777777
}

local cells: array(16, word)
local line: array(4, word)
local output: array(4, word)
local score = 0
local state = 0       -- 0 playing, 1 reached 2048, 2 no moves left
local changed = 0
local anim_timer = 0
local anim_dir = 0    -- 1 left, 2 right, 3 up, 4 down

local OX = 32
local OY = 29
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

function update()
  if anim_timer > 0 then anim_timer = anim_timer - 1 end
  if btnp(A) then init()  return end
  if state ~= 0 then return end

  local acted = 0
  if btnp(LEFT) then move_left()  anim_dir = 1  acted = 1
  elseif btnp(RIGHT) then move_right()  anim_dir = 2  acted = 1
  elseif btnp(UP) then move_up()  anim_dir = 3  acted = 1
  elseif btnp(DOWN) then move_down()  anim_dir = 4  acted = 1 end

  if acted == 1 then
    anim_timer = 4
    if changed == 1 and state == 0 then spawn_tile() end
    if state == 0 and can_move() == 0 then state = 2 end
  end
end

function fill_sprite(value)
  if value == 2 then return fill_2 end
  if value == 4 then return fill_4 end
  if value == 8 then return fill_8 end
  if value == 16 then return fill_16 end
  if value == 32 then return fill_32 end
  if value == 64 then return fill_64 end
  if value == 128 then return fill_128 end
  if value == 256 then return fill_256 end
  if value == 512 then return fill_512 end
  if value == 1024 then return fill_1024 end
  return fill_2048
end

function number_color(value)
  if value <= 16 or value == 2048 then return 0 end
  return 7
end

function draw_tile(index)
  local x = draw_ox + (index % 4) * 16
  local y = draw_oy + (index / 4) * 16
  sprn(panel_tl, x, y, 2, 2, 0)

  local value = cells[index]
  if value ~= 0 then
    spr(fill_sprite(value), x + 4, y + 4, 0)
    local nx = x
    if value < 10 then nx = x + 6
    elseif value < 100 then nx = x + 4
    elseif value < 1000 then nx = x + 2 end
    number(value, nx, y + 5, number_color(value))
    entity(OX + (index % 4) * 16, OY + (index / 4) * 16, value)
  end
end

function draw()
  cls(1)
  text("2048", 56, 5, 7)
  text("SCORE", 34, 17, 6)
  number(score, 76, 17, 10)

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

  if state == 1 then
    text("YOU WIN", 50, 103, 11)
    text("PRESS A", 50, 112, 7)
  elseif state == 2 then
    text("GAME OVER", 46, 103, 8)
    text("PRESS A", 50, 112, 7)
  end
  entity(score, state, 30)
  entity(anim_dir, anim_timer, 31)
end
