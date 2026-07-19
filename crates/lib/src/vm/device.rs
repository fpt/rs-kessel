//! Varvara-lite device layer, reached through the VM's `DEI`/`DEO` opcodes.
//!
//! A port is a single byte: the high nibble selects the device, the low nibble
//! selects a register. Devices are deliberately tiny and deterministic so a
//! whole run can be snapshotted and replayed:
//!
//! | dev | name    | registers |
//! |-----|---------|-----------|
//! | 0x0 | system  | 0 halt · 1 pal-index · 2 pal-r · 3 pal-g · 4 pal-b (commit) |
//! | 0x1 | screen  | 0 vector · 1 x · 2 y · 3 color · 4 pixel · 5 sprite · 6 cls · 7 cam-x · 8 cam-y · 9 flags · a blit-id · b tileset-base · c glyph(code) |
//! | 0x2 | gamepad | 0 buttons · 1 pressed (edge) · 2 released (edge) — all read |
//! | 0x3 | rng     | 0 next (read) / set-seed (write) |
//! | 0x4 | storage | 0 addr · 1 read · 2 write |
//! | 0x5 | debug   | 0 ent-x · 1 ent-y · 2 ent-commit(tag) |
//! | 0x6 | console | 0 write-byte |
//! | 0x7 | tilemap | 0 base · 1 width · 2 tx · 3 ty · 4 sx · 5 sy · 6 tw · 7 th · 8 draw |
//! | 0x8 | time    | 0 frame-count (read) |
//! | 0x9 | sound   | 0 sfx(id) · 1 music(id) · 2 music-stop (recorded, no audio yet) |
//! | 0xa | sprn    | 0 base-id · 1 w · 2 h · 3 draw (w×h block at screen x/y) |

/// Screen edge length in pixels (square framebuffer).
pub const SCREEN_DIM: usize = 128;
/// Total framebuffer cells (palette indices).
pub const SCREEN_PIXELS: usize = SCREEN_DIM * SCREEN_DIM;

/// Gamepad button bits, matching the values the host pushes.
pub const BTN_LEFT: u8 = 0x01;
pub const BTN_RIGHT: u8 = 0x02;
pub const BTN_UP: u8 = 0x04;
pub const BTN_DOWN: u8 = 0x08;
pub const BTN_A: u8 = 0x10;
pub const BTN_B: u8 = 0x20;
pub const BTN_START: u8 = 0x40;
pub const BTN_SELECT: u8 = 0x80;

/// The 3×5 pixel rows for one glyph (ASCII `code`), top to bottom. Each row is
/// 3 bits — bit 2 is the leftmost column. Covers `A-Z` (lowercase folds up),
/// `0-9`, space, and `: ! . -`; anything else is blank. Small enough to inline
/// scores, titles and `GAME OVER` without a font ROM.
fn glyph_rows(code: u8) -> [u8; 5] {
    let c = code.to_ascii_uppercase();
    match c {
        b'0' => [7, 5, 5, 5, 7],
        b'1' => [2, 6, 2, 2, 7],
        b'2' => [7, 1, 7, 4, 7],
        b'3' => [7, 1, 7, 1, 7],
        b'4' => [5, 5, 7, 1, 1],
        b'5' => [7, 4, 7, 1, 7],
        b'6' => [7, 4, 7, 5, 7],
        b'7' => [7, 1, 2, 2, 2],
        b'8' => [7, 5, 7, 5, 7],
        b'9' => [7, 5, 7, 1, 7],
        b'A' => [7, 5, 7, 5, 5],
        b'B' => [6, 5, 6, 5, 6],
        b'C' => [7, 4, 4, 4, 7],
        b'D' => [6, 5, 5, 5, 6],
        b'E' => [7, 4, 7, 4, 7],
        b'F' => [7, 4, 7, 4, 4],
        b'G' => [7, 4, 5, 5, 7],
        b'H' => [5, 5, 7, 5, 5],
        b'I' => [7, 2, 2, 2, 7],
        b'J' => [1, 1, 1, 5, 7],
        b'K' => [5, 6, 4, 6, 5],
        b'L' => [4, 4, 4, 4, 7],
        b'M' => [5, 7, 7, 5, 5],
        b'N' => [5, 7, 5, 5, 5],
        b'O' => [7, 5, 5, 5, 7],
        b'P' => [7, 5, 7, 4, 4],
        b'Q' => [7, 5, 5, 7, 3],
        b'R' => [7, 5, 7, 6, 5],
        b'S' => [7, 4, 7, 1, 7],
        b'T' => [7, 2, 2, 2, 2],
        b'U' => [5, 5, 5, 5, 7],
        b'V' => [5, 5, 5, 5, 2],
        b'W' => [5, 5, 7, 7, 5],
        b'X' => [5, 5, 2, 5, 5],
        b'Y' => [5, 5, 2, 2, 2],
        b'Z' => [7, 1, 2, 4, 7],
        b':' => [0, 2, 0, 2, 0],
        b'!' => [2, 2, 2, 0, 2],
        b'.' => [0, 0, 0, 0, 2],
        b'-' => [0, 0, 7, 0, 0],
        _ => [0, 0, 0, 0, 0], // space + unknown
    }
}

