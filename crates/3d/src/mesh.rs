//! Meshes: the `mesh NAME { … }` grammar, and the bytes it compiles to.
//!
//! The grammar is OBJ's, cut down, because it is the one a model already
//! knows: `v x y z` declares a vertex, `vt u v` a texture coordinate, and
//! `f 1 2 3 4` a face by **1-based** index, `f 1/1 2/2 3/3` with texture
//! coordinates. What OBJ spreads over a material file is a few state lines
//! that apply to every face after them:
//!
//! ```text
//! mesh crate {
//!   v -8 -8 -8   v 8 -8 -8   v 8 8 -8   v -8 8 -8     -- one line each, normally
//!   color 9                -- untextured faces from here are palette colour 9
//!   tex wood               -- …or textured with sprite `wood` (power-of-two size)
//!   tex wood 2             -- …drawn through sprite bank 2
//!   smooth | flat          -- light per vertex normal, or per face (default)
//!   double | single        -- draw both sides, or cull the back (default)
//!   blend add              -- semi-transparent: avg, add, sub, quarter; `blend off`
//!   f 1 2 3 4              -- counter-clockwise seen from the front
//! }
//! ```
//!
//! A face is a triangle or a convex quad. Front is **counter-clockwise**, as in
//! OBJ; a face seen from behind is culled unless `double`.
//!
//! A textured face without `/vt` indices maps the texture's corners in order:
//! the first vertex is the bottom-left `(0, h)`, then `(w, h)`, `(w, 0)`,
//! `(0, 0)` — so a quad listed counter-clockwise from its bottom-left shows the
//! sprite the right way up.
//!
//! ## The bytes
//!
//! Big-endian, because the VM is: a game can `peek16` a vertex, or build a mesh
//! in RAM with `poke16` and draw that.
//!
//! ```text
//! header  'K' '3' nverts:u16 nfaces:u16
//! vertex  x y z:i16  nx ny nz:i16 (Q14 vertex normal)                12 bytes
//! face    n:u8 flags:u8 color:u8 bank:u8  tile:u16 wl:u8 hl:u8
//!         nx ny nz:i16 (Q14 face normal)  i0 i1 i2 i3:u16 (0-based)
//!         (u v:u16 10.6) × 4                                         38 bytes
//! ```
//!
//! Normals are computed here, at compile time, so the device never takes a
//! square root at draw time.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fixed::normalize;
#[cfg(test)]
use crate::fixed::ONE;

pub const MAGIC: [u8; 2] = *b"K3";
pub const HEADER: usize = 6;
pub const VERTEX: usize = 12;
pub const FACE: usize = 38;

/// Face flag: textured.
pub const F_TEX: u8 = 1;
/// Face flag: lit by vertex normals rather than the face normal.
pub const F_SMOOTH: u8 = 2;
/// Face flag: drawn from both sides.
pub const F_DOUBLE: u8 = 4;
/// Face flag: semi-transparent; the equation is `flags[5:4]`.
pub const F_SEMI: u8 = 8;

/// What a texture name resolves to: a sprite's first tile and its size in
/// pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpriteRef {
    pub tile: u16,
    pub w: u16,
    pub h: u16,
}

/// A diagnostic: the line it is on, and what is wrong.
pub type MeshError = (usize, String);

#[derive(Clone, Copy)]
struct State {
    color: u8,
    tex: Option<(SpriteRef, u8)>,
    smooth: bool,
    double: bool,
    blend: Option<u8>,
}

struct Face {
    idx: [u16; 4],
    n: u8,
    st: State,
    uv: [(u16, u16); 4],
    line: usize,
}

