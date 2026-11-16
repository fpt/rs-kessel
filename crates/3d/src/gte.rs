//! The geometry engine: meshes in, `TRI` words out.
//!
//! This is the half of a PS1 that was the CPU and the GTE — transform, light,
//! fog, clip, project — and it is the half that runs on the RP2350 beside the
//! VM. The FPGA's 3D GPU takes triangles in **screen** coordinates and does
//! nothing else, so everything a scene needs before that is here, in integers.
//!
//! ## Spaces and units
//!
//! - **World and model** coordinates are the VM's own `i16` units. What a unit
//!   *is* is the game's choice; the default near plane (16 units) suits
//!   "a unit is about a centimetre".
//! - **View** space is x right, y up, z forward, in units × 16 (4 fraction
//!   bits): a vertex a hundred units away still lands to 1/16 of a unit, which
//!   is well under a pixel.
//! - **Screen** coordinates are 12.4 pixels, the GPU's own.
//!
//! ## What happens to a face
//!
//! 1. Cull: a single-sided face whose normal points away from the eye is
//!    dropped before anything is spent on it.
//! 2. Light, per vertex: `ambient + max(0, n·L) × light`, per channel, where
//!    128 is 1×. An untextured face multiplies its palette colour by that; a
//!    textured one hands it to the GPU, which modulates the texel by it.
//! 3. Fog, per vertex by view depth. An untextured face fades *toward* the fog
//!    colour; a textured one can only be darkened — the GPU's modulate
//!    multiplies, it cannot add — so it fades toward black. Same as a PS1.
//! 4. Clip against the near plane and the guard band (the GPU accepts
//!    -512..+831 pixels and draws only the screen), interpolating from the
//!    inside vertex so two faces sharing an edge cut it at the same point.
//! 5. Project, and turn the polygon into a fan of triangles.
//!
//! Opaque triangles are Z-buffered and sent in submission order; the
//! semi-transparent ones are held back, sorted far to near, and sent after
//! them — the GPU has no order-independent transparency, so the host sorts.

use alloc::vec::Vec;

use crate::cmd::{self, TexState, TriVertex, FLAG_GOURAUD, FLAG_SEMI, FLAG_TEX, FLAG_Z};
use crate::fixed::{cos, degrees, normalize, round_shift, sin, Mat3};
use crate::mesh::{MeshView, F_DOUBLE, F_SEMI, F_SMOOTH, F_TEX};
use crate::raster::SCENE_MAX_TRIS;

/// Fraction bits of a view-space coordinate.
const VF: u32 = 4;
const CX16: i64 = 160 * 16;
const CY16: i64 = 120 * 16;
/// Guard band, pixels: a little inside the GPU's -512..+831.
const GB_LO: i64 = -480;
const GB_HI: i64 = 800;

/// The neutral light level: 1×.
pub const NEUTRAL: u8 = 128;

/// Where textures go in the GPU's SDRAM, in words: byte `0x280000`, which
/// the spec leaves to the application, up to the Z buffer at `0x380000`.
pub const TEX_BASE: u32 = 0xA_0000;
pub const TEX_END: u32 = 0xE_0000;

/// A texture as a face names it: a run of sheet tiles, its size, and the
/// sprite bank its nibbles are read through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexKey {
    pub tile: u16,
    pub wl: u8,
    pub hl: u8,
    pub bank: u8,
}

#[derive(Clone, Copy, Debug)]
struct Tri {
    flags: u8,
    color: u16,
    vs: [TriVertex; 3],
    tex: Option<TexKey>,
}

/// What a frame's geometry cost, for the observation record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GteStats {
    /// Triangles sent to the GPU.
    pub tris: u32,
    /// Faces culled as back-facing.
    pub culled: u32,
    /// Triangles past the scene's 4,096 limit, not sent.
    pub dropped: u32,
    /// `draw3d` calls whose address held no mesh.
    pub bad_meshes: u32,
    /// Textures the SDRAM region had no room for; their faces draw flat.
    pub tex_overflow: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fog {
    near4: i64,
    far4: i64,
    rgb: [i32; 3],
}

