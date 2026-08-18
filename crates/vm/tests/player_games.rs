//! `VmPlayer` must load every shipped game the way a host with no filesystem
//! does: hand each `#include` target over first, then load the game.
//!
//! This lived in `player.rs` as a unit test over a hand-picked seven games and
//! a second copy of the include list. A `tests/common` module cannot be reached
//! from `src`, so the copy was the price of it being there — and it drifted
//! immediately: outrun's art was in that list and shooter's never was, so the
//! game most likely to break this path was the one it did not run.

use kessel_vm::device::BTN_RIGHT;
use kessel_vm::VmPlayer;

mod common;
use common::{GAMES, INCLUDES};

/// The Android path, over the whole corpus: `write_source` each include, then
/// `load`. Getting it wrong is a load error on that host and fine everywhere
/// else, which is why it is checked against the same list the compile guard
/// uses rather than a sample.
#[test]
fn every_shipped_game_loads_through_the_player() {
    for (name, src) in GAMES {
        let p = VmPlayer::new();
        for (path, lib) in INCLUDES {
            assert!(
                p.write_source(path, lib).is_empty(),
                "{name}: handing over {path} failed"
            );
        }
        let err = p.load((*src).to_string(), (*name).to_string());
        assert!(err.is_empty(), "{name} failed to load: {err}");
        p.tick(0);
        p.tick(BTN_RIGHT);
        assert!(
            p.framebuffer_rgba().is_some(),
            "{name} drew no frame after loading"
        );
    }
}
