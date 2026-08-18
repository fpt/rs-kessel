# tools — build-time asset tools

Nothing here runs at play time. These turn generated art into the `sprite`
declarations a `games/*.lua` file carries, and they touch the network and the
disk, which is why they live outside `crates/`.

The pipeline is two steps, and they are separate on purpose: generation costs an
API call and is nondeterministic, quantisation is free and is the part you
iterate on.

```
prompt ──imgen.py gen──▶ 1024² PNG on magenta ──imgen.py process──▶ one trimmed PNG per cell
                                                                          │
                                                         spritegen.py ────┘
                                                                          ▼
                                                luax `sprite` blocks (+ `pal` lines)
```

Both are Python, run through uv — the dependencies are in `pyproject.toml`:

```bash
uv run --project tools tools/imgen.py --help
```

## imgen.py

`gen` calls OpenAI's image model, `edit` does the same conditioned on reference
images, and `process` keys out the background and splits a grid sheet into cells.
Generation needs `OPENAI_API_KEY` (this repo's `.envrc` loads `.env`); `process`
is offline.

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

And always look at `--preview`. Quantisation failures do not error, they just
look slightly wrong: a magenta pixel in a white number plate, a grey tower gone
purple, a tree outline that ate the whole canopy.
