//! The two shared effects: one chorus, one reverb, for the whole mix.
//!
//! **Never per voice.** Sixteen reverbs cost sixteen times as much to produce
//! an effect nobody can localize anyway — a room is a property of the room, not
//! of each instrument in it. So voices declare how much of themselves to *send*
//! (`reverb = 40`, `chorus = 15`), the sends sum into two buses, and each bus
//! goes through one unit:
//!
//! ```text
//! voice ─┬─ dry ───────────────────────────► master
//!        ├─ × reverb ─► [reverb bus] ─► Reverb ─► master
//!        └─ × chorus ─► [chorus bus] ─► Chorus ─► master
//! ```
//!
//! Both units are stateful (delay lines), so they are *not* affected by how the
//! caller chunks its render — the state carries across calls. That is the
//! property `Synth` relies on to keep its output independent of the device's
//! buffer size.

/// Delay-line lengths from the Freeverb tuning, in samples at 44.1 kHz.
///
/// Freeverb uses eight combs per channel; four is the design decision recorded
/// in `docs/SYNTH.md` — the remaining four thicken the tail on a real room and
/// cost as much as the first four on a phone.
const COMB_TUNING: [usize; 4] = [1116, 1188, 1277, 1356];
const ALLPASS_TUNING: [usize; 2] = [556, 441];

/// Samples the right channel's delays are offset by, so the two sides do not
/// produce identical tails and collapse the image to the middle.
const STEREO_SPREAD: usize = 23;

/// Level the bus is attenuated by on the way in.
///
/// Load-bearing, not taste. Four combs run in *parallel* and their outputs sum,
/// so an un-scaled network has a gain of roughly `4 / (1 - feedback)` — at the
/// largest room that is two hundred, and a reverb that leaves the bus a hundred
/// times louder than the dry mix would duck the whole game through the master
/// limiter every time a sound was sent to it. Freeverb scales its input for the
/// same reason.
const INPUT_GAIN: f32 = 0.06;

/// A delay line with one write/read head, sized once.
struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    fn new(len: usize) -> Self {
        Delay {
            buf: vec![0.0; len.max(1)],
            pos: 0,
        }
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        // The head too, not just the samples. Every read here is *relative* to
        // `pos`, so leaving it put happens to produce identical audio today —
        // but "cleared" meaning "half cleared" is the kind of state that makes
        // a later change (an absolute index, a second read head) quietly
        // non-deterministic across a `Panic`, and the failure would show up as
        // a replay that does not match.
        self.pos = 0;
    }

    #[inline]
    fn advance(&mut self) {
        self.pos += 1;
        if self.pos >= self.buf.len() {
            self.pos = 0;
        }
    }
}

/// A lowpass-feedback comb filter — the part of a Freeverb that rings.
struct Comb {
    delay: Delay,
    /// One-pole lowpass in the feedback path: each pass round the loop loses
    /// more treble, which is what makes a tail sound like a room rather than a
    /// metal pipe.
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Comb {
            delay: Delay::new(len),
            store: 0.0,
        }
    }

    fn clear(&mut self) {
        self.delay.clear();
        self.store = 0.0;
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.delay.buf[self.delay.pos];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.delay.buf[self.delay.pos] = input + self.store * feedback;
        self.delay.advance();
        out
    }
}

/// An all-pass — passes every frequency at the same level but smears them in
/// time, which is what turns four ringing combs into something diffuse.
struct Allpass {
    delay: Delay,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Allpass {
            delay: Delay::new(len),
        }
    }

    fn clear(&mut self) {
        self.delay.clear();
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        const FEEDBACK: f32 = 0.5;
        let buffered = self.delay.buf[self.delay.pos];
        let out = -input + buffered;
        self.delay.buf[self.delay.pos] = input + buffered * FEEDBACK;
        self.delay.advance();
        out
    }
}