/// An entity record the running game reports to the debug port for observation.
/// These are authored by the game (not inferred), so the harness can expose or
/// hide internal state per experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entity {
    pub tag: u16,
    pub x: u16,
    pub y: u16,
}

/// What a game asked the (silent, for now) sound device to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    Sfx,
    Music,
    MusicStop,
}

/// A sound trigger the running game emitted this frame. The VM stays
/// deterministic and headless — these are recorded for the observation record
/// (so the agent sees that a sound "played") and for a future host audio path;
/// nothing is synthesized yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundEvent {
    pub kind: SoundKind,
    pub id: u16,
}

/// The default 16-colour palette (PICO-8's), RGB. Index 0 is treated as
/// transparent when blitting sprites.
pub const DEFAULT_PALETTE: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00),
    (0x1D, 0x2B, 0x53),
    (0x7E, 0x25, 0x53),
    (0x00, 0x87, 0x51),
    (0xAB, 0x52, 0x36),
    (0x5F, 0x57, 0x4F),
    (0xC2, 0xC3, 0xC7),
    (0xFF, 0xF1, 0xE8),
    (0xFF, 0x00, 0x4D),
    (0xFF, 0xA3, 0x00),
    (0xFF, 0xEC, 0x27),
    (0x00, 0xE4, 0x36),
    (0x29, 0xAD, 0xFF),
    (0x83, 0x76, 0x9C),
    (0xFF, 0x77, 0xA8),
    (0xFF, 0xCC, 0xAA),
];

/// All device-side state. Cloned wholesale for snapshots.
#[derive(Clone)]
pub struct Devices {
    /// 128×128 palette-index framebuffer.
    pub framebuffer: Vec<u8>,
    pub palette: [(u8, u8, u8); 16],
    /// Current gamepad button bitfield.
    pub gamepad: u8,
    /// Gamepad bitfield from the *previous* frame, for edge detection
    /// (`btnp`/`btnr`).
    pub prev_gamepad: u8,
    /// Frames elapsed since power-on (wraps at 65536; drives blink/timers).
    pub frame_count: u16,
    /// The frame vector the game installed via `screen/vector`; 0 = none.
    pub frame_vector: u16,
    /// Set when the game writes a non-zero value to `system/halt`.
    pub halt_requested: bool,
    /// Entities reported this frame (cleared each frame by the console).
    pub entities: Vec<Entity>,
    /// Bytes written to the console this frame (cleared each frame).
    pub console: Vec<u8>,
    /// Sound triggers emitted this frame (cleared each frame). Recorded only;
    /// no audio is synthesized yet.
    pub sound: Vec<SoundEvent>,
    /// Persistent storage (survives resets? no — power-on state, but survives frames).
    pub storage: [u8; 256],

