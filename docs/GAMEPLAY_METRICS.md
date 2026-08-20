# Gameplay metrics — giving the agent a sense of play

A plan, in the same spirit as [`SYNTH.md`](SYNTH.md): written first as the record
of what is to be built and why, kept afterwards as the record of where the build
diverged from it.

The problem it exists to solve: **an agent driving this console can tell whether
a game looks right, and has no way at all to tell whether it plays right.** The
loop today is run → hold a key → screenshot → judge. That produces games that
look like games. It does not produce a car whose handling is worth feeling, or a
stage whose enemies arrive in a rhythm the player can learn.

The shape in one line: **the deliverable of a play session is a report, not a
picture.**

## The diagnosis

This project has already solved this exact problem once, for sound.

`kessel-vm`'s own rule is that the agent has no ears, and the answer was *not*
to hand it a WAV. It was [`AudioSummary::report`](../crates/vm/src/audio.rs) —
prose, in numbers, with the failures named:

```
no sound events — the game never called sfx()/music(). Nothing was going to play.
```

The comment on that function says why it is prose rather than JSON: it is read
by a model deciding what to fix next. `VM_AUDIO.md` puts it more bluntly — the
WAV alone would be an artefact nobody in the loop can read.

**A screenshot is the WAV of game feel.** A frame is a *spatial* observation of a
*single instant*. Game feel is a property of how state changes *in response to
input*, *over time*. It is not under-sampled by screenshots; it is absent from
them. No number of frames fixes that, because the missing axis is not resolution.

So the agent is not short of taste. It is short of a **sense organ**. Everything
below is one.

## Two things the machine already has and does not use

**Determinism and snapshots.** The reason `vm_snapshot`/`vm_restore` exist is
frame-exact reproducibility — but reproducibility is also what makes *controlled
experiments* possible: the same starting state, the same seed, a different pair
of hands. Nothing in the screenshot loop uses this. It is the machine's one
unique capability with respect to this problem, and it is sitting idle.

**`entity(x, y, tag)`.** Already in [`device.rs`](../crates/vm/src/device.rs),
with a comment that gets the design exactly right:

> These are authored by the game (not inferred), so the harness can expose or
> hide internal state per experiment.

That is the correct principle and it is already written down. In practice the
port is used almost only as an assertion channel in the luax tests.

And the time series is already being computed and thrown away:
`vm_run_frames` builds an `Observation` every frame, reduces the lot to
`frames_with_screen_change`, and returns only the last one. The trace is on the
floor.

## The surfaces

Lettered as they were argued, not as they will be built — the build order is at
the end.

### A. Differential play (`vm_playtest`)

Run the same span from the same snapshot under several input policies, and
**compare**:

```
idle  /  mash A  /  hold RIGHT  /  random  /  a scripted "competent" run
```

The headline check this buys:

> **If idle and skilled produce the same outcome, the game is not responding to
> skill.**

That is the mechanical definition of "there is no gameplay here", and it is
decidable. The plumbing mostly exists: `vm_run_frames` already takes a `script`
of `{buttons, frames}` segments, and snapshot/restore already give a common
starting state. What is new is running several and diffing them.

Secondary readings from the same trace:

- **Event density over time** — spawns, hits, deaths, per second. "A wave
  structure" is literally a shape in a time series; show the shape.
- **Stakes** — did the run ever reach a terminal state? did score ever move?
  A run where nothing can go wrong is a run with no tension, and that is
  detectable without watching it.

### B. Latency probe

Snapshot, one frame idle, restore, one frame with the button, diff the entities.
Repeat for a few frames.

```
A press: +1 frame 0px, +2 frames 0px, +3 frames 2px
```

Frames-to-first-motion and how much motion is the most diagnostic single number
in game feel — but only for a game whose controls are *wrong*, and it is cheap
enough to add whenever one is. Deferred: both current testbeds already feel fine
to the hand.

### C. `signal(name, value)` — the sibling of `entity`

`entity` hands the harness **space**. Its missing sibling hands over **scalars
over time**: speed, hp, score, distance remaining, combo. Same port family, same
*authored, not inferred* rule, roughly the same implementation cost.

