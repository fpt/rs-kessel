//! The 3D GPU's command words, as `docs/SPECIFICATION_3D.md` in tang-verilog
//! defines them.
//!
//! This module only *encodes*. The words it produces are the whole interface:
//! a host with the FPGA sends them over HSTX, and [`crate::raster::SoftGpu`]
//! executes the very same words on a PC. Nothing draws from anything else,
//! which is what makes "the PC shows what the board shows" a property of the
//! design rather than a hope.

use alloc::vec::Vec;

/// Logical screen width.
pub const W: usize = 320;
/// Logical screen height.
pub const H: usize = 240;

pub const OP_FILL_RECT: u32 = 0x02;
pub const OP_SWAP: u32 = 0x03;
pub const OP_UPLOAD: u32 = 0x04;
pub const OP_TRI: u32 = 0x20;
pub const OP_SET_TEX: u32 = 0x21;
pub const OP_BEGIN_SCENE: u32 = 0x25;
pub const OP_END_SCENE: u32 = 0x26;
pub const OP_CLUT_WRITE: u32 = 0x28;

/// `TRI` flag: interpolate a colour per vertex.
pub const FLAG_GOURAUD: u8 = 1;
/// `TRI` flag: textured.
pub const FLAG_TEX: u8 = 2;
/// `TRI` flag: semi-transparent; the mode is `flags[5:4]`.
pub const FLAG_SEMI: u8 = 4;
/// `TRI` flag: Z-tested (and Z-written unless semi-transparent).
pub const FLAG_Z: u8 = 8;

/// `BEGIN_SCENE` flags: clear the draw surface, start Z at `0xFFFF`.
pub const SCENE_CLEAR: u8 = 1;
pub const SCENE_CLEAR_Z: u8 = 2;

/// The four semi-transparency equations, `TRI` flags `[5:4]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// `(B + F) / 2`
    Average = 0,
    /// `B + F`, saturating
    Add = 1,
    /// `B - F`, saturating at zero
    Subtract = 2,
    /// `B + F / 4`, saturating
    Quarter = 3,
}

impl Blend {
    pub fn from_bits(bits: u8) -> Blend {
        match bits & 3 {
            0 => Blend::Average,
            1 => Blend::Add,
            2 => Blend::Subtract,
            _ => Blend::Quarter,
        }
    }
}

/// Texture formats for `SET_TEX`. Kessel only ever sends 4bpp: that is what a
/// sprite is.
pub const TEX_4BPP: u32 = 2;

/// One screen vertex of a `TRI`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TriVertex {
    /// 12.4 pixels.
    pub x: i16,
    pub y: i16,
    pub rgb: [u8; 3],
    pub z: u16,
    /// 10.6 texels.
    pub u: u16,
    pub v: u16,
}

/// Append a `TRI`. The words per vertex follow the flags: position, then
/// colour if Gouraud, Z if Z, uv if textured.
pub fn tri(out: &mut Vec<u32>, flags: u8, color: u16, vs: &[TriVertex; 3]) {
    out.push(OP_TRI << 24 | (flags as u32) << 16 | color as u32);
    for v in vs {
        out.push((v.y as u16 as u32) << 16 | v.x as u16 as u32);
        if flags & FLAG_GOURAUD != 0 {
            out.push((v.rgb[0] as u32) << 16 | (v.rgb[1] as u32) << 8 | v.rgb[2] as u32);
        }
        if flags & FLAG_Z != 0 {
            out.push(v.z as u32);
        }
        if flags & FLAG_TEX != 0 {
            out.push((v.v as u32) << 16 | v.u as u32);
        }
    }
}

/// Words in a `TRI` with these flags, the opcode word included.
pub fn tri_len(flags: u8) -> usize {
    let per = 1
        + (flags & FLAG_GOURAUD != 0) as usize
        + (flags & FLAG_Z != 0) as usize
        + (flags & FLAG_TEX != 0) as usize;
    1 + 3 * per
}

