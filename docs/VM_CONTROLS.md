# Kessel Controls

Everything the console reads from the player: buttons, an analog stick, touch,
and the gestures derived from touch. Split out of [`VM.md`](VM.md), which owns
the machine itself; its siblings are [`VM_GRAPHICS.md`](VM_GRAPHICS.md) and
[`VM_AUDIO.md`](VM_AUDIO.md).

The load-bearing idea: **a frame's input is one thing.** `device::Input` carries
the buttons, the stick and four touch points together, because they are one
snapshot — the state of the player's hands at a frame boundary. A recording that
replayed the buttons but not the stick would be a *plausibly* wrong replay,
which is the failure mode this machine exists to avoid.

Everything here is **optional**. A ROM that reads only `btn` sees exactly the
console it always saw.

## Ports

| Port | Dir | Meaning |
|------|-----|---------|
| `0x20` | in | gamepad buttons bitfield (held) |
| `0x21` `0x22` | in | gamepad edges: just-pressed / just-released this frame |
| `0x23` `0x24` | in | analog stick x / y — signed 8.8 fixed (-256..256), 0 centred |
| `0xd0` | out | select the touch slot the `0xd1`–`0xd7` reads describe |
| `0xd0` | in | how many fingers are down |
| `0xd1` `0xd2` | in | the slot's x / y, in console pixels |
| `0xd3` | in | the slot's state — bit0 down, bit1 pressed, bit2 released |
| `0xd4` | in | the slot's swipe this frame — one direction bit, or 0 |
| `0xd5` `0xd6` | in | the slot's drag dx / dy — **signed** px from where the press began |
| `0xd7` | in | frames the slot's press has been held |

Button bits, and the same four values a swipe reports:

```
LEFT 0x01   RIGHT 0x02   UP 0x04   DOWN 0x08
A    0x10   B     0x20   START 0x40  SELECT 0x80
```

## Buttons

