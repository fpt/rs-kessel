"""Turn generated PNGs into luax `sprite` declarations.

The console's sprites are 4bpp: a pixel names a nibble 0-15, where nibble 0 is
transparent in every bank. So art arrives here as RGBA images and has to leave as
at most 15 colours on a grid whose sides are multiples of 8 — one declaration
per image, since a sprite's body states its own size and the compiler slices it.

Two ways to land those 15 colours:

  --palette pico   quantise to the console's default 16 (BASE_16, indices 0-15),
                   so the sprite draws through bank 0 like every other one.
  --palette own    keep the art's own 15 most common colours and emit the `pal()`
                   calls that install them in a bank. The sprites then have to be
                   drawn between sprbank(n) and sprbank(0), and the ROM must not
                   use palette indices n*16+1 .. n*16+15 for anything else.

Several images may be passed at once, and with --palette own they share ONE
palette — which is the point of a bank. Fifteen colours across a whole set of
scenery is the console's actual constraint; quantising each object alone gives
each its own fifteen and there is nowhere to put them.

Usage:
  uv run --project tools tools/spritegen.py car.png \
      --names car --tiles 4x4 --palette own --bank 1 --out sprites.txt

  uv run --project tools tools/spritegen.py bldg.png palm.png tree.png sign.png \
      --names bldg,palm,tree,sign --tiles 2x2 --palette own --bank 2

Downscaling averages over the source block (premultiplied by alpha) rather than
point-sampling: a 400px "pixel art" render has soft edges and antialiased
outlines, and picking one source pixel per destination pixel drops the outline
on half the silhouette.
"""

from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np
from PIL import Image

# The 16 colours a ROM gets without touching the palette — crates/vm/src/device.rs
BASE_16 = [
    (0x00, 0x00, 0x00), (0x1D, 0x2B, 0x53), (0x7E, 0x25, 0x53), (0x00, 0x87, 0x51),
    (0xAB, 0x52, 0x36), (0x5F, 0x57, 0x4F), (0xC2, 0xC3, 0xC7), (0xFF, 0xF1, 0xE8),
    (0xFF, 0x00, 0x4D), (0xFF, 0xA3, 0x00), (0xFF, 0xEC, 0x27), (0x00, 0xE4, 0x36),
    (0x29, 0xAD, 0xFF), (0x83, 0x76, 0x9C), (0xFF, 0x77, 0xA8), (0xFF, 0xCC, 0xAA),
]
NIBBLES = "0123456789abcdef"


def load(path: Path, key: tuple[int, int, int] | None, key_dist: float) -> np.ndarray:
    """RGBA float array, shape (h, w, 4), channels 0..255.

    Anything still close to the chroma key is cleared here. imgen's flood fill
    removes the background but leaves its antialiased fringe a pixel or two deep,
    and a fringe survives quantisation as a *palette entry* — two of fifteen
    slots spent on magenta, and a magenta pixel in the middle of a white number
    plate where the average landed between the two.
    """
    img = np.asarray(Image.open(path).convert("RGBA"), dtype=np.float64).copy()
    if key is not None:
        d = np.sqrt(((img[:, :, :3] - np.asarray(key, dtype=np.float64)) ** 2).sum(2))
        img[d < key_dist, 3] = 0.0
    return img


def fit_scale(sw: int, sh: int, bw: int, bh: int) -> float:
    """The largest factor that fits a sw*sh image inside the bw*bh block.

    Aspect is kept because a car is wider than it is tall and a tower is not:
    stretching either to fill the block undoes the reason for drawing it at this
    size in the first place.
    """
    return min(bw / sw, bh / sh)


def fit(sw: int, sh: int, s: float) -> tuple[int, int]:
    return max(1, round(sw * s)), max(1, round(sh * s))


def downscale(img: np.ndarray, w: int, h: int, alpha_cut: float) -> tuple[np.ndarray, np.ndarray]:
    """Area-average `img` to w*h. Returns (rgb, opaque-mask)."""
    sh, sw = img.shape[:2]
    rgb = np.zeros((h, w, 3))
    keep = np.zeros((h, w), dtype=bool)
    ys = (np.arange(h + 1) * sh / h).round().astype(int)
    xs = (np.arange(w + 1) * sw / w).round().astype(int)
    for y in range(h):
        for x in range(w):
            block = img[ys[y]:max(ys[y + 1], ys[y] + 1), xs[x]:max(xs[x + 1], xs[x] + 1)]
            a = block[:, :, 3]
            total = a.sum()
            keep[y, x] = a.mean() / 255.0 >= alpha_cut
            if total > 0:
                # premultiplied mean: a half-transparent fringe must not drag the
                # colour towards black
                rgb[y, x] = (block[:, :, :3] * a[:, :, None]).sum((0, 1)) / total
    return rgb, keep


