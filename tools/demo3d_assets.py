#!/usr/bin/env python3
"""Generate games/demo3d/{textures,meshes}.lua from tang-verilog's demo3d.py.

The four scenes of tang-verilog's G5 demo (tools/harness/demo3d.py there) —
logo, tunnel, room, city — rebuilt for Kessel3D. That demo computes textures
and geometry in Python floats; a ROM cannot, so what is fixed in the demo is
generated here once and checked in:

- **Textures**: the demo's own `tex_pixel` procedures, sampled and quantised
  to 15 colours each (Kessel's textures are 4bpp sprites through a CLUT, and
  nibble 0 is transparent). Each texture gets its own sprite bank.
- **Meshes**: the logo (KESSEL where the demo has TANG) extruded from the demo's 5x7 font, tunnel segments,
  the room, the ball, the city's buildings in eight heights, and the small
  pieces (light rings, bands, lamps, shadows, sky quads).
- **The city grid**: which block holds which building, from the demo's hash.

What moves — cameras, the tunnel's curve, the bouncing ball, fades — is
computed per frame in games/demo3d.lua, in fixed point.

Standard library only:

    python3 tools/demo3d_assets.py          # rewrites games/demo3d/*.lua
"""
import math
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "games", "demo3d")

U = 64  # world units per demo unit


# ---- the demo's texture procedures (verbatim but for the 565 packing) ----

def _hash(x, y, s=0):
    h = (x * 374761393 + y * 668265263 + s * 2246822519) & 0xFFFFFFFF
    h = ((h ^ (h >> 13)) * 1274126177) & 0xFFFFFFFF
    return (h ^ (h >> 16)) & 0xFFFF


def _c(r, g, b):
    return (max(0, min(255, int(r))), max(0, min(255, int(g))), max(0, min(255, int(b))))


