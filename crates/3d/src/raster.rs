//! The 3D GPU in software: execute the command words and produce the picture
//! the board would show.
//!
//! This is a port of tang-verilog's `tools/harness/raster.py` (the drawing
//! rules) and the scene half of `gpu_model.py` (tiles, the per-scene and
//! per-tile limits), and it has to stay one. Those two are "the rule" over
//! there — they produce the RTL's CRC bit for bit — so every integer step
//! below mirrors one of theirs: the 12.4 edge functions with the top-left
//! rule, the 17-bit reciprocal mantissa the attribute gradients are built
//! from, the wrapping 32-bit accumulators, the texel block layout, the
//! modulate and blend equations. A "cleaner" version of any of these is a
//! picture that differs from the board by a pixel somewhere, which nobody
//! would notice until it mattered.
//!
//! Only the commands Kessel sends are executed (`UPLOAD`, `CLUT_WRITE`,
//! `SET_TEX`, `TRI`, `BEGIN_SCENE`/`END_SCENE`, `FILL_RECT`, `SWAP`); anything
//! else is counted as an error, exactly as the GPU raises `GPU_STATUS` bit1.

use alloc::vec;
use alloc::vec::Vec;

use crate::cmd::*;

/// Triangles one scene can hold; the GPU drops the rest.
pub const SCENE_MAX_TRIS: usize = 4096;
/// Triangles one 32×32 tile can hold; the GPU drops the rest *in that tile*.
pub const TILE_MAX_TRIS: usize = 1024;
const TILES_X: usize = 10;
const TILES: usize = 80;

/// Fixed-point fraction bits of the attribute accumulators.
const F: u32 = 12;

fn sat32(v: i64) -> i64 {
    v.clamp(i32::MIN as i64, i32::MAX as i64)
}

/// A vertex as the setup sees it: 12.4 position and the six attributes
/// `r, g, b, z, u, v`.
#[derive(Clone, Copy, Debug, Default)]
struct Vtx {
    x: i64,
    y: i64,
    a: [i64; 6],
}

/// One triangle after setup, ready to walk.
struct Setup {
    bbox: (i64, i64, i64, i64),
    /// Per edge: (value at the first pixel, step per pixel, step per row).
    edges: [(i64, i64, i64); 3],
    /// Per attribute: (accumulator at the first pixel, step per pixel, step per row).
    attrs: [(i32, i32, i32); 6],
}

/// `raster.py`'s `setup`. `None` when nothing would be drawn.
fn setup(vs: [Vtx; 3], clip: (i64, i64, i64, i64)) -> Option<Setup> {
    let [v0, mut v1, mut v2] = vs;
    let mut area2 = (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x);
    if area2 == 0 {
        return None;
    }
    if area2 < 0 {
        core::mem::swap(&mut v1, &mut v2);
        area2 = -area2;
    }
    let (x0, y0, x1, y1, x2, y2) = (v0.x, v0.y, v1.x, v1.y, v2.x, v2.y);
    let (cx0, cy0, cx1, cy1) = clip;
    let bx0 = cx0.max((x0.min(x1).min(x2) + 7) >> 4);
    let bx1 = cx1.min((x0.max(x1).max(x2) - 8) >> 4);
    let by0 = cy0.max((y0.min(y1).min(y2) + 7) >> 4);
    let by1 = cy1.min((y0.max(y1).max(y2) - 8) >> 4);
    if bx0 > bx1 || by0 > by1 {
        return None;
    }
    let (sx, sy) = (bx0 * 16 + 8, by0 * 16 + 8);
    let mut edges = [(0, 0, 0); 3];
    for (k, ((xa, ya), (xb, yb))) in [
        ((x1, y1), (x2, y2)),
        ((x2, y2), (x0, y0)),
        ((x0, y0), (x1, y1)),
    ]
    .into_iter()
    .enumerate()
    {
        let (dx, dy) = (xb - xa, yb - ya);
        let top_left = dy < 0 || (dy == 0 && dx > 0);
        let e = dx * (sy - ya) - dy * (sx - xa) + if top_left { 0 } else { -1 };
        edges[k] = (e, -dy * 16, dx * 16);
    }
    // 17-bit reciprocal mantissa: 2^16 <= m < 2^17.
    let n = 64 - area2.leading_zeros();
    let m = ((1i64 << (n + 16)) - 1) / area2;
    let mut attrs = [(0, 0, 0); 6];
    for (i, slot) in attrs.iter_mut().enumerate() {
        let (a0, a1, a2) = (v0.a[i], v1.a[i], v2.a[i]);
        let (d1, d2) = (a1 - a0, a2 - a0);
        let nx = d1 * (y2 - y0) + d2 * (y0 - y1);
        let ny = d1 * (x0 - x2) + d2 * (x1 - x0);
        let gx = sat32((nx * m) >> n);
        let gy = sat32((ny * m) >> n);
        let s = sat32((a0 << F) + ((gx * (sx - x0) + gy * (sy - y0)) >> 4));
        *slot = (s as i32, gx as i32, gy as i32);
    }
    Some(Setup {
        bbox: (bx0, by0, bx1, by1),
        edges,
        attrs,
    })
}

