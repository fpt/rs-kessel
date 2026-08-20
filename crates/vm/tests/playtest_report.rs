//! The differential-play report, printed for a human to read.
//!
//! `cargo test -p kessel-vm --test playtest_report -- --nocapture` prints what
//! `vm_playtest` would hand an agent. Kept as a test rather than an example so
//! it runs against the same embedded corpus every other guard uses.

use kessel_vm::playtest::{default_policies, playtest};
use kessel_vm::VmConsole;

mod common;
use common::{GAMES, INCLUDES};

fn console_with(game: &str) -> VmConsole {
    let mut c = VmConsole::new();
    for (path, src) in INCLUDES {
        c.write_source(path, src).expect("include");
    }
    let (_, src) = GAMES.iter().find(|(n, _)| *n == game).expect("game");
    c.write_source(game, src).expect("source");
    let a = c.assemble(game).expect("assemble");
    assert!(a.diagnostics.is_empty(), "{game}: {:?}", a.diagnostics);
    c.load_rom(game).expect("load");
    c
}

#[test]
fn shooter_playtest_report() {
    let mut c = console_with("shooter.lua");
    // Past the invulnerability grace, so every policy starts somewhere a real
    // player could be rather than inside 90 frames of nothing can hurt you.
    for _ in 0..120 {
        c.run_frame(0u8);
    }
    let frames = 600;
    let s = playtest(&mut c, &default_policies(frames), frames).expect("playtest");
    println!("\n{}", s.report());
}
