"""Measure whether two sprite frames actually differ, and whether a set lines up.

The two ways an animated sprite goes wrong at console resolution are both
invisible in a still and both obvious in numbers:

* **The frames are the same.** A model asked for "walking" moves a leg by less
  than one console pixel, so the two frames quantise to nearly the same nibbles
  and the animation reads as a held pose. Measured on a real 16×16 sheet: 7 of
  256 nibbles differed. Respun with the delta spelled out in pixels: 73.
* **The frames are not the same size.** If one direction's art is 13 rows tall
  and another's is 16, the hero pops when the player turns. `spritegen
  --uniform` fixes the scale *factor*, not the content bounds — the prompt has
  to do its half, and this is how you find out whether it did.

Reads `sprite NAME { … }` blocks out of anything: `spritegen.py` output, a
`games/*.lua`, a scratch file.

Usage:
  uv run --project tools tools/spritegen.py … > /tmp/set.txt
  uv run --project tools tools/framediff.py /tmp/set.txt --pairs wd0:wd1,wu0:wu1
  uv run --project tools tools/framediff.py games/rogue.lua        # just the sizes
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

BLOCK = re.compile(r"sprite\s+(\w+)\s*\{\n(.*?)\n\s*\}", re.S)

# Thresholds from the sheets that produced this tool, not from theory. They are
# **fractions of the sprite's area**, not counts: the numbers were measured on
# 16×16 bodies (7 of 256 nibbles for a pair that read as a held pose, 73 for one
# that animated) and a count would silently mean something different the moment
# the same set is padded out to 24×16 for a sword that leaves the silhouette.
LIMP = 0.06
READS = 0.15


def blocks(paths: list[Path]) -> dict[str, list[str]]:
    out: dict[str, list[str]] = {}
    for p in paths:
        for name, body in BLOCK.findall(p.read_text()):
            rows = [r.strip() for r in body.strip().split("\n")]
            if name in out and out[name] != rows:
                print(f"framediff: {name} defined twice, differently", file=sys.stderr)
            out[name] = rows
    return out


def bounds(rows: list[str]) -> tuple[int, int]:
    """Width and height of the drawn part — nibble 0 is transparent."""
    ys = [i for i, r in enumerate(rows) if r.strip(".")]
    if not ys:
        return 0, 0
    xs = [x for r in rows for x, c in enumerate(r) if c != "."]
    return max(xs) - min(xs) + 1, ys[-1] - ys[0] + 1


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("files", nargs="+", type=Path)
    ap.add_argument("--pairs", help="comma-separated a:b frame pairs to compare")
    args = ap.parse_args()

    sprites = blocks(args.files)
    if not sprites:
        print("framediff: no sprite blocks found", file=sys.stderr)
        raise SystemExit(1)

    print(f"{len(sprites)} sprite(s):")
    heights = set()
    for name, rows in sprites.items():
        w, h = bounds(rows)
        heights.add(h)
        print(f"  {name:<12} {len(rows[0])}x{len(rows)} declared, {w}x{h} drawn")
    # Only worth saying when the caller has declared these belong together, which
    # `--pairs` is. Over a whole game's sprite list the heights differ because a
    # sword is not a chest, and the note would be noise on every run.
    if args.pairs and len(heights) > 1:
        print(
            f"  NOTE: drawn heights differ ({min(heights)}..{max(heights)}). These are\n"
            "        states of one character, so it will change size as it changes state."
        )

    if not args.pairs:
        return

    print("\nframe pairs:")
    for pair in args.pairs.split(","):
        a, _, b = pair.partition(":")
        a, b = a.strip(), b.strip()
        if a not in sprites or b not in sprites:
            print(f"  {pair}: no such sprite")
            continue
        ra, rb = sprites[a], sprites[b]
        if len(ra) != len(rb) or len(ra[0]) != len(rb[0]):
            print(f"  {a} vs {b}: different declared sizes, not comparable")
            continue
        total = len(ra) * len(ra[0])
        diff = sum(1 for x, y in zip(ra, rb) for i, j in zip(x, y) if i != j)
        same = sum(1 for x, y in zip(ra, rb) if x == y)
        share = diff / total
        verdict = ""
        if share <= LIMP:
            verdict = "  <- too close; this will read as a held pose, not a step"
        elif share >= READS:
            verdict = "  <- reads as motion"
        print(
            f"  {a} vs {b}: {diff}/{total} nibbles differ ({share:.0%}), "
            f"{same}/{len(ra)} rows identical{verdict}"
        )


if __name__ == "__main__":
    main()