/// The semi-transparency equations, per RGB565 channel.
fn blend(mode: Blend, b: u16, f: u16) -> u16 {
    let mut out = 0u16;
    for (sh, mx) in [(11u16, 31i32), (5, 63), (0, 31)] {
        let bc = ((b >> sh) as i32) & mx;
        let fc = ((f >> sh) as i32) & mx;
        let c = match mode {
            Blend::Average => (bc + fc) >> 1,
            Blend::Add => mx.min(bc + fc),
            Blend::Subtract => 0.max(bc - fc),
            Blend::Quarter => mx.min(bc + (fc >> 2)),
        };
        out |= (c as u16) << sh;
    }
    out
}

/// A texel modulated by a vertex colour (128 = 1×), per channel, saturating.
fn modulate(t: u16, rgb: [i32; 3]) -> u16 {
    let r = 31.min(((t >> 11) as i32 * rgb[0]) >> 7);
    let g = 63.min((((t >> 5) & 63) as i32 * rgb[1]) >> 7);
    let b = 31.min(((t & 31) as i32 * rgb[2]) >> 7);
    (r << 11 | g << 5 | b) as u16
}

/// Texels per word (log2), per format.
fn tex_shift(fmt: u32) -> u32 {
    match fmt {
        0 => 1,
        1 => 2,
        _ => 3,
    }
}

/// `raster.py`'s `tex_index`: the word, within a texture, holding (tu, tv).
/// Textures are stored as 8-word blocks — the GPU's cache line.
pub fn tex_index(fmt: u32, wl: u32, tu: u32, tv: u32) -> u32 {
    let sh = tex_shift(fmt);
    let lwpr = (fmt == 0) as u32;
    let (lbw, lbh) = (sh + lwpr, 3 - lwpr);
    ((tv >> lbh) << (wl + 3 - lbw))
        | ((tu >> lbw) << 3)
        | ((tv & ((1 << lbh) - 1)) << lwpr)
        | ((tu >> sh) & ((1 << lwpr) - 1))
}

/// What a frame cost and what the GPU had to drop. Mirrors `GPU_STATUS`
/// bit1, but counted — an agent can read "12 triangles lost in tile 34"
/// where the board can only say "error".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuStats {
    /// Triangles recorded into scenes.
    pub tris: u32,
    /// Triangles past the 4,096-per-scene limit, never drawn.
    pub scene_dropped: u32,
    /// (triangle, tile) pairs past the 1,024-per-tile limit.
    pub tile_dropped: u32,
    /// Words that were not a command this model knows.
    pub errors: u32,
}

/// One recorded scene triangle.
struct SceneTri {
    flags: u8,
    color: u16,
    vs: [Vtx; 3],
    tex: TexState,
}

/// The GPU's state as the commands leave it.
#[derive(Clone)]
pub struct SoftGpu {
    /// The surface being drawn, and the one on show.
    draw: Vec<u16>,
    display: Vec<u16>,
    /// A depth per pixel for the scene in flight.
    zbuf: Vec<u16>,
    clut: [u16; 256],
    tex: TexState,
    /// SDRAM, sparsely: only what was uploaded. Reading anything else gives
    /// 0 — a texture that was never uploaded is transparent, not garbage.
    sdram: Vec<(u32, Vec<u32>)>,
    stats: GpuStats,
}

impl Default for SoftGpu {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftGpu {
    pub fn new() -> Self {
        SoftGpu {
            draw: vec![0; W * H],
            display: vec![0; W * H],
            zbuf: vec![0xFFFF; W * H],
            clut: [0; 256],
            tex: TexState {
                fmt: TEX_4BPP,
                clut_off: 0,
                wl: 3,
                hl: 3,
                base: 0,
            },
            sdram: Vec::new(),
            stats: GpuStats::default(),
        }
    }

    /// The surface on show: RGB565, `W × H`, row-major.
    pub fn display(&self) -> &[u16] {
        &self.display
    }

