//! Render the synth to WAV files you can listen to.
//!
//! The tests in this crate check what a machine can check — frequency, range,
//! envelope lifetime, determinism. None of that says whether a kick sounds like
//! a kick, so:
//!
//! ```sh
//! cargo run -p kessel-audio --example preview          # → target/audio-preview
//! cargo run -p kessel-audio --example preview /tmp/snd
//! ```

use std::path::PathBuf;

use kessel_audio::{
    samples_per_frame, wav, AudioEvent, Patch, Synth, SynthConfig, Waveform, MAX_VOICES,
};

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/audio-preview"));
    std::fs::create_dir_all(&dir).expect("create output directory");

    // One instrument per waveform, then the sounds those waveforms are for.
    let lead = |wave| Patch {
        wave,
        attack_ms: 4,
        decay_ms: 120,
        sustain: 170,
        release_ms: 120,
        ..Patch::default()
    };
    let kick = Patch {
        wave: Waveform::Sine,
        attack_ms: 0,
        decay_ms: 90,
        sustain: 0,
        pitch_env: 36,
        pitch_decay_ms: 60,
        volume: 255,
        ..Patch::default()
    };
    let snare = Patch {
        wave: Waveform::Noise,
        attack_ms: 0,
        decay_ms: 110,
        sustain: 0,
        pitch_env: -12,
        pitch_decay_ms: 40,
        ..Patch::default()
    };
    let laser = Patch {
        wave: Waveform::Saw,
        attack_ms: 0,
        decay_ms: 180,
        sustain: 0,
        pitch_env: 48,
        pitch_decay_ms: 120,
        ..Patch::default()
    };
    let coin = Patch {
        wave: Waveform::Square,
        attack_ms: 0,
        decay_ms: 70,
        sustain: 0,
        ..Patch::default()
    };

    for (name, patch) in [
        ("sine", lead(Waveform::Sine)),
        ("triangle", lead(Waveform::Triangle)),
        ("saw", lead(Waveform::Saw)),
        ("square", lead(Waveform::Square)),
        ("noise", lead(Waveform::Noise)),
    ] {
        // A rising phrase, so pitch tracking is audible.
        write(
            &dir,
            name,
            &[patch],
            &[(0, 60), (12, 64), (24, 67), (36, 72)].map(|(at, note)| (at, play(0, note, 200, 20))),
            120,
        );
    }

    write(
        &dir,
        "kick",
        &[kick],
        &[(0, ()), (30, ()), (60, ()), (90, ())].map(|(at, ())| (at, play(0, 36, 255, 6))),
        120,
    );
    write(
        &dir,
        "snare",
        &[snare],
        &[(0, ()), (30, ()), (60, ()), (90, ())].map(|(at, ())| (at, play(0, 60, 220, 6))),
        120,
    );
    write(&dir, "laser", &[laser], &[(0, play(0, 72, 220, 8))], 60);
    write(
        &dir,
        "coin",
        &[coin],
        &[(0, play(0, 84, 220, 4)), (5, play(0, 91, 220, 10))],
        60,
    );

    // Every voice at once: what stealing and the missing master stage sound
    // like. Velocities are modest because sixteen voices sum well past unity
    // and the limiter is not built yet.
    let chord = lead(Waveform::Triangle);
    let mut events = Vec::new();
    for i in 0..MAX_VOICES + 4 {
        events.push((i as u32 * 3, play(0, 48 + (i as u8 * 3), 90, 90)));
    }
    write(&dir, "polyphony", &[chord], &events, 180);

    println!("previews are in {}", dir.display());
}

fn play(inst: u8, note: u8, vel: u8, frames: u16) -> AudioEvent {
    AudioEvent::Play {
        inst,
        note,
        vel,
        frames,
    }
}

/// Render `frames` console frames, applying each event at its frame, and write
/// the result as a WAV. This is the offline renderer in miniature — one frame
/// of samples at a time, events landing on frame boundaries.
fn write(
    dir: &std::path::Path,
    name: &str,
    instruments: &[Patch],
    events: &[(u32, AudioEvent)],
    frames: u32,
) {
    let cfg = SynthConfig::default();
    let mut synth = Synth::new(cfg);
    synth.set_instruments(instruments);

    let spf = samples_per_frame(cfg.sample_rate) as usize;
    let mut block = vec![0.0f32; spf * 2];
    let mut all = Vec::with_capacity(frames as usize * spf * 2);
    for frame in 0..frames {
        for (at, ev) in events {
            if *at == frame {
                synth.handle(*ev);
            }
        }
        synth.render(&mut block);
        all.extend_from_slice(&block);
    }

    let path = dir.join(format!("{name}.wav"));
    std::fs::write(&path, wav::encode_pcm16(cfg.sample_rate, 2, &all)).expect("write wav");
    let peak = all.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    let stats = synth.stats();
    println!(
        "{:10} peak {:.2}  started {}  stolen {}  → {}",
        name,
        peak,
        stats.started,
        stats.stolen,
        path.display()
    );
}
