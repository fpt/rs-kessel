-- lib/motion.lua — movement helpers, shared by including them:
--
--   #include "lib/motion.lua"
--
-- Files under `lib/` are not games: the library screen lists what sits at the
-- top of `games/`, so a helper file goes in here and stays out of it.
--
-- What `#include` is, in one paragraph. It splices this file's declarations
-- into yours at the directive — one flat namespace, no module value, nothing
-- to bind. It is *not* Lua's `require`, which returns a table this machine has
-- no way to hold; writing `require` is a diagnostic that says so. A file is
-- included at most once however many files ask for it, so two of your sources
-- can both `#include "lib/motion.lua"` without declaring anything twice. Only
-- the game's own file may carry `screen` and `controls` — a library that
-- silently changed your screen size would be a long afternoon.

-- A thing that moves. `x`/`y` are screen coordinates (unsigned, like every
-- coordinate the console draws with); `vx`/`vy` are **signed**, so a leftward
-- step compares as less than zero rather than as 65535.
record Body { x, y, vx: int, vy: int }

-- Advance one frame.
function move(b: Body)
  b.x = b.x + b.vx
  b.y = b.y + b.vy
end

-- Reverse a step at the edges of [lo, hi].
--
-- Only when it is heading *into* the wall: reversing on position alone leaves a
-- body that starts on an edge flipping every frame and going nowhere, which
-- looks like a physics bug and is really a missing half of the condition.
function bounce_x(b: Body, lo, hi)
  if b.x <= lo and b.vx < 0 then b.vx = 0 - b.vx end
  if b.x >= hi and b.vx > 0 then b.vx = 0 - b.vx end
end

function bounce_y(b: Body, lo, hi)
  if b.y <= lo and b.vy < 0 then b.vy = 0 - b.vy end
  if b.y >= hi and b.vy > 0 then b.vy = 0 - b.vy end
end

-- Move `v` by `d`, staying inside [lo, hi].
--
-- `v` is unsigned, so "below lo" is a wrap to 65535 rather than a negative
-- number — the check has to happen *before* the subtraction. Doing it after is
-- the bug where walking off the left edge teleports you off the right one.
function nudge(v, d: int, lo, hi)
  if d < 0 then
    local back = 0 - d
    if v <= lo + back then return lo end
    return v - back
  end
  if v + d >= hi then return hi end
  return v + d
end

-- Do two `size`-square blocks at these corners overlap?
function hits(ax, ay, bx, by, size)
  return ax < bx + size and bx < ax + size and ay < by + size and by < ay + size
end

-- Fill a `size`-square block. Every game that draws a solid sprite-less thing
-- writes this loop; here it is once.
function block(x, y, size, color)
  for dy = 0, size - 1 do
    for dx = 0, size - 1 do
      pset(x + dx, y + dy, color)
    end
  end
end
