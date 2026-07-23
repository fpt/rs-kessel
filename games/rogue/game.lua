-- rogue — a tiny top-down action game. Arrows move the hero tile-by-tile
-- around a walled dungeon; A swings a sword in the facing direction. Orcs
-- shuffle toward you and damage one of five hearts on contact. At 0 HP press A
-- to restart.
--
--   kessel --play games/rogue/game.lua
--
-- Sprite order sets the tile ids the tilemap draws: floor=0, wall=1 (hero=2 and
-- orc=3 are drawn as free sprites, not tiles).

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move and set sword direction
  a = "sword / restart"
  pause = START
}

sprite floor {
  55555555
  55555555
  55555565
  55555555
  55655555
  55555555
  55555555
  56555555
}

sprite wall {
  66666666
  64444446
  64444446
  66666666
  44466644
  44466644
  66666666
  64444446
}

sprite hero {
  ..7777..
  .777777.
  77c77c77
  77777777
  77777777
  .777777.
  ..7..7..
  .7....7.
}

sprite orc {
  ..3333..
  .333333.
  33833833
  33333333
  33333333
  .333333.
  ..3..3..
  .3....3.
}

sprite sword_h {
  ........
  ........
  ........
  6677777a
  6677777a
  ........
  ........
  ........
}

sprite sword_v {
  ...66...
  ...66...
  ...77...
  ...77...
  ...77...
  ...77...
  ...77...
  ...aa...
}

sprite heart {
  .88.88..
  8888888.
  8888888.
  .88888..
  ..888...
  ...8....
  ........
  ........
}

sprite heart_empty {
  .66.66..
  6..6..6.
  6.....6.
  .6...6..
  ..6.6...
  ...6....
  ........
  ........
}

sprite chest {
  .aaaaaa.
  a999999a
  a9aaaa9a
  aaaaaaaa
  a944449a
  a94aa49a
  a944449a
  .aaaaaa.
}

sprite chest_open {
  aa....aa
  a9aaaa9a
  .aaaaaa.
  a999999a
  a944449a
  a94aa49a
  a944449a
  .aaaaaa.
}

sprite stairs {
  11111166
  11116666
  11666666
  66666666
  65555555
  65555555
  65555555
  66666666
}

tilemap dungeon(16, 16)

record Mob { x, y, alive }

local enemies: array(5, Mob)
local hx = 2        -- hero tile coords
local hy = 2
local hp = 5
local mcd = 0       -- hero move cooldown (tiles, not per-frame)
local etick = 0
local facing = 0    -- 0 right, 1 left, 2 up, 3 down
local attack_timer = 0
local invuln = 0
local stage = 1
local loot = 0
local chest_x = 7
local chest_y = 2
local chest_opened = 0
local stair_x = 13
local stair_y = 13

function build_level()
  fset(wall, SOLID, 1)
  local y = 0
  while y < 16 do
    local x = 0
    while x < 16 do
      if x == 0 or y == 0 or x == 15 or y == 15 then
        mset(x, y, wall)
      else
        mset(x, y, floor)
      end
      x = x + 1
    end
    y = y + 1
  end
  -- a few interior walls
  mset(4, 4, wall)   mset(5, 4, wall)   mset(6, 4, wall)
  mset(10, 8, wall)  mset(10, 9, wall)  mset(10, 10, wall)
  mset(4, 11, wall)  mset(5, 11, wall)
end

function load_stage(n)
  build_level()
  stage = n
  mcd = 0  etick = 0  facing = 0  attack_timer = 0  invuln = 0
  chest_opened = 0
  local i = 0
  while i < 5 do enemies[i].alive = 0  i = i + 1 end

  local layout = (n - 1) % 4
  if layout == 0 then
    hx = 2  hy = 2  chest_x = 7  chest_y = 2  stair_x = 13  stair_y = 13
    enemies[0].x = 12  enemies[0].y = 3   enemies[0].alive = 1
    enemies[1].x = 12  enemies[1].y = 12  enemies[1].alive = 1
    enemies[2].x = 3   enemies[2].y = 13  enemies[2].alive = 1
  elseif layout == 1 then
    hx = 13  hy = 2  chest_x = 8  chest_y = 6  stair_x = 2  stair_y = 13
    enemies[0].x = 3   enemies[0].y = 3   enemies[0].alive = 1
    enemies[1].x = 11  enemies[1].y = 6   enemies[1].alive = 1
    enemies[2].x = 6   enemies[2].y = 12  enemies[2].alive = 1
  elseif layout == 2 then
    hx = 2  hy = 13  chest_x = 7  chest_y = 8  stair_x = 13  stair_y = 2
    enemies[0].x = 12  enemies[0].y = 12  enemies[0].alive = 1
    enemies[1].x = 9   enemies[1].y = 3   enemies[1].alive = 1
    enemies[2].x = 3   enemies[2].y = 7   enemies[2].alive = 1
  else
    hx = 13  hy = 13  chest_x = 5  chest_y = 6  stair_x = 2  stair_y = 2
    enemies[0].x = 11  enemies[0].y = 11  enemies[0].alive = 1
    enemies[1].x = 8   enemies[1].y = 5   enemies[1].alive = 1
    enemies[2].x = 4   enemies[2].y = 12  enemies[2].alive = 1
  end