/// Compile a mesh body. `lines` is the body split into lines of tokens, each
/// with its source line number; `sprite` resolves a texture name.
pub fn compile(
    lines: &[(usize, Vec<&str>)],
    sprite: &dyn Fn(&str) -> Option<SpriteRef>,
) -> Result<Vec<u8>, Vec<MeshError>> {
    let mut errs: Vec<MeshError> = Vec::new();
    let mut verts: Vec<[i16; 3]> = Vec::new();
    let mut tcoords: Vec<(i32, i32)> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    let mut st = State {
        color: 7,
        tex: None,
        smooth: false,
        double: false,
        blend: None,
    };

    for (line, toks) in lines {
        let line = *line;
        let Some((&kw, args)) = toks.split_first() else {
            continue;
        };
        let mut bad = |msg: String| errs.push((line, msg));
        match kw {
            "v" => match args {
                [x, y, z] => match (int16(x), int16(y), int16(z)) {
                    (Some(x), Some(y), Some(z)) => verts.push([x, y, z]),
                    _ => bad(format!("a vertex is three whole numbers in -32768..32767, not '{}'", args.join(" "))),
                },
                _ => bad("a vertex is 'v x y z'".to_string()),
            },
            "vt" => match args {
                [u, v] => match (texel(u), texel(v)) {
                    (Some(u), Some(v)) => tcoords.push((u, v)),
                    _ => bad(format!("a texture coordinate is two texel positions in 0..1023, not '{}'", args.join(" "))),
                },
                _ => bad("a texture coordinate is 'vt u v' in texels".to_string()),
            },
            "color" | "colour" => match args {
                [c] => match c.parse::<u8>() {
                    Ok(c) => {
                        st.color = c;
                        st.tex = None;
                    }
                    Err(_) => bad(format!("'color' takes a palette index 0..255, not '{c}'")),
                },
                _ => bad("'color' takes one palette index".to_string()),
            },
            "tex" => {
                let (name, bank) = match args {
                    [name] => (*name, Some(0)),
                    [name, bank] => (*name, bank.parse::<u8>().ok().filter(|b| *b < 16)),
                    _ => {
                        bad("'tex' takes a sprite name and an optional bank".to_string());
                        continue;
                    }
                };
                let Some(bank) = bank else {
                    bad("a texture's bank is 0..15, as for sprbank".to_string());
                    continue;
                };
                match sprite(name) {
                    Some(s) if pow2_side(s.w) && pow2_side(s.h) => st.tex = Some((s, bank)),
                    Some(s) => bad(format!(
                        "sprite '{name}' is {}x{} pixels; a texture's sides must each be 8, 16, 32, 64, 128 or 256",
                        s.w, s.h
                    )),
                    None => bad(format!("'tex {name}' names no sprite")),
                }
            }
            "smooth" | "flat" if args.is_empty() => st.smooth = kw == "smooth",
            "double" | "single" if args.is_empty() => st.double = kw == "double",
            "blend" => match args {
                [mode] => match *mode {
                    "off" | "none" => st.blend = None,
                    "avg" | "average" => st.blend = Some(0),
                    "add" => st.blend = Some(1),
                    "sub" | "subtract" => st.blend = Some(2),
                    "quarter" => st.blend = Some(3),
                    _ => bad(format!("unknown blend '{mode}' (expected avg, add, sub, quarter or off)")),
                },
                _ => bad("'blend' takes one of avg, add, sub, quarter, off".to_string()),
            },
            "f" => {
                if !(3..=4).contains(&args.len()) {
                    bad(format!("a face is 3 or 4 vertices, not {}", args.len()));
                    continue;
                }
                let mut idx = [0u16; 4];
                let mut ts: [Option<usize>; 4] = [None; 4];
                let mut ok = true;
                for (k, a) in args.iter().enumerate() {
                    let (vi, ti) = match a.split_once('/') {
                        Some((v, t)) => (v, Some(t)),
                        None => (*a, None),
                    };
                    match index(vi, verts.len()) {
                        Some(i) => idx[k] = i as u16,
                        None => {
                            bad(format!("'{vi}' is not one of the {} vertices declared so far (they count from 1)", verts.len()));
                            ok = false;
                        }
                    }
                    if let Some(t) = ti {
                        match index(t, tcoords.len()) {
                            Some(i) => ts[k] = Some(i),
                            None => {
                                bad(format!("'{t}' is not one of the {} texture coordinates declared so far", tcoords.len()));
                                ok = false;
                            }
                        }
                    }
                }
                let n = args.len();
                let given = ts[..n].iter().filter(|t| t.is_some()).count();
                if given != 0 && given != n {
                    bad("give every vertex of a face a texture coordinate, or none of them".to_string());
                    ok = false;
                }
                if given != 0 && st.tex.is_none() {
                    bad("texture coordinates on an untextured face — say 'tex NAME' first".to_string());
                    ok = false;
                }
                if !ok {
                    continue;
                }
                let mut uv = [(0u16, 0u16); 4];
                if let Some((s, _)) = st.tex {
                    let corners = [(0, s.h as i32), (s.w as i32, s.h as i32), (s.w as i32, 0), (0, 0)];
                    let raw: Vec<(i32, i32)> = (0..n)
                        .map(|k| match ts[k] {
                            Some(t) => tcoords[t],
                            None => (corners[k].0 * 64, corners[k].1 * 64),
                        })
                        .collect();
                    // A coordinate on the texture's far edge would wrap to its
                    // first texel; pull the face's far edge in by 1/64 texel.
                    let (umax, vmax) = (
                        raw.iter().map(|t| t.0).max().unwrap_or(0),
                        raw.iter().map(|t| t.1).max().unwrap_or(0),
                    );
                    for (k, &(u, v)) in raw.iter().enumerate() {
                        let pull = |c: i32, max: i32, side: u16| {
                            let side = side as i32 * 64;
                            if c == max && c > 0 && c % side == 0 { c - 1 } else { c }
                        };
                        uv[k] = (pull(u, umax, s.w) as u16, pull(v, vmax, s.h) as u16);
                    }
                }
                faces.push(Face { idx, n: n as u8, st, uv, line });
            }
            _ => bad(format!(
                "unknown mesh line '{kw}' (expected v, vt, f, color, tex, smooth, flat, double, single or blend)"
            )),
        }
    }

    if verts.len() > u16::MAX as usize || faces.len() > u16::MAX as usize {
        errs.push((
            lines.first().map_or(0, |l| l.0),
            "a mesh holds at most 65535 vertices and faces".to_string(),
        ));
    }

    // Face normals (Newell's method, so a slightly bent quad still gets a
    // sensible one), then vertex normals from the smooth faces around each.
    let mut fnormals = Vec::with_capacity(faces.len());
    let mut vsum = alloc::vec![[0i64; 3]; verts.len()];
    for f in &faces {
        let n = newell(&verts, &f.idx[..f.n as usize]);
        if n == [0; 3] {
            errs.push((
                f.line,
                "this face has no area (its vertices are in a line)".to_string(),
            ));
        }
        if f.st.smooth {
            for &i in &f.idx[..f.n as usize] {
                for c in 0..3 {
                    vsum[i as usize][c] += n[c] as i64;
                }
            }
        }
        fnormals.push(n);
    }
    if !errs.is_empty() {
        return Err(errs);
    }

    let mut out = Vec::with_capacity(HEADER + verts.len() * VERTEX + faces.len() * FACE);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(verts.len() as u16).to_be_bytes());
    out.extend_from_slice(&(faces.len() as u16).to_be_bytes());
    for (v, s) in verts.iter().zip(&vsum) {
        for c in v {
            out.extend_from_slice(&c.to_be_bytes());
        }
        for c in normalize(*s) {
            out.extend_from_slice(&(c as i16).to_be_bytes());
        }
    }
    for (f, n) in faces.iter().zip(&fnormals) {
        let mut flags = 0;
        if f.st.tex.is_some() {
            flags |= F_TEX;
        }
        if f.st.smooth {
            flags |= F_SMOOTH;
        }
        if f.st.double {
            flags |= F_DOUBLE;
        }
        if let Some(b) = f.st.blend {
            flags |= F_SEMI | b << 4;
        }
        let (s, bank) = f.st.tex.unwrap_or((
            SpriteRef {
                tile: 0,
                w: 8,
                h: 8,
            },
            0,
        ));
        out.extend_from_slice(&[f.n, flags, f.st.color, bank]);
        out.extend_from_slice(&s.tile.to_be_bytes());
        out.extend_from_slice(&[log2(s.w), log2(s.h)]);
        for c in n {
            out.extend_from_slice(&(*c as i16).to_be_bytes());
        }
        for i in f.idx {
            out.extend_from_slice(&i.to_be_bytes());
        }
        for (u, v) in f.uv {
            out.extend_from_slice(&u.to_be_bytes());
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
    Ok(out)
}

fn int16(s: &str) -> Option<i16> {
    s.parse::<i16>().ok()
}

/// A texel position as 10.6, from `12` or `12.5`.
fn texel(s: &str) -> Option<i32> {
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    let whole: i32 = whole.parse().ok()?;
    if !(0..1024).contains(&whole) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Round the decimal fraction to the nearest 1/64 using integers only.
    let digits = &frac[..frac.len().min(6)];
    let num: i64 = if digits.is_empty() {
        0
    } else {
        digits.parse().ok()?
    };
    let den = 10i64.pow(digits.len() as u32);
    let sixty_fourths = ((num * 64 * 2 + den) / (den * 2)) as i32;
    Some((whole * 64 + sixty_fourths).min(1024 * 64 - 1))
}

/// A 1-based OBJ index (negative counts back from the latest) → 0-based.
fn index(s: &str, len: usize) -> Option<usize> {
    let i: i64 = s.parse().ok()?;
    let i = if i < 0 { len as i64 + i } else { i - 1 };
    (0..len as i64).contains(&i).then_some(i as usize)
}

fn pow2_side(n: u16) -> bool {
    n.is_power_of_two() && (8..=256).contains(&n)
}

fn log2(n: u16) -> u8 {
    n.trailing_zeros() as u8
}

/// The outward unit normal (Q14) of a counter-clockwise polygon.
///
/// Newell's sum gives the normal a right-handed system would call outward;
/// Kessel3D's space is x right, y up, z *forward*, which is left-handed, so
/// the outward one is its negation.
fn newell(verts: &[[i16; 3]], idx: &[u16]) -> [i32; 3] {
    let mut n = [0i64; 3];
    for k in 0..idx.len() {
        let a = verts[idx[k] as usize].map(|c| c as i64);
        let b = verts[idx[(k + 1) % idx.len()] as usize].map(|c| c as i64);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    normalize([-n[0], -n[1], -n[2]])
}

/// A face as the engine reads it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceView {
    pub n: usize,
    pub flags: u8,
    pub color: u8,
    pub bank: u8,
    pub tile: u16,
    pub wl: u8,
    pub hl: u8,
    pub normal: [i32; 3],
    pub idx: [u16; 4],
    pub uv: [(u16, u16); 4],
}

/// A mesh in memory, checked once so the reads after it cannot run off the
/// end.
pub struct MeshView<'a> {
    bytes: &'a [u8],
    pub nverts: usize,
    pub nfaces: usize,
}

