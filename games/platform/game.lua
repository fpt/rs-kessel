-- platform — a scrolling tile platformer with coins, patrolling enemies,
-- stomp attacks, knockback, wall-jumps, and smooth movement.
-- Arrows move, A (Z key) jumps or wall-jumps.
--
--   kessel --play games/platform/game.lua

-- Host-UI control metadata (ignored by the VM; see docs/VM.md).
controls {
  dpad = true       -- arrows move
  a = "jump / wall-jump"
  pause = START
}

-- Tile id 0 fills untouched tilemap cells, so it must be background rather than
-- the player sprite.
sprite sky {
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
  cccccccc
}

sprite hero {
  ..7777..
  .777777.
  77777777
  77.77.77
  77777777
  .777777.
  ..7777..
  .77..77.
}

sprite block {
  55555555
  54444445
  54444445
  54444445
  54444445
  54444445
  54444445
  55555555
}

sprite coin {
  ..aaaa..
  .a9999a.
  a99aa99a
  a9aaaa9a
  a9aaaa9a
  a99aa99a
  .a9999a.
  ..aaaa..
}

sprite enemy {
  ..8888..
  .888888.
  88288288
  88888888
  .888888.
  ..8..8..
  .88..88.
  ........
}

tilemap level(32, 16)

local WORLD_TILES = 32
local CAMERA_MAX = 128

-- y4 and vy use quarter-pixel units for a smoother, less abrupt jump arc.
record Player { x, y, y4: int, vy: int, grounded }
record Enemy { x: int, y, dir: int, alive }
record Coin { x, y, taken }
local p: Player
local enemies: array(4, Enemy)
local coins: array(8, Coin)
local cam_x: int = 0
local coins_collected = 0
local invuln = 0
local knock_timer = 0
local knock_dir: int = 0
local wall_jump_timer = 0
local wall_jump_dir: int = 0
local enemy_tick = 0

function init()
  fset(block, SOLID, 1)          -- the block tile is solid

  -- A two-screen stage with a floor, boundary walls, and rising platforms.
  local i = 0
  while i < WORLD_TILES do mset(i, 14, block)  i = i + 1 end
  for y = 0, 13 do mset(0, y, block)  mset(WORLD_TILES - 1, y, block) end
  mset(4, 11, block)  mset(5, 11, block)  mset(6, 11, block)
  mset(10, 9, block)  mset(11, 9, block)
  mset(16, 11, block)  mset(17, 11, block)  mset(18, 11, block)
  mset(22, 8, block)   mset(23, 8, block)   mset(24, 8, block)
  mset(28, 10, block)  mset(29, 10, block)
  -- Pillars above platforms provide wall-jump routes while leaving the floor open.
  for y = 8, 10 do
    mset(6, y, block)  mset(18, y, block)
  end
  for y = 5, 7 do
    mset(24, y, block)
  end

  p.x = 16  p.y = 96  p.y4 = 96 * 4  p.vy = 0  p.grounded = 0
  cam_x = 0  coins_collected = 0  invuln = 0  knock_timer = 0  knock_dir = 0
  wall_jump_timer = 0  wall_jump_dir = 0  enemy_tick = 0

  enemies[0].x = 64   enemies[0].y = 104  enemies[0].dir = 1      enemies[0].alive = 1
  enemies[1].x = 128  enemies[1].y = 104  enemies[1].dir = 0 - 1  enemies[1].alive = 1
  enemies[2].x = 208  enemies[2].y = 104  enemies[2].dir = 1      enemies[2].alive = 1
  enemies[3].x = 136  enemies[3].y = 80   enemies[3].dir = 1      enemies[3].alive = 1

  coins[0].x = 24   coins[0].y = 104
  coins[1].x = 40   coins[1].y = 80
  coins[2].x = 88   coins[2].y = 64
  coins[3].x = 152  coins[3].y = 104
  coins[4].x = 184  coins[4].y = 56
  coins[5].x = 232  coins[5].y = 72
  coins[6].x = 224  coins[6].y = 104
  coins[7].x = 112  coins[7].y = 104
  local c = 0
  while c < 8 do coins[c].taken = 0  c = c + 1 end
end

function update_enemy(i)
  local step: int = enemies[i].dir
  local nx = collide_x(enemies[i].x, enemies[i].y, 8, 8, step, SOLID)
  local foot: int = nx
  if step > 0 then foot = nx + 7 end
  if nx == enemies[i].x or solid(foot, enemies[i].y + 8) == 0 then
    enemies[i].dir = 0 - enemies[i].dir
  else
    enemies[i].x = nx
  end
end

function collect_coins()
  local i = 0
  while i < 8 do
    if coins[i].taken == 0 and rect_overlap(p.x, p.y, 8, 8, coins[i].x, coins[i].y, 8, 8) then
      coins[i].taken = 1
      coins_collected = coins_collected + 1
      sfx(0)
    end
    i = i + 1
  end