end

function init()
  hp = 5
  loot = 0
  load_stage(1)
end

-- Move the hero to (nx,ny) unless a wall or enemy occupies that tile.
function try_move(nx, ny)
  if solid(nx * 8, ny * 8) == 1 then return end
  local i = 0
  while i < 5 do
    if enemies[i].alive == 1 and enemies[i].x == nx and enemies[i].y == ny then
      return
    end
    i = i + 1
  end
  hx = nx  hy = ny

  if chest_opened == 0 and hx == chest_x and hy == chest_y then
    chest_opened = 1
    loot = loot + 1
    hp = min(hp + 1, 5)     -- heal one, capped at full (hp is non-negative)
  end
  if hx == stair_x and hy == stair_y then load_stage(stage + 1) end
end

function swing_sword()
  local tx = hx
  local ty = hy
  if facing == 0 then tx = tx + 1 end
  if facing == 1 then tx = tx - 1 end
  if facing == 2 then ty = ty - 1 end
  if facing == 3 then ty = ty + 1 end

  local i = 0
  while i < 5 do
    if enemies[i].alive == 1 and enemies[i].x == tx and enemies[i].y == ty then
      enemies[i].alive = 0
    end
    i = i + 1
  end
  attack_timer = 8
end

-- step orc `i` one tile toward the hero; on contact it damages the hero
function move_enemy(i)
  local ex = enemies[i].x
  local ey = enemies[i].y
  local nx = ex
  local ny = ey
  if hx < ex then nx = ex - 1 end
  if hx > ex then nx = ex + 1 end
  if hx == ex then
    if hy < ey then ny = ey - 1 end
    if hy > ey then ny = ey + 1 end
  end
  if nx == hx and ny == hy then
    if invuln == 0 and hp > 0 then
      hp = hp - 1
      invuln = 45
    end
    return
  end
  if solid(nx * 8, ny * 8) == 1 then return end
  enemies[i].x = nx  enemies[i].y = ny
end

function update()
  if hp == 0 then
    if btnp(A) then init() end
    return
  end

  if invuln > 0 then invuln = invuln - 1 end
  if attack_timer > 0 then attack_timer = attack_timer - 1 end
  if btnp(A) and attack_timer == 0 then swing_sword() end

  if mcd > 0 then mcd = mcd - 1 end
  if mcd == 0 and attack_timer == 0 then
    local moved = 0
    if btn(LEFT)  then facing = 1  try_move(hx - 1, hy)  moved = 1 end
    if btn(RIGHT) then facing = 0  try_move(hx + 1, hy)  moved = 1 end
    if btn(UP)    then facing = 2  try_move(hx, hy - 1)  moved = 1 end
    if btn(DOWN)  then facing = 3  try_move(hx, hy + 1)  moved = 1 end
    if moved == 1 then mcd = 8 end
  end

  etick = etick + 1
  if etick % 20 == 0 then
    local i = 0
    while i < 5 do
      if enemies[i].alive == 1 then move_enemy(i) end
      i = i + 1
    end
  end
end

function draw()
  cls(0)
  map(0, 0, 0, 0, 16, 16)
  if chest_opened == 0 then
    spr(chest, chest_x * 8, chest_y * 8, 0)
    entity(chest_x * 8, chest_y * 8, 20)
  else
    spr(chest_open, chest_x * 8, chest_y * 8, 0)
    entity(chest_x * 8, chest_y * 8, 22)
  end
  spr(stairs, stair_x * 8, stair_y * 8, 0)
  entity(stair_x * 8, stair_y * 8, 21)
  local i = 0
  while i < 5 do
    if enemies[i].alive == 1 then
      spr(orc, enemies[i].x * 8, enemies[i].y * 8, 0)
      entity(enemies[i].x * 8, enemies[i].y * 8, 10)
    end
    i = i + 1
  end

  if hp > 0 and (invuln == 0 or invuln % 6 < 3) then spr(hero, hx * 8, hy * 8, 0) end
  if hp > 0 and attack_timer > 0 then
    if facing == 0 then spr(sword_h, hx * 8 + 8, hy * 8, 0) end
    if facing == 1 then spr(sword_h, hx * 8 - 8, hy * 8, 1) end
    if facing == 2 then spr(sword_v, hx * 8, hy * 8 - 8, 2) end
    if facing == 3 then spr(sword_v, hx * 8, hy * 8 + 8, 0) end
  end

  i = 0
  while i < 5 do
    if i < hp then spr(heart, 2 + i * 9, 2, 0) else spr(heart_empty, 2 + i * 9, 2, 0) end
    i = i + 1
  end
  text("STAGE", 74, 2, 7)
  number(stage, 116, 2, 10)
  text("LOOT", 82, 10, 7)
  number(loot, 116, 10, 10)
  if hp == 0 then
    text("GAME OVER", 46, 54, 8)
    text("PRESS A", 50, 64, 7)
  end
  entity(hx * 8, hy * 8, hp)
  entity(stage, loot, 30)
end
