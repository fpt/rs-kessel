//! Source → bank → sound: the whole path a game's `sfx(boom)` takes.
//!
//! The unit tests on either side check their own half — `kessel-audio` proves
//! the grammar and the engine, `luax.rs` proves the declarations compile. This
//! is the one that fails if the two halves disagree about what an id means,
//! which is the failure neither of them can see.

use kessel_audio::{samples_per_frame, AudioEngine, SynthConfig, Waveform};
use kessel_vm::VmConsole;

const GAME: &str = r#"
instrument kick {
  wave = sine
  attack = 0  decay = 90  sustain = 0
  pitch_env = 36  pitch_decay = 60
  volume = 255
}

instrument blip {
  wave = square
  attack = 0  decay = 40  sustain = 0
  pan = -60
}

sfx boom {
  inst = kick
  speed = 3
  notes = "40 - 36"
}

sfx coin {
  inst = blip
  speed = 2
  notes = "84 91"
}

local t: word

function update()
  t = t + 1
  if t == 5 then sfx(boom) end
  if t == 20 then sfx(coin) end
end

function draw()
  cls(0)
end
"#;

fn loaded() -> VmConsole {
    let mut c = VmConsole::new();
    c.write_source("game.lua", GAME).unwrap();
    let built = c.assemble("game.lua").unwrap();
    assert!(built.ok(), "diagnostics: {:?}", built.diagnostics);
    c.load_rom("game.lua").unwrap();
    c
}

#[test]
fn declarations_become_a_bank_on_the_console() {
    let c = loaded();
    let bank = c.sound_bank();
    assert_eq!(bank.instrument_names, ["kick", "blip"]);
    assert_eq!(bank.sfx_names, ["boom", "coin"]);

    // The keys mean the same thing here as in a standalone patch file, because
    // `kessel-audio` applied them in both cases.
    assert_eq!(bank.instruments[0].wave, Waveform::Sine);
    assert_eq!(bank.instruments[0].pitch_env, 36);
    assert_eq!(bank.instruments[1].wave, Waveform::Square);
    assert_eq!(bank.instruments[1].pan, -60);

    // `inst = kick` resolved to the instrument declared above it.
    assert_eq!(bank.sfx[0].inst, 0);
    assert_eq!(bank.sfx[1].inst, 1);
    assert_eq!(bank.sfx[1].speed, 2);
}

#[test]
fn sfx_by_name_emits_the_right_id() {
    let mut c = loaded();
    // `sfx(boom)` — a name, not a number — must reach the device as id 0, and
    // `sfx(coin)` as id 1.
    for _ in 0..4 {
        assert!(c.run_frame(0).sound.is_empty());
    }
    let boom = c.run_frame(0); // the game's 5th update
    assert_eq!(boom.sound.len(), 1);
    assert_eq!(boom.sound[0].id, 0);

    for _ in 0..14 {
        assert!(c.run_frame(0).sound.is_empty());
    }
    let coin = c.run_frame(0);
    assert_eq!(coin.sound.len(), 1);
    assert_eq!(coin.sound[0].id, 1);
}

#[test]
fn the_ids_a_game_emits_render_through_the_engine() {
    // The step's whole point: run the game, take the events it emitted, hand
    // them to the engine with the bank the same source produced, and hear
    // something.
    let mut c = loaded();
    let mut engine = AudioEngine::new(SynthConfig::default());
    engine.set_bank(c.sound_bank().clone());

    let spf = samples_per_frame(engine.sample_rate()) as usize;
    let mut out = Vec::new();
    let mut block = vec![0.0f32; spf * 2];

    for frame in 0..40u64 {
        let obs = c.run_frame(0);
        for s in &obs.sound {
            engine.submit(
                kessel_audio::AudioEvent::PlaySfx { id: s.id },
                engine.frame_at(frame),
            );
        }
        engine.render(&mut block);
        out.extend_from_slice(&block);
    }

    assert_eq!(engine.stats().unknown_sfx, 0, "a game id missed the bank");
    // `boom` is three rows at speed 3 (two notes) and `coin` two rows at speed
    // 2 (two notes).
    assert_eq!(engine.synth_stats().started, 4);
    assert_eq!(engine.synth_stats().dropped, 0);

    let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    assert!(peak > 0.1, "the game was silent: peak {peak}");
    assert!(out.iter().all(|s| s.is_finite()));

    // The game's 5th update is loop iteration 4, and its 20th is iteration 19.
    // So the first four frames of audio are silent and those two are not: the
    // sounds landed where the game asked rather than in a heap at the start.
    let frame = |n: usize| &out[n * spf * 2..(n + 1) * spf * 2];
    let loud = |b: &[f32]| b.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    for n in 0..4 {
        assert_eq!(
            loud(frame(n)),
            0.0,
            "sound arrived before the game asked it to"
        );
    }
    assert!(loud(frame(4)) > 0.0, "boom never played");
    assert!(loud(frame(19)) > 0.0, "coin never played");
}

#[test]
fn a_rom_without_sound_declarations_has_an_empty_bank() {
    let mut c = VmConsole::new();
    c.write_source(
        "quiet.lua",
        "function update() end\nfunction draw() cls(0) end",
    )
    .unwrap();
    assert!(c.assemble("quiet.lua").unwrap().ok());
    c.load_rom("quiet.lua").unwrap();
    assert!(c.sound_bank().is_empty());
}

#[test]
fn bad_sound_declarations_are_diagnostics_not_silence() {
    // A patch that doesn't compile has to say so. The alternative — dropping
    // the instrument and playing something else — is a bug report about the
    // wrong subsystem.
    let mut c = VmConsole::new();
    c.write_source(
        "bad.lua",
        r#"
        instrument oops {
          wave = trumpet
          cutoff = 900
        }
        sfx s { inst = ghost }
        function update() end
        function draw() cls(0) end
        "#,
    )
    .unwrap();
    let built = c.assemble("bad.lua").unwrap();
    assert!(!built.ok());
    let text = format!("{:?}", built.diagnostics);
    assert!(text.contains("unknown wave 'trumpet'"), "{text}");
    assert!(text.contains("0..=255"), "{text}");
    assert!(text.contains("no instrument named 'ghost'"), "{text}");
}
