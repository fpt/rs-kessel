//! The parity scene, shared by `tests/parity.rs` and `examples/parity_scene.rs`.

use kessel_3d::cmd;
use kessel_3d::mesh::{compile, SpriteRef};
use kessel_3d::{clut_from_palette, frame_crc, Gte, SoftGpu, TexCache};

/// A palette with a little of everything: greys, a colour ramp, and a true
/// black at a non-zero nibble (the CLUT has to keep it opaque).
pub fn palette() -> [(u8, u8, u8); 256] {
    let mut p = [(0u8, 0u8, 0u8); 256];
    for (i, c) in p.iter_mut().enumerate() {
        let i = i as u32;
        *c = (
            (i * 37 % 256) as u8,
            (i * 91 % 256) as u8,
            (i * 13 % 256) as u8,
        );
    }
    p[1] = (0, 0, 0);
    p[7] = (255, 255, 255);
    p[8] = (200, 40, 40);
    p[9] = (40, 160, 60);
    p[10] = (60, 80, 220);
    p
}

/// Sheet tile `t`: a diagonal ramp of nibbles with a transparent notch, so
/// texel order, the nibble-0 rule and the bank offset all show.
pub fn tile(t: u16) -> [u8; 32] {
    let mut out = [0u8; 32];
    for row in 0..8u16 {
        for col in 0..8u16 {
            let n = if row == col && t % 2 == 0 {
                0
            } else {
                ((row + col + t) % 15 + 1) as u8
            };
            let byte = &mut out[(row * 4 + col / 2) as usize];
            if col % 2 == 0 {
                *byte |= n << 4;
            } else {
                *byte |= n;
            }
        }
    }
    out
}

fn sprite(name: &str) -> Option<SpriteRef> {
    match name {
        "crate" => Some(SpriteRef {
            tile: 4,
            w: 16,
            h: 16,
        }),
        "glass" => Some(SpriteRef {
            tile: 8,
            w: 8,
            h: 8,
        }),
        "strip" => Some(SpriteRef {
            tile: 16,
            w: 32,
            h: 8,
        }),
        _ => None,
    }
}

fn mesh(src: &str) -> Vec<u8> {
    let lines: Vec<(usize, Vec<&str>)> = src
        .lines()
        .enumerate()
        .map(|(n, l)| (n + 1, l.split_whitespace().collect()))
        .collect();
    compile(&lines, &sprite).unwrap_or_else(|e| panic!("{e:?}"))
}

const CUBE: &str = "
v -20 -20 -20
v  20 -20 -20
v  20  20 -20
v -20  20 -20
v -20 -20  20
v  20 -20  20
v  20  20  20
v -20  20  20
tex crate
f 1 2 3 4
f 6 5 8 7
tex crate 3
f 5 1 4 8
f 2 6 7 3
color 9
f 4 3 7 8
smooth
color 10
f 5 6 2 1
";

/// A floor big enough to run behind the eye and off both sides.
const FLOOR: &str = "
v -400 0 -400
v  400 0 -400
v  400 0  400
v -400 0  400
vt 0 0
vt 63 0
vt 63 63
vt 0 63
tex strip 1
f 1/1 4/4 3/3 2/2
color 8
f 1 2 3 4
";

const PANES: &str = "
v -30 -15 0
v  30 -15 0
v  30  15 0
v -30  15 0
double
blend avg
color 7
f 1 2 3 4
blend add
tex glass 2
f 1 2 3 4
";

pub fn words() -> Vec<u32> {
    let pal = palette();
    let mut gte = Gte::new();
    gte.camera([10, 60, -170], 0xF800, 0xF400, 0x0200);
    gte.lens(75, 12);
    gte.ambient([60, 60, 70]);
    gte.light([-3, 5, -2], [200, 190, 170]);
    gte.fog(150, 600, [30, 30, 60]);
    gte.begin_frame();
    let cube = mesh(CUBE);
    let floor = mesh(FLOOR);
    let panes = mesh(PANES);
    gte.draw(&floor, [0, -30, 0], [0, 0, 0], &pal);
    gte.draw(&cube, [0, 0, 0], [0x1800, 0x0C00, 0x0400], &pal);
    gte.draw(&cube, [90, 10, 140], [0x5000, 0, 0x2000], &pal);
    gte.draw(&cube, [-120, 0, 400], [0x2000, 0x3000, 0], &pal);
    // All four equations, one pane each, overlapping the cubes.
    for (k, mode) in ["avg", "add", "sub", "quarter"].iter().enumerate() {
        let src = PANES.replace("blend avg", &format!("blend {mode}"));
        let m = mesh(&src);
        gte.draw(
            &m,
            [-60 + 40 * k as i16, 20, -30 + 10 * k as i16],
            [0x0800 * k as u16, 0, 0],
            &pal,
        );
    }
    gte.draw(&panes, [0, 0, -60], [0, 0, 0x1000], &pal);

    let mut out = Vec::new();
    let mut cache = TexCache::default();
    gte.emit(
        &mut out,
        &mut cache,
        &tile,
        &clut_from_palette(&pal),
        cmd::to565(20, 20, 40),
    );
    cmd::fill_rect(&mut out, 4, 4, 40, 6, 0xFFE0);
    cmd::swap(&mut out);
    let stats = gte.stats();
    assert!(stats.tris > 30, "{stats:?}");
    out
}

pub fn render_crc(words: &[u32]) -> u32 {
    let mut gpu = SoftGpu::new();
    gpu.run(words);
    assert_eq!(gpu.stats().errors, 0);
    frame_crc(gpu.display())
}