    // --- transient device registers ---
    rng_state: u32,
    storage_addr: u8,
    // pending screen coords / colour
    sx: u16,
    sy: u16,
    scolor: u8,
    // pending palette entry being built
    pidx: u8,
    pr: u8,
    pg: u8,
    // pending entity coords
    ex: u16,
    ey: u16,
    // camera offset (world→screen translation), signed
    cam_x: i16,
    cam_y: i16,
    // flip flags for the next sprite blit: bit0 = flip-x, bit1 = flip-y
    sprite_flags: u8,
    // base address of the sprite sheet (32-byte 4bpp tiles) for blit-by-id
    tileset_base: u16,
    // tilemap device (page 0x7): base/width of the tile-id grid + pending region
    map_base: u16,
    map_width: u16,
    map_tx: u16,
    map_ty: u16,
    map_sx: u16,
    map_sy: u16,
    map_tw: u16,
    map_th: u16,
    // composite-sprite device (page 0xa): base tile id + block size
    sprn_id: u16,
    sprn_w: u16,
    sprn_h: u16,
}

impl Default for Devices {
    fn default() -> Self {
        Self::new()
    }
}

impl Devices {
    pub fn new() -> Self {
        Devices {
            framebuffer: vec![0u8; SCREEN_PIXELS],
            palette: DEFAULT_PALETTE,
            gamepad: 0,
            prev_gamepad: 0,
            frame_count: 0,
            frame_vector: 0,
            halt_requested: false,
            entities: Vec::new(),
            console: Vec::new(),
            sound: Vec::new(),
            storage: [0u8; 256],
            rng_state: 0x1234_5678,
            storage_addr: 0,
            sx: 0,
            sy: 0,
            scolor: 0,
            pidx: 0,
            pr: 0,
            pg: 0,
            ex: 0,
            ey: 0,
            cam_x: 0,
            cam_y: 0,
            sprite_flags: 0,
            tileset_base: 0,
            map_base: 0,
            map_width: 0,
            map_tx: 0,
            map_ty: 0,
            map_sx: 0,
            map_sy: 0,
            map_tw: 0,
            map_th: 0,
            sprn_id: 0,
            sprn_w: 0,
            sprn_h: 0,
        }
    }

    /// Read a device register (`DEI`).
    pub fn read(&mut self, port: u8) -> u16 {
        let dev = port >> 4;
        let reg = port & 0x0f;
        match (dev, reg) {
            (0x2, 0x0) => self.gamepad as u16,
            // just-pressed this frame (rising edge): held now, not held before.
            (0x2, 0x1) => (self.gamepad & !self.prev_gamepad) as u16,
            // just-released this frame (falling edge): held before, not now.
            (0x2, 0x2) => (!self.gamepad & self.prev_gamepad) as u16,
            (0x3, 0x0) => self.next_rand(),
            // time device: frames since power-on.
            (0x8, 0x0) => self.frame_count,
            (0x4, 0x1) => self.storage[self.storage_addr as usize] as u16,
            _ => 0,
        }
    }