    /// Counters for the last [`run`](Self::run).
    pub fn stats(&self) -> GpuStats {
        self.stats
    }

    fn read(&self, addr: u32) -> u32 {
        for (base, words) in &self.sdram {
            if addr >= *base && ((addr - base) as usize) < words.len() {
                return words[(addr - base) as usize];
            }
        }
        0
    }

    fn write(&mut self, addr: u32, data: &[u32]) {
        // Extend a region this lands in or next to; otherwise start one.
        for (base, words) in self.sdram.iter_mut() {
            let end = *base + words.len() as u32;
            if addr >= *base && addr <= end {
                let at = (addr - *base) as usize;
                if words.len() < at + data.len() {
                    words.resize(at + data.len(), 0);
                }
                words[at..at + data.len()].copy_from_slice(data);
                return;
            }
        }
        self.sdram.push((addr, data.to_vec()));
    }

    fn texel(&self, tex: &TexState, u: i32, v: i32) -> u16 {
        // The spec allows 3..=8; what the board does outside that is not
        // defined, so clamp rather than shift by something absurd.
        let (wl, hl) = ((tex.wl as u32).clamp(3, 8), (tex.hl as u32).clamp(3, 8));
        let tu = ((u >> 18) as u32) & ((1 << wl) - 1);
        let tv = ((v >> 18) as u32) & ((1 << hl) - 1);
        let w = self.read((tex.base | tex_index(tex.fmt, wl, tu, tv)) & 0x1F_FFFF);
        match tex.fmt {
            0 => (w >> (16 * (tu & 1))) as u16,
            1 => {
                self.clut
                    [(((w >> (8 * (tu & 3))) & 0xFF) as u8).wrapping_add(tex.clut_off) as usize]
            }
            _ => {
                self.clut
                    [(((w >> (28 - 4 * (tu & 7))) & 0xF) as u8).wrapping_add(tex.clut_off) as usize]
            }
        }
    }

    /// Execute a frame's words.
    pub fn run(&mut self, words: &[u32]) {
        self.stats = GpuStats::default();
        let mut scene: Option<(u8, u16, Vec<SceneTri>)> = None;
        let mut i = 0;
        while i < words.len() {
            let w0 = words[i];
            let op = w0 >> 24;
            let arg = |k: usize| words.get(i + k).copied().unwrap_or(0);
            match op {
                OP_UPLOAD => {
                    let n = (w0 & 0xFF_FFFF) as usize;
                    let end = (i + 2 + n).min(words.len());
                    let data = words[(i + 2).min(end)..end].to_vec();
                    self.write(arg(1) & 0x1F_FFFF, &data);
                    i += 2 + n;
                }
                OP_CLUT_WRITE => {
                    let index = ((w0 >> 16) & 0xFF) as usize;
                    let n = (w0 & 0x1FF) as usize;
                    for k in 0..n {
                        let word = arg(1 + k / 2);
                        self.clut[(index + k) & 0xFF] = (word >> (16 * (k % 2))) as u16;
                    }
                    i += 1 + n.div_ceil(2);
                }
                OP_SET_TEX => {
                    self.tex = TexState {
                        fmt: (w0 >> 16) & 3,
                        clut_off: (w0 >> 8) as u8,
                        wl: ((w0 >> 4) & 15) as u8,
                        hl: (w0 & 15) as u8,
                        base: arg(1) & 0x1F_FFFF,
                    };
                    i += 2;
                }
                OP_TRI => {
                    let flags = (w0 >> 16) as u8;
                    let len = tri_len(flags);
                    let tri = parse_tri(&words[i..(i + len).min(words.len())], flags, self.tex);
                    match scene.as_mut() {
                        Some((_, _, tris)) => tris.push(tri),
                        // Outside a scene a TRI is a one-triangle scene that
                        // keeps the surface and its Z. Kessel never sends one.
                        None => self.end_scene(0, 0, vec![tri]),
                    }
                    i += len;
                }
                OP_BEGIN_SCENE => {
                    scene = Some(((w0 >> 16) as u8, w0 as u16, Vec::new()));
                    i += 1;
                }
                OP_END_SCENE => {
                    if let Some((flags, color, tris)) = scene.take() {
                        self.end_scene(flags, color, tris);
                    }
                    i += 1;
                }
                OP_FILL_RECT => {
                    let (x, y) = ((arg(1) & 0xFFFF) as usize, (arg(1) >> 16) as usize);
                    let (w, h) = ((arg(2) & 0xFFFF) as usize, (arg(2) >> 16) as usize);
                    for py in y..(y + h).min(H) {
                        for px in x..(x + w).min(W) {
                            self.draw[py * W + px] = w0 as u16;
                        }
                    }
                    i += 3;
                }
                OP_SWAP => {
                    core::mem::swap(&mut self.draw, &mut self.display);
                    i += 1;
                }
                0 => i += 1,
                _ => {
                    self.stats.errors += 1;
                    i += 1;
                }
            }
        }
    }