def key_tinted(c: tuple[int, int, int], key: tuple[int, int, int], bias: float) -> bool:
    """Is `c` the chroma key blended with something, rather than real art?

    Distance to the key cannot answer this: a dark magenta outline fringe sits
    ~210 away from #FF00FF and so does white, so any threshold that rejects the
    fringe also rejects the billboard. What separates them is the key's *hue* —
    every channel the key maxes out is well above every channel it zeroes. White
    and grey have no such gap, and a genuinely purple colour has a small one.

    The gap has to be read two ways. An absolute `bias` catches a bright fringe,
    but a *dark* one — #200131, where a black outline met the key — has a gap of
    only 31 and sails through, then spends a palette slot and turns every outline
    in the set purple. Relative to how dark the colour is, though, that gap is
    everything it is made of, so a colour whose key-side channels are well over
    half again its other side is a tint however dark it is.
    """
    hi = [c[i] for i in range(3) if key[i] >= 128]
    lo = [c[i] for i in range(3) if key[i] < 128]
    if not hi or not lo:
        return False
    gap = min(hi) - max(lo)
    return gap > bias or gap > max(lo) * 0.6


def own_palette(images: list[np.ndarray], n: int, key: tuple[int, int, int] | None,
                bias: float, buckets: int = 6) -> list[tuple[int, int, int]]:
    """The `n` most common colours across all the opaque art, near-duplicates merged.

    Coarse buckets first (the generated art is nominally flat but carries a
    gradient in every panel), then the mean of each bucket's members — so the
    reported colour is one the art actually contains, not a bucket centre.
    """
    px = np.concatenate([im.reshape(-1, 4) for im in images])
    px = px[px[:, 3] > 200][:, :3]
    if len(px) == 0:
        raise SystemExit("spritegen: the art is fully transparent")
    bucket = (px // (256 / buckets)).astype(int)
    flat = bucket[:, 0] * buckets * buckets + bucket[:, 1] * buckets + bucket[:, 2]
    ids, counts = np.unique(flat, return_counts=True)
    out = []
    for i in np.argsort(-counts):
        if len(out) == n:
            break
        c = tuple(int(round(v)) for v in px[flat == ids[i]].mean(0))
        if key is not None and key_tinted(c, key, bias):
            continue
        out.append(c)
    return out


def quantise(rgb: np.ndarray, keep: np.ndarray,
             palette: list[tuple[int, int, int]]) -> list[str]:
    """Map every kept pixel to its nearest palette entry, as nibble rows.

    The palette always starts at nibble 1: nibble 0 is transparent in every bank,
    so a sprite cannot name a first colour even when one exists at that index.
    """
    pal = np.asarray(palette, dtype=np.float64)
    h, w = keep.shape
    rows = []
    for y in range(h):
        row = []
        for x in range(w):
            if not keep[y, x]:
                row.append(".")
            else:
                row.append(NIBBLES[1 + int(((pal - rgb[y, x]) ** 2).sum(1).argmin())])
        rows.append("".join(row))
    return rows


def block(art: list[str], bw: int, bh: int, align: str) -> list[str]:
    """Centre the art horizontally in a bw*bh grid, aligned vertically."""
    w, h = len(art[0]), len(art)
    pad = (bw - w) // 2
    top = {"bottom": bh - h, "top": 0, "middle": (bh - h) // 2}[align]
    grid = ["." * bw for _ in range(top)]
    grid += ["." * pad + r + "." * (bw - w - pad) for r in art]
    return grid + ["." * bw for _ in range(bh - top - h)]


def emit(name: str, grid: list[str]) -> str:
    """One `sprite` declaration at the block's full size.

    A declaration carries its own size — rows are the height, characters the
    width — so the compiler does the slicing and the source stays a picture of
    the sprite. It also means the padding `block` adds is load-bearing: past one
    tile the grid must be exact in both dimensions, because a short row shifts
    every tile after it and every id after that.
    """
    return f"sprite {name} {{\n" + "\n".join("  " + r for r in grid) + "\n}"


def emit_pal(palette: list[tuple[int, int, int]], bank: int) -> str:
    lines = [f"  -- sprite bank {bank}: nibble n draws as {bank * 16} + n"]
    for i, (r, g, b) in enumerate(palette):
        lines.append(f"  pal({bank * 16 + 1 + i}, {r}, {g}, {b})")
    return "\n".join(lines)


def preview(path: Path, grids: list[list[str]], palette: list[tuple[int, int, int]],
            zoom: int) -> None:
    """All objects side by side, transparency as a checkerboard."""
    h = max(len(g) for g in grids)
    w = sum(len(g[0]) + 2 for g in grids)
    out = Image.new("RGB", (w * zoom, h * zoom))
    px = out.load()
    cells: list[list[str]] = [["." * w] for _ in range(h)]
    rows = []
    for y in range(h):
        line = ""
        for g in grids:
            line += (g[y] if y < len(g) else "." * len(g[0])) + ".."
        rows.append(line)
    del cells
    for y in range(h * zoom):
        for x in range(w * zoom):
            ch = rows[y // zoom][x // zoom]
            if ch in ".0":
                shade = ((x // zoom + y // zoom) // 2) % 2
                px[x, y] = (0x40, 0x40, 0x40) if shade else (0x30, 0x30, 0x30)
            else:
                px[x, y] = palette[NIBBLES.index(ch) - 1]
    out.save(path)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("images", nargs="+", type=Path)
    ap.add_argument("--names", required=True,
                    help="comma-separated sprite name per image, e.g. bldg,palm,tree")
    ap.add_argument("--tiles", default="4x4",
                    help="sprite size in 8x8 tiles, COLSxROWS — e.g. 4x4 for 32x32")
    ap.add_argument("--palette", choices=("pico", "own"), default="own")
    ap.add_argument("--bank", type=int, default=1, help="sprite bank for --palette own")
    ap.add_argument("--uniform", action="store_true",
                    help="scale every image by one factor — for a set that is one "
                         "object seen several ways (steering angles, animation frames)")
    ap.add_argument("--align", choices=("bottom", "top", "middle"), default="bottom",
                    help="where the art sits when it is shorter than the block")
    ap.add_argument("--alpha-cut", type=float, default=0.45)
    ap.add_argument("--key", default="255,0,255",
                    help="chroma key still present as a fringe; 'none' to keep it")
    ap.add_argument("--key-dist", type=float, default=130.0)
    ap.add_argument("--key-bias", type=float, default=40.0,
                    help="reject palette candidates this far towards the key's hue")
    ap.add_argument("--out", type=Path)
    ap.add_argument("--preview", type=Path)
    ap.add_argument("--zoom", type=int, default=8)
    args = ap.parse_args()

    names = [n.strip() for n in args.names.split(",")]
    if len(names) != len(args.images):
        raise SystemExit(f"spritegen: {len(args.images)} images but {len(names)} names")
    cols, rows = (int(v) for v in args.tiles.lower().split("x"))
    bw, bh = cols * 8, rows * 8

    key = None if args.key == "none" else tuple(int(v) for v in args.key.split(","))
    imgs = [load(p, key, args.key_dist) for p in args.images]
    if args.palette == "own":
        palette = own_palette(imgs, 15, key, args.key_bias)
    else:
        palette = BASE_16[1:]

    # One scale for the whole set, when the set is one thing seen several ways.
    # Fitting each image to its own box independently is right for a tree beside
    # a tower and wrong for a car at four steering angles: the drifting car is a
    # wider silhouette than the straight one, so per-image fitting shrinks it to
    # the same width and the car visibly pulses as the player steers.
    scales = [fit_scale(im.shape[1], im.shape[0], bw, bh) for im in imgs]
    grids, sizes = [], []
    for img, own in zip(imgs, scales):
        sh, sw = img.shape[:2]
        w, h = fit(sw, sh, min(scales) if args.uniform else own)
        rgb, keep = downscale(img, w, h, args.alpha_cut)
        grids.append(block(quantise(rgb, keep, palette), bw, bh, args.align))
        sizes.append(f"{w}x{h}")

    text = "\n".join(emit(n, g) for n, g in zip(names, grids))
    if args.palette == "own":
        text += "\n\n-- in init():\n" + emit_pal(palette, args.bank)
    if args.out:
        args.out.write_text(text + "\n")
        print(f"wrote {args.out}")
    else:
        print(text)
    if args.preview:
        preview(args.preview, grids, palette, args.zoom)
        print(f"wrote {args.preview}: {', '.join(f'{n} {s}' for n, s in zip(names, sizes))}"
              f" in {bw}x{bh} blocks")


if __name__ == "__main__":
    main()