end

function resolve_enemies()
  -- Knockback invulnerability removes the player's enemy hitbox entirely.
  if invuln > 0 then return end
  local i = 0
  while i < 4 do
    if enemies[i].alive == 1 and rect_overlap(p.x, p.y, 8, 8, enemies[i].x, enemies[i].y, 8, 8) then
      if p.vy > 0 and p.y + 8 <= enemies[i].y + 4 then
        enemies[i].alive = 0
        p.y = enemies[i].y - 8
        p.y4 = p.y * 4
        p.vy = 0 - 11
        sfx(1)
      elseif invuln == 0 then
        invuln = 45
        knock_timer = 5
        if p.x < enemies[i].x then knock_dir = 0 - 1 else knock_dir = 1 end
        p.x = collide_x(p.x, p.y, 8, 8, knock_dir * 3, SOLID)
        p.vy = 0 - 8
        sfx(2)
        return
      end
    end
    i = i + 1
  end
end

function update()
  if invuln > 0 then invuln = invuln - 1 end
  local dx: int = 0
  if knock_timer > 0 then
    dx = knock_dir * 2
    knock_timer = knock_timer - 1
  elseif wall_jump_timer > 0 then
    -- Preserve the reflected launch while allowing half-strength air steering.
    dx = wall_jump_dir * 2
    if wall_jump_timer % 2 == 0 then
      if btn(LEFT) then dx = dx - 1 end
      if btn(RIGHT) then dx = dx + 1 end
    end
    wall_jump_timer = wall_jump_timer - 1
  else
    if btn(LEFT) then dx = 0 - 1 end
    if btn(RIGHT) then dx = 1 end
  end
  p.x = collide_x(p.x, p.y, 8, 8, dx, SOLID)

  local on_left = touching_left(p.x, p.y, 8, 8, SOLID)
  local on_right = touching_right(p.x, p.y, 8, 8, SOLID)
  if btnp(A) then
    if p.grounded == 1 then
      p.vy = 0 - 15
    elseif on_left == 1 then
      p.vy = 0 - 15
      wall_jump_dir = 1
      wall_jump_timer = 6
      p.x = collide_x(p.x, p.y, 8, 8, 2, SOLID)
    elseif on_right == 1 then
      p.vy = 0 - 15
      wall_jump_dir = 0 - 1
      wall_jump_timer = 6
      p.x = collide_x(p.x, p.y, 8, 8, 0 - 2, SOLID)
    end
  end

  p.vy = p.vy + 1                   -- 0.25 px/frame squared gravity
  if p.vy > 12 then p.vy = 12 end   -- 3 px/frame terminal fall speed
  if p.grounded == 0 and p.vy > 6 and (on_left == 1 or on_right == 1) then
    p.vy = 6                         -- grip the wall for a controlled slide
  end

  local next_y4: int = p.y4 + p.vy
  local target_y: int = next_y4 / 4
  local dy: int = target_y - p.y
  local ny = collide_y(p.x, p.y, 8, 8, dy, SOLID)
  p.y = ny
  if ny ~= target_y then
    p.vy = 0
    p.y4 = ny * 4
  else
    p.y4 = next_y4
  end

  p.grounded = 0
  if p.vy >= 0 and touching_floor(p.x, p.y, 8, 8, SOLID) then
    p.vy = 0
    p.y4 = p.y * 4
    p.grounded = 1
  end

  enemy_tick = enemy_tick + 1
  if enemy_tick % 3 == 0 then
    local i = 0
    while i < 4 do
      if enemies[i].alive == 1 then update_enemy(i) end
      i = i + 1
    end
  end
  collect_coins()
  resolve_enemies()

  -- Keep the player near the horizontal centre, clamped at both stage edges.
  cam_x = p.x - 60
  if cam_x < 0 then cam_x = 0 end
  if cam_x > CAMERA_MAX then cam_x = CAMERA_MAX end
end

function draw()
  cls(12)
  camera(cam_x, 0)
  map(0, 0, 0, 0, WORLD_TILES, 16)
  entity(p.x, p.y, 1)
  local i = 0
  while i < 8 do
    if coins[i].taken == 0 then
      spr(coin, coins[i].x, coins[i].y, 0)
      entity(coins[i].x, coins[i].y, 3)
    end
    i = i + 1
  end
  i = 0
  while i < 4 do
    if enemies[i].alive == 1 then
      spr(enemy, enemies[i].x, enemies[i].y, 0)
      entity(enemies[i].x, enemies[i].y, 2)
    end
    i = i + 1
  end
  if invuln == 0 or invuln % 6 < 3 then spr(hero, p.x, p.y, 0) end
  entity(coins_collected, invuln, 30)

  camera(0, 0)
  text("COINS", 2, 2, 6)
  number(coins_collected, 44, 2, 10)
end