    /// `gpu_model.py`'s `end_scene`: the scene's triangles in submission
    /// order, each kept only in the tiles that still had room for it.
    fn end_scene(&mut self, flags: u8, color: u16, tris: Vec<SceneTri>) {
        if flags & SCENE_CLEAR != 0 {
            self.draw.fill(color);
        }
        if flags & SCENE_CLEAR_Z != 0 {
            self.zbuf.fill(0xFFFF);
        }
        let full = (0, 0, W as i64 - 1, H as i64 - 1);
        let mut counts = [0usize; TILES];
        self.stats.scene_dropped += tris.len().saturating_sub(SCENE_MAX_TRIS) as u32;
        for tri in tris.iter().take(SCENE_MAX_TRIS) {
            self.stats.tris += 1;
            let Some(rec) = setup(tri.vs, full) else {
                continue;
            };
            let (bx0, by0, bx1, by1) = rec.bbox;
            let mut listed = [false; TILES];
            for ty in (by0 >> 5)..=(by1 >> 5) {
                for tx in (bx0 >> 5)..=(bx1 >> 5) {
                    let t = ty as usize * TILES_X + tx as usize;
                    if counts[t] < TILE_MAX_TRIS {
                        counts[t] += 1;
                        listed[t] = true;
                    } else {
                        self.stats.tile_dropped += 1;
                    }
                }
            }
            self.draw_tri(tri, &rec, &listed);
        }
    }

    fn draw_tri(&mut self, tri: &SceneTri, rec: &Setup, listed: &[bool; TILES]) {
        let (bx0, by0, bx1, by1) = rec.bbox;
        let mut er = rec.edges.map(|(e, _, _)| e);
        let mut ar = rec.attrs.map(|(s, _, _)| s);
        let max = [255, 255, 255, 65535];
        for y in by0..=by1 {
            let mut e = er;
            let mut a = ar;
            for x in bx0..=bx1 {
                let inside = e.iter().all(|&v| v >= 0);
                let tile = (y as usize >> 5) * TILES_X + (x as usize >> 5);
                if inside && listed[tile] {
                    let mut vals = [0i32; 4];
                    for k in 0..4 {
                        vals[k] = (a[k] >> F).clamp(0, max[k]);
                    }
                    self.shade(tri, x as usize, y as usize, vals, a[4], a[5]);
                }
                for k in 0..3 {
                    e[k] += rec.edges[k].1;
                }
                for k in 0..6 {
                    a[k] = a[k].wrapping_add(rec.attrs[k].1);
                }
            }
            for k in 0..3 {
                er[k] += rec.edges[k].2;
            }
            for k in 0..6 {
                ar[k] = ar[k].wrapping_add(rec.attrs[k].2);
            }
        }
    }

