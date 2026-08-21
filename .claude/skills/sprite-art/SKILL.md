---
name: "sprite-art"
description: "Generate sprite art for a Kessel game and turn it into luax `sprite` declarations — single sprites, multi-object sheets, and animation sets (walk cycles, attack frames) for a character. Use when asked to draw, generate, or animate any sprite, character, tile, or scenery for games/*.lua. Carries the prompt recipe that survives quantisation to 8-24 pixel sprites and the measurements that catch a frame set which will not read as motion."
---

# Sprite art for Kessel

`tools/README.md` is the reference for the commands. This is the part that is not
in the flags: **what to write in the prompt so the art survives being shrunk to
a 16×16 sprite, and how to know it did.**

Every number below was measured on this repo's own sheets. None of it is theory.

## The loop

```bash
# 1. generate a sheet (subscription by default, no API key)
uv run --project tools tools/imgen.py gen  --out /tmp/sheet.png --prompt "…"
uv run --project tools tools/imgen.py edit --out /tmp/sheet.png --ref REF.png --prompt "…"

# 2. key out the magenta and split the grid
uv run --project tools tools/imgen.py process --in /tmp/sheet.png --out-dir /tmp/cells \
    --rows 3 --cols 2 --labels d0,d1,u0,u1,l0,l1

# 3. quantise to a bank and print `sprite` blocks
uv run --project tools tools/spritegen.py /tmp/cells/{d0,d1,u0,u1,l0,l1}.png \
    --names wd0,wd1,wu0,wu1,wl0,wl1 --tiles 3x2 --uniform --align bottom \
    --palette own --bank 1 --preview /tmp/set.png > /tmp/set.txt

# 4. CHECK IT. Step 3 cannot fail loudly; this is what tells you it worked.
uv run --project tools tools/framediff.py /tmp/set.txt --pairs wd0:wd1,wu0:wu1,wl0:wl1
```

Step 4 is not optional. Steps 1–3 all succeed on art that produces a motionless
animation, and a still frame cannot show you that.

## Nine rules, and the number behind each

**1. Name the pixel budget, in the target grid's pixels.**
"Treat each object as if drawn on a 16x16 pixel grid, so no detail is finer than
one of those pixels." Without it the model returns beautiful 512 px art whose
every feature is smaller than one console pixel and dissolves on the way down.

**2. One sheet per generation, and name the magenta between the cells.**
"A 2x2 grid of four separate objects … The background everywhere, **including
between the cells**, is solid uniform magenta #FF00FF." One generation, four
cells in one consistent style, one bill. Say the grid shape *and* the background;
omit the second and the cells get their own backdrops and `process` splits
nothing.

**3. For a set, feed the character back as `--ref`, and say it is the character.**
Not "a hero like this" — this exact wording held one hero identical across six
cells:

> The reference image is THE character: reproduce exactly the same hero in every
> cell — identical spiky blue hair, identical light plate armour with red cloth,
> identical sword, identical colours. Only the legs and arms change between cells.

Enumerate the invariants. "The same character" alone drifts; a list does not.

**4. State the frame delta in pixels. This is the one that matters.**
A model's default idea of "walking" is a delta smaller than one console pixel.
Measured, same character, same everything else:

| prompt | nibbles differing at 16×16 |
|---|---|
| "walking toward the viewer; column 1 left leg forward, column 2 right leg forward" | **7 / 256** — a held pose |
| "in column 1 the legs are together and straight; in column 2 the legs are **wide apart** with one leg clearly swung forward and the other back, the arms swapped, and the entire body drawn **one pixel lower** as if bobbing" | **73 / 256** — reads |

And give the minimum size explicitly, in budget pixels: "the leg swing has to be
at least two of those pixels wide to be visible". Same for a weapon: "the
extended blade has to be at least four of those pixels long to read".

**5. Pin the baseline and the height, or the sprite pops when it turns.**
> CRITICAL: every hero must be exactly the same height and width, with the top of
> the hair on the same line and the feet on the same line in all six cells.

First attempt without this: cells came out 13, 14, 15 and 16 rows tall, so
changing direction made the character jump. `spritegen --uniform` fixes the scale
*factor*, not the content bounds — it cannot rescue art drawn at four sizes. Add
`--align bottom` so what variance is left sits on the floor rather than
floating.

**6. Put every state through ONE `spritegen` call.**
Walk and attack in separate invocations get separate scale factors, and the hero
changes size the moment it swings. One call, `--uniform`, and `--tiles` sized for
the **widest** frame in the whole set — an extended blade leaves the body's
silhouette, so a 16×16 walk plus an attack becomes a `3x2` (24×16) set with
transparent padding on the walk frames. Padding is free; a pulsing hero is not.

**7. Three directions, not four.** `sprn`'s flip flag makes right out of left, so
generate front / back / side-facing-left and mirror in the game. Four rows asked
of the model produced two side views that were not mirrors of each other and
one more cell's worth of drift. Fewer cells is more consistency and less money.

**8. Crop the reference to the subject, and forbid text.**
A reference of four characters and a logo puts four characters and a logo in the
output. Crop to the one thing you want the style of. And always add: "No text,
no letters, no logos, no signature anywhere in the image." Asked for "a small
roadside billboard" with no such clause, the model returned one reading **SEGA
OUTRUN** with a red Ferrari on it — a real trademark, unprompted, one paste from
being committed.

If the reference is art from a commercial game, say what it is for and what it is
not: "The reference image is a STYLE reference only … Do NOT copy the character
in the reference and do NOT depict any character from any existing game — invent
an original." That produced an original hero in the reference's idiom, which is
the thing you actually wanted.

**9. Know what does not survive.** A face does not survive 16×16 — a chibi head
is a third of the body, so the features land inside two pixels and quantise to a
smudge of skin tone. Blue hair, a sword, armour colour and a shield all survive.
Budget the silhouette, not the detail: at this size a character is read by its
outline and its two or three biggest colour blocks.

## Reading `framediff.py`

```
wd0 vs wd1: 66/384 nibbles differ (17%), 3/16 rows identical  <- reads as motion
wu0 vs wu1: 36/384 nibbles differ (9%), 4/16 rows identical
```

The share of the sprite that changed, not the count — the same set padded from
16×16 to 24×16 would otherwise look like it improved. Under 6% is a held pose,
15% and over reads; in between, run it and look. It also prints each sprite's
*drawn* bounds and complains when they disagree, which is rule 5 failing.

## Where the output goes

`--palette own` prints `pal()` lines with the blocks. Paste both, do not retype
either: nibbles are palette *slots*, so one transposed character is a legal
sprite in the wrong colour. A bank `n` means the ROM now owns palette indices
`n*16+1 .. n*16+15` and nothing else in the game may use them —
`games/outrun.lua` is the worked example, and it picks every road, grass and sky
colour from outside 16–47 for exactly that reason.

Look at `--preview` every time. Quantisation failures do not error; they look
slightly wrong.
