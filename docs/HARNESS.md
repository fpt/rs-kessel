# Kessel as an Agent-Facing Game-Debugging Harness — Direction

> Status: **design / direction note**, not an implementation spec. It records
> *why* the next phase of Kessel should shift from "add more game features" to
> "prove a generated game actually works," and *what* to build in that order.
> The VM/luax reference lives in [VM.md](VM.md); this doc sits above it.

## 1. Where we are

The MVP works. Kessel is a deterministic Uxn-style stack VM with a device layer
(screen, gamepad, rng, tilemap, sound, debug), a small statically-typed Lua-ish
front-end (`luax`) that compiles to it, and a set of `vm_*` tools that let a
model **write → assemble → load → run a frame → observe → snapshot → debug**.
That closed loop already exists and is what makes the project interesting.

Concretely, the pieces already in place (see [VM.md](VM.md)):

- **Determinism** — a seeded xorshift RNG (port `0x30`), no wall-clock, no
  hidden state; the same ROM + same inputs + same seed replays identically.
- **Frame-stepped execution** — `run_frame(buttons)` advances exactly one frame
  with an injected gamepad bitfield.
- **Snapshots** — `VmConsole::snapshot()` / `restore()` clone the whole machine.
- **Observation** — every frame returns an `Observation`: `framebuffer_hash`
  (FNV-1a), `changed_pixels_bbox`, `entities` (game-reported tags/coords),
  `sound`, `fault`, `pc`, `data_stack`, `console`.
- **A latent symbol table** — the assembler already returns
  `labels: name → address`, and luax lowers globals to named labels. We are one
  step from source-level symbol info.

## 2. The concern

Making a Lua-ish language that a small model can emit is **not defensible on its
own**. Bolting an AI harness onto Lua, pygame, or Godot is not hard, and those
ecosystems bring libraries, editors, tutorials, assets, and communities that
Kessel cannot match head-on. Two specific liabilities:

1. **Compilation is a treadmill, not a moat.** Every luax feature we add means a
   parser, type-checker, codegen path, error messages, debugger support, docs,
   and model-facing examples to maintain. The closer luax drifts toward "real
   Lua," the more users and models expect *actual* Lua — and the more every gap
   reads as a bug.
2. **"Compiles" ≠ "playable."** A green compile says nothing about whether the
   player can move, whether the score increments, whether the game ends, whether
   a button does what was asked, or whether it survives a minute of play. The
   harness is strong at *building* a ROM and weak at *judging* one.

If a competitor ships "pygame/Godot + an LLM," Kessel does not win by being
smaller. It wins only if it does something they structurally can't.

## 3. The thesis

**Kessel's value is not the VM or luax. It is the closed loop, and specifically
the half of it that lets an agent *verify and repair* its own game.**

```
model writes code
  → compile (fast, deterministic)
  → run deterministically, frame by frame
  → observe screen + internal state
  → inject input
  → reproduce a failure exactly
  → fix
```

pygame and Godot are excellent for humans but hostile to *machine* observation.
To answer "did the player clip through the wall / did the score rise / does it
game-over after 10s / does the piece rotate on a single press / is the run
reproducible?" in those engines, you must build most of a test harness first.
Kessel already has determinism, snapshots, frame stepping, input injection, and
entity/memory reporting. Investing *there* turns it from "a fantasy console" into
**a game-development environment where the AI can test its own games** — which a
thin pygame/Godot wrapper is not.

So the strategic reframing is:

- **Freeze luax roughly where it is.** Treat it not as a Lua replacement but as
  *an agent-friendly game-description format* optimized for small games,
  deterministic execution, static memory, fast compilation, and rich
  observation. Resist unbounded language growth.
- **Spend the next phase on the observe/verify/repair half of the loop**, not on
  more drawing builtins.

## 4. Direction: capabilities to build

Ordered roughly by leverage. Each is a harness/tooling feature, exposed to the
model as a **JSON tool**, not (primarily) a human GUI.

### 4.1 Source maps & a symbol table (foundation)

Have the compiler emit debug info alongside the ROM:

```json
{
  "symbols": {
    "player.x": { "address": 4200, "type": "int" },
    "score":    { "address": 4210, "type": "word" }
  },
  "functions": { "update": { "start": 600, "end": 910 } },
  "source_map": { "723": { "file": "game.lua", "line": 84 } }
}
```

This is the enabler for almost everything below. We already have `labels`
(name→address) from the assembler; luax needs to additionally carry
source-variable names, types, function extents, and a PC→line map through its IR.

Immediate payoff — runtime errors become source-level:

```
division by zero at game.lua:84  (in update)
    local speed = distance / remaining
```
```
instruction-cap exceeded; likely loop at game.lua:132
    while enemy.alive == 1 do
```

### 4.2 Named-variable inspection

With symbols, replace "read address 0x1068" with `inspect("player.vy") → -1`.
Reading state by name is dramatically easier for an LLM than decoding raw memory
and stacks.

### 4.3 Frame history & time-travel

A snapshotable VM is ideal for this. Keep a ring buffer of periodic (or
per-frame) snapshots so the agent can walk backward:

```
frame 312: collision started
frame 313: player entered wall
frame 314: x wrapped to 65530
```

When a user says "I just got stuck in the wall," hand the model the last few
seconds of state. Doing this robustly in a general engine is hard; here it is
almost free.

### 4.4 Reproducible input traces

Because runs are deterministic, a failure is fully described by seed + inputs:

```json
{
  "seed": 712,
  "failure_frame": 438,
  "inputs": [["RIGHT", 24], ["A", 1], [], ["LEFT", 18]]
}
```

