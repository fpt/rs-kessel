//! The master stage: a peak limiter, then a clamp that should never fire.
//!
//! Sixteen voices sum well past unity, and something has to decide what that
//! means. Hard clamping (what the skeleton did) turns a loud chord into a
//! square wave; wrapping turns it into noise. A limiter pulls the whole mix
//! down for as long as it is too loud and lets go afterwards, which is the only
//! one of the three that sounds like a mixing decision rather than a fault.
//!
//! **The master does not soft clip.** The design sketch said it should, and
//! measuring it says otherwise: [`soft_clip`] costs 6.8% at 0.5 and 19% at 0.9,
//! so putting it after the limiter would attenuate and distort every quiet mix
//! to catch peaks an instant-attack limiter has already caught. It stays where
//! it is a deliberate effect — per voice, behind `distortion`.

/// Where the limiter starts working. Under unity so that f32 rounding in the
/// gain calculation still lands below full scale.
const THRESHOLD: f32 = 0.95;

/// A cheap `tanh`-shaped saturator, exact enough and branch-light.
///
/// Padé approximation of `tanh`, which saturates at exactly ±1 when the input
/// is clamped to ±3 — beyond that the rational form turns back around, so the
/// clamp is load-bearing, not defensive.
///
/// This is a *distortion*, not a safety net: it is already 6.8% down at 0.5.
/// Use it where that curve is the point.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

/// A feed-forward peak limiter with instant attack and exponential release.
///
/// No lookahead: a lookahead limiter needs a delay line and adds latency to
/// every frame, and with instant attack there is nothing left for it to catch.
/// Gain only ever moves down instantly and up slowly, so a kick cannot pump the
/// whole mix.
///
/// Because the gain for a sample is computed from that same sample, the output
/// cannot exceed the ceiling. The final clamp is there for `NaN` and f32
/// rounding and in normal operation never does anything — which is what lets a
/// quiet mix come out bit-unchanged.
#[derive(Debug, Clone, Copy)]
pub struct Limiter {
    gain: f32,
    release_coef: f32,
    engaged: u64,
}

impl Limiter {
    /// `release_ms` is how long the mix takes to come back up after a peak.
    /// Too short and it distorts bass; too long and one explosion ducks the
    /// music for a second. 150 ms is the usual compromise.
    pub fn new(sample_rate: u32, release_ms: u16) -> Self {
        let tau = (release_ms.max(1) as f32 / 1000.0) * sample_rate as f32 / 4.6;
        Limiter {
            gain: 1.0,
            release_coef: (-1.0 / tau).exp(),
            engaged: 0,
        }
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
    }

    /// Samples the limiter has pulled down since it was built.
    ///
    /// The number an offline render reports. A peak of 0.95 is what the
    /// limiter produces *and* what a mix that merely happens to be loud
    /// produces, so peak alone cannot tell a caller whether anything was
    /// turned down; this can.
    pub fn engaged(&self) -> u64 {
        self.engaged
    }