    /// `gpu_model.py`'s per-pixel half of `tri`.
    fn shade(&mut self, tri: &SceneTri, x: usize, y: usize, vals: [i32; 4], u: i32, v: i32) {
        let flags = tri.flags;
        let rgb = [vals[0], vals[1], vals[2]];
        let c = if flags & FLAG_TEX != 0 {
            let t = self.texel(&tri.tex, u, v);
            if t == 0 {
                return; // transparent
            }
            if flags & FLAG_GOURAUD != 0 {
                modulate(t, rgb)
            } else {
                t
            }
        } else if flags & FLAG_GOURAUD != 0 {
            to565(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8)
        } else {
            tri.color
        };
        let at = y * W + x;
        if flags & FLAG_Z != 0 {
            let z = vals[3] as u16;
            if z >= self.zbuf[at] {
                return;
            }
            if flags & FLAG_SEMI == 0 {
                self.zbuf[at] = z;
            }
        }
        let c = if flags & FLAG_SEMI != 0 {
            blend(Blend::from_bits(flags >> 4), self.draw[at], c)
        } else {
            c
        };
        self.draw[at] = c;
    }
}

fn s16(w: u32) -> i64 {
    w as u16 as i16 as i64
}

fn parse_tri(words: &[u32], flags: u8, tex: TexState) -> SceneTri {
    let get = |k: usize| words.get(k).copied().unwrap_or(0);
    let mut vs = [Vtx::default(); 3];
    let mut k = 1;
    for v in vs.iter_mut() {
        v.x = s16(get(k));
        v.y = s16(get(k) >> 16);
        k += 1;
        if flags & FLAG_GOURAUD != 0 {
            let c = get(k);
            v.a[0] = ((c >> 16) & 0xFF) as i64;
            v.a[1] = ((c >> 8) & 0xFF) as i64;
            v.a[2] = (c & 0xFF) as i64;
            k += 1;
        }
        if flags & FLAG_Z != 0 {
            v.a[3] = (get(k) & 0xFFFF) as i64;
            k += 1;
        }
        if flags & FLAG_TEX != 0 {
            v.a[4] = (get(k) & 0xFFFF) as i64;
            v.a[5] = ((get(k) >> 16) & 0xFFFF) as i64;
            k += 1;
        }
    }
    SceneTri {
        flags,
        color: get(0) as u16,
        vs,
        tex,
    }
}

/// The board's `FRAME_CRC` for a picture: CRC-32 (zlib's) over the 640×480
/// RGB888 it scans out — every pixel doubled both ways and widened with
/// [`expand565`]. Equal to tang-verilog's `fbmodel.expected_crc`.
pub fn frame_crc(fb: &[u16]) -> u32 {
    let mut crc = Crc32::new();
    let mut row = vec![0u8; W * 6];
    for y in 0..H {
        for x in 0..W {
            let [r, g, b] = expand565(fb[y * W + x]);
            row[x * 6..x * 6 + 6].copy_from_slice(&[r, g, b, r, g, b]);
        }
        crc.update(&row);
        crc.update(&row);
    }
    crc.finish()
}

struct Crc32 {
    table: [u32; 256],
    crc: u32,
}

impl Crc32 {
    fn new() -> Self {
        let mut table = [0u32; 256];
        for (n, slot) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        Crc32 { table, crc: !0 }
    }

    fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.crc = self.table[((self.crc ^ b as u32) & 0xFF) as usize] ^ (self.crc >> 8);
        }
    }

    fn finish(&self) -> u32 {
        !self.crc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: i16, y: i16) -> TriVertex {
        TriVertex {
            x: x * 16,
            y: y * 16,
            ..Default::default()
        }
    }

    fn scene(tris: &[(u8, u16, [TriVertex; 3])]) -> SoftGpu {
        let mut out = vec![];
        begin_scene(&mut out, SCENE_CLEAR | SCENE_CLEAR_Z, 0);
        for (flags, color, vs) in tris {
            tri(&mut out, *flags, *color, vs);
        }
        end_scene(&mut out);
        swap(&mut out);
        let mut gpu = SoftGpu::new();
        gpu.run(&out);
        gpu
    }

    #[test]
    fn a_rectangle_split_in_two_fills_exactly_like_a_rect() {
        // The spec's own promise: two triangles of a rectangle paint the same
        // pixels FILL_RECT would, no gap and no double-cover on the diagonal.
        let red = 0xF800;
        let gpu = scene(&[
            (0, red, [v(10, 10), v(30, 10), v(30, 20)]),
            (0, red, [v(10, 10), v(30, 20), v(10, 20)]),
        ]);
        let fb = gpu.display();
        for y in 0..H {
            for x in 0..W {
                let want = (10..30).contains(&x) && (10..20).contains(&y);
                assert_eq!(fb[y * W + x] == red, want, "({x},{y})");
            }
        }
    }

    #[test]
    fn nearer_z_wins_and_a_tie_keeps_the_first() {
        let z = |vs: [TriVertex; 3], z: u16| {
            vs.map(|mut p| {
                p.z = z;
                p
            })
        };
        let quad = [v(0, 0), v(16, 0), v(0, 16)];
        let gpu = scene(&[
            (FLAG_Z, 1, z(quad, 500)),
            (FLAG_Z, 2, z(quad, 400)), // nearer: wins
            (FLAG_Z, 3, z(quad, 400)), // tie: loses
        ]);
        assert_eq!(gpu.display()[W + 1], 2);
    }

    #[test]
    fn blend_equations() {
        let white = 0xFFFF;
        assert_eq!(blend(Blend::Add, white, 0x0841), white);
        assert_eq!(blend(Blend::Subtract, 0, white), 0);
        assert_eq!(blend(Blend::Average, 0, white), 0x7BEF);
    }

    #[test]
    fn crc_of_a_blank_frame_matches_zlib() {
        // zlib.crc32(bytes(640 * 480 * 3)) — the board's FRAME_CRC of black.
        assert_eq!(frame_crc(&[0u16; W * H]), 0xFD15_E9AA);
    }
}
