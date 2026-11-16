//! Kessel3D's device page (`0xf`): what a ROM on the `Kessel3D` screen draws
//! with, and how its frame becomes a picture.
//!
//! The screen is the 3D GPU's — 320×240 RGB565 — and a frame is built the way
//! the board builds one:
//!
//! ```text
//! UPLOAD    textures whose tiles changed
//! CLUT      the palette, nibble 0 transparent in every bank
//! BEGIN_SCENE (clear to cls's colour, Z from far)
//!   TRI …   opaque, in draw3d order, Z-buffered
//!   TRI …   semi-transparent, far to near
//! END_SCENE
//! FILL_RECT …  the 2D overlay
//! SWAP
//! ```
//!
//! **The 2D API is an overlay.** Every 2D call — `pset`, `rect`, `spr`, `map`,
//! `text`, all of it — draws exactly as it does on a 2D screen, into an index
//! layer that is sent over the scene as runs of `FILL_RECT`. So a HUD is
//! written the way it always was, and it lands *on top* of the 3D scene
//! whatever order the calls came in: the GPU draws a scene's triangles at
//! `END_SCENE` and forbids 2D commands inside one, so "3D under, 2D over" is
//! the hardware's rule before it is this one's. Only the 2D *lighting* ops do
//! nothing here; the 3D GPU has no light layer, and a 3D scene lights itself.
//!
//! The picture comes from [`kessel_3d::SoftGpu`] running the words — never from
//! the engine's own idea of what it drew. The words in [`Scene3d::words`] are
//! the ones a board would be sent, so what an agent sees on the PC is what
//! the FPGA would show.

use kessel_3d::cmd;
use kessel_3d::{clut_from_palette, Gte, SoftGpu, TexCache};

/// What a 3D frame cost, reported in the observation record.
///
/// The limits are the GPU's, and an agent cannot see them any other way: a
/// scene past 4,096 triangles, or a tile past 1,024, simply loses geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats3d {
    /// Triangles sent.
    pub tris: u32,
    /// Faces culled as back-facing.
    pub culled: u32,
    /// Triangles lost to the GPU's limits: past 4,096 in the scene, or
    /// (triangle, tile) pairs past 1,024 in a 32×32 tile.
    pub dropped: u32,
    /// `draw3d` calls whose address held no mesh, or a mesh with a bad index.
    pub bad_meshes: u32,
    /// Textures that found no room in the GPU's texture memory.
    pub tex_overflow: u32,
    /// Command words the frame took — what the link has to carry.
    pub words: u32,
}

#[derive(Clone)]
pub struct Scene3d {
    gte: Gte,
    cache: TexCache,
    gpu: SoftGpu,
    /// Which overlay pixels the 2D API drew this frame.
    pub(crate) drawn: Vec<bool>,
    /// `cls`'s colour: the scene clears to it.
    pub(crate) clear: u8,
    /// Argument latches `0xf1..=0xf6`; a command register commits them.
    args: [u16; 8],
    words: Vec<u32>,
    stats: Stats3d,
}

impl Scene3d {
    pub fn new(pixels: usize) -> Self {
        Scene3d {
            gte: Gte::new(),
            cache: TexCache::default(),
            gpu: SoftGpu::new(),
            drawn: vec![false; pixels],
            clear: 0,
            args: [0; 8],
            words: Vec::new(),
            stats: Stats3d::default(),
        }
    }