/// The engine's state: camera, lens, lights, fog, and the frame being built.
#[derive(Clone, Debug)]
pub struct Gte {
    view: Mat3,
    eye: [i64; 3],
    f16: i64,
    near4: i64,
    light_dir: [i32; 3],
    light_rgb: [i32; 3],
    ambient: [i32; 3],
    fog: Option<Fog>,
    opaque: Vec<Tri>,
    /// (depth key, submission order, triangle).
    semi: Vec<(i64, u32, Tri)>,
    /// View-space vertices of the mesh being drawn; kept so a frame of many
    /// draws allocates once.
    scratch: Vec<[i64; 3]>,
    stats: GteStats,
}

impl Default for Gte {
    fn default() -> Self {
        Self::new()
    }
}

/// A vertex on its way through clipping.
#[derive(Clone, Copy, Debug)]
struct CVert {
    p: [i64; 3],
    c: [i64; 3],
    uv: [i64; 2],
}

/// Enough room for a quad cut by all five planes (each adds at most one
/// vertex), so clipping never touches the allocator — on the RP2350 it runs
/// once per face per frame.
const MAX_POLY: usize = 4 + 5;

/// A polygon in a fixed buffer.
#[derive(Clone, Copy)]
struct Poly {
    v: [CVert; MAX_POLY],
    n: usize,
}

impl Poly {
    fn new() -> Self {
        Poly {
            v: [CVert {
                p: [0; 3],
                c: [0; 3],
                uv: [0; 2],
            }; MAX_POLY],
            n: 0,
        }
    }

    fn push(&mut self, v: CVert) {
        if self.n < MAX_POLY {
            self.v[self.n] = v;
            self.n += 1;
        }
    }

    fn verts(&self) -> &[CVert] {
        &self.v[..self.n]
    }
}

impl Gte {
    pub fn new() -> Self {
        let mut g = Gte {
            view: Mat3::IDENTITY,
            eye: [0; 3],
            f16: 0,
            near4: 0,
            light_dir: [0; 3],
            light_rgb: [0; 3],
            ambient: [NEUTRAL as i32; 3],
            fog: None,
            opaque: Vec::new(),
            semi: Vec::new(),
            scratch: Vec::new(),
            stats: GteStats::default(),
        };
        g.lens(70, 16);
        g
    }

    /// Place the eye at `pos`, looking along the orientation
    /// `yaw · pitch · roll` (see [`Mat3::euler`]).
    pub fn camera(&mut self, pos: [i16; 3], yaw: u16, pitch: u16, roll: u16) {
        self.eye = pos.map(|c| c as i64);
        self.view = Mat3::euler(yaw, pitch, roll).transpose();
    }

    /// Horizontal field of view in degrees (clamped to 10..=170) and the near
    /// plane in units.
    pub fn lens(&mut self, fov: u16, near: u16) {
        let half = degrees(fov.clamp(10, 170) as i32) / 2;
        self.f16 = CX16 * cos(half) as i64 / sin(half) as i64;
        self.near4 = (near.max(1) as i64) << VF;
    }

    /// A directional light shining *from* `dir` (it need not be unit length),
    /// with colour `rgb` where 128 is 1×.
    pub fn light(&mut self, dir: [i16; 3], rgb: [u8; 3]) {
        self.light_dir = normalize(dir.map(|c| c as i64));
        self.light_rgb = rgb.map(|c| c as i32);
    }

    /// The light every surface gets regardless of facing; 128 is 1×. The
    /// default is 128 with no directional light, which draws every face in
    /// exactly its palette colour.
    pub fn ambient(&mut self, rgb: [u8; 3]) {
        self.ambient = rgb.map(|c| c as i32);
    }

    /// Fog from `near` to `far` units, toward `rgb`. `far <= near` turns it off.
    pub fn fog(&mut self, near: u16, far: u16, rgb: [u8; 3]) {
        self.fog = (far > near).then_some(Fog {
            near4: (near as i64) << VF,
            far4: (far as i64) << VF,
            rgb: rgb.map(|c| c as i32),
        });
    }

    /// Start a frame: drop the last one's triangles. Camera, lens, light and
    /// fog carry over, the way the 2D `camera` does.
    pub fn begin_frame(&mut self) {
        self.opaque.clear();
        self.semi.clear();
        self.stats = GteStats::default();
    }

    pub fn stats(&self) -> GteStats {
        self.stats
    }

    fn queued(&self) -> usize {
        self.opaque.len() + self.semi.len()
    }