Every bug the harness finds should be emitted as a replayable trace.

### 4.5 State diffs

Instead of dumping all memory each frame, return the *meaningful* change:

```json
{
  "frame": 121,
  "changed_globals": { "player.x": [52, 54], "player.vy": [-2, -1], "grounded": [1, 0] },
  "entities_added": [], "entities_removed": [],
  "sound": ["jump"]
}
```

Needs only the symbol table + a previous-frame snapshot to diff against.

### 4.6 Watch expressions & breakpoints (as tools)

No full expression evaluator in the VM is required initially. Start with:

- watch a global; stop when it changes
- stop at a specific frame
- stop when an entity tag appears
- stop on `fault`

Exposed to the model as tools rather than a GUI:

```
vm_watch("player.vy")
vm_run_until_change("player.vy", max_frames=120)
```

### 4.7 Assertions & invariants

Let games declare invariants that the runtime checks (and that release builds can
strip):

```lua
assert(player.x >= 0)
assert(player.x < 128)
assert(active_bullets <= 32)
assert(score >= previous_score)
```

Plus **automatic** runtime warnings the VM can raise without the game asking:
array out-of-bounds, invalid sprite id, tilemap out-of-bounds, suspicious
coordinate wrap, excessive SFX in one frame, approaching the per-frame
instruction cap. The goal shifts from "the VM didn't crash" to **"the VM can
explain the suspicious behavior."**

### 4.8 Automated random playtesting (fuzzing)

The state space is tiny, so search it. Try pruned input sequences
(`LEFT/RIGHT/UP/DOWN/A/B/none`) across many seeds — e.g. **100 seeds × 600
frames** — and flag: faults, hangs, off-screen escapes, unreachable states, or
"progress stalled." No need to *beat* the game; random play finds a lot, and
determinism means every failure replays exactly (§4.4).

### 4.9 Visual regression (high-level)

We already hash the framebuffer. Golden-image exact-match is brittle for
LLM-authored games; assert *properties* instead:

```yaml
expect_visual:
  - non_background_pixels: "> 200"   # screen isn't blank
  - entity_tag_visible: 1            # the player is on screen
  - changed_bbox_within: [0, 0, 127, 127]
```

### 4.10 A mechanical "Playable Check"

Define the minimum bar for "playable" and run it automatically after generation:

```
✓ compiles
✓ starts without fault
✓ renders a non-empty frame
✓ responds to at least one declared control
✓ state changes over time
✓ restart works
✓ survives 1,000 random-input frames
✓ stays within the frame budget
```

Surface it plainly to user and model:

```
Playable: 6/7
Warning: START pause control declared but unused
```

This is where the `controls {}` metadata (see [VM.md](VM.md)) pays off — declared
controls give the check something concrete to exercise and to warn about.

### 4.11 Prompt → acceptance tests

The highest-leverage idea: **turn the user's request into a test**, and make the
model produce the test alongside the code. "Add a double jump" becomes:

```
press A
wait until falling
press A again → expect vertical velocity becomes negative
press A a third time → expect it does NOT
```

"Add a hard drop":

```
spawn a piece; press UP → expect the piece locks within 2 frames
```

Request, implementation, and verification live in one loop. Host-side scenarios
(YAML) read better than embedding them in luax:

```yaml
name: jump reaches platform
seed: 42
steps:
  - hold: RIGHT
    frames: 20
  - press: A
  - run: 30
expect:
  - entity.player.y < 70
  - fault: null
```

The model runs these after every edit and judges **behavior**, not "it compiled."

## 5. Language & architecture strategy

Keep luax, but structure the compiler so debug features are first-class:

```
luax parser → typed IR → Kessel bytecode
                  │
                  └── symbols, source map, function extents
```

Build the debugging features (§4) against the **IR + symbol info**, not the
surface syntax. That keeps the door open to alternate front-ends later — a JSON
AST, a different syntax, or a stricter "real Lua subset" — all lowering to the
same IR, without reworking the debugger each time.

For small models, consider **structured edits** over whole-file rewrites, to
reduce the chance of a model corrupting a working game:

```json
{ "operation": "replace_function", "name": "drop_delay", "source": "function drop_delay() ... end" }
```

## 6. Relationship to pygame / Godot

Not competitors — potential *backends*. The reusable asset is the **harness
layer**, not the VM:

```
extract requirements from a prompt
  → generate test scenarios
  → inject input
  → observe the screen
  → inspect state
  → reproduce a failure
  → manage the fix loop
```

That layer could later target a `pygame` backend, a `Godot` backend, a
`Lua/LÖVE` backend. But leading with those explodes complexity. Kessel is the
small, controllable **reference environment** to develop and productize the
harness first. In other words: the VM is not the goal — it is the
*reference implementation of an agent-facing game-development harness.*

## 7. Priority order

If we add anything next, prefer this over more rendering features:

1. Compiler source map + symbol table (§4.1) — the foundation
2. Named-variable inspection (§4.2)
3. Frame history & rewind (§4.3)
4. Reproducible input traces (§4.4)
5. Assertions & invariants + auto runtime warnings (§4.7)
6. Automated random playtesting (§4.8)
7. Prompt → acceptance-test generation (§4.11)
8. Image/state regression checks (§4.9)

## 8. Positioning

The MVP is meaningful as an MVP. The point of the next phase is **not** to pile
on game features, but to change Kessel into a debugging/testing environment that
can *prove a game is actually playable* rather than letting a model assume "it
worked."

The one-line difference we are buying:

> Not "Kessel wins because it's small," but
> **"Kessel is designed so an AI can observe, verify, and repair its own games."**
