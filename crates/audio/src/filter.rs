//! A 2-pole state-variable-free biquad, and the byte→Hz mapping around it.
//!
//! Games never see Hz or Q. They write `cutoff = 160` and `resonance = 40`,
//! which is both the byte world the VM already lives in and the range a model
//! writes correctly without a units table — "lower the cutoff from 180 to 80"
//! is a sentence anyone can act on; "set f0 to 431.7 Hz" is not.

/// What a voice's filter does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterMode {
    #[default]
    Off,
    Lpf,
    Hpf,
}

/// The lowest cutoff a game can ask for. Below this an LPF is just a mute.
pub const CUTOFF_MIN_HZ: f32 = 80.0;
/// The highest. Above this nothing is being filtered on a 48 kHz output.
pub const CUTOFF_MAX_HZ: f32 = 18_000.0;

/// `cutoff` byte → Hz, exponentially: `0 → 80 Hz`, `128 → ~1.2 kHz`,
/// `255 → 18 kHz`.
///
/// Exponential because pitch is: a linear sweep spends most of its travel in
/// the top octave, where nothing audible happens.
pub fn cutoff_hz(cutoff: u8) -> f32 {
    CUTOFF_MIN_HZ * (CUTOFF_MAX_HZ / CUTOFF_MIN_HZ).powf(cutoff as f32 / 255.0)
}

/// `resonance` byte → Q, from Butterworth (flat) to a strong peak.
pub fn resonance_q(resonance: u8) -> f32 {
    std::f32::consts::FRAC_1_SQRT_2 * (resonance as f32 / 255.0 * 3.5).exp2()
}

/// Biquad coefficients, already normalized by `a0`.
///
/// Computed once when a patch is compiled — nothing recomputes these per
/// sample. When an LFO reaches the cutoff it will recompute them per *block*,
/// which is why this is a plain value and not baked into the filter state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coefs {
    /// Design a filter from the game-facing bytes. `FilterMode::Off` gives
    /// `None`, so a voice with no filter pays nothing per sample.
    pub fn design(mode: FilterMode, cutoff: u8, resonance: u8, sample_rate: u32) -> Option<Coefs> {
        let mode = match mode {
            FilterMode::Off => return None,
            m => m,
        };
        let fs = sample_rate as f32;
        // Keep the corner well under Nyquist: the bilinear transform warps
        // hard as it approaches, and a game asking for 18 kHz at 44.1 kHz
        // should get a filter, not a divide-by-nearly-zero.
        let f0 = cutoff_hz(cutoff).clamp(10.0, fs * 0.45);
        let q = resonance_q(resonance);

        let w0 = std::f32::consts::TAU * f0 / fs;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * q);
        let a0 = 1.0 + alpha;

        let (b0, b1, b2) = match mode {
            FilterMode::Lpf => {
                let k = (1.0 - cos_w0) / 2.0;
                (k, 1.0 - cos_w0, k)
            }
            FilterMode::Hpf => {
                let k = (1.0 + cos_w0) / 2.0;
                (k, -(1.0 + cos_w0), k)
            }
            FilterMode::Off => unreachable!(),
        };

        Some(Coefs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: -2.0 * cos_w0 / a0,
            a2: (1.0 - alpha) / a0,
        })
    }
}

