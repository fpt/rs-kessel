-- rogue.lua — a tiny top-down roguelike. Arrows move the hero tile-by-tile
-- around a walled dungeon; walk into an orc to cut it down. Orcs shuffle toward
-- you and chip your HP on contact. At 0 HP press A to restart.
--
--   kessel --play games/rogue.lua
--
-- Sprite order sets the tile ids the tilemap draws: floor=0, wall=1 (hero=2 and
-- orc=3 are drawn as free sprites, not tiles).

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move / bump an orc to attack
  a = "restart"
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

tilemap dungeon(16, 16)

record Mob { x, y, alive }

local enemies: array(5, Mob)
local hx = 2        -- hero tile coords
local hy = 2
local hp = 5
local mcd = 0       -- hero move cooldown (tiles, not per-frame)
local etick = 0

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

function init()
  build_level()
  hx = 2  hy = 2  hp = 5
  mcd = 0  etick = 0
  enemies[0].x = 12  enemies[0].y = 3   enemies[0].alive = 1
  enemies[1].x = 12  enemies[1].y = 12  enemies[1].alive = 1
  enemies[2].x = 3   enemies[2].y = 13  enemies[2].alive = 1
  enemies[3].alive = 0
  enemies[4].alive = 0
end

-- move the hero to (nx,ny) unless blocked; walking into an orc kills it instead
function try_move(nx, ny)
  if solid(nx * 8, ny * 8) == 1 then return end
  local i = 0
  while i < 5 do
    if enemies[i].alive == 1 and enemies[i].x == nx and enemies[i].y == ny then
      enemies[i].alive = 0
      return
    end
    i = i + 1
  end
  hx = nx  hy = ny
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
    if hp > 0 then hp = hp - 1 end
    return
  end
  if solid(nx * 8, ny * 8) == 1 then return end
  enemies[i].x = nx  enemies[i].y = ny
end

function update()
  if hp == 0 then
    if btn(A) then init() end
    return
  end

  if mcd > 0 then mcd = mcd - 1 end
  if mcd == 0 then
    local moved = 0
    if btn(LEFT)  then try_move(hx - 1, hy)  moved = 1 end
    if btn(RIGHT) then try_move(hx + 1, hy)  moved = 1 end
    if btn(UP)    then try_move(hx, hy - 1)  moved = 1 end
    if btn(DOWN)  then try_move(hx, hy + 1)  moved = 1 end
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
  local i = 0
  while i < 5 do
    if enemies[i].alive == 1 then spr(orc, enemies[i].x * 8, enemies[i].y * 8, 0) end
    i = i + 1
  end
  spr(hero, hx * 8, hy * 8, 0)
  entity(hx * 8, hy * 8, 1)
end
