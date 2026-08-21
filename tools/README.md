# tools — build-time asset tools

Nothing here runs at play time. These turn generated art into the `sprite`
declarations a `games/*.lua` file carries, and they touch the network and the
disk, which is why they live outside `crates/`.

The pipeline is two steps, and they are separate on purpose: generation costs a
paid turn and is nondeterministic, quantisation is free and is the part you
iterate on.

```
prompt ──imgen.py gen──▶ one big PNG on magenta ──imgen.py process──▶ one trimmed PNG per cell
                                                                          │
                                                         spritegen.py ────┘
                                                                          ▼
                                                luax `sprite` blocks (+ `pal` lines)
                                                                          │
                                                       framediff.py ──────┘
                                                                          ▼
                                                  "will this actually animate?"
```

Both are Python, run through uv — the dependencies are in `pyproject.toml`:

```bash
uv run --project tools tools/imgen.py --help
uv sync --project tools --extra codex     # once, for the default backend
```

## imgen.py

`gen` calls an image model, `edit` does the same conditioned on reference images,
and `process` keys out the background and splits a grid sheet into cells.
`process` is offline; the other two are not.

### Two backends, and `codex` is the default

| | `--backend codex` (default) | `--backend api` |
|---|---|---|
| pays | a ChatGPT/Codex subscription | API credits |
| needs | `codex login status` saying **ChatGPT** | `OPENAI_API_KEY` (this repo's `.envrc` loads `.env`) |
| how | the local Codex agent's built-in `image_gen` tool, driven over its Python SDK | `POST /v1/images/{generations,edits}` |
| control | none | `--model`, `--size`, `--quality`, exact `-n` |

The subscription path is the default because it is the one that does not spend
credits per sprite, and iterating on art means generating a lot of sprites. What
it costs you is every knob: the built-in tool takes a prompt and returns an image
at a size of its own choosing (1254² turned up in testing, which is not even a
size the API would accept), writes it to
`$CODEX_HOME/generated_images/<thread>/<call_id>.png`, and the only way to learn
that path is the `imageGeneration` thread item the SDK hands back. `imgen.py`
reads it from there and copies the file to `--out`, leaving the original in place
so a rerun does not destroy the attempt you were comparing against.

So reach for `--backend api` when the size matters — a sheet whose cell count
divides its pixel width crops without rounding — or when you need
`gpt-image-1.5`'s true transparency instead of the magenta key.

Two more differences worth knowing before you blame the prompt:

* **`-n` is exact on `api` and a request on `codex`.** The agent decides how many
  built-in calls to make; `imgen.py` reports the shortfall rather than hiding it.
* **The skill rewrites your prompt.** Codex's `imagegen` skill normalises what
  you wrote into its own `Use case: / Subject: / Style:` spec. It is told here
  not to add creative requirements, and mostly does not, but the prompt the model
  saw is not the prompt you typed. On `api` it is.

The SDK is an **extra**, because `openai-codex` hard-depends on a ~300 MB Codex
binary and the half of this pipeline you actually iterate on does not need it:

```bash
uv sync --project tools --extra codex
```

Without it, `gen`/`edit` say so and point at `--backend api`.

If the codex backend reports no image at all, check `codex login status` first.
Under **API-key** auth the built-in tool is simply absent from the agent's
toolset — and misleadingly, `codex features list` still shows
`image_generation  stable  true`, because the flag is on and the *provider
capability* underneath it is not.

Reach for `edit` when the sprite is a *particular* thing rather than a generic
one. `games/outrun.lua`'s car came from two photographs of a real Lotus Esprit —
a straight rear view and a rear three-quarter — passed as `--ref`, and it is
recognisably that car in a way no prompt describing it was. Crop the references
to the subject first: a showroom floor in the reference puts a showroom floor in
the sprite.

The model cannot emit transparency, hence the magenta key. Ask for a **2×2 grid
in one image** rather than `-n 4`: it is one billed generation and the four cells
come out in a consistent style, which is what you want for a set of scenery that
will share one 15-colour sprite bank.

```bash
uv run --project tools tools/imgen.py gen --out /tmp/sheet.png --quality medium \
  --prompt "A 2x2 grid of four … chunky low-resolution pixel art, … \
            The background everywhere, including between the cells, is solid uniform magenta #FF00FF."

uv run --project tools tools/imgen.py process --in /tmp/sheet.png --out-dir /tmp/cells \
  --rows 2 --cols 2 --labels bldg,palm,tree,sign
```

Two things the prompt has to say or the art is unusable at this size: name the
grid **and** the magenta between the cells, and give the pixel budget ("treat
each object as if drawn on a 16×16 grid"). Without the second, the model returns
beautiful 512 px art whose every detail is smaller than one console pixel.

This started as `imgen` in the [`go-rrs`](../../go-rrs) repo (`cmd/imgen`) and was
ported here so the console's asset pipeline is not a prebuilt binary from another
project. The port is exact: on the sheets that produced `games/outrun.lua`'s art
it emits cells pixel-identical to the Go tool's, chroma key and all. The flood
fill is the one thing written differently — a whole-array grow in numpy rather
than a per-pixel BFS, because a million-iteration Python loop is not a port worth
having.

## spritegen.py

Downscales a cell to a sprite-sized grid and quantises it to at most 15 colours,
then prints one `sprite` declaration per image — at full size, since a
declaration's body *is* its size and the compiler does the slicing.

```bash
# the player's car: 4x4 tiles = 32x32, in its own sprite bank
uv run --project tools tools/spritegen.py /tmp/cells/testa.png \
  --names car --tiles 4x4 --palette own --bank 1 --preview /tmp/car.png

# four scenery objects, 2x2 tiles each, sharing one bank
uv run --project tools tools/spritegen.py /tmp/cells/{bldg,palm,tree,sign}.png \
  --names bldg,palm,tree,sign --tiles 2x2 --palette own --bank 2
```

`--tiles` is what pads the art out to a whole number of tiles, and past one tile
that padding is not cosmetic: every row must be the same length and both sides a
multiple of 8, or a short row shifts every tile after it in the sprite and every
id after that.

`--uniform` scales the whole set by one factor instead of fitting each image to
its own block. Use it whenever the images are **one object seen several ways** —
steering angles, animation frames:

```bash
uv run --project tools tools/spritegen.py /tmp/cells/s{0,1,2,3}.png \
  --names car,car1,car2,car3 --tiles 6x4 --uniform --palette own --bank 1
```

Without it, a yawed car — a wider silhouette than a straight one — is shrunk to
the same width as the straight one, and the car visibly pulses as the player
steers. The same goes for a smoke puff that is supposed to grow.

`--palette pico` quantises to the console's default sixteen instead, for a tile
that draws through bank 0 like every other sprite.

`--palette own` prints `pal()` lines alongside the tiles. Paste them into
`init()` and draw the sprites between `sprbank(n)` and `sprbank(0)` — and
remember the ROM then **owns palette indices n*16+1 .. n*16+15**, so nothing else
in the game may use them. `games/outrun.lua` is the worked example: the car has
bank 1, the scenery bank 2, and every road, grass and sky colour is picked from
outside 16–47 for exactly that reason.

Paste the output, do not retype it. Nibbles are palette *slots*, so a transposed
character is a legal sprite in the wrong colour — one `d` for an `f` put
dark-green posts under `games/outrun.lua`'s billboard, and it took regenerating
the art and diffing it against the game to find it. Re-running the two commands
above and diffing is the check.

## framediff.py

Reads `sprite` blocks back — out of `spritegen.py`'s output, or out of a
`games/*.lua` — and answers the two questions a still frame cannot:

```bash
uv run --project tools tools/framediff.py /tmp/set.txt --pairs wd0:wd1,wu0:wu1
```

```
wd0 vs wd1: 66/384 nibbles differ (17%), 3/16 rows identical  <- reads as motion
wu0 vs wu1: 36/384 nibbles differ (9%), 4/16 rows identical
```

**Do two frames differ enough to read as motion?** A model asked for "walking"
moves a leg by less than one console pixel: measured on a real 16×16 sheet, the
two frames differed by 7 nibbles out of 256 and animated into a held pose. The
same character respun with the delta spelled out in pixels differed by 73. The
threshold is a *share* of the sprite, not a count, because the same set padded
out to 24×16 for a sword that leaves the silhouette would otherwise look like it
had improved on its own.

**Are the frames the same size?** With `--pairs` it also reports each sprite's
drawn bounds and complains when they disagree — art drawn at four different
heights makes the character pop as the player turns, and `spritegen --uniform`
cannot fix that because it scales by one factor rather than fitting a silhouette.

`.claude/skills/sprite-art/SKILL.md` has the prompt recipe these numbers came
from: what to write so a set comes out consistent in the first place.

## Notes

And always look at `--preview`. Quantisation failures do not error, they just
look slightly wrong: a magenta pixel in a white number plate, a grey tower gone
purple, a tree outline that ate the whole canopy.

Look at the *generation* too, and not only for quality. Asked for "a small
roadside billboard", the model returned one reading **SEGA OUTRUN** with a red
Ferrari on it — a real trademark, unprompted, in a file that was one paste away
from being committed. Say what the sign says, or say that it is blank.
