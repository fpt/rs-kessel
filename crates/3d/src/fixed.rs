//! Fixed-point maths: angles, sine, square root and 3×3 rotation matrices.
//!
//! Everything here is integer arithmetic, so a PC and an RP2350 compute the
//! same bits. Floating point would not: `f32::sin` is whatever the platform's
//! libm says, and a scene that drifts by one pixel between the agent's
//! machine and the board is the plausibly-wrong picture this project exists
//! to catch.
//!
//! - An **angle** is a `u16` binary angle: `0x10000` is a full turn, so
//!   `0x4000` is 90°. Adding to it wraps for free on a 16-bit machine.
//! - A **unit value** (a sine, a matrix entry, a normal component) is `Q14`:
//!   `16384` is 1.0.

/// `1.0` in Q14.
pub const ONE: i32 = 1 << 14;

/// One full turn of a binary angle.
pub const TURN: u32 = 1 << 16;

/// Entries in the quarter-wave sine table, past the last one included.
const QUARTER: usize = 1024;

/// π in Q30, rounded.
const PI_Q30: i64 = 3_373_259_426;

/// `sin(k · 90° / 1024)` in Q14 for `k = 0..=1024`.
///
/// Built at compile time from a Taylor series in Q30 integers. A table typed
/// in by hand would be a thousand numbers nobody can check; one computed with
/// `f64::sin` in a build script would differ by a bit wherever the host's libm
/// rounds differently. This is neither.
static SIN_TABLE: [i16; QUARTER + 1] = build_sin_table();

const fn build_sin_table() -> [i16; QUARTER + 1] {
    let mut table = [0i16; QUARTER + 1];
    let mut k = 0;
    while k <= QUARTER {
        // x = k · (π/2) / 1024, in Q30.
        let x = k as i64 * PI_Q30 / (2 * QUARTER as i64);
        let x2 = (x * x) >> 30;
        let mut term = x;
        let mut sum = x;
        let mut n = 1;
        // Up to x^15/15!: below one part in 2^30 at π/2.
        while n < 8 {
            term = -((term * x2) >> 30) / ((2 * n) * (2 * n + 1));
            sum += term;
            n += 1;
        }
        table[k] = ((sum + (1 << 15)) >> 16) as i16;
        k += 1;
    }
    table
}

/// `sin(a)` in Q14, for a binary angle `a`.
///
/// The table has 4096 steps to a turn; the low four bits of the angle
/// interpolate between two of them, so a camera turning a few units a frame
/// turns smoothly rather than in 0.09° jumps.
///
/// The angle is folded into the first quadrant *before* interpolating, so
/// `sin(-a) == -sin(a)` and `sin(180° - a) == sin(a)` hold exactly — a mesh
/// spun half a turn lands on the same pixels as one mirrored.
pub fn sin(a: u16) -> i32 {
    let (neg, a) = if a >= 0x8000 {
        (true, a - 0x8000)
    } else {
        (false, a)
    };
    let a = if a > 0x4000 { 0x8000 - a } else { a };
    let step = (a >> 4) as usize;
    let frac = (a & 15) as i32;
    let s0 = SIN_TABLE[step] as i32;
    let s1 = SIN_TABLE[(step + 1).min(QUARTER)] as i32;
    let s = s0 + (((s1 - s0) * frac) >> 4);
    if neg {
        -s
    } else {
        s
    }
}

/// `cos(a)` in Q14.
pub fn cos(a: u16) -> i32 {
    sin(a.wrapping_add(0x4000))
}

/// A binary angle from whole degrees.
pub fn degrees(deg: i32) -> u16 {
    (deg.rem_euclid(360) as u32 * TURN / 360) as u16
}