def tex_pixel(name, u, v):
    if name == "stone":
        row = v // 16
        off = 8 if row % 2 else 0
        bx = (u + off) // 16
        edge = (u + off) % 16 == 0 or v % 16 == 0
        n = _hash(bx, row) % 40 + _hash(u, v, 1) % 24
        if edge:
            return _c(40, 34, 30)
        return _c(120 + n, 105 + n, 90 + n // 2)
    if name in ("env", "gold"):
        dx, dy = u - 31.5, v - 31.5
        r2 = (dx * dx + dy * dy) / (32 * 32)
        if v < 30:
            t = v / 30
            r, g_, b = 60 + 180 * t, 90 + 150 * t, 200 + 55 * t
            if (u // 8 + v // 10) % 5 == 0 and v > 10:
                r, g_, b = 255, 250, 220
        elif v < 34:
            r, g_, b = 255, 255, 255
        else:
            c = 90 if ((u // 8) + (v // 8)) % 2 else 40
            r, g_, b = c, c, c + 10
        k = 1.0 - 0.35 * r2
        if name == "gold":
            return _c(min(255, r * k), min(255, g_ * k * 0.8), min(255, b * k * 0.35))
        return _c(min(255, r * k), min(255, g_ * k), min(255, b * k))
    if name == "floor":
        a = ((u // 32) + (v // 32)) % 2
        n = _hash(u // 2, v // 2, 3) % 20
        vein = 30 if (u + 2 * v + _hash(u // 8, v // 8, 4) % 6) % 23 == 0 else 0
        if a:
            return _c(220 - n - vein, 215 - n - vein, 205 - n - vein)
        return _c(50 + n + vein, 55 + n + vein, 70 + n + vein)
    if name == "wall":
        x = u % 32
        if x < 2 or v % 64 < 2:
            return _c(60, 40, 30)
        n = _hash(u // 4, v, 5) % 16
        return _c(150 + n, 110 + n, 80 + n // 2) if x < 30 else _c(90, 64, 45)
    if name == "window":
        wx, wy = u // 16, v // 16
        x, y = u % 16, v % 16
        if 3 <= x < 13 and 4 <= y < 13:
            if _hash(wx, wy, 7) % 3:
                k = 200 + _hash(wx, wy, 8) % 55
                return _c(k, k * 0.85, k * 0.45)
            return _c(20, 25, 40)
        return _c(70, 75, 90)
    if name == "ground":
        if u % 64 < 2:
            return _c(200, 200, 160)
        n = _hash(u, v, 9) % 24
        return _c(40 + n, 40 + n, 46 + n)
    raise ValueError(name)


def gradient(top, bottom):
    def px(u, v):
        t = v / 63
        return _c(*(a + (b - a) * t for a, b in zip(top, bottom)))
    return px


# name -> (bank, w, h, pixel(u, v) at the texture's own size)
TEXTURES = {
    "stone": (1, 64, 64, lambda u, v: tex_pixel("stone", u, v)),
    "gold": (2, 32, 32, lambda u, v: tex_pixel("gold", 2 * u, 2 * v)),
    "env": (3, 32, 32, lambda u, v: tex_pixel("env", 2 * u, 2 * v)),
    "marble": (4, 64, 64, lambda u, v: tex_pixel("floor", u, v)),
    "panels": (5, 64, 64, lambda u, v: tex_pixel("wall", u, v)),
    "windows": (6, 64, 64, lambda u, v: tex_pixel("window", u, v)),
    "asphalt": (7, 64, 64, lambda u, v: tex_pixel("ground", u, v)),
    "dusk": (8, 8, 64, gradient((10, 10, 40), (120, 40, 90))),
    "night": (9, 8, 64, gradient((5, 5, 25), (40, 30, 70))),
}

# Bank 0: the untextured colours a mesh's `color` names.
FLAT = {
    1: ((255, 255, 255), "white: lights, tinted per draw by ambient3d"),
    2: ((30, 30, 40), "the room's ceiling"),
    3: ((255, 250, 220), "the ceiling's light panel"),
    4: ((68, 67, 74), "roofs (the demo's 120,118,130 under the city's light)"),
    5: ((220, 160, 60), "street lamps"),
    6: ((4, 4, 6), "the dark round the room's floor"),
}


def quantise(pixels, n=15, rounds=12):
    """k-means, deterministic: start from luminance quantiles."""
    uniq = sorted(set(pixels), key=lambda c: (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2], c))
    if len(uniq) <= n:
        centres = list(uniq)
    else:
        centres = [uniq[(2 * i + 1) * len(uniq) // (2 * n)] for i in range(n)]
    for _ in range(rounds):
        sums = [[0, 0, 0, 0] for _ in centres]
        for c in pixels:
            i = min(range(len(centres)), key=lambda i: sum((a - b) ** 2 for a, b in zip(c, centres[i])))
            s = sums[i]
            s[0] += c[0]; s[1] += c[1]; s[2] += c[2]; s[3] += 1
        centres = [(s[0] // s[3], s[1] // s[3], s[2] // s[3]) if s[3] else centres[i] for i, s in enumerate(sums)]
    centres.sort(key=lambda c: 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2])
    index = [min(range(len(centres)), key=lambda i: sum((a - b) ** 2 for a, b in zip(c, centres[i]))) for c in pixels]
    return centres, index


def textures_lua():
    out = ["-- Generated by tools/demo3d_assets.py from tang-verilog's demo3d.py. Do not edit.",
           "--",
           "-- Each texture is one of that demo's procedures, quantised to 15 colours",
           "-- in a sprite bank of its own (nibble 0 stays transparent).", ""]
    pal = []
    for name, (bank, w, h, px) in TEXTURES.items():
        pixels = [px(u, v) for v in range(h) for u in range(w)]
        centres, index = quantise(pixels)
        out.append(f"sprite {name} {{")
        for v in range(h):
            out.append("  " + "".join("123456789abcdef"[index[v * w + u]] for u in range(w)))
        out.append("}")
        out.append("")
        for i, (r, g, b) in enumerate(centres):
            pal.append(f"  pal({bank * 16 + i + 1}, {r}, {g}, {b})")
        pal.append("")
    out.append("-- The palette: bank 0 for untextured faces, then one bank per texture.")
    out.append("function demo_palette()")
    out.append("  pal(0, 0, 0, 0)")
    for i, ((r, g, b), why) in FLAT.items():
        out.append(f"  pal({i}, {r}, {g}, {b})   -- {why}")
    out.append("")
    out += pal
    out.append("end")
    return "\n".join(out) + "\n"


# ---- meshes ----

def sub(a, b):
    return tuple(x - y for x, y in zip(a, b))


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def newell(pts):
    n = [0.0, 0.0, 0.0]
    for i in range(len(pts)):
        p, q = pts[i], pts[(i + 1) % len(pts)]
        n[0] += (p[1] - q[1]) * (p[2] + q[2])
        n[1] += (p[2] - q[2]) * (p[0] + q[0])
        n[2] += (p[0] - q[0]) * (p[1] + q[1])
    return n


class Mesh:
    """Vertices and texture coordinates de-duplicated; faces oriented so that
    `outward` is their front (Kessel: counter-clockwise from the front)."""

    def __init__(self, name, comment):
        self.name, self.comment = name, comment
        self.v, self.vi, self.t, self.ti = [], {}, [], {}
        self.lines = []

    def state(self, line):
        self.lines.append(line)

    def _v(self, p):
        p = tuple(int(round(c)) for c in p)
        if p not in self.vi:
            self.vi[p] = len(self.v) + 1
            self.v.append(p)
        return self.vi[p]

    def _t(self, uv):
        uv = tuple(round(c * 64) / 64 for c in uv)
        if uv not in self.ti:
            self.ti[uv] = len(self.t) + 1
            self.t.append(uv)
        return self.ti[uv]

    def face(self, pts, outward, uvs=None):
        if dot(newell(pts), outward) > 0:
            pts = pts[::-1]
            uvs = uvs[::-1] if uvs else None
        idx = [self._v(p) for p in pts]
        if uvs:
            self.lines.append("f " + " ".join(f"{i}/{self._t(t)}" for i, t in zip(idx, uvs)))
        else:
            self.lines.append("f " + " ".join(str(i) for i in idx))

    def lua(self):
        def num(x):
            return str(int(x)) if x == int(x) else f"{x:g}"
        out = [f"-- {self.comment}", f"mesh {self.name} {{"]
        out += [f"  v {x} {y} {z}" for x, y, z in self.v]
        out += [f"  vt {num(u)} {num(v)}" for u, v in self.t]
        out += ["  " + line for line in self.lines]
        out.append("}")
        return "\n".join(out)


FONT = {
    "T": ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."],
    "A": [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    "N": ["#...#", "##..#", "#.#.#", "#.#.#", "#..##", "#...#", "#...#"],
    "G": [".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".###."],
    "K": ["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"],
    "E": ["#####", "#....", "#....", "####.", "#....", "#....", "#####"],
    "S": [".####", "#....", "#....", ".###.", "....#", "....#", "####."],
    "L": ["#....", "#....", "#....", "#....", "#....", "#....", "#####"],
}


def logo(text="KESSEL", cell=40):
    """The demo's logo_quads (which spelt TANG), at `cell` units a cell — six
    letters at the demo's 64 would not fit its camera. Every face takes the gold
    environment texture by its (x, y): the top of a letter is sky, the middle
    the bright horizon, the bottom the floor — chrome, without env mapping."""
    m = Mesh("logo", f"{text}, extruded from the demo's 5x7 font: {cell} units a cell, {round(1.2 * cell)} deep")
    m.state("tex gold 2")
    cells = set()
    for i, ch in enumerate(text):
        for y, row in enumerate(FONT[ch]):
            for x, c in enumerate(row):
                if c == "#":
                    cells.add((i * 6 + x, y))
    w = len(text) * 6 - 1
    ox, oy, d = w / 2, 3.5, 0.6

    def P(x, y, z):
        return ((x - ox) * cell, (oy - y) * cell, z * cell)

    def T(x, y):
        return (min(31.9, x * 32 / w), min(31.9, y * 32 / 7))

    quads = []
    for y in range(7):
        x = 0
        while x < w:
            if (x, y) in cells:
                x1 = x
                while (x1 + 1, y) in cells:
                    x1 += 1
                c = [(x, y + 1), (x1 + 1, y + 1), (x1 + 1, y), (x, y)]
                quads.append(([P(a, b, -d) for a, b in c], (0, 0, -1), [T(a, b) for a, b in c]))
                quads.append(([P(a, b, d) for a, b in c], (0, 0, 1), [T(a, b) for a, b in c]))
                x = x1 + 1
            else:
                x += 1
    for (x, y) in sorted(cells):
        sides = []
        if (x - 1, y) not in cells:
            sides.append(([(x, y + 1, d), (x, y + 1, -d), (x, y, -d), (x, y, d)], (-1, 0, 0)))
        if (x + 1, y) not in cells:
            sides.append(([(x + 1, y + 1, -d), (x + 1, y + 1, d), (x + 1, y, d), (x + 1, y, -d)], (1, 0, 0)))
        if (x, y - 1) not in cells:
            sides.append(([(x, y, -d), (x + 1, y, -d), (x + 1, y, d), (x, y, d)], (0, 1, 0)))
        if (x, y + 1) not in cells:
            sides.append(([(x, y + 1, d), (x + 1, y + 1, d), (x + 1, y + 1, -d), (x, y + 1, -d)], (0, -1, 0)))
        for c, n in sides:
            quads.append(([P(*p) for p in c], n, [T(p[0], p[1]) for p in c]))
    for pts, n, uvs in quads:
        m.face(pts, n, uvs)
    return m


R_TUNNEL = 2.2 * U
SIDES = 10


def tunnel_segment(phase):
    """One ring-to-ring piece of the tunnel, facing in. 64 units long plus 8
    of overlap, so the bend between two pieces leaves no crack. `phase` is the
    demo's `s % 4`: which quarter of the stone the piece shows."""
    m = Mesh(f"seg{phase}", f"tunnel piece, stone rows {phase * 16}..{phase * 16 + 16}")
    m.state("tex stone 1")
    v0, v1 = phase * 16.0, phase * 16.0 + 18.0
    for j in range(SIDES):
        a0, a1 = 2 * math.pi * j / SIDES, 2 * math.pi * (j + 1) / SIDES
        am = (a0 + a1) / 2
        p = [(R_TUNNEL * math.cos(a0), R_TUNNEL * math.sin(a0), 0),
             (R_TUNNEL * math.cos(a1), R_TUNNEL * math.sin(a1), 0),
             (R_TUNNEL * math.cos(a1), R_TUNNEL * math.sin(a1), 72),
             (R_TUNNEL * math.cos(a0), R_TUNNEL * math.sin(a0), 72)]
        uv = [(j * 16.0, v0), (j * 16.0 + 16, v0), (j * 16.0 + 16, v1), (j * 16.0, v1)]
        m.face(p, (-math.cos(am), -math.sin(am), 0), uv)
    return m


def light_ring():
    m = Mesh("halo", "the tunnel's ring of light: additive, tinted per draw")
    m.state("color 1")
    m.state("double")
    m.state("blend add")
    ro, ri = R_TUNNEL * 0.97, R_TUNNEL * 0.75
    for j in range(SIDES):
        a0, a1 = 2 * math.pi * j / SIDES, 2 * math.pi * (j + 1) / SIDES
        m.face([(ro * math.cos(a0), ro * math.sin(a0), 0), (ro * math.cos(a1), ro * math.sin(a1), 0),
                (ri * math.cos(a1), ri * math.sin(a1), 0), (ri * math.cos(a0), ri * math.sin(a0), 0)], (0, 0, -1))
    return m


ROOM, HY = 6.0 * U, 5.0 * U


def room_walls():
    m = Mesh("walls", "the room's four walls, facing in, two panels each")
    m.state("tex panels 5")
    walls = [([(-ROOM, 0, ROOM), (ROOM, 0, ROOM)], (0, 0, -1)),
             ([(ROOM, 0, -ROOM), (-ROOM, 0, -ROOM)], (0, 0, 1)),
             ([(-ROOM, 0, -ROOM), (-ROOM, 0, ROOM)], (1, 0, 0)),
             ([(ROOM, 0, ROOM), (ROOM, 0, -ROOM)], (-1, 0, 0))]
    for ((x0, _, z0), (x1, _, z1)), n in walls:
        for s in range(2):
            f0, f1 = s / 2, (s + 1) / 2
            p0 = (x0 + (x1 - x0) * f0, 0, z0 + (z1 - z0) * f0)
            p1 = (x0 + (x1 - x0) * f1, 0, z0 + (z1 - z0) * f1)
            q = [p0, p1, (p1[0], HY, p1[2]), (p0[0], HY, p0[2])]
            m.face(q, n, [(s * 64.0, 63.9), (s * 64.0 + 64, 63.9), (s * 64.0 + 64, 0.0), (s * 64.0, 0.0)])
    return m


def room_ceiling():
    m = Mesh("ceiling", "the ceiling and its light panel, facing down")
    m.state("color 2")
    m.face([(-ROOM, HY, ROOM), (ROOM, HY, ROOM), (ROOM, HY, -ROOM), (-ROOM, HY, -ROOM)], (0, -1, 0))
    m.state("color 3")
    p = 1.5 * U
    m.face([(-p, HY - 1, p), (p, HY - 1, p), (p, HY - 1, -p), (-p, HY - 1, -p)], (0, -1, 0))
    return m


def room_floor():
    m = Mesh("floor", "the marble floor: averaged over the room's reflection")
    m.state("tex marble 4")
    m.state("blend avg")
    n4 = 4
    for i in range(n4):
        for j in range(n4):
            x0, x1 = -ROOM + 2 * ROOM * i / n4, -ROOM + 2 * ROOM * (i + 1) / n4
            z0, z1 = -ROOM + 2 * ROOM * j / n4, -ROOM + 2 * ROOM * (j + 1) / n4
            q = [(x0, 0, z0), (x0, 0, z1), (x1, 0, z1), (x1, 0, z0)]
            uv = [(i * 48.0, j * 48.0), (i * 48.0, j * 48.0 + 48), (i * 48.0 + 48, j * 48.0 + 48), (i * 48.0 + 48, j * 48.0)]
            m.face(q, (0, 1, 0), uv)
    return m


def room_skirt():
    """Opaque dark ground round the floor, so the reflection under it cannot
    be seen past the floor's edge from outside the room."""
    m = Mesh("skirt", "dark ground round the room, hiding the reflection's edges")
    m.state("color 6")
    o, r = 30 * U, ROOM
    for q in ([(-o, 0, -o), (o, 0, -o), (o, 0, -r), (-o, 0, -r)],
              [(-o, 0, r), (o, 0, r), (o, 0, o), (-o, 0, o)],
              [(-o, 0, -r), (-r, 0, -r), (-r, 0, r), (-o, 0, r)],
              [(r, 0, -r), (o, 0, -r), (o, 0, r), (r, 0, r)]):
        m.face(q, (0, 1, 0))
    return m


def ball(nlat=6, nlon=10, r=1.1 * U):
    m = Mesh("ball", "the mirror ball: smooth, the environment texture by latitude")
    m.state("tex env 3")
    m.state("smooth")

    def P(i, j):
        a = math.pi * i / nlat - math.pi / 2
        b = 2 * math.pi * j / nlon
        if i in (0, nlat):
            return (0, r * math.sin(a), 0)
        return (r * math.cos(a) * math.cos(b), r * math.sin(a), r * math.cos(a) * math.sin(b))

    def T(i, j):
        return (j * 6.4, min(31.9, 32 * (1 - i / nlat)))

    for i in range(nlat):
        for j in range(nlon):
            q = [(i, j), (i, j + 1), (i + 1, j + 1), (i + 1, j)]
            if i == 0:
                q = [q[0], q[2], q[3]]
            elif i == nlat - 1:
                q = [q[0], q[1], q[2]]
            pts = [P(*c) for c in q]
            ctr = tuple(sum(p[k] for p in pts) / len(pts) for k in range(3))
            m.face(pts, ctr, [T(*c) for c in q])
    return m


def shadow(k, r):
    m = Mesh(f"shadow{k}", f"the ball's shadow, radius {r}: subtractive")
    m.state("color 1")
    m.state("blend sub")
    pts = [(r * math.cos(2 * math.pi * i / 10), 1, r * math.sin(2 * math.pi * i / 10)) for i in range(10)]
    for i in range(10):
        m.face([(0, 1, 0), pts[i], pts[(i + 1) % 10]], (0, 1, 0))
    return m


def sky(name, tex):
    """A quad 3,200 units ahead of the eye, drawn with the camera's own
    position and angles so it sits in view space: the demo's `sky()`."""
    m = Mesh(name, f"the {tex} sky: drawn at the eye, with the camera's angles")
    m.state(f"tex {tex} {TEXTURES[tex][0]}")
    hw, hh, d = 2600, 1950, 3200
    m.face([(-hw, -hh, d), (hw, -hh, d), (hw, hh, d), (-hw, hh, d)], (0, 0, -1),
           [(0.0, 63.9), (7.9, 63.9), (7.9, 0.0), (0.0, 0.0)])
    return m


def band():
    m = Mesh("band", "a band of light under the logo: additive, tinted per draw")
    m.state("color 1")
    m.state("double")
    m.state("blend add")
    m.face([(-14 * U, 0, 0), (14 * U, 0, 0), (14 * U, 0, 96), (-14 * U, 0, 96)], (0, 1, 0))
    return m


def road():
    m = Mesh("road", "a row of seven road blocks, 128 units square")
    m.state("tex asphalt 7")
    for i in range(7):
        x0, z0 = (i * 2.0 - 1) * U, 0
        q = [(x0, 0, z0), (x0, 0, z0 + 128), (x0 + 128, 0, z0 + 128), (x0 + 128, 0, z0)]
        m.face(q, (0, 1, 0), [(0.0, 0.0), (0.0, 32.0), (32.0, 32.0), (32.0, 0.0)])
    return m


HEIGHTS = [2, 3, 4, 6, 8, 10, 12, 14]


def building(k, h):
    m = Mesh(f"tower{k + 1}", f"a {h}-unit building ({h * U} high), windows on four sides")
    m.state("tex windows 6")
    hw = 0.8 * U
    top = h * U
    segs = max(1, min(3, h // 4))
    us = 1.6 * 24
    faces = [([(-hw, -hw), (hw, -hw)], (0, 0, -1)), ([(hw, hw), (-hw, hw)], (0, 0, 1)),
             ([(-hw, hw), (-hw, -hw)], (-1, 0, 0)), ([(hw, -hw), (hw, hw)], (1, 0, 0))]
    for ((xa, za), (xb, zb)), n in faces:
        for s in range(segs):
            y0, y1 = top * s / segs, top * (s + 1) / segs
            q = [(xa, y0, za), (xb, y0, zb), (xb, y1, zb), (xa, y1, za)]
            v0, v1 = h * 24 * s / segs, h * 24 * (s + 1) / segs
            m.face(q, n, [(0.0, v0), (us, v0), (us, v1), (0.0, v1)])
    m.state("color 4")
    m.face([(-hw, top, -hw), (hw, top, -hw), (hw, top, hw), (-hw, top, hw)], (0, 1, 0))
    return m


def lamp():
    m = Mesh("lamp", "a street lamp's pool of light: additive")
    m.state("color 5")
    m.state("double")
    m.state("blend add")
    p = 38
    m.face([(-p, 1, -p), (p, 1, -p), (p, 1, p), (-p, 1, p)], (0, 1, 0))
    return m


def city_grid():
    """Rows j = 0..31, columns i = -1..7: 0 for a road, else 1 + the tower."""
    rows = []
    for j in range(32):
        row = ""
        for i in range(-1, 8):
            if i % 3 == 0 or j % 4 == 0:
                row += "."
                continue
            h = 2.0 + (_hash(i, j, 11) % 100) / 100 * 7.0
            if _hash(i, j, 12) % 5 == 0:
                h += 6.0
            k = min(range(len(HEIGHTS)), key=lambda k: abs(HEIGHTS[k] - h))
            row += str(k + 1)
        rows.append(row)
    return rows


def meshes_lua():
    out = ["-- Generated by tools/demo3d_assets.py from tang-verilog's demo3d.py. Do not edit.",
           "--",
           f"-- {U} world units to one of that demo's units.", ""]
    ms = [logo(), band(), sky("sky_dusk", "dusk"), sky("sky_night", "night")]
    ms += [tunnel_segment(p) for p in range(4)] + [light_ring()]
    ms += [room_walls(), room_ceiling(), room_floor(), room_skirt(), ball()]
    ms += [shadow(k, round(1.1 * U * (1 - 0.3 * k / 3))) for k in range(4)]
    ms += [road(), lamp()] + [building(k, h) for k, h in enumerate(HEIGHTS)]
    for m in ms:
        out.append(m.lua())
        out.append("")
    out.append("-- The city: a row per 128 units of z, columns x = -1..7 blocks.")
    out.append("-- '.' is road; a digit is tower1..tower8, from the demo's own hash.")
    out.append("data city {")
    out += ["  " + r for r in city_grid()]
    out.append("}")
    return "\n".join(out) + "\n"


def main():
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, "textures.lua"), "w") as f:
        f.write(textures_lua())
    with open(os.path.join(OUT, "meshes.lua"), "w") as f:
        f.write(meshes_lua())
    print("wrote games/demo3d/textures.lua, games/demo3d/meshes.lua")


if __name__ == "__main__":
    main()