/// One channel of the reverb: four combs in parallel, then two all-passes.
struct ReverbChannel {
    combs: [Comb; 4],
    allpasses: [Allpass; 2],
}

impl ReverbChannel {
    fn new(sample_rate: u32, offset: usize) -> Self {
        let scale = |n: usize| (n * sample_rate as usize) / 44_100 + offset;
        ReverbChannel {
            combs: std::array::from_fn(|i| Comb::new(scale(COMB_TUNING[i]))),
            allpasses: std::array::from_fn(|i| Allpass::new(scale(ALLPASS_TUNING[i]))),
        }
    }

    fn clear(&mut self) {
        for c in &mut self.combs {
            c.clear();
        }
        for a in &mut self.allpasses {
            a.clear();
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let input = input * INPUT_GAIN;
        // Parallel: every comb sees the same input and their outputs sum.
        let mut out = 0.0;
        for c in &mut self.combs {
            out += c.process(input, feedback, damp);
        }
        // Series: each all-pass diffuses what the previous one produced.
        for a in &mut self.allpasses {
            out = a.process(out);
        }
        out
    }
}

/// A Schroeder/Freeverb-shaped room.
///
/// Not convolution: a game console does not want to carry impulse responses,
/// and four combs into two all-passes is both far cheaper and far more
/// controllable from two bytes.
pub struct Reverb {
    left: ReverbChannel,
    right: ReverbChannel,
    feedback: f32,
    damp: f32,
}

/// Feedback at `room_size = 0`. Short but still a room, not a click.
const ROOM_MIN: f32 = 0.7;
/// Feedback at `room_size = 255`. Below 1.0 by enough that the tail always
/// decays — a reverb that can be driven into self-oscillation is a bug
/// waiting for someone to write `reverb_size = 255`.
const ROOM_MAX: f32 = 0.98;

impl Reverb {
    pub fn new(sample_rate: u32) -> Self {
        Reverb {
            left: ReverbChannel::new(sample_rate, 0),
            right: ReverbChannel::new(sample_rate, STEREO_SPREAD),
            feedback: room_feedback(128),
            damp: damping(128),
        }
    }

    /// `room_size` and `damping` are the game-facing bytes.
    pub fn set(&mut self, room_size: u8, damp: u8) {
        self.feedback = room_feedback(room_size);
        self.damp = damping(damp);
    }

    /// Drop the tail. Used by `Panic`: after a rewind the previous timeline's
    /// room is still ringing over a game that never made a sound in it.
    pub fn clear(&mut self) {
        self.left.clear();
        self.right.clear();
    }

    /// Process an interleaved stereo bus **in place**.
    pub fn process(&mut self, bus: &mut [f32]) {
        for frame in bus.chunks_exact_mut(2) {
            // Both channels are fed the summed input — a send bus is not a
            // stereo source, and feeding each side only its own half would
            // make a hard-panned voice reverberate on one side only.
            let input = (frame[0] + frame[1]) * 0.5;
            frame[0] = self.left.process(input, self.feedback, self.damp);
            frame[1] = self.right.process(input, self.feedback, self.damp);
        }
    }
}

fn room_feedback(room_size: u8) -> f32 {
    ROOM_MIN + (ROOM_MAX - ROOM_MIN) * (room_size as f32 / 255.0)
}

fn damping(damp: u8) -> f32 {
    // Capped below 1.0: a fully damped comb would never pass anything.
    (damp as f32 / 255.0) * 0.95
}

/// Longest delay the chorus can reach, in milliseconds. Past ~30 ms it stops
/// sounding like doubling and starts sounding like an echo.
const CHORUS_MAX_MS: f32 = 30.0;
/// Where the delay sits with no modulation.
const CHORUS_CENTRE_MS: f32 = 15.0;

/// One modulated delay line, read twice with the two LFO phases a quarter cycle
/// apart.
///
/// The phase offset is the whole trick: with both sides modulated identically a
/// chorus is just a slightly detuned copy in mono, and the width people expect
/// from the effect comes entirely from the two sides disagreeing.
pub struct Chorus {
    buf: Vec<f32>,
    pos: usize,
    phase: f32,
    /// LFO cycles per sample.
    rate: f32,
    /// Modulation depth in samples.
    depth: f32,
    centre: f32,
    sample_rate: f32,
}

impl Chorus {
    pub fn new(sample_rate: u32) -> Self {
        let len = ((CHORUS_MAX_MS / 1000.0) * sample_rate as f32) as usize + 2;
        let mut c = Chorus {
            buf: vec![0.0; len],
            pos: 0,
            phase: 0.0,
            rate: 0.0,
            depth: 0.0,
            centre: (CHORUS_CENTRE_MS / 1000.0) * sample_rate as f32,
            sample_rate: sample_rate as f32,
        };
        c.set(40, 80);
        c
    }