    /// Transform, light, clip and queue one mesh at `pos` with orientation
    /// `rot = (yaw, pitch, roll)`. `palette` resolves an untextured face's
    /// colour index.
    pub fn draw(
        &mut self,
        bytes: &[u8],
        pos: [i16; 3],
        rot: [u16; 3],
        palette: &[(u8, u8, u8); 256],
    ) {
        let Some(mesh) = MeshView::new(bytes) else {
            self.stats.bad_meshes += 1;
            return;
        };
        let orient = Mat3::euler(rot[0], rot[1], rot[2]);
        let mv = self.view.mul(&orient);
        let d = [0, 1, 2].map(|i| pos[i] as i64 - self.eye[i]);
        let t = self.view.apply(d).map(|c| round_shift(c, 14 - VF));
        let mut verts = core::mem::take(&mut self.scratch);
        verts.clear();
        verts.extend((0..mesh.nverts).map(|i| {
            let (p, _) = mesh.vertex(i);
            let r = mv.apply(p.map(|c| c as i64));
            [0, 1, 2].map(|k| round_shift(r[k], 14 - VF) + t[k])
        }));

        for fi in 0..mesh.nfaces {
            let f = mesh.face(fi);
            // A mesh can be built in RAM by the game, so nothing read from it
            // is trusted: an index past the vertices, or a texture side
            // outside 8..256, is a bad face rather than a panic.
            let bad_tex =
                f.flags & F_TEX != 0 && !((3..=8).contains(&f.wl) && (3..=8).contains(&f.hl));
            if bad_tex || f.idx[..f.n].iter().any(|&i| i as usize >= mesh.nverts) {
                self.stats.bad_meshes += 1;
                continue;
            }
            let p0 = verts[f.idx[0] as usize];
            if f.flags & F_DOUBLE == 0 {
                let nv = mv.apply(f.normal.map(|c| c as i64));
                let facing: i64 = (0..3).map(|k| (nv[k] >> 14) * p0[k]).sum();
                if facing >= 0 {
                    self.stats.culled += 1;
                    continue;
                }
            }
            let textured = f.flags & F_TEX != 0;
            let base = if textured {
                [NEUTRAL as i32; 3]
            } else {
                let (r, g, b) = palette[f.color as usize];
                [r as i32, g as i32, b as i32]
            };
            let mut poly = Poly::new();
            for k in 0..f.n {
                let vi = f.idx[k] as usize;
                let normal = if f.flags & F_SMOOTH != 0 {
                    mesh.vertex(vi).1
                } else {
                    f.normal
                };
                let p = verts[vi];
                let c = self.vertex_color(&orient, normal, base, textured, p[2]);
                poly.push(CVert {
                    p,
                    c,
                    uv: [f.uv[k].0 as i64, f.uv[k].1 as i64],
                });
            }
            let clipped = self.clip(poly);
            let clipped = clipped.verts();
            if clipped.len() < 3 {
                continue;
            }
            let tex = textured.then_some(TexKey {
                tile: f.tile,
                wl: f.wl,
                hl: f.hl,
                bank: f.bank,
            });
            let semi = (f.flags & F_SEMI != 0).then_some((f.flags >> 4) & 3);
            let key = clipped.iter().map(|v| v.p[2]).sum::<i64>() / clipped.len() as i64;
            let mut screen = [TriVertex::default(); MAX_POLY];
            for (s, v) in screen.iter_mut().zip(clipped) {
                *s = self.project(v);
            }
            for k in 1..clipped.len() - 1 {
                if self.queued() >= SCENE_MAX_TRIS {
                    self.stats.dropped += 1;
                    continue;
                }
                let vs = [screen[0], screen[k], screen[k + 1]];
                let tri = make_tri(vs, tex, semi);
                match semi {
                    Some(_) => {
                        let order = self.semi.len() as u32;
                        self.semi.push((key, order, tri));
                    }
                    None => self.opaque.push(tri),
                }
            }
        }
        self.scratch = verts;
    }

