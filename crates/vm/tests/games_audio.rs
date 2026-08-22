//! Every shipped game must *sound* correct, not just run.
//!
//! `games_compile.rs` proves the corpus compiles and survives 300 frames.
//! Nothing proved its sound was wired up: a game could name an `sfx` that isn't
//! declared, hand a note an instrument the bank lacks, or drive the mix so hard
//! that the master limiter never lets go — all while faulting nowhere and
//! passing every other test.
//!
//! Those are the failures a person only finds by listening, so they get counted
//! here instead. The renderer already produces every number needed; this is the
//! guard that reads them.

use kessel_vm::device::{Input, Touch};
use kessel_vm::VmConsole;

mod common;
use common::{GAMES, INCLUDES as LIBS};

/// The same input cycle `games_compile.rs` uses, so a game's sound is exercised
/// on the paths its gameplay test already walks.
const INPUTS: [u8; 9] = [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x18, 0x02];

fn loaded(name: &str, src: &str) -> VmConsole {
    let mut c = VmConsole::new();
    for (path, lib) in LIBS {
        c.write_source(path, lib).unwrap();
    }
    c.write_source("g.lua", src).unwrap();
    let built = c
        .assemble("g.lua")
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(
        built.ok(),
        "{name} did not compile: {:?}",
        built.diagnostics
    );
    c.load_rom("g.lua")
        .unwrap_or_else(|e| panic!("{name} load_rom: {e}"));
    c
}

/// 300 frames of every game, through the synth.
#[test]
fn every_game_renders_clean_audio() {
    for (name, src) in GAMES {
        let mut c = loaded(name, src);
        let script: Vec<(Input, u64)> = (0..300)
            .map(|f| (Input::from(INPUTS[f % INPUTS.len()]), 1))
            .collect();
        let r = c
            .render_audio(&script)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let s = &r.summary;

        // Every one of these means a game names something its own bank does not
        // have — a typo that is completely silent at runtime.
        assert_eq!(
            s.unknown_sfx, 0,
            "{name} triggers an sfx id it never declared"
        );
        assert_eq!(s.unknown_track, 0, "{name} plays a track it never declared");
        assert_eq!(
            s.notes_dropped, 0,
            "{name} plays a note on an instrument its bank lacks"
        );
        assert_eq!(
            s.bad_note_args, 0,
            "{name} passes an out-of-range argument to a note port"
        );
        assert_eq!(
            s.queue_overflow, 0,
            "{name} asks for more sound at once than the engine can schedule"
        );

        assert!(
            r.samples.iter().all(|v| v.is_finite()),
            "{name} rendered a non-finite sample"
        );
        assert!(s.peak <= 1.0, "{name} left full scale: peak {}", s.peak);

        // Sustained limiting means the mix is simply too loud: the game will
        // pump and duck whenever anything else plays. A brief engagement on a
        // peak is fine and expected.
        let frames = (r.samples.len() / 2) as u64;
        assert!(
            s.limited * 4 < frames,
            "{name} keeps the master limiter engaged for {} of {frames} samples — \
             turn the instrument volumes down",
            s.limited
        );
    }
}

/// The games that declare sound must actually make some.
///
/// Without this, deleting an `sfx()` call — or breaking the path that carries
/// `init()`'s triggers to frame 0 — would leave every assertion above happily
/// passing over silence.
#[test]
fn the_games_with_sound_are_audible() {
    // Each of these declares instruments, so each must be heard.
    for (name, src) in GAMES
        .iter()
        .filter(|(_, src)| src.contains("\ninstrument "))
    {
        let mut c = loaded(name, src);
        // The same rotation as above, not just "hold A": a coin in a platformer
        // has to be walked to before it can be collected. piano.lua is the one
        // exception — it has no buttons that make sound at all, only touch, so
        // it gets a finger held on a key instead of the shared button rotation.
        let script: Vec<(Input, u64)> = if *name == "piano.lua" {
            let mut key = Input::default();
            key.touches[0] = Touch {
                x: 4,
                y: 200,
                down: true,
            };
            vec![(key, 300)]
        } else {
            (0..300)
                .map(|f| (Input::from(INPUTS[f % INPUTS.len()]), 1))
                .collect()
        };
        let r = c.render_audio(&script).unwrap();
        assert!(
            !r.summary.events.is_empty(),
            "{name} declares instruments but never triggers anything"
        );
        assert!(
            r.summary.peak > 0.01,
            "{name} triggered sound but rendered silence: peak {}",
            r.summary.peak
        );
    }
}

/// outrun holds a note forever, so its mix has to be measured over more than a
/// glance.
///
/// Every other game in the corpus makes sound in *bursts*: a shot, a coin, a
/// row of a track. outrun holds one engine note from `init()` until the run
/// ends, and a continuous voice under the music is a sustained level rather
/// than a peak — which is the one thing 300 frames cannot show. The first mix
/// passed the shared guard above comfortably and pinned the master limiter for
/// a third of a 1300-frame render.
///
/// It is a test of one game rather than a longer run of all of them because it
/// is a property of one game: nothing else here holds a voice, and tripling the
/// corpus render to catch it would cost every game the runtime.
#[test]
fn outruns_held_engine_does_not_pin_the_limiter() {
    let (name, src) = *GAMES
        .iter()
        .find(|(n, _)| *n == "outrun.lua")
        .expect("outrun is in the corpus");
    // Every throttle: `A` reaches top speed and then bogs down in the dirt,
    // `UP` holds a mid-range note on the road for far longer, and that second
    // one is what the first tuning failed on.
    for buttons in [0x10u8, 0x04, 0x01] {
        let mut c = loaded(name, src);
        let r = c
            .render_audio(&[(Input::from(buttons), 1300)])
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let s = &r.summary;
        let frames = (r.samples.len() / 2) as u64;
        assert!(
            s.limited * 4 < frames,
            "outrun on buttons {buttons:#04x} keeps the master limiter engaged for \
             {} of {frames} samples — the engine and the track are fighting",
            s.limited
        );
        assert!(s.peak <= 1.0, "outrun left full scale: peak {}", s.peak);
        // Held, not retriggered: a `rev` that re-sounded on every frame instead
        // of on a step would put thousands of voices through here.
        assert!(
            s.voices_started < 1000,
            "outrun started {} voices in 1300 frames — the engine is retriggering, \
             not holding",
            s.voices_started
        );
    }
}

/// A game with no sound declarations must render silence rather than noise.
///
/// The synth is shared state across a render; a game that never asks for a
/// sound getting one would mean something is leaking between banks.
#[test]
fn a_game_without_sound_is_silent() {
    for (name, src) in GAMES
        .iter()
        .filter(|(_, src)| !src.contains("\ninstrument "))
    {
        let mut c = loaded(name, src);
        let r = c.render_audio(&[(Input::default(), 120)]).unwrap();
        assert_eq!(
            r.summary.peak, 0.0,
            "{name} declares no instruments but made a sound"
        );
    }
}