    /// Process an interleaved stereo buffer in place.
    ///
    /// Both channels share one gain — independent per-channel limiting would
    /// swing the stereo image every time one side got loud.
    pub fn process(&mut self, out: &mut [f32]) {
        for frame in out.chunks_exact_mut(2) {
            let peak = frame[0].abs().max(frame[1].abs());
            let target = if peak > THRESHOLD {
                THRESHOLD / peak
            } else {
                1.0
            };
            if target < self.gain {
                self.gain = target; // instant attack: never overshoot
            } else {
                self.gain += (target - self.gain) * (1.0 - self.release_coef);
            }
            if self.gain < 1.0 {
                self.engaged += 1;
            }
            frame[0] = (frame[0] * self.gain).clamp(-1.0, 1.0);
            frame[1] = (frame[1] * self.gain).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_clip_is_bounded_and_linear_near_zero() {
        assert!((soft_clip(0.0)).abs() < 1e-9);
        // Small signals pass through essentially untouched.
        assert!((soft_clip(0.1) - 0.1).abs() < 0.002);
        assert!((soft_clip(-0.1) + 0.1).abs() < 0.002);
        // Nothing escapes, including the region where the rational form
        // would otherwise turn back around.
        for x in [1.0f32, 3.0, 10.0, 1e6, -1.0, -3.0, -10.0, -1e6] {
            let y = soft_clip(x);
            assert!(y.abs() <= 1.0, "soft_clip({x}) = {y}");
        }
        assert!(soft_clip(1e6) > 0.99);
    }

    #[test]
    fn soft_clip_is_monotonic() {
        // ±3 is the curve's maximum, where the derivative is zero and f32
        // rounding can step backwards by an ulp. Anything larger than that
        // would be audible fold-back.
        let mut prev = soft_clip(-3.5);
        let mut x = -3.5f32;
        while x < 3.5 {
            x += 0.01;
            let y = soft_clip(x);
            assert!(y >= prev - 1e-6, "folded back at {x}: {prev} → {y}");
            prev = y;
        }
    }

    #[test]
    fn a_quiet_mix_passes_untouched() {
        let mut lim = Limiter::new(48_000, 150);
        let mut buf: Vec<f32> = (0..2000).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
        let before = buf.clone();
        lim.process(&mut buf);
        // Exactly unchanged, not approximately: anything the master does to a
        // mix that never reaches the ceiling is colour nobody asked for.
        assert_eq!(before, buf);
    }

    #[test]
    fn engagement_counts_only_what_was_turned_down() {
        let mut lim = Limiter::new(48_000, 150);
        let mut quiet = vec![0.3f32; 2000];
        lim.process(&mut quiet);
        assert_eq!(lim.engaged(), 0, "a quiet mix was reported as limited");

        let mut loud = vec![4.0f32; 2000];
        lim.process(&mut loud);
        assert_eq!(
            lim.engaged(),
            1000,
            "every stereo frame should have counted"
        );
    }

    #[test]
    fn a_loud_mix_is_pulled_under_unity() {
        let mut lim = Limiter::new(48_000, 150);
        let mut buf: Vec<f32> = (0..48_000).map(|i| 4.0 * (i as f32 * 0.01).sin()).collect();
        lim.process(&mut buf);
        assert!(buf.iter().all(|s| s.abs() <= 1.0));
        // And it is still a signal, not a mute.
        let peak = buf.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.5, "limiter squashed the mix to {peak}");
    }

    #[test]
    fn attack_is_instant() {
        // A step from silence to well over unity must not let a single sample
        // through above the ceiling.
        let mut lim = Limiter::new(48_000, 150);
        let mut buf = vec![0.0f32; 200];
        for s in buf.iter_mut().skip(100) {
            *s = 8.0;
        }
        lim.process(&mut buf);
        assert!(buf.iter().all(|s| s.abs() <= 1.0), "a peak got through");
    }

    #[test]
    fn gain_recovers_after_a_peak() {
        let mut lim = Limiter::new(48_000, 50);
        let mut hit = vec![6.0f32; 200];
        lim.process(&mut hit);
        // Half a second of quiet audio afterwards should be back to normal.
        let mut quiet = vec![0.2f32; 24_000];
        lim.process(&mut quiet);
        let tail = quiet[quiet.len() - 1];
        assert!((tail - 0.2).abs() < 0.01, "still ducked at {tail}");
    }

    #[test]
    fn both_channels_share_one_gain() {
        // Loud on the left only: the right must duck by the same amount, or
        // the image swings whenever one side gets loud.
        let mut lim = Limiter::new(48_000, 150);
        let mut buf = vec![0.0f32; 400];
        for frame in buf.chunks_exact_mut(2) {
            frame[0] = 4.0;
            frame[1] = 0.5;
        }
        lim.process(&mut buf);
        let last = &buf[buf.len() - 2..];
        assert!(last[0] <= 1.0);
        assert!(
            last[1] < 0.4,
            "right channel ignored the left's gain: {}",
            last[1]
        );
    }
}
