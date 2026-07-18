//! Varvara-lite device layer, reached through the VM's `DEI`/`DEO` opcodes.
//!
//! A port is a single byte: the high nibble selects the device, the low nibble
//! selects a register. Devices are deliberately tiny and deterministic so a
//! whole run can be snapshotted and replayed:
//!
//! | dev | name    | registers |
//! |-----|---------|-----------|
//! | 0x0 | system  | 0 halt · 1 pal-index · 2 pal-r · 3 pal-g · 4 pal-b (commit) |
//! | 0x1 | screen  | 0 vector · 1 x · 2 y · 3 color · 4 pixel · 5 sprite · 6 cls |
//! | 0x2 | gamepad | 0 buttons (read) |
//! | 0x3 | rng     | 0 next (read) / set-seed (write) |
//! | 0x4 | storage | 0 addr · 1 read · 2 write |
//! | 0x5 | debug   | 0 ent-x · 1 ent-y · 2 ent-commit(tag) |
//! | 0x6 | console | 0 write-byte |

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

/// An entity record the running game reports to the debug port for observation.
/// These are authored by the game (not inferred), so the harness can expose or
/// hide internal state per experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entity {
    pub tag: u16,
    pub x: u16,
    pub y: u16,
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
    /// The frame vector the game installed via `screen/vector`; 0 = none.
    pub frame_vector: u16,
    /// Set when the game writes a non-zero value to `system/halt`.
    pub halt_requested: bool,
    /// Entities reported this frame (cleared each frame by the console).
    pub entities: Vec<Entity>,
    /// Bytes written to the console this frame (cleared each frame).
    pub console: Vec<u8>,
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
            frame_vector: 0,
            halt_requested: false,
            entities: Vec::new(),
            console: Vec::new(),
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
        }
    }

    /// Read a device register (`DEI`).
    pub fn read(&mut self, port: u8) -> u16 {
        let dev = port >> 4;
        let reg = port & 0x0f;
        match (dev, reg) {
            (0x2, 0x0) => self.gamepad as u16,
            (0x3, 0x0) => self.next_rand(),
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
                    let c = (val & 0x0f) as u8;
                    for px in self.framebuffer.iter_mut() {
                        *px = c;
                    }
                }
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
            _ => {}
        }
    }

    /// Clear the per-frame reported state (entities + console output).
    pub fn begin_frame(&mut self, buttons: u8) {
        self.gamepad = buttons;
        self.entities.clear();
        self.console.clear();
        self.halt_requested = false;
    }

    fn put_pixel(&mut self, x: u16, y: u16, color: u8) {
        let (x, y) = (x as usize, y as usize);
        if x < SCREEN_DIM && y < SCREEN_DIM {
            self.framebuffer[y * SCREEN_DIM + x] = color & 0x0f;
        }
    }

    /// Blit an 8×8, 4-bits-per-pixel sprite from `mem[addr..addr+32]` at the
    /// current (sx, sy). Two pixels per byte (high nibble = left). Colour 0 is
    /// transparent.
    fn blit_sprite(&mut self, addr: u16, mem: &[u8]) {
        for row in 0u16..8 {
            for col in 0u16..8 {
                let byte_addr = addr.wrapping_add(row * 4 + col / 2) as usize;
                let byte = mem.get(byte_addr).copied().unwrap_or(0);
                let ci = if col % 2 == 0 { byte >> 4 } else { byte & 0x0f };
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