/// A texture binding as `SET_TEX` carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexState {
    pub fmt: u32,
    pub clut_off: u8,
    /// log2 of the width and height in texels, 3..=8.
    pub wl: u8,
    pub hl: u8,
    /// SDRAM word address, a multiple of the texture's own size in words.
    pub base: u32,
}

pub fn set_tex(out: &mut Vec<u32>, t: &TexState) {
    out.push(
        OP_SET_TEX << 24
            | (t.fmt & 3) << 16
            | (t.clut_off as u32) << 8
            | (t.wl as u32 & 15) << 4
            | (t.hl as u32 & 15),
    );
    out.push(t.base & 0x1F_FFFF);
}

pub fn upload(out: &mut Vec<u32>, addr: u32, data: &[u32]) {
    out.push(OP_UPLOAD << 24 | data.len() as u32);
    out.push(addr);
    out.extend_from_slice(data);
}

/// `CLUT_WRITE` of `colors` from `index`, two to a word, low half first.
pub fn clut_write(out: &mut Vec<u32>, index: u8, colors: &[u16]) {
    out.push(OP_CLUT_WRITE << 24 | (index as u32) << 16 | (colors.len() as u32 & 0x1FF));
    for pair in colors.chunks(2) {
        let hi = pair.get(1).copied().unwrap_or(0) as u32;
        out.push(hi << 16 | pair[0] as u32);
    }
}

pub fn begin_scene(out: &mut Vec<u32>, flags: u8, color: u16) {
    out.push(OP_BEGIN_SCENE << 24 | (flags as u32) << 16 | color as u32);
}

pub fn end_scene(out: &mut Vec<u32>) {
    out.push(OP_END_SCENE << 24);
}

/// `FILL_RECT`: unsigned, so the caller has already clipped to the screen.
pub fn fill_rect(out: &mut Vec<u32>, x: u16, y: u16, w: u16, h: u16, color: u16) {
    out.push(OP_FILL_RECT << 24 | color as u32);
    out.push((y as u32) << 16 | x as u32);
    out.push((h as u32) << 16 | w as u32);
}

pub fn swap(out: &mut Vec<u32>) {
    out.push(OP_SWAP << 24);
}

/// RGB888 → RGB565, truncating — the rule the GPU uses for a Gouraud colour.
pub fn to565(r: u8, g: u8, b: u8) -> u16 {
    (r as u16 >> 3) << 11 | (g as u16 >> 2) << 5 | (b as u16 >> 3)
}

/// RGB565 → RGB888 the way the board's scan-out does it: each channel's top
/// bits repeated into its bottom ones, so full scale stays full scale.
pub fn expand565(p: u16) -> [u8; 3] {
    let (r, g, b) = (p >> 11, (p >> 5) & 0x3F, p & 0x1F);
    [
        (r << 3 | r >> 2) as u8,
        (g << 2 | g >> 4) as u8,
        (b << 3 | b >> 2) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn tri_length_matches_the_spec_table() {
        assert_eq!(tri_len(0), 4);
        assert_eq!(tri_len(FLAG_GOURAUD | FLAG_Z | FLAG_TEX), 13);
        let mut out = vec![];
        tri(
            &mut out,
            FLAG_GOURAUD | FLAG_TEX,
            0,
            &[TriVertex::default(); 3],
        );
        assert_eq!(out.len(), tri_len(FLAG_GOURAUD | FLAG_TEX));
    }

    #[test]
    fn negative_coordinates_pack_as_sixteen_bits() {
        let mut out = vec![];
        let v = TriVertex {
            x: -16,
            y: -32,
            ..Default::default()
        };
        tri(&mut out, 0, 0, &[v; 3]);
        assert_eq!(out[1], 0xFFE0_FFF0);
    }

    #[test]
    fn expand_keeps_full_scale_full() {
        assert_eq!(expand565(0xFFFF), [255, 255, 255]);
        assert_eq!(expand565(0), [0, 0, 0]);
        assert_eq!(expand565(to565(255, 0, 0)), [255, 0, 0]);
    }
}