/// Integer square root, rounded down.
pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    // Bit-by-bit: no division, no float, the same answer everywhere.
    let mut x = n;
    let mut result = 0u64;
    let mut bit = 1u64 << ((63 - n.leading_zeros()) & !1);
    while bit != 0 {
        if x >= result + bit {
            x -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result
}

/// Scale `v` to length `ONE` (Q14). A zero vector stays zero.
pub fn normalize(v: [i64; 3]) -> [i32; 3] {
    // Bring big inputs down first so the squares cannot overflow.
    let mut v = v;
    while v.iter().any(|c| c.abs() > 1 << 30) {
        v = [v[0] >> 1, v[1] >> 1, v[2] >> 1];
    }
    let len = isqrt((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) as u64) as i64;
    if len == 0 {
        return [0; 3];
    }
    [
        (v[0] * ONE as i64 / len) as i32,
        (v[1] * ONE as i64 / len) as i32,
        (v[2] * ONE as i64 / len) as i32,
    ]
}

/// A 3×3 matrix of Q14 entries, row-major.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mat3(pub [[i32; 3]; 3]);

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3([[ONE, 0, 0], [0, ONE, 0], [0, 0, ONE]]);

    /// The orientation `yaw · pitch · roll`: roll about the forward axis
    /// first, then pitch, then yaw. y is up and z is forward, so:
    ///
    /// - positive **yaw** turns right (+z toward +x),
    /// - positive **pitch** looks up (+z toward +y),
    /// - positive **roll** turns the top toward +x.
    pub fn euler(yaw: u16, pitch: u16, roll: u16) -> Mat3 {
        let (sy, cy) = (sin(yaw), cos(yaw));
        let (sp, cp) = (sin(pitch), cos(pitch));
        let (sr, cr) = (sin(roll), cos(roll));
        let ry = Mat3([[cy, 0, sy], [0, ONE, 0], [-sy, 0, cy]]);
        let rx = Mat3([[ONE, 0, 0], [0, cp, sp], [0, -sp, cp]]);
        let rz = Mat3([[cr, sr, 0], [-sr, cr, 0], [0, 0, ONE]]);
        ry.mul(&rx).mul(&rz)
    }

    pub fn transpose(&self) -> Mat3 {
        let m = &self.0;
        Mat3([
            [m[0][0], m[1][0], m[2][0]],
            [m[0][1], m[1][1], m[2][1]],
            [m[0][2], m[1][2], m[2][2]],
        ])
    }

    /// `self · other`, rounded to nearest.
    pub fn mul(&self, other: &Mat3) -> Mat3 {
        let mut out = [[0i32; 3]; 3];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                let sum: i64 = (0..3)
                    .map(|k| self.0[i][k] as i64 * other.0[k][j] as i64)
                    .sum();
                *cell = ((sum + (1 << 13)) >> 14) as i32;
            }
        }
        Mat3(out)
    }

    /// `self · v` with the Q14 scale still in: the caller shifts it out,
    /// which is where it chooses how many fractional bits to keep.
    pub fn apply(&self, v: [i64; 3]) -> [i64; 3] {
        let m = &self.0;
        [
            m[0][0] as i64 * v[0] + m[0][1] as i64 * v[1] + m[0][2] as i64 * v[2],
            m[1][0] as i64 * v[0] + m[1][1] as i64 * v[1] + m[1][2] as i64 * v[2],
            m[2][0] as i64 * v[0] + m[2][1] as i64 * v[1] + m[2][2] as i64 * v[2],
        ]
    }
}

/// `v >> shift`, rounded to nearest (ties toward +∞), for a signed value.
pub fn round_shift(v: i64, shift: u32) -> i64 {
    (v + (1 << (shift - 1))) >> shift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_hits_the_landmarks() {
        assert_eq!(sin(0), 0);
        assert_eq!(sin(0x4000), ONE);
        assert_eq!(sin(0x8000), 0);
        assert_eq!(sin(0xC000), -ONE);
        assert_eq!(cos(0), ONE);
        // sin 30° = 0.5 exactly.
        assert!((sin(degrees(30)) - ONE / 2).abs() <= 1);
    }

    #[test]
    fn sine_is_odd_and_close_to_the_real_thing() {
        for a in (0..=u16::MAX).step_by(97) {
            assert_eq!(sin(a.wrapping_neg()), -sin(a), "odd at {a}");
            let want = (a as f64 / 65536.0 * core::f64::consts::TAU).sin() * ONE as f64;
            assert!(
                (sin(a) as f64 - want).abs() <= 1.5,
                "sin({a}) = {} vs {want}",
                sin(a)
            );
        }
    }

    #[test]
    fn isqrt_rounds_down() {
        for n in [
            0u64,
            1,
            2,
            3,
            4,
            15,
            16,
            17,
            1 << 40,
            (1 << 40) - 1,
            u64::MAX,
        ] {
            let r = isqrt(n);
            assert!(r * r <= n, "{n}");
            assert!((r + 1).checked_mul(r + 1).is_none_or(|sq| sq > n), "{n}");
        }
    }

    #[test]
    fn yaw_turns_forward_toward_right_and_pitch_looks_up() {
        let fwd = [0, 0, ONE as i64];
        let quarter = 0x4000;
        let yawed = Mat3::euler(quarter, 0, 0).apply(fwd).map(|c| c >> 14);
        assert_eq!(yawed, [ONE as i64, 0, 0]);
        let pitched = Mat3::euler(0, quarter, 0).apply(fwd).map(|c| c >> 14);
        assert_eq!(pitched, [0, ONE as i64, 0]);
        let up = [0, ONE as i64, 0];
        let rolled = Mat3::euler(0, 0, quarter).apply(up).map(|c| c >> 14);
        assert_eq!(rolled, [ONE as i64, 0, 0]);
    }

    #[test]
    fn a_rotation_times_its_transpose_is_close_to_identity() {
        let m = Mat3::euler(12345, 2345, 40000);
        let p = m.mul(&m.transpose());
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { ONE } else { 0 };
                assert!((p.0[i][j] - want).abs() <= 3, "{p:?}");
            }
        }
    }
}