impl<'a> MeshView<'a> {
    /// `None` if `bytes` does not start with a whole mesh — a stray address
    /// passed to `draw3d` draws nothing rather than reading garbage as faces.
    pub fn new(bytes: &'a [u8]) -> Option<MeshView<'a>> {
        if bytes.len() < HEADER || bytes[..2] != MAGIC {
            return None;
        }
        let nverts = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
        let nfaces = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let len = HEADER + nverts * VERTEX + nfaces * FACE;
        (bytes.len() >= len).then(|| MeshView {
            bytes: &bytes[..len],
            nverts,
            nfaces,
        })
    }

    fn i16_at(&self, at: usize) -> i16 {
        i16::from_be_bytes([self.bytes[at], self.bytes[at + 1]])
    }

    fn u16_at(&self, at: usize) -> u16 {
        u16::from_be_bytes([self.bytes[at], self.bytes[at + 1]])
    }

    /// Vertex `i`: position and Q14 normal.
    pub fn vertex(&self, i: usize) -> ([i16; 3], [i32; 3]) {
        let at = HEADER + i * VERTEX;
        (
            [self.i16_at(at), self.i16_at(at + 2), self.i16_at(at + 4)],
            [
                self.i16_at(at + 6) as i32,
                self.i16_at(at + 8) as i32,
                self.i16_at(at + 10) as i32,
            ],
        )
    }

    pub fn face(&self, i: usize) -> FaceView {
        let at = HEADER + self.nverts * VERTEX + i * FACE;
        let b = &self.bytes[at..at + FACE];
        let mut idx = [0u16; 4];
        let mut uv = [(0u16, 0u16); 4];
        for k in 0..4 {
            idx[k] = self.u16_at(at + 14 + 2 * k);
            uv[k] = (self.u16_at(at + 22 + 4 * k), self.u16_at(at + 24 + 4 * k));
        }
        FaceView {
            n: if b[0] == 4 { 4 } else { 3 },
            flags: b[1],
            color: b[2],
            bank: b[3],
            tile: self.u16_at(at + 4),
            wl: b[6],
            hl: b[7],
            normal: [
                self.i16_at(at + 8) as i32,
                self.i16_at(at + 10) as i32,
                self.i16_at(at + 12) as i32,
            ],
            idx,
            uv,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn lines(src: &str) -> Vec<(usize, Vec<&str>)> {
        src.lines()
            .enumerate()
            .map(|(n, l)| (n + 1, l.split_whitespace().collect()))
            .collect()
    }

    fn no_sprites(_: &str) -> Option<SpriteRef> {
        None
    }

    #[test]
    fn a_counter_clockwise_floor_faces_up() {
        let src = "v 0 0 0\nv 0 0 10\nv 10 0 10\nv 10 0 0\nf 1 2 3 4";
        // Seen from above (looking down -y) with +x right and +z up the page,
        // 1→2→3→4 runs (0,0)→(0,10)→(10,10)→(10,0): clockwise. So this is the
        // floor's *underside*, and its normal points down.
        let bytes = compile(&lines(src), &no_sprites).unwrap();
        let m = MeshView::new(&bytes).unwrap();
        assert_eq!(m.face(0).normal, [0, -ONE, 0]);
        let src = "v 0 0 0\nv 10 0 0\nv 10 0 10\nv 0 0 10\nf 1 2 3 4";
        let bytes = compile(&lines(src), &no_sprites).unwrap();
        assert_eq!(MeshView::new(&bytes).unwrap().face(0).normal, [0, ONE, 0]);
    }

    #[test]
    fn indices_are_one_based_and_checked() {
        let err = compile(&lines("v 0 0 0\nv 1 0 0\nf 1 2 3"), &no_sprites).unwrap_err();
        assert!(
            err[0].1.contains("'3' is not one of the 2 vertices"),
            "{err:?}"
        );
        let err = compile(&lines("v 0 0 0\nf 0 1 1"), &no_sprites).unwrap_err();
        assert!(err[0].1.contains("count from 1"), "{err:?}");
    }

    #[test]
    fn a_texture_must_be_a_power_of_two_sprite() {
        let sprite = |n: &str| match n {
            "wood" => Some(SpriteRef {
                tile: 4,
                w: 16,
                h: 16,
            }),
            "wide" => Some(SpriteRef {
                tile: 8,
                w: 24,
                h: 8,
            }),
            _ => None,
        };
        let src = "v 0 0 0\nv 1 0 0\nv 1 1 0\ntex wood\nf 1 2 3";
        let bytes = compile(&lines(src), &sprite).unwrap();
        let f = MeshView::new(&bytes).unwrap().face(0);
        assert_eq!((f.flags & F_TEX, f.tile, f.wl, f.hl), (F_TEX, 4, 4, 4));
        // Default corners, with the far edge pulled in so it cannot wrap.
        assert_eq!(f.uv[0], (0, 16 * 64 - 1));
        assert_eq!(f.uv[1], (16 * 64 - 1, 16 * 64 - 1));
        let err = compile(&lines("tex wide"), &sprite).unwrap_err();
        assert!(err[0].1.contains("24x8"), "{err:?}");
        let err = compile(&lines("tex nope"), &sprite).unwrap_err();
        assert!(err[0].1.contains("names no sprite"), "{err:?}");
    }

    #[test]
    fn texture_coordinates_take_quarter_texels() {
        assert_eq!(texel("12"), Some(12 * 64));
        assert_eq!(texel("12.5"), Some(12 * 64 + 32));
        assert_eq!(texel("0.25"), Some(16));
        assert_eq!(texel("-1"), None);
    }

    #[test]
    fn a_degenerate_face_is_reported() {
        let err = compile(&lines("v 0 0 0\nv 1 0 0\nv 2 0 0\nf 1 2 3"), &no_sprites).unwrap_err();
        assert!(err[0].1.contains("no area"), "{err:?}");
    }

    #[test]
    fn a_short_buffer_is_not_a_mesh() {
        let bytes = compile(&lines("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3"), &no_sprites).unwrap();
        assert!(MeshView::new(&bytes).is_some());
        assert!(MeshView::new(&bytes[..bytes.len() - 1]).is_none());
        assert!(MeshView::new(&vec![0u8; 64]).is_none());
    }
}
