//! What a live host actually does: `VmPlayer::tick_collecting` into an engine.
//!
//! The offline renderer and the player reach the synth by different routes, and
//! a game that renders correctly offline but is silent in `kessel run` is the
//! failure neither side's own tests can see.

use kessel_audio::{samples_per_frame, AudioEngine, SynthConfig};
use kessel_vm::player::VmPlayer;

fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |a, s| a.max(s.abs()))
}

/// Drive a player the way `kessel run` does and return what came out.
fn play(src: &str, frames: usize) -> (f32, u64) {
    let player = VmPlayer::new();
    let err = player.load(src.to_string(), "game.lua".to_string());
    assert!(err.is_empty(), "{err}");

    let mut engine = AudioEngine::new(SynthConfig::default());
    engine.set_bank(player.sound_bank());

    let spf = samples_per_frame(engine.sample_rate()) as usize;
    let mut block = vec![0.0f32; spf * 2];
    let mut worst = 0.0f32;
    for _ in 0..frames {
        player.tick_collecting(0, &mut |ev| engine.handle(ev));
        engine.render(&mut block);
        worst = worst.max(peak(&block));
    }
    (worst, engine.synth_stats().started)
}

#[test]
fn music_started_in_init_reaches_a_live_host() {
    let (peak, started) = play(
        r#"
        instrument bass { wave = triangle  attack = 0  decay = 0  sustain = 200  release = 20 }
        track theme { tempo = 4  bass = "36 - 43 -" }
        function init() music(theme) end
        function update() end
        function draw() cls(0) end
        "#,
        60,
    );
    assert!(started > 2, "the track never started: {started} notes");
    assert!(peak > 0.1, "the track was silent: peak {peak}");
}

#[test]
fn the_shipped_shooter_plays_its_music() {
    // The corpus game, through the host path, exactly as `kessel run` drives it.
    let (peak, started) = play(include_str!("../../../games/shooter.lua"), 120);
    assert!(started > 10, "shooter started only {started} notes");
    assert!(peak > 0.05, "shooter was silent: peak {peak}");
}

#[test]
fn sound_effects_reach_a_live_host_too() {
    let (peak, started) = play(
        r#"
        instrument blip { wave = square  attack = 0  decay = 60  sustain = 0 }
        sfx ping { inst = blip  notes = "72" }
        local t: word
        function update() t = t + 1  if t == 3 then sfx(ping) end end
        function draw() cls(0) end
        "#,
        30,
    );
    assert_eq!(started, 1);
    assert!(peak > 0.1, "the effect was silent: peak {peak}");
}