    /// A vertex's colour: lit, then fogged.
    fn vertex_color(
        &self,
        orient: &Mat3,
        normal: [i32; 3],
        base: [i32; 3],
        textured: bool,
        z4: i64,
    ) -> [i64; 3] {
        let n = orient.apply(normal.map(|c| c as i64)).map(|c| c >> 14);
        let ndotl = ((n[0] * self.light_dir[0] as i64
            + n[1] * self.light_dir[1] as i64
            + n[2] * self.light_dir[2] as i64)
            >> 14)
            .max(0);
        let mut c = [0i64; 3];
        for k in 0..3 {
            let level =
                (self.ambient[k] as i64 + ((ndotl * self.light_rgb[k] as i64) >> 14)).min(255);
            c[k] = if textured {
                level
            } else {
                ((base[k] as i64 * level) >> 7).min(255)
            };
        }
        if let Some(fog) = self.fog {
            let t = (((z4 - fog.near4) << 8) / (fog.far4 - fog.near4)).clamp(0, 256);
            for k in 0..3 {
                let toward = if textured { 0 } else { fog.rgb[k] as i64 };
                c[k] = (c[k] * (256 - t) + toward * t) >> 8;
            }
        }
        c
    }

    /// The five planes a vertex must be inside of, as signed distances:
    /// near, then the guard band's left, right, top, bottom.
    fn planes(&self, p: &[i64; 3]) -> [i64; 5] {
        let (x, y, z) = (p[0], p[1], p[2]);
        [
            z - self.near4,
            self.f16 * x + (CX16 / 16 - GB_LO) * 16 * z,
            -self.f16 * x + (GB_HI - CX16 / 16) * 16 * z,
            -self.f16 * y + (CY16 / 16 - GB_LO) * 16 * z,
            self.f16 * y + (GB_HI - CY16 / 16) * 16 * z,
        ]
    }

    /// Sutherland–Hodgman against [`planes`](Self::planes).
    fn clip(&self, poly: Poly) -> Poly {
        let mut cur = poly;
        for plane in 0..5 {
            if cur.n < 3 {
                return Poly::new();
            }
            let mut dist = [0i64; MAX_POLY];
            for (d, v) in dist.iter_mut().zip(cur.verts()) {
                *d = self.planes(&v.p)[plane];
            }
            if dist[..cur.n].iter().all(|&d| d >= 0) {
                continue;
            }
            let mut out = Poly::new();
            for i in 0..cur.n {
                let j = (i + 1) % cur.n;
                let (a, b) = (&cur.v[i], &cur.v[j]);
                let (da, db) = (dist[i], dist[j]);
                if da >= 0 {
                    out.push(*a);
                }
                if (da >= 0) != (db >= 0) {
                    // Always from the inside vertex, so the faces either side
                    // of an edge compute the same cut.
                    let (inn, out_v, di, d_o) = if da >= 0 {
                        (a, b, da, db)
                    } else {
                        (b, a, db, da)
                    };
                    let den = di - d_o;
                    let lerp = |p: i64, q: i64| p + (q - p) * di / den;
                    out.push(CVert {
                        p: [0, 1, 2].map(|k| lerp(inn.p[k], out_v.p[k])),
                        c: [0, 1, 2].map(|k| lerp(inn.c[k], out_v.c[k])),
                        uv: [0, 1].map(|k| lerp(inn.uv[k], out_v.uv[k])),
                    });
                }
            }
            cur = out;
        }
        if cur.n < 3 {
            return Poly::new();
        }
        cur
    }

    fn project(&self, v: &CVert) -> TriVertex {
        let z = v.p[2].max(1);
        let sx = (CX16 + self.f16 * v.p[0] / z).clamp(-512 * 16, 831 * 16);
        let sy = (CY16 - self.f16 * v.p[1] / z).clamp(-512 * 16, 831 * 16);
        // 1/z is what is linear across the screen, so it is what the GPU may
        // interpolate: 0 at the near plane, toward 65535 far away.
        let zb = (65535 - (self.near4 << 16) / z).clamp(0, 65534);
        TriVertex {
            x: sx as i16,
            y: sy as i16,
            rgb: v.c.map(|c| c.clamp(0, 255) as u8),
            z: zb as u16,
            u: v.uv[0].clamp(0, 0xFFFF) as u16,
            v: v.uv[1].clamp(0, 0xFFFF) as u16,
        }
    }