This is what makes the two examples that started the discussion expressible:

- **A car.** Hold RIGHT for 60 frames from rest, log speed each frame. Out comes
  the acceleration curve, the top speed, and the frames to reach it. Those three
  numbers *are* the handling. An agent writing `x = x + 1` today has no way to
  learn it has built a brick sliding on ice.
- **Enemy waves.** Spawn events per second, as a series. Flat reads as flat.

### D. The judgement goes in the report, not in the model's priors

Measurement alone does not change behaviour. `top speed in 4 frames` is a fact
an agent can read and do nothing with. This does change behaviour:

```
top speed reached in 4 frames — reads as instant.
A vehicle usually wants 20-40; under 10 feels like a brick that teleports.
```

The audio report already works this way and it is easy to miss: `limiter
engaged on N samples` is not a measurement, it is a **verdict**. Every line of a
playtest report should be willing to be one. This is the step that turns
instrumentation into a designer's eye, and it is the step most likely to be
skipped because the numbers look useful on their own.

### E. The corpus as a reference for feel

`games/` is the luax reference corpus for *syntax* — the agent adapts from it.
If each game carried its measured feel as a committed baseline, "make a racing
game" would arrive with "here is what outrun's handling measures, and why".
`crates/vm/tests/games_audio.rs` is the shape to copy: a guard that fails when a
tweak quietly makes the car unresponsive.

## What this is deliberately not

- **No single "fun score".** Handing a model a number to maximize produces a
  model that maximizes the number. The report describes, warns, and cites
  reference ranges; it does not rank.
- **No inferring which entity is the player.** Fragile, and against the rule
  `entity` already states. The game declares it.
- **No LLM judge inside `kessel-vm`.** The host-free rule is the load-bearing
  constraint of the whole project; this feature does not get to be the exception.
  The report is generated; the judging happens in whoever reads it.

## The two testbeds

The mapping is not a choice — each game selects its own surface.

### shooter → A

[`games/shooter.lua`](../games/shooter.lua) has outcomes already: `lives`,
`score`, `state` (0 playing / 1 game over / 2 stage clear), a boss with `bhp`,
`power`, `bombs`. Differential play means something only where a result exists.

It also has the finding that makes it the right calibration target. The whole
stage's pacing is one line:

```lua
spawn = spawn + 1
if spawn % 40 == 0 then spawn_wave() end
```

and `spawn_wave()`, despite the name, adds **exactly one foe**, at a random x.
So the stage is:

- 1.5 foes per second, constant, start to finish
- no formations — the "wave" is a single unit
- variation only in *kind*, switched by `scroll` across sea / beach / inland
- no rest beats, no crescendo, no pressure ramp into the boss

That is the exact inverse of "enemy timing arrives in waves, so the player's
hands find a pattern". There is no wave anywhere in it, and the cause is one
line.

This is why shooter goes first: **the answer is known before the instrument is
built**, so the first report either says

```
spawns: 44 events, interval 40 frames +/-0, 1 foe each — one rhythm, never varied
```

or the instrument is wrong. Calibrate on a target whose value you already know.

And A can be prototyped with **no VM change at all** — a few `entity()` calls in
shooter, including a deliberately ugly one that packs score and lives into a
coordinate pair. The moment that packing becomes annoying is the moment the
shape of `signal` is known. A port lives close to the ROM's identity and is hard
to change later, so it is worth designing second.

### outrun → C

[`games/outrun.lua`](../games/outrun.lua) says it itself, in its own header
comment:

> There is no crash — it is an endless cruise.

**outrun has no outcome.** No goal, no crash, no timer, no score. Differential
play against it can only report that no policy differs from any other, which is
already known. A is the wrong instrument for this game.

C is right, and for a reason better than instrumentation: the value
`signal("speed", speed)` would export is **the same value the HUD should be
displaying**. There is no speedometer and no distance remaining, which is a
gameplay defect on its own terms — the player is given no feedback about the
one quantity the whole game is about. Build the instrument the *player* needs,
and the harness reads it for free.

