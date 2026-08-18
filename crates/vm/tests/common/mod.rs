//! Shared fixtures for the `games/` integration tests.
//!
//! Only the include list lives here so far, and only because two test binaries
//! need the same one: `games_compile.rs` and `games_audio.rs` both have to stand
//! a game's `#include` targets up in the workspace before compiling it, and a
//! list that drifted between them would mean a game compiles in one and not the
//! other for no reason a reader could see.

/// Every source under `games/` that is included rather than played: the shared
/// helpers in `lib/`, and the per-game directories a big game splits its art
/// into.
///
/// Embedded with `include_str!` for the same reason the games are — renaming one
/// must break this build rather than quietly stop being tested. Adding a new
/// include file and forgetting this list is caught too, just later: the game
/// that includes it stops compiling.
pub const INCLUDES: &[(&str, &str)] = &[
    (
        "lib/motion.lua",
        include_str!("../../../../games/lib/motion.lua"),
    ),
    (
        "outrun/car.lua",
        include_str!("../../../../games/outrun/car.lua"),
    ),
    (
        "outrun/scenery.lua",
        include_str!("../../../../games/outrun/scenery.lua"),
    ),
    (
        "outrun/smoke.lua",
        include_str!("../../../../games/outrun/smoke.lua"),
    ),
    (
        "shooter/ship.lua",
        include_str!("../../../../games/shooter/ship.lua"),
    ),
    (
        "shooter/foes.lua",
        include_str!("../../../../games/shooter/foes.lua"),
    ),
    (
        "shooter/boss.lua",
        include_str!("../../../../games/shooter/boss.lua"),
    ),
    (
        "shooter/fx.lua",
        include_str!("../../../../games/shooter/fx.lua"),
    ),
];