    /// The scene, as words: textures first, then the palette as the CLUT,
    /// then every triangle between `BEGIN_SCENE` and `END_SCENE`. The caller
    /// appends whatever it draws over the scene, and `SWAP`.
    ///
    /// `tile(t)` reads sheet tile `t` (32 bytes, 4bpp, one row per 4 bytes).
    pub fn emit(
        &mut self,
        out: &mut Vec<u32>,
        cache: &mut TexCache,
        tile: &dyn Fn(u16) -> [u8; 32],
        clut: &[u16; 256],
        clear: u16,
    ) {
        let mut semi = core::mem::take(&mut self.semi);
        let opaque = core::mem::take(&mut self.opaque);
        // Far to near; equal depths keep the order they were drawn in.
        semi.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let order: Vec<Tri> = opaque.into_iter().chain(semi.iter().map(|s| s.2)).collect();

        // Uploads must land before the scene that samples them.
        let mut bases: Vec<(TexKey, Option<u32>)> = Vec::new();
        for t in &order {
            if let Some(key) = t.tex {
                if !bases.iter().any(|(k, _)| *k == key) {
                    let base = cache.ensure(key, tile, out);
                    if base.is_none() {
                        self.stats.tex_overflow += 1;
                    }
                    bases.push((key, base));
                }
            }
        }
        cmd::clut_write(out, 0, clut);
        cmd::begin_scene(out, cmd::SCENE_CLEAR | cmd::SCENE_CLEAR_Z, clear);
        let mut bound: Option<TexState> = None;
        for t in &order {
            let mut t = *t;
            if let Some(key) = t.tex {
                match bases.iter().find(|(k, _)| *k == key).and_then(|(_, b)| *b) {
                    Some(base) => {
                        let state = TexState {
                            fmt: cmd::TEX_4BPP,
                            clut_off: key.bank << 4,
                            wl: key.wl,
                            hl: key.hl,
                            base,
                        };
                        if bound != Some(state) {
                            cmd::set_tex(out, &state);
                            bound = Some(state);
                        }
                    }
                    // No room for the texture: draw the face lit but bare,
                    // rather than sampling some other texture's memory.
                    None => t.flags = (t.flags & !FLAG_TEX) | FLAG_GOURAUD,
                }
            }
            cmd::tri(out, t.flags, t.color, &t.vs);
            self.stats.tris += 1;
        }
        cmd::end_scene(out);
    }
}

fn make_tri(vs: [TriVertex; 3], tex: Option<TexKey>, semi: Option<u8>) -> Tri {
    let mut flags = FLAG_Z;
    let mut color = 0;
    if tex.is_some() {
        flags |= FLAG_TEX;
        // A texel at 1× is the texel: skip three colour words when every
        // vertex is neutral.
        if vs.iter().any(|v| v.rgb != [NEUTRAL; 3]) {
            flags |= FLAG_GOURAUD;
        }
    } else if vs.iter().all(|v| v.rgb == vs[0].rgb) {
        let [r, g, b] = vs[0].rgb;
        color = cmd::to565(r, g, b);
    } else {
        flags |= FLAG_GOURAUD;
    }
    if let Some(mode) = semi {
        flags |= FLAG_SEMI | mode << 4;
    }
    Tri {
        flags,
        color,
        vs,
        tex,
    }
}

/// Which textures are in the GPU's SDRAM, where, and with what contents.
///
/// A texture gets a slot the first time a frame uses it, aligned to its own
/// size as the GPU requires, and keeps it. Each frame its tiles are read
/// again and re-uploaded only if they changed — so a game that animates a
/// texture by rewriting its sprite pays for exactly that.
///
/// This is derived state: the pixels never depend on *where* a texture went,
/// only on what was in it. A host whose GPU lost its SDRAM (a reconfigure, a
/// snapshot restored on the board) calls [`invalidate`](Self::invalidate)
/// and the next frame uploads everything again.
#[derive(Clone, Debug, Default)]
pub struct TexCache {
    slots: Vec<Slot>,
    next: u32,
}

#[derive(Clone, Debug)]
struct Slot {
    key: (u16, u8, u8),
    base: u32,
    words: Vec<u32>,
}

impl TexCache {
    pub fn invalidate(&mut self) {
        for s in &mut self.slots {
            s.words.clear();
        }
    }