    /// A write to the `0xf` page. Registers 1–6 latch arguments; 8–13 are
    /// commands that commit them, each taking its first argument as the value
    /// written — the call's first argument is the one a stack machine hands
    /// over last, the same shape as `pal` and `light`.
    pub fn write(&mut self, reg: u8, val: u16, mem: &[u8], palette: &[(u8, u8, u8); 256]) {
        let a = self.args;
        match reg {
            0x1..=0x6 => self.args[reg as usize] = val,
            // draw3d(mesh, x, y, z, yaw, pitch, roll)
            0x8 => {
                let bytes = mem.get(val as usize..).unwrap_or(&[]);
                self.gte.draw(
                    bytes,
                    [a[1] as i16, a[2] as i16, a[3] as i16],
                    [a[4], a[5], a[6]],
                    palette,
                );
            }
            // camera3d(x, y, z, yaw, pitch, roll)
            0x9 => self
                .gte
                .camera([val as i16, a[1] as i16, a[2] as i16], a[3], a[4], a[5]),
            // light3d(dx, dy, dz, r, g, b)
            0xa => self.gte.light(
                [val as i16, a[1] as i16, a[2] as i16],
                [a[3] as u8, a[4] as u8, a[5] as u8],
            ),
            // ambient3d(r, g, b)
            0xb => self.gte.ambient([val as u8, a[1] as u8, a[2] as u8]),
            // fog3d(near, far, colour): the colour is a palette index, read now.
            0xc => {
                let (r, g, b) = palette[(a[2] & 0xff) as usize];
                self.gte.fog(val, a[1], [r, g, b]);
            }
            // lens3d(fov_degrees, near)
            0xd => self.gte.lens(val, a[1]),
            _ => {}
        }
    }

    pub fn begin_frame(&mut self) {
        self.gte.begin_frame();
        self.drawn.fill(false);
    }

    /// Build the frame's words and run them. `overlay` is the 2D index layer
    /// (`width` wide) and `tileset_base` where the sprite sheet starts.
    pub fn end_frame(
        &mut self,
        mem: &[u8],
        tileset_base: u16,
        palette: &[(u8, u8, u8); 256],
        overlay: &[u8],
        width: usize,
    ) {
        let rgb565 = |i: u8| {
            let (r, g, b) = palette[i as usize];
            cmd::to565(r, g, b)
        };
        let tile = |t: u16| {
            let mut out = [0u8; 32];
            let at = tileset_base.wrapping_add(t.wrapping_mul(32)) as usize;
            if let Some(src) = mem.get(at..at + 32) {
                out.copy_from_slice(src);
            }
            out
        };
        self.words.clear();
        self.gte.emit(
            &mut self.words,
            &mut self.cache,
            &tile,
            &clut_from_palette(palette),
            rgb565(self.clear),
        );
        // The overlay, a run of one colour at a time. A HUD is a few hundred
        // of these; the HSTX link carries over 600 KiB a frame.
        for (y, row) in overlay.chunks_exact(width).enumerate() {
            let drawn = &self.drawn[y * width..(y + 1) * width];
            let mut x = 0;
            while x < width {
                if !drawn[x] {
                    x += 1;
                    continue;
                }
                let c = row[x];
                let start = x;
                while x < width && drawn[x] && row[x] == c {
                    x += 1;
                }
                cmd::fill_rect(
                    &mut self.words,
                    start as u16,
                    y as u16,
                    (x - start) as u16,
                    1,
                    rgb565(c),
                );
            }
        }
        cmd::swap(&mut self.words);
        self.gpu.run(&self.words);
        let g = self.gte.stats();
        let r = self.gpu.stats();
        self.stats = Stats3d {
            tris: g.tris,
            culled: g.culled,
            dropped: g.dropped + r.scene_dropped + r.tile_dropped,
            bad_meshes: g.bad_meshes,
            tex_overflow: g.tex_overflow,
            words: self.words.len() as u32,
        };
    }

    /// The picture on show: RGB565, 320×240.
    pub fn display(&self) -> &[u16] {
        self.gpu.display()
    }

    /// The last frame's command words — what a board with the FPGA's 3D GPU
    /// would be sent for it.
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    pub fn stats(&self) -> Stats3d {
        self.stats
    }

    /// Forget which textures the GPU holds, so the next frame uploads them
    /// all. For a host whose real GPU lost its memory; the software one never
    /// does.
    pub fn invalidate_textures(&mut self) {
        self.cache.invalidate();
    }
}