    /// Write a device register (`DEO`). `mem` is the VM's main memory, needed by
    /// the sprite blitter which reads tile data from it.
    pub fn write(&mut self, port: u8, val: u16, mem: &[u8]) {
        let dev = port >> 4;
        let reg = port & 0x0f;
        match dev {
            0x0 => match reg {
                0x0 => {
                    if val != 0 {
                        self.halt_requested = true;
                    }
                }
                0x1 => self.pidx = val as u8,
                0x2 => self.pr = val as u8,
                0x3 => self.pg = val as u8,
                0x4 => {
                    self.palette[(self.pidx & 0x0f) as usize] = (self.pr, self.pg, val as u8);
                }
                _ => {}
            },
            0x1 => match reg {
                0x0 => self.frame_vector = val,
                0x1 => self.sx = val,
                0x2 => self.sy = val,
                0x3 => self.scolor = (val & 0x0f) as u8,
                0x4 => self.put_pixel(self.sx, self.sy, self.scolor),
                0x5 => self.blit_sprite(val, mem),
                0x6 => {
                    // cls ignores the camera — it clears the whole screen.
                    let c = (val & 0x0f) as u8;
                    for px in self.framebuffer.iter_mut() {
                        *px = c;
                    }
                }
                0x7 => self.cam_x = val as i16,
                0x8 => self.cam_y = val as i16,
                0x9 => self.sprite_flags = val as u8,
                // Blit sprite by id from the tileset (base + id*32).
                0xa => {
                    let addr = self
                        .tileset_base
                        .wrapping_add((val & 0xff).wrapping_mul(32));
                    self.blit_sprite(addr, mem);
                }
                0xb => self.tileset_base = val,
                // Draw one 3×5 font glyph (ascii code = val) at (sx,sy) in scolor.
                0xc => self.draw_glyph(val as u8),
                _ => {}
            },
            0x3 => {
                if reg == 0x0 && val != 0 {
                    self.rng_state = val as u32;
                }
            }
            0x4 => match reg {
                0x0 => self.storage_addr = val as u8,
                0x2 => self.storage[self.storage_addr as usize] = val as u8,
                _ => {}
            },
            0x5 => match reg {
                0x0 => self.ex = val,
                0x1 => self.ey = val,
                0x2 => self.entities.push(Entity {
                    tag: val,
                    x: self.ex,
                    y: self.ey,
                }),
                _ => {}
            },
            0x6 => {
                if reg == 0x0 {
                    self.console.push(val as u8);
                }
            }
            // Tilemap device: set base/width/region, then draw a tw×th block of
            // tiles from the map (ids in memory) via the sprite sheet.
            0x7 => match reg {
                0x0 => self.map_base = val,
                0x1 => self.map_width = val,
                0x2 => self.map_tx = val,
                0x3 => self.map_ty = val,
                0x4 => self.map_sx = val,
                0x5 => self.map_sy = val,
                0x6 => self.map_tw = val,
                0x7 => self.map_th = val,
                0x8 => self.draw_map(mem),
                _ => {}
            },
            // Sound device: record a trigger (no audio synthesized yet).
            0x9 => match reg {
                0x0 => self.sound.push(SoundEvent { kind: SoundKind::Sfx, id: val }),
                0x1 => self.sound.push(SoundEvent { kind: SoundKind::Music, id: val }),
                0x2 => self.sound.push(SoundEvent { kind: SoundKind::MusicStop, id: 0 }),
                _ => {}
            },
            // Composite-sprite device: draw a w×h block of sheet tiles at the
            // pending screen (sx,sy) with the current sprite flags.
            0xa => match reg {
                0x0 => self.sprn_id = val,
                0x1 => self.sprn_w = val,
                0x2 => self.sprn_h = val,
                0x3 => self.draw_sprn(mem),
                _ => {}
            },
            _ => {}
        }
    }

    /// Draw a `sprn_w × sprn_h` block of sheet tiles anchored at the pending
    /// screen `(sx,sy)`. Tile ids are row-major and contiguous from `sprn_id`
    /// (id at col/row = `sprn_id + row*w + col`), each 8 px cell blitted from the
    /// tileset. The current `sprite_flags` (flip) apply to every tile; the block
    /// layout itself is not mirrored.
    fn draw_sprn(&mut self, mem: &[u8]) {
        let (base_x, base_y) = (self.sx, self.sy);
        for row in 0..self.sprn_h {
            for col in 0..self.sprn_w {
                let id = self
                    .sprn_id
                    .wrapping_add(row.wrapping_mul(self.sprn_w))
                    .wrapping_add(col);
                let addr = self.tileset_base.wrapping_add(id.wrapping_mul(32));
                self.sx = base_x.wrapping_add(col.wrapping_mul(8));
                self.sy = base_y.wrapping_add(row.wrapping_mul(8));
                self.blit_sprite(addr, mem);
            }
        }
        self.sx = base_x;
        self.sy = base_y;
    }