Eight bits, and **the four direction bits are whatever the ROM says they are** —
a d-pad, or four labelled keys with no directional meaning at all. See
[`controls`](#controls-metadata).

- `btn(mask)→0/1` — held
- `btnp(mask)→0/1` — pressed *this* frame (the rising edge)
- `btnr(mask)→0/1` — released this frame

Use `btnp` for jumps, menu steps and fire-on-press, so a game never has to track
the previous frame's buttons by hand.

This is deliberately not a wider gamepad. Eight bits is eight labelled keys,
which covers the layouts this console is for; widening it would touch the attach
protocol, the C ABI and every host's plumbing to buy one more key.

## The analog stick

`stick_x()→int` / `stick_y()→int`, signed 8.8 fixed in `[-256, 256]` — the same
scale and the same `int` type `sin`/`cos` return, so `±256` is full deflection
and `0` is centred.

Same scale means the **same caveat**: `/` is always unsigned on this machine, so
a negative deflection has to have its sign branched on before its magnitude is
divided. Feeding `-256` (delivered as `0xFF00`) straight into `/ 256` gives 255,
not -1:

```lua
function travel(v: int)
  if v < 0 then return (0 - v) * SPEED / 256 end
  return v * SPEED / 256
end
```

The console applies **no deadzone**. That is a decision about a particular piece
of hardware, and a host that needs one applies it before the VM sees the frame;
a game that wants one compares against a threshold it chose.

## Touch

Up to **four** points, in **console pixels**. Each host undoes its own
letterboxing and upscale first, so these are the coordinates the game draws
with — a game never learns the window size.

Select a slot by writing `0xd0`, then read it. Same shape as the trig device:
one register selects, the next answers.

- `touch_count()→word` — fingers down
- `touch_x(slot)` / `touch_y(slot)` — position, console pixels
- `touch_down|pressed|released(slot)→0/1` — the held/edge predicates, the touch
  equivalents of `btn`/`btnp`/`btnr`

**A slot is a finger's identity**, for that finger's whole life. Edges are
computed per slot, so `touch_pressed(0)` means "the finger in slot 0 landed this
frame", not "some finger landed". A host that renumbers its fingers between
frames therefore reports a release and a press that never happened — which is
why Android keys slots off `PointerId` rather than position in a list.

Reading a slot the console does not have gives an empty one rather than wrapping
onto a real finger, the same do-nothing answer an off-screen `pset` gets.

## Gestures

The console recognizes gestures **itself**, in the device layer, from touch
state it already owns. That is what makes a swipe identical on every host and
exact under snapshot/replay — a host-side recognizer would make one recorded
frame mean different things depending on who replayed it. It also means
`kessel run`'s mouse, Android's fingers and an agent's scripted `touch:`
argument all get gestures for free.

- `swipe(slot)` — a `LEFT`/`RIGHT`/`UP`/`DOWN` bit on the one frame the gesture
  is recognized, else 0
- `touch_dx(slot)→int` / `touch_dy(slot)→int` — **signed** displacement from
  where the press began
- `touch_frames(slot)→word` — frames this press has been held

```lua
-- The two input paths collapse into one, because a swipe and a btnp are the
-- same kind of event: an edge that fires once, reported as the same constants.
function direction()
  local s = swipe(0)
  if s ~= 0 then return s end
  if btnp(LEFT) then return LEFT end
  ...
end
```

`games/2048.lua` is the worked example.

### How it compares to iOS and Android

Both platforms split this into **two** recognizers, and the split is worth
understanding because it explains what is and isn't here:

| | discrete flick | continuous drag |
|---|---|---|
| iOS | `UISwipeGestureRecognizer` — `direction` only, no distance or velocity, thresholds private | `UIPanGestureRecognizer` — `translation(in:)` (cumulative), `velocity(in:)`, `.began/.changed/.ended` |
| Android | `GestureDetector.onFling(e1, e2, velocityX, velocityY)` — `e1` is the original down event, so the origin | `onScroll` (delta since last call), or Compose `detectDragGestures` with `onDragStart(Offset)` |
| Kessel | `swipe(slot)` | `touch_dx/dy(slot)`, `touch_frames(slot)` |

Four decisions follow, and each is a place this console deliberately picks a
side:

**A swipe fires mid-gesture, not on release.** iOS recognizes while the finger
is still down; Android's `onFling` waits for `ACTION_UP` so it can compute
velocity. A console wants the board to move *as* you swipe. The cost is that
reversing direction inside one press does nothing — the first recognized
direction wins.

**One press is one swipe.** Without that, a finger held past the threshold would
re-report the same direction every frame for as long as it stayed down.

**The device exposes the delta, not the origin.** Android's `onFling` hands you
`e1` and lets you subtract; here the game already has the current position from
`touch_x`, so a *signed* delta gives the origin back for free (`x - dx`) **and**
avoids the trap. Exposing the origin instead would leave every swipe game
subtracting two `u16`s and wrapping on any leftward drag. Note iOS's
`translation` is cumulative from the gesture's start, like this; Android's
`onScroll` `distanceX/Y` is the delta *since the last call*, which is a
well-known footgun.

**The threshold is `dim / 8`** — 16 px on Classic128, 30 on Extended240. Screen-
relative, so the gesture is the same *physical* size on both screens; a fixed
pixel count would feel shorter on the denser one. Android reaches the same place
from the other direction with `ViewConfiguration.getScaledTouchSlop`, which is
in dp precisely so it means one physical distance.

### Left out on purpose

- **Velocity as a built-in gate.** `touch_frames` plus `touch_dx` makes it
  computable at a fixed 60 Hz, and a fling threshold is a per-game feel decision
  the console would only be guessing at. This is the one place it diverges from
  `onFling` deliberately.
- **Diagonals.** Dominant axis only; every game that wants a swipe wants one of
  four answers. An exact tie goes to X, so a perfectly diagonal drag stays
  deterministic under replay.
- **Multi-finger swipe** (iOS's `numberOfTouchesRequired`) — a game that wants it
  ANDs two slots.
- **Configurable thresholds** — additive later; shipping one now means testing a
  knob nothing turns.

## `controls` metadata

An optional top-level `controls { … }` block records the game's input layout
**as ROM metadata**, so a host UI (on-screen buttons, help text, a phone's
virtual pad) reads it instead of guessing from source comments. It is
**irrelevant to VM execution** — the machine only ever sees the raw bitfield and
the raw touch points.

```lua
controls {
  dpad = true       -- is the movement pad used
  a = "jump"        -- action labels for the A / B / Start / Select buttons
  b = "dash"
  stick = "aim"     -- the analog stick is read, and what for
  touch = "draw"    -- the game reads touches or gestures on the screen
  pause = START     -- which physical button pauses (default START)
}
```

Keys: `dpad` (bool), `a`/`b`/`start`/`select` and `left`/`right`/`up`/`down` (a
`"..."` label), `stick`/`touch` (a `"..."` label), and `pause` (a button name).
Entries are separated by whitespace (commas optional).

Every game has a **pause** binding by default (`START`) even with no block, so
the host always has a pause control to offer — the play window freezes/resumes
on that button and says so in the title. `VmPlayer.controls_json()` hands the
whole layout to a host as JSON.

> **A game that reads `touch_*` or `swipe` must declare `touch`.** The ports work
> regardless, but a host only routes screen touches to a game that asks for
> them — so a swipe game that forgets the line works on a keyboard and does
> nothing on a phone. Same rule for `stick` and the on-screen thumbstick.

### Buttons instead of a d-pad

Labelling the four direction bits says they are plain keys rather than a
direction, and a host lays them out as a row — the pop'n-music shape, where
nothing on the pad means "up":

```lua
controls {
  dpad  = false
  left  = "red"    down = "green"  up = "blue"  right = "yellow"
  a     = "white"  b    = "black"
  pause = START
}
```

`dpad = true` together with a direction label is a **diagnostic**, not a silent
winner: the two are claims about the same four bits that mean opposite things.
The JSON carries the resolved answer as `dir_layout` (`"dpad"` / `"buttons"` /
`"none"`), so every host draws the same pad rather than re-deriving the rule
three times.

`games/popn.lua` is the worked example, and note what it has to get right: the
lane order on screen must match the order a host lays the keys out (`left`,
`down`, `up`, `right`, then `a`, `b`).

## What each host provides

| host | buttons | stick | touch |
|------|---------|-------|-------|
| `kessel run` | keyboard (below) | the direction keys, diagonals normalized to ±181 so they aren't √2 faster | the mouse, as slot 0 |
| Android | d-pad, button row, or labelled keys per `dir_layout` | an on-screen thumbstick, when the ROM declares `stick` | fingers on the game surface, when the ROM declares `touch` |
| `vm_run_frame(s)` | the `buttons: [names]` argument | `stick: [x, y]` | `touch: [[x, y], …]` |
| `kessel attach` | whatever the attached window has | the same | the same |

Gestures are not in this table on purpose: every host gets them from its touch
points, for free.

### Desktop keys (`kessel run`)

| Key | Button |
|-----|--------|
| Arrows / WASD | D-pad — and the analog stick, at full deflection |
| `Z` or `J` (or Space) | A |
| `X` or `K` | B |
| Return | START |
| Shift | SELECT |
| Mouse drag | touch slot 0 — so swipes work |
| `R` | reload the file from disk |
| Esc | quit |

## Sample games

- `games/2048.lua` — swipe, folded into the same `direction()` as the arrows
- `games/paint.lua` — touch and the stick, with the branch-on-the-sign idiom
- `games/popn.lua` — six labelled keys, no d-pad