/// Two samples of filter memory, in transposed direct form II.
///
/// That form is chosen for its numerical behaviour in f32: the state holds
/// partial sums rather than raw input history, so a high-Q filter at a low
/// cutoff doesn't lose the signal in rounding.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, c: &Coefs, x: f32) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Peak of a sine at `hz` after `n` samples through the filter, after
    /// letting the transient settle.
    fn response(coefs: &Coefs, hz: f32) -> f32 {
        let mut f = Biquad::default();
        let inc = std::f32::consts::TAU * hz / SR as f32;
        // Settle first, then measure over a few cycles.
        for i in 0..SR as usize / 10 {
            f.process(coefs, (i as f32 * inc).sin());
        }
        let mut peak = 0.0f32;
        for i in SR as usize / 10..SR as usize / 5 {
            peak = peak.max(f.process(coefs, (i as f32 * inc).sin()).abs());
        }
        peak
    }

    #[test]
    fn cutoff_bytes_span_the_documented_range() {
        assert!((cutoff_hz(0) - 80.0).abs() < 0.01);
        assert!((cutoff_hz(255) - 18_000.0).abs() < 1.0);
        let mid = cutoff_hz(128);
        assert!(
            (1_000.0..1_500.0).contains(&mid),
            "midpoint landed at {mid} Hz"
        );
        // Monotonic, so "lower the cutoff" always means what it says.
        for c in 1..=255u8 {
            assert!(cutoff_hz(c) > cutoff_hz(c - 1));
        }
    }

    #[test]
    fn resonance_bytes_start_flat_and_rise() {
        assert!((resonance_q(0) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!(resonance_q(255) > 7.0);
        for r in 1..=255u8 {
            assert!(resonance_q(r) > resonance_q(r - 1));
        }
    }

    #[test]
    fn off_designs_no_filter() {
        assert!(Coefs::design(FilterMode::Off, 128, 0, SR).is_none());
    }

    #[test]
    fn lpf_passes_below_and_stops_above() {
        // Cutoff byte 128 ≈ 1.2 kHz.
        let c = Coefs::design(FilterMode::Lpf, 128, 0, SR).unwrap();
        let pass = response(&c, 200.0);
        let corner = response(&c, cutoff_hz(128));
        let stop = response(&c, 10_000.0);
        assert!((pass - 1.0).abs() < 0.05, "passband gain {pass}");
        // A Butterworth corner is -3 dB.
        assert!((corner - 0.707).abs() < 0.05, "corner gain {corner}");
        assert!(stop < 0.05, "stopband gain {stop}");
    }

    #[test]
    fn hpf_stops_below_and_passes_above() {
        let c = Coefs::design(FilterMode::Hpf, 128, 0, SR).unwrap();
        assert!(response(&c, 100.0) < 0.05);
        assert!((response(&c, 10_000.0) - 1.0).abs() < 0.05);
    }

    #[test]
    fn resonance_peaks_at_the_corner() {
        let flat = Coefs::design(FilterMode::Lpf, 128, 0, SR).unwrap();
        let peaky = Coefs::design(FilterMode::Lpf, 128, 255, SR).unwrap();
        let f0 = cutoff_hz(128);
        assert!(
            response(&peaky, f0) > response(&flat, f0) * 5.0,
            "resonance did not peak"
        );
    }

    #[test]
    fn stays_stable_at_the_extremes() {
        // Every corner of the parameter space, at both sample rates a host
        // might hand us, driven with full-scale noise.
        for sr in [44_100u32, 48_000] {
            for mode in [FilterMode::Lpf, FilterMode::Hpf] {
                for cutoff in [0u8, 1, 128, 254, 255] {
                    for res in [0u8, 128, 255] {
                        let c = Coefs::design(mode, cutoff, res, sr).unwrap();
                        let mut f = Biquad::default();
                        let mut rng = 12345u32;
                        let mut peak = 0.0f32;
                        for _ in 0..sr {
                            rng ^= rng << 13;
                            rng ^= rng >> 17;
                            rng ^= rng << 5;
                            let x = ((rng >> 8) as f32 / 8_388_608.0) - 1.0;
                            let y = f.process(&c, x);
                            assert!(y.is_finite(), "{mode:?} {cutoff} {res} @{sr} blew up");
                            peak = peak.max(y.abs());
                        }
                        // Resonance can ring above unity, but not without bound.
                        assert!(peak < 20.0, "{mode:?} {cutoff} {res} @{sr} peak {peak}");
                    }
                }
            }
        }
    }
}
