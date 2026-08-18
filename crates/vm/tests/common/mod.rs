//! Shared fixtures for the `games/` integration tests.
//!
//! The corpus is the "adapt this" reference the model is pointed at, so what is
//! in `games/` and what the guards actually run has to be the same set. Three
//! test binaries need that set — `games_compile.rs`, `games_audio.rs` and
//! `player_games.rs` — and lists that drifted between them would mean a game
//! compiles in one and not the other for no reason a reader could see.
//!
//! [`registration_gap`] closes the half `include_str!` cannot: see its docs.

/// Every playable game at the top of `games/`.
///
/// Embedded with `include_str!` (compile-time, CWD-independent) so renaming or
/// removing a game fails this build rather than quietly dropping it from every
/// guard at once.
pub const GAMES: &[(&str, &str)] = &[
    ("2048.lua", include_str!("../../../../games/2048.lua")),
    ("bounce.lua", include_str!("../../../../games/bounce.lua")),
    ("brick.lua", include_str!("../../../../games/brick.lua")),
    ("mover.lua", include_str!("../../../../games/mover.lua")),
    ("outrun.lua", include_str!("../../../../games/outrun.lua")),
    ("paint.lua", include_str!("../../../../games/paint.lua")),
    ("piano.lua", include_str!("../../../../games/piano.lua")),
    (
        "platform.lua",
        include_str!("../../../../games/platform.lua"),
    ),
    ("popn.lua", include_str!("../../../../games/popn.lua")),
    ("rogue.lua", include_str!("../../../../games/rogue.lua")),
    ("shooter.lua", include_str!("../../../../games/shooter.lua")),
    ("snake.lua", include_str!("../../../../games/snake.lua")),
    ("sokoban.lua", include_str!("../../../../games/sokoban.lua")),
    (
        "spectrum.lua",
        include_str!("../../../../games/spectrum.lua"),
    ),
    ("sprite.lua", include_str!("../../../../games/sprite.lua")),
    ("swarm.lua", include_str!("../../../../games/swarm.lua")),
    ("tetris.lua", include_str!("../../../../games/tetris.lua")),
];

/// Every source under `games/` that is included rather than played: the shared
/// helpers in `lib/`, and the per-game directories a big game splits its art
/// into.
///
/// Embedded for the same reason [`GAMES`] is.
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

/// The corpus directory itself, found from the crate rather than the CWD.
///
/// `dead_code` because this module is compiled into all three test binaries and
/// only `games_compile.rs` runs the guard — the directory should be read once,
/// not three times.
#[allow(dead_code)]
fn corpus_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games")
}

/// What is in `games/` on disk but in neither const above — reported as lines
/// ready to paste, or empty when the lists are complete.
///
/// `include_str!` catches a **rename**: the path stops resolving and this test
/// binary fails to build. It cannot catch an **addition**, and that is the half
/// that bites. A new `games/newgame.lua` that nobody adds to [`GAMES`] is not a
/// failure anywhere — the compile guard, the audio guard and the 300-frame
/// fault guard all skip it, the suite stays green, and a broken example sits in
/// the corpus the model copies from.
///
/// So this reads the directory instead of trusting the list. Only the one
/// direction: a const entry naming a file that is *gone* already fails at build
/// time, which is a better error than any assertion here could produce.
///
/// `dead_code` for the same reason [`corpus_dir`] is.
#[allow(dead_code)]
pub fn registration_gap() -> Vec<String> {
    let root = corpus_dir();
    let mut gap = Vec::new();

    for entry in std::fs::read_dir(&root).expect("games/ is not readable") {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();

        if entry.file_type().unwrap().is_dir() {
            for f in std::fs::read_dir(entry.path()).unwrap() {
                let f = f.unwrap();
                let leaf = f.file_name().to_string_lossy().to_string();
                // One level deep is the shape the corpus has, and the shape
                // Android's `GameCatalog.includePaths` walks — a source nested
                // any deeper would be unreachable on the device while working
                // fine under `kessel run`.
                assert!(
                    !f.file_type().unwrap().is_dir(),
                    "games/{name}/{leaf} is nested two directories deep; \
                     Android's asset walk is one level and would never find it"
                );
                if leaf.ends_with(".lua")
                    && !INCLUDES.iter().any(|(p, _)| *p == format!("{name}/{leaf}"))
                {
                    gap.push(format!(
                        "        (\"{name}/{leaf}\", include_str!(\"../../../../games/{name}/{leaf}\")),  // -> INCLUDES"
                    ));
                }
            }
        } else if name.ends_with(".lua") && !GAMES.iter().any(|(n, _)| *n == name) {
            gap.push(format!(
                "        (\"{name}\", include_str!(\"../../../../games/{name}\")),  // -> GAMES"
            ));
        }
    }

    gap.sort();
    gap
}