Distance remaining first needs a decision about what is being driven toward:

| | |
|---|---|
| **Distance goal** | Stage-based. Simplest. |
| **Checkpoint extension** | The arcade OutRun answer, and the recommendation. One number — time left — simultaneously supplies the reason to go fast, the penalty for leaving the tarmac, and a tension curve that steepens. It also maps onto `signal` cleanly. |
| **Endless + distance score** | Keeps the current character; cannot build a tension curve. |

That is a design decision, not an implementation one, and it is upstream of the
work.

## Build order

1. ~~**shooter: prototype A with `entity()` only.**~~ **Built** — see below.
2. ~~**Design `signal` from what step 1 could not express.**~~ **Built** — see
   *Signals* below.
3. **outrun: goal (probably checkpoints) + `signal`,** exposing speed and time
   remaining to the HUD and the harness in one move.
4. D and E follow the first real report — reference ranges are worth writing down
   only once there are measurements to range.

B stays on the shelf until a game arrives whose controls are actually suspect.

## What was built, and where it diverged from the plan

`crates/vm/src/playtest.rs` and the `vm_playtest` tool, plus five `entity()`
call sites in `games/shooter.lua`. Four things came out differently from the
plan above, and they are the useful part.

**"No VM change at all" was wrong.** The plan assumed A could be prototyped from
outside because the trace was already there. It is computed and then discarded:
`vm_run_frames` builds an `Observation` per frame and returns only the last one,
exactly as it does for `console`. Over MCP the per-frame entity series is
therefore unreachable, and one round trip per frame is not a loop anybody can
run. So the comparison had to live in the VM. The precedent was already sitting
in `RunFrames`, which accumulates the sound log across frames for precisely this
reason and stops there.

**A policy is not a script.** `vm_run_frames` plays a script once because it is
staging a scenario. A way of *playing* is a habit, so a policy's segments loop
to fill the run — which is what makes "mash A" two segments instead of six
hundred, and is why `Policy` is its own type.

**Classifying a tag turned out to be the hard part, and got it wrong twice.**
Both failures were confident nonsense rather than crashes, which is the shape
this whole document is about:

- A tag reported once per frame is present on 600 of 600 frames under *every*
  policy, so comparing presence alone accused the score of ignoring the player —
  the one tag that was obviously responding. Sameness now requires the final
  value to match too.
- shooter reports its game-over state as tag 2, which appears on 129 consecutive
  frames — a minority of the run, so the first classifier read it as an event
  and reported a rhythm of "every 1 frames, sd 0.0", metronome warning and all.
  An event has to fire on *isolated* frames; a tag that persists is a population
  that started late.

**`idle` had to become an explicit control.** Comparing policies pairwise almost
never fires: any policy that moves the ship ends somewhere else, so "every policy
ended somewhere different" was technically true and useless. Measuring every
policy *against doing nothing* is what produces the finding.

### What running it found

Five policies, 600 frames each, from 120 frames in:

```
tag 40   idle           15 fires, every 40 frames (40..40, sd 0.0)
tag 40   mash-a         15 fires, every 40 frames (40..40, sd 0.0)
tag 40   hold-right     11 fires, every 40 frames (40..40, sd 0.0)
tag 40   sweep          15 fires, every 40 frames (40..40, sd 0.0)
tag 40   random         15 fires, every 40 frames (40..40, sd 0.0)
         NOTE: the interval never varies. That is a metronome, not a
         rhythm — there is no wave for the player's hands to learn,
         and no rest to make the busy stretch feel busy.
         NOTE: identical under every policy. This tag's timing does not
         depend on how the game is played at all.
```

The instrument named `spawn % 40` on the first run without being told, which is
what the target was chosen for. Three findings arrived that were *not* known in
advance:

- **`mash-a` differs from `idle` on 3 of 11 tags** — ten seconds of holding the
  fire button down moves the game almost exactly as far as nobody touching it.
  Both lose their first life on the identical frame. Standing still and shooting
  is not a way to play this game, and the game never says so.
- **`hold-right` is the only policy that dies,** three times. Parking against the
  right edge is the worst thing a player can do, and there is nothing on screen
  that suggests it.