    /// `rate` byte → 0.05–8 Hz, `depth` byte → up to 10 ms of sweep.
    pub fn set(&mut self, rate: u8, depth: u8) {
        let hz = 0.05 + (rate as f32 / 255.0) * 7.95;
        self.rate = hz / self.sample_rate;
        let max_depth = (10.0 / 1000.0) * self.sample_rate;
        self.depth = (depth as f32 / 255.0) * max_depth;
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.phase = 0.0;
    }

    /// Read the delay line `delay` samples back, interpolating between the two
    /// neighbouring samples.
    ///
    /// The interpolation is what makes a *moving* delay usable: stepping to the
    /// nearest whole sample as the LFO sweeps produces a click per step, which
    /// on a slow sweep is a rhythmic tick rather than an obvious fault.
    #[inline]
    fn read(&self, delay: f32) -> f32 {
        let len = self.buf.len();
        let d = delay.clamp(1.0, len as f32 - 2.0);
        let back = d.floor();
        let frac = d - back;
        let i = (self.pos + len - back as usize) % len;
        let j = (i + len - 1) % len;
        self.buf[i] * (1.0 - frac) + self.buf[j] * frac
    }

    /// Process an interleaved stereo bus **in place**.
    pub fn process(&mut self, bus: &mut [f32]) {
        for frame in bus.chunks_exact_mut(2) {
            let input = (frame[0] + frame[1]) * 0.5;
            self.buf[self.pos] = input;

            let tau = std::f32::consts::TAU;
            let l = self.centre + self.depth * (self.phase * tau).sin();
            // A quarter cycle apart.
            let r = self.centre + self.depth * ((self.phase + 0.25) * tau).sin();
            frame[0] = self.read(l);
            frame[1] = self.read(r);

            self.pos = (self.pos + 1) % self.buf.len();
            self.phase += self.rate;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A short burst of full-scale noise, then silence — an impulse-ish input
    /// with enough energy to measure a tail from.
    fn burst(frames: usize, on: usize) -> Vec<f32> {
        let mut rng = 0x1234_5678u32;
        (0..frames * 2)
            .map(|i| {
                if i / 2 >= on {
                    return 0.0;
                }
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                ((rng >> 8) as f32 / 8_388_608.0) - 1.0
            })
            .collect()
    }

    fn rms(buf: &[f32]) -> f32 {
        if buf.is_empty() {
            return 0.0;
        }
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
    }

    #[test]
    fn reverb_rings_after_the_input_stops() {
        let mut r = Reverb::new(SR);
        let mut bus = burst(SR as usize / 2, 480); // 10 ms in, half a second long
        r.process(&mut bus);
        // Well after the input ended there is still signal, and it is decaying.
        let early = rms(&bus[4800 * 2..9600 * 2]);
        let late = rms(&bus[19200 * 2..24000 * 2]);
        assert!(early > 1e-4, "no tail at all: {early}");
        assert!(late < early, "the tail is not decaying: {early} → {late}");
        assert!(bus.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_bigger_room_rings_longer() {
        let tail = |size: u8| {
            let mut r = Reverb::new(SR);
            r.set(size, 100);
            let mut bus = burst(SR as usize, 480);
            r.process(&mut bus);
            rms(&bus[(SR as usize / 2) * 2..])
        };
        let small = tail(0);
        let big = tail(255);
        assert!(big > small * 2.0, "room_size did nothing: {small} vs {big}");
    }

    #[test]
    fn damping_takes_the_treble_out_of_the_tail() {
        let brightness = |damp: u8| {
            let mut r = Reverb::new(SR);
            r.set(200, damp);
            let mut bus = burst(SR as usize / 2, 480);
            r.process(&mut bus);
            // Mean absolute difference between neighbouring samples: high
            // frequencies move a lot between samples, low ones barely.
            let tail: Vec<f32> = bus[9600 * 2..].chunks_exact(2).map(|f| f[0]).collect();
            let slew: f32 =
                tail.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / tail.len() as f32;
            slew / rms(&tail).max(1e-9)
        };
        assert!(
            brightness(255) < brightness(0) * 0.8,
            "damping did not darken the tail: {} vs {}",
            brightness(0),
            brightness(255)
        );
    }

    #[test]
    fn the_reverb_returns_a_usable_level() {
        // A send that comes back a hundred times louder than it went in is not
        // a reverb, it is a fault the master limiter would hide by ducking the
        // whole game. Across the room range the return should be in the same
        // neighbourhood as the input.
        for size in [0u8, 128, 255] {
            let mut r = Reverb::new(SR);
            r.set(size, 100);
            let input = burst(SR as usize / 2, SR as usize / 2);
            let mut bus = input.clone();
            r.process(&mut bus);
            let ratio = rms(&bus) / rms(&input);
            assert!(
                (0.05..3.0).contains(&ratio),
                "room {size} returned {ratio}× the input"
            );
        }
    }

    #[test]
    fn the_reverb_never_runs_away() {
        // The largest room, driven with sustained DC — the worst case a comb
        // network can be given, and not something an oscillator produces. It
        // has to stay bounded and finite; the master limiter is the backstop
        // for the level, not for stability.
        let mut r = Reverb::new(SR);
        r.set(255, 0);
        let mut worst = 0.0f32;
        let mut bus = vec![0.0f32; 4800 * 2];
        for _ in 0..100 {
            bus.fill(1.0);
            r.process(&mut bus);
            worst = worst.max(bus.iter().fold(0.0f32, |a, s| a.max(s.abs())));
            assert!(bus.iter().all(|s| s.is_finite()));
        }
        assert!(worst < 20.0, "reverb ran away to {worst}");
    }

    #[test]
    fn clearing_is_the_same_as_starting_over() {
        // The invariant `Panic` needs: after a clear, each unit behaves exactly
        // as a newly built one. Stronger than "the tail is gone", and it is
        // what keeps a rewound timeline reproducible — two runs that panic at
        // different moments must sound the same afterwards.
        let probe = |used: Option<usize>| {
            let mut r = Reverb::new(SR);
            let mut c = Chorus::new(SR);
            if let Some(frames) = used {
                let mut pre = burst(frames, frames);
                r.process(&mut pre);
                c.process(&mut pre);
                r.clear();
                c.clear();
            }
            let mut a = vec![0.0f32; 8000 * 2];
            a[0] = 1.0;
            a[1] = 1.0;
            let mut b = a.clone();
            r.process(&mut a);
            c.process(&mut b);
            (a, b)
        };
        let fresh = probe(None);
        // Two different amounts of prior use, so a leftover head position would
        // land somewhere different in each.
        assert!(
            probe(Some(1000)) == fresh,
            "clear left reverb/chorus state behind"
        );
        assert!(
            probe(Some(1731)) == fresh,
            "clear left reverb/chorus state behind"
        );
    }

    #[test]
    fn panic_drops_the_tail() {
        let mut r = Reverb::new(SR);
        let mut bus = burst(4800, 480);
        r.process(&mut bus);
        r.clear();
        let mut after = vec![0.0f32; 4800 * 2];
        r.process(&mut after);
        assert_eq!(after.iter().fold(0.0f32, |a, s| a.max(s.abs())), 0.0);
    }

    #[test]
    fn the_two_sides_of_the_chorus_differ() {
        // The whole point of the effect. Identical channels would mean the
        // phase offset was lost, and it would sound like a detune in mono.
        let mut c = Chorus::new(SR);
        c.set(120, 200);
        let mut bus: Vec<f32> = (0..SR as usize / 4)
            .flat_map(|i| {
                let s = (i as f32 * 0.02).sin() * 0.5;
                [s, s]
            })
            .collect();
        c.process(&mut bus);
        let diff: f32 = bus
            .chunks_exact(2)
            .map(|f| (f[0] - f[1]).abs())
            .sum::<f32>()
            / (bus.len() / 2) as f32;
        assert!(diff > 0.01, "both sides came out the same: {diff}");
        assert!(bus.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_deeper_chorus_moves_more() {
        // Depth sweeps the delay, and a swept delay is vibrato — so measure the
        // pitch wobble directly, by how much the zero-crossing spacing varies.
        //
        // A windowed-amplitude metric was tried first and is no good here: a
        // steady sine's RMS wanders between windows anyway, and the noise floor
        // of that measurement is the same size as the effect.
        let wobble = |depth: u8| {
            let mut c = Chorus::new(SR);
            c.set(60, depth);
            let mut bus: Vec<f32> = (0..SR as usize)
                .flat_map(|i| {
                    let s = (i as f32 * 0.05).sin() * 0.5;
                    [s, s]
                })
                .collect();
            c.process(&mut bus);
            let left: Vec<f32> = bus.chunks_exact(2).map(|f| f[0]).collect();
            let mut gaps = Vec::new();
            let mut last = None;
            for i in 1..left.len() {
                if left[i - 1] <= 0.0 && left[i] > 0.0 {
                    if let Some(prev) = last {
                        gaps.push((i - prev) as f32);
                    }
                    last = Some(i);
                }
            }
            let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
            (gaps.iter().map(|g| (g - mean).powi(2)).sum::<f32>() / gaps.len() as f32).sqrt()
        };
        let flat = wobble(0);
        let deep = wobble(255);
        assert!(
            deep > flat * 3.0 + 0.05,
            "depth produced no vibrato: {flat} vs {deep}"
        );
    }

    #[test]
    fn chorus_output_is_bounded() {
        let mut c = Chorus::new(SR);
        c.set(255, 255);
        let mut bus = vec![1.0f32; 48_000 * 2];
        c.process(&mut bus);
        let peak = bus.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak <= 1.001, "chorus amplified to {peak}");
        assert!(bus.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn chunking_does_not_change_either_effect() {
        // `Synth` renders in fixed internal chunks while a device asks for
        // whatever it likes; both units have to be indifferent to that, or the
        // buffer size a sound card happens to pick would change the sound.
        let reverb = |chunk: usize| {
            let mut r = Reverb::new(SR);
            let mut bus = burst(4800, 480);
            for part in bus.chunks_mut(chunk * 2) {
                r.process(part);
            }
            bus
        };
        assert_eq!(
            reverb(4800),
            reverb(137),
            "reverb depends on the block size"
        );

        let chorus = |chunk: usize| {
            let mut c = Chorus::new(SR);
            let mut bus = burst(4800, 480);
            for part in bus.chunks_mut(chunk * 2) {
                c.process(part);
            }
            bus
        };
        assert_eq!(
            chorus(4800),
            chorus(137),
            "chorus depends on the block size"
        );
    }
}