    /// Draw one 3×5 glyph (`code` = ASCII) at the pending `(sx,sy)` in `scolor`.
    /// The caller advances x between characters (4 px/char). Unknown codes draw
    /// nothing. Subject to the camera (via `put_pixel`) — reset `camera(0,0)`
    /// before HUD text.
    fn draw_glyph(&mut self, code: u8) {
        let (x0, y0, color) = (self.sx, self.sy, self.scolor);
        for (r, bits) in glyph_rows(code).iter().enumerate() {
            for col in 0..3u16 {
                if bits & (0x4 >> col) != 0 {
                    self.put_pixel(x0 + col, y0 + r as u16, color);
                }
            }
        }
    }

    /// Draw the pending map region: for each cell, read the tile id from
    /// `mem[map_base + (map_ty+row)*map_width + (map_tx+col)]` and blit that
    /// sheet tile at screen `(map_sx+col*8, map_sy+row*8)`. The camera applies
    /// (via `put_pixel`); sprite flip is forced off for map tiles.
    fn draw_map(&mut self, mem: &[u8]) {
        let saved_flags = self.sprite_flags;
        self.sprite_flags = 0;
        for row in 0..self.map_th {
            for col in 0..self.map_tw {
                let mx = self.map_tx.wrapping_add(col);
                let my = self.map_ty.wrapping_add(row);
                let cell = self
                    .map_base
                    .wrapping_add(my.wrapping_mul(self.map_width))
                    .wrapping_add(mx) as usize;
                let id = mem.get(cell).copied().unwrap_or(0) as u16;
                let addr = self.tileset_base.wrapping_add(id.wrapping_mul(32));
                self.sx = self.map_sx.wrapping_add(col.wrapping_mul(8));
                self.sy = self.map_sy.wrapping_add(row.wrapping_mul(8));
                self.blit_sprite(addr, mem);
            }
        }
        self.sprite_flags = saved_flags;
    }

    /// Clear the per-frame reported state (entities + console output) and
    /// advance per-frame input/timing: the previous gamepad snapshot (for
    /// `btnp`/`btnr` edge detection) and the frame counter.
    pub fn begin_frame(&mut self, buttons: u8) {
        self.prev_gamepad = self.gamepad;
        self.gamepad = buttons;
        self.frame_count = self.frame_count.wrapping_add(1);
        self.entities.clear();
        self.console.clear();
        self.sound.clear();
        self.halt_requested = false;
    }

    /// Draw a pixel at world coordinate (x, y). The camera offset translates
    /// world→screen; off-screen pixels are clipped.
    fn put_pixel(&mut self, x: u16, y: u16, color: u8) {
        let sx = x as i32 - self.cam_x as i32;
        let sy = y as i32 - self.cam_y as i32;
        let dim = SCREEN_DIM as i32;
        if (0..dim).contains(&sx) && (0..dim).contains(&sy) {
            self.framebuffer[sy as usize * SCREEN_DIM + sx as usize] = color & 0x0f;
        }
    }

    /// Blit an 8×8, 4-bits-per-pixel sprite from `mem[addr..addr+32]` at the
    /// current (sx, sy). Two pixels per byte (high nibble = left). Colour 0 is
    /// transparent. `sprite_flags` bit0/bit1 mirror the source horizontally /
    /// vertically. The destination position is subject to the camera (via
    /// `put_pixel`).
    fn blit_sprite(&mut self, addr: u16, mem: &[u8]) {
        let flip_x = self.sprite_flags & 0x01 != 0;
        let flip_y = self.sprite_flags & 0x02 != 0;
        for row in 0u16..8 {
            for col in 0u16..8 {
                let src_col = if flip_x { 7 - col } else { col };
                let src_row = if flip_y { 7 - row } else { row };
                let byte_addr = addr.wrapping_add(src_row * 4 + src_col / 2) as usize;
                let byte = mem.get(byte_addr).copied().unwrap_or(0);
                let ci = if src_col % 2 == 0 { byte >> 4 } else { byte & 0x0f };
                if ci != 0 {
                    self.put_pixel(self.sx.wrapping_add(col), self.sy.wrapping_add(row), ci);
                }
            }
        }
    }