- **Score under the five: 0, 10, 0, 70, 130.** The random flailer outscores
  every deliberate policy, which is what a stage with no formations to read
  looks like from the inside.

None of those are visible in a screenshot, and the first two are not visible in
any single run however long.

## Fixing what the instrument found

`shooter.lua:710` is gone. A wave is now a *formation* — one enemy kind on one
of four shapes, released a few frames apart — followed by a rest that shrinks as
the boss approaches. The shape cycles rather than being drawn at random, because
a pattern seen a second time is the only kind anyone can learn.

The instrument reads the change:

| | before | after |
|---|---|---|
| tag 40 (an enemy enters play) | `every 40 frames (40..40, sd 0.0)` + metronome note | `every 35 frames (3..93, sd 35.3)`, note gone |
| tag 43 (a formation begins) | — | `every 91 frames (80..101, sd 7.6)` |

The 3 is a release inside a formation and the 93 is the rest between two, so the
same report now shows the rhythm at both scales.

It also found a **regression, immediately**: `hold-right` went from dying three
times to differing from `idle` on 2 of 14 readings. Formations cluster around
`wv_base`, so parking against the right edge became safe. That is the loop
working — the instrument caught a fault introduced by the fix to the fault it
caught.

Two guards broke, and both broke *usefully*: `games_compile.rs` patches the
shooter source by string replacement to aim spawns at the player, and those
strings no longer existed. A brittle technique, but it fails loudly, which is
the property that matters.

## Signals

`signal NAME` / `signal NAME: int`, reported with `signal(name, value)`. Ports
`0x53`/`0x54`, names carried as ROM metadata beside `controls` and the sound
bank. `docs/VM.md` has the surface; what follows is why it is shaped this way.

**`entity` is places, `signal` is numbers.** The prototype packed `(score,
lives)` into tag 30's coordinates and `(power, bombs)` into tag 8's, and the
report could only ever print `tag 30: 0,2`. Every reading of it had to be done
by a human with the source open — which is exactly the failure this whole
document is about, one level up.

**A signal's name is resolved at the call site, not bound as a global
constant.** This is the one place luax's *one name means one thing* rule is
deliberately set aside, and the reason is that signals are unlike every other
named thing in the language. A sprite or an `sfx` is named after an *asset*, so
one shared namespace costs nothing. A signal is named after the *variable it
mirrors*, so `signal score` beside `local score` is not an edge case, it is what
every use looks like. Binding it globally would have compiled `score = score +
10` into arithmetic on an id — silently, correctly, and wrongly. The alternative
was making every game invent a second word for one thing. luax already resolves
a name contextually in the sound blocks (`sfx { inst = blip }`), so the shape
was not new.

**Signedness is declared, not guessed.** `signal speed: int` reads back `-3`;
`signal score` reads back `40000`. One default gets one of them wrong — unsigned
turns every velocity into 65533, signed wraps a score at 32767 — and only the
author knows which this is. luax spells this `: int` everywhere else, so it cost
one optional token.

**`state` became a signal, and that fixed a shape problem.** shooter reported it
as `entity(px, py, state + 1)`, so a game over arrived as a *different tag*
rather than a different value: the set of things being reported changed when the
player died, and every table in the report grew a hole. A run's state is a
number about the run, not a thing with a position.

### What it bought, in one line of report

The finding that took a paragraph of hand-analysis in the first prototype is now
printed:

```
signals — where each way of playing left each named scalar:
  signal           idle     mash-a hold-right      sweep     random
  score               0         40          0         40        130
  lives               2          3          2          2          0
  power               1          1          1          1          1
  over the run: bombs 0..3, foes_alive 0..6, lives 0..3, score 0..130
  NOTE: power never moved under any policy — nothing anyone did touched it.
```

The upgrade curve never starting inside the first ten seconds was something the
first version could only reveal to someone who already suspected it. Now the
report says it.

The differential names things too — `differs on 7 of 15 reported values:
foes_alive, lives, score, tag 10, …` rather than a list of numbers.