    /// The SDRAM word address of `key`'s texture, uploading it first if it
    /// is new or its tiles changed. `None` when the region is full.
    pub fn ensure(
        &mut self,
        key: TexKey,
        tile: &dyn Fn(u16) -> [u8; 32],
        out: &mut Vec<u32>,
    ) -> Option<u32> {
        // The bank is a CLUT offset, not texture memory: two banks of one
        // sprite share a slot.
        let id = (key.tile, key.wl, key.hl);
        let tiles = 1u32 << (key.wl as u32 - 3 + key.hl as u32 - 3);
        let mut words = Vec::with_capacity(tiles as usize * 8);
        for t in 0..tiles {
            let bytes = tile(key.tile.wrapping_add(t as u16));
            for row in bytes.chunks_exact(4) {
                words.push(u32::from_be_bytes([row[0], row[1], row[2], row[3]]));
            }
        }
        let at = match self.slots.iter().position(|s| s.key == id) {
            Some(at) => at,
            None => {
                let size = words.len() as u32;
                let base = (TEX_BASE + self.next).next_multiple_of(size);
                if base + size > TEX_END {
                    return None;
                }
                self.next = base + size - TEX_BASE;
                self.slots.push(Slot {
                    key: id,
                    base,
                    words: Vec::new(),
                });
                self.slots.len() - 1
            }
        };
        let slot = &mut self.slots[at];
        if slot.words != words {
            cmd::upload(out, slot.base, &words);
            slot.words = words;
        }
        Some(slot.base)
    }
}

/// The CLUT a 4bpp texture is read through: the palette, with Kessel's sprite
/// rule kept — nibble 0 is transparent in every bank, so entries `16k` are
/// `0x0000`, the GPU's transparent texel. Any *other* entry that truncates to
/// `0x0000` is nudged to the darkest green there is, so a black texel stays a
/// black texel instead of a hole.
pub fn clut_from_palette(palette: &[(u8, u8, u8); 256]) -> [u16; 256] {
    let mut clut = [0u16; 256];
    for (i, &(r, g, b)) in palette.iter().enumerate() {
        clut[i] = if i % 16 == 0 {
            0
        } else {
            match cmd::to565(r, g, b) {
                0 => 0x0020,
                c => c,
            }
        };
    }
    clut
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{HEADER, MAGIC};
    use alloc::vec;

    /// A mesh can come from RAM the game wrote, so the engine takes garbage
    /// without panicking: whatever the bytes say, a face is drawn, culled or
    /// counted bad.
    #[test]
    fn hostile_meshes_do_not_panic() {
        let palette = [(10u8, 20u8, 30u8); 256];
        let tile = |_: u16| [0x5Au8; 32];
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut gte = Gte::new();
        gte.camera([0, 0, -200], 0, 0, 0);
        let mut cache = TexCache::default();
        for _ in 0..2000 {
            let mut bytes = vec![0u8; 600];
            for b in bytes.iter_mut() {
                *b = next() as u8;
            }
            bytes[..2].copy_from_slice(&MAGIC);
            // Small counts, so the buffer usually holds the whole mesh and the
            // faces are actually walked.
            bytes[2..HEADER].copy_from_slice(&[0, (next() % 12) as u8, 0, (next() % 12) as u8]);
            gte.begin_frame();
            gte.draw(
                &bytes,
                [0, 0, 0],
                [next() as u16, next() as u16, 0],
                &palette,
            );
            let mut out = Vec::new();
            gte.emit(&mut out, &mut cache, &tile, &[0x1234; 256], 0);
        }
    }

    #[test]
    fn a_face_through_the_near_plane_is_cut_not_dropped() {
        // A floor running from behind the eye to in front of it.
        let src = "v -50 -20 -100\nv 50 -20 -100\nv 50 -20 100\nv -50 -20 100\nf 1 2 3 4";
        let lines: Vec<(usize, Vec<&str>)> = src
            .lines()
            .enumerate()
            .map(|(n, l)| (n + 1, l.split_whitespace().collect()))
            .collect();
        let bytes = crate::mesh::compile(&lines, &|_| None).unwrap();
        let mut gte = Gte::new();
        gte.begin_frame();
        gte.draw(&bytes, [0, 0, 0], [0, 0, 0], &[(200, 200, 200); 256]);
        // Cut by the near plane and, close to the eye, the bottom of the guard
        // band too — so more than a quad's two triangles, all of them real.
        assert!(gte.queued() >= 2, "the visible part is drawn");
        for t in &gte.opaque {
            for v in &t.vs {
                assert!(v.y >= 120 * 16, "below the horizon: {v:?}");
                assert!(
                    (-512 * 16..=831 * 16).contains(&(v.y as i32)),
                    "inside the GPU's range: {v:?}"
                );
            }
        }
    }
}