    /// xorshift32 — deterministic given the seed, returns the low 16 bits.
    fn next_rand(&mut self) -> u16 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        (x & 0xffff) as u16
    }

    /// Expand the palette-index framebuffer into packed RGBA (4 bytes/pixel),
    /// for the host window and the PNG encoder.
    pub fn framebuffer_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SCREEN_PIXELS * 4);
        for &idx in &self.framebuffer {
            let (r, g, b) = self.palette[(idx & 0x0f) as usize];
            out.extend_from_slice(&[r, g, b, 0xff]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_and_cls() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x13, 5, &mem); // color = 5
        d.write(0x11, 10, &mem); // x = 10
        d.write(0x12, 20, &mem); // y = 20
        d.write(0x14, 0, &mem); // pixel
        assert_eq!(d.framebuffer[20 * SCREEN_DIM + 10], 5);
        d.write(0x16, 3, &mem); // cls color 3
        assert!(d.framebuffer.iter().all(|&p| p == 3));
    }

    #[test]
    fn pixel_out_of_bounds_ignored() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x11, 999, &mem);
        d.write(0x12, 999, &mem);
        d.write(0x14, 0, &mem); // should be a no-op, not a panic
        assert!(d.framebuffer.iter().all(|&p| p == 0));
    }

    #[test]
    fn sprite_blit_4bpp() {
        let mut d = Devices::new();
        // Top row: pixels [1,2,3,4,0,0,0,0] -> bytes 0x12, 0x34, 0x00, 0x00
        let mut mem = [0u8; 64];
        mem[0] = 0x12;
        mem[1] = 0x34;
        d.write(0x11, 0, &mem); // x
        d.write(0x12, 0, &mem); // y
        d.write(0x15, 0, &mem); // sprite from addr 0
        assert_eq!(d.framebuffer[0], 1);
        assert_eq!(d.framebuffer[1], 2);
        assert_eq!(d.framebuffer[2], 3);
        assert_eq!(d.framebuffer[3], 4);
        assert_eq!(d.framebuffer[4], 0); // transparent stays background
    }

    #[test]
    fn camera_offset_translates_and_clips() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x17, 10, &mem); // cam_x = 10
        d.write(0x18, 5, &mem); // cam_y = 5
        d.write(0x13, 6, &mem); // colour 6
        // World (12,7) -> screen (2,2).
        d.write(0x11, 12, &mem);
        d.write(0x12, 7, &mem);
        d.write(0x14, 0, &mem); // pixel
        assert_eq!(d.framebuffer[2 * SCREEN_DIM + 2], 6);
        // World (0,0) -> screen (-10,-5) -> clipped.
        d.write(0x11, 0, &mem);
        d.write(0x12, 0, &mem);
        d.write(0x14, 0, &mem);
        assert_eq!(d.framebuffer[0], 0);
    }

    #[test]
    fn sprite_flip_x() {
        let mut d = Devices::new();
        // Top row pixels [1,2,3,4,0,0,0,0].
        let mut mem = [0u8; 64];
        mem[0] = 0x12;
        mem[1] = 0x34;
        d.write(0x19, 0x01, &mem); // flip-x
        d.write(0x11, 0, &mem);
        d.write(0x12, 0, &mem);
        d.write(0x15, 0, &mem); // sprite
        // Mirrored: col 7 <- src 0 (=1), col 4 <- src 3 (=4).
        assert_eq!(d.framebuffer[7], 1);
        assert_eq!(d.framebuffer[6], 2);
        assert_eq!(d.framebuffer[5], 3);
        assert_eq!(d.framebuffer[4], 4);
        assert_eq!(d.framebuffer[0], 0); // src 7 was transparent
    }

    #[test]
    fn blit_sprite_by_id() {
        let mut d = Devices::new();
        let mut mem = [0u8; 128];
        // Tile id 1 lives at base(0) + 1*32 = 32; its top-left pixel is colour 5.
        mem[32] = 0x50;
        d.write(0x1b, 0, &mem); // tileset base = 0
        d.write(0x11, 3, &mem); // x = 3
        d.write(0x12, 4, &mem); // y = 4
        d.write(0x1a, 1, &mem); // blit id 1
        assert_eq!(d.framebuffer[4 * SCREEN_DIM + 3], 5);
    }

    #[test]
    fn draw_map_blits_cells() {
        let mut d = Devices::new();
        let mut mem = vec![0u8; 256];
        // Sheet at 100: tile 1's top-left pixel is colour 5 (tile 0 stays blank).
        let sheet = 100usize;
        mem[sheet + 32] = 0x50;
        d.write(0x1b, sheet as u16, &mem); // tileset base
        // 2x2 map at 0, width 2: cells [0,1 / 1,0].
        mem[0] = 0;
        mem[1] = 1;
        mem[2] = 1;
        mem[3] = 0;
        d.write(0x70, 0, &mem); // map base
        d.write(0x71, 2, &mem); // map width
        d.write(0x72, 0, &mem); // tx
        d.write(0x73, 0, &mem); // ty
        d.write(0x74, 0, &mem); // sx
        d.write(0x75, 0, &mem); // sy
        d.write(0x76, 2, &mem); // tw
        d.write(0x77, 2, &mem); // th
        d.write(0x78, 0, &mem); // draw
        assert_eq!(d.framebuffer[8], 5); // cell (1,0) = tile 1 -> screen (8,0)
        assert_eq!(d.framebuffer[8 * SCREEN_DIM], 5); // cell (0,1) -> screen (0,8)
        assert_eq!(d.framebuffer[0], 0); // cell (0,0) = tile 0 (blank)
    }

    #[test]
    fn gamepad_and_entities() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.begin_frame(BTN_LEFT | BTN_A);
        assert_eq!(d.read(0x20), (BTN_LEFT | BTN_A) as u16);
        d.write(0x50, 34, &mem); // ent x
        d.write(0x51, 110, &mem); // ent y
        d.write(0x52, 1, &mem); // commit tag 1
        assert_eq!(d.entities, vec![Entity { tag: 1, x: 34, y: 110 }]);
    }

    #[test]
    fn rng_is_deterministic_and_seedable() {
        let mut a = Devices::new();
        let mut b = Devices::new();
        let mem = [0u8; 8];
        a.write(0x30, 42, &mem);
        b.write(0x30, 42, &mem);
        let sa: Vec<u16> = (0..5).map(|_| a.read(0x30)).collect();
        let sb: Vec<u16> = (0..5).map(|_| b.read(0x30)).collect();
        assert_eq!(sa, sb);
        // Different seed -> different stream (overwhelmingly likely).
        let mut c = Devices::new();
        c.write(0x30, 43, &mem);
        let sc: Vec<u16> = (0..5).map(|_| c.read(0x30)).collect();
        assert_ne!(sa, sc);
    }

    #[test]
    fn storage_persists() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x40, 7, &mem); // addr = 7
        d.write(0x42, 99, &mem); // write 99
        d.write(0x40, 7, &mem); // addr = 7
        assert_eq!(d.read(0x41), 99);
    }

    #[test]
    fn palette_write_commits_on_blue() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x01, 2, &mem); // index 2
        d.write(0x02, 0x11, &mem); // r
        d.write(0x03, 0x22, &mem); // g
        d.write(0x04, 0x33, &mem); // b -> commit
        assert_eq!(d.palette[2], (0x11, 0x22, 0x33));
    }

    #[test]
    fn framebuffer_rgba_uses_palette() {
        let mut d = Devices::new();
        let mem = [0u8; 8];
        d.write(0x16, 7, &mem); // cls to color 7 = (0xFF,0xF1,0xE8)
        let rgba = d.framebuffer_rgba();
        assert_eq!(&rgba[0..4], &[0xFF, 0xF1, 0xE8, 0xFF]);
        assert_eq!(rgba.len(), SCREEN_PIXELS * 4);
    }
}
