//! The shipped sample games (`games/*.lua`) are the "adapt this" references the
//! model is pointed at, and the human-playable `kessel --play` demos. Guard them
//! so a luax/assembler/VM change can't silently break the examples: each must
//! (1) run the full `.lua` → luax compile → assemble → ROM pipeline with no
//! diagnostics, and (2) execute 300 frames — under both idle and a rotating
//! button pattern — without ever faulting or halting.
//!
//! Sources are embedded with `include_str!` (compile-time, CWD-independent) so
//! the test binary itself fails to build if a game file is renamed or removed.

use kessel_core::vm::{assembler, luax, VmConsole};

fn assert_game_ok(name: &str, src: &str) {
    // --- compile (luax) ---
    let compiled = luax::compile(src);
    let luax_errs: Vec<_> = compiled
        .diagnostics
        .iter()
        .map(|d| format!("  L{}: {}", d.line, d.message))
        .collect();
    assert!(
        compiled.ok(),
        "{name}.lua failed to compile (luax):\n{}",
        luax_errs.join("\n")
    );
    assert!(
        !compiled.asm.trim().is_empty(),
        "{name}.lua compiled to empty assembly"
    );

    // --- assemble ---
    let built = assembler::assemble(&compiled.asm);
    let asm_errs: Vec<_> = built
        .diagnostics
        .iter()
        .map(|d| format!("  L{}: {}", d.line, d.message))
        .collect();
    assert!(
        built.ok(),
        "{name}.lua failed to assemble:\n{}",
        asm_errs.join("\n")
    );
    assert!(!built.rom.is_empty(), "{name}.lua assembled to an empty ROM");

    // --- run (300 frames, cycling inputs to exercise move/fire/rotate/restart) ---
    let mut c = VmConsole::new();
    c.write_source("g.lua", src);
    c.assemble("g.lua")
        .unwrap_or_else(|e| panic!("{name}.lua assemble via VmConsole: {e}"));
    c.load_rom("g.lua")
        .unwrap_or_else(|e| panic!("{name}.lua load_rom (reset vector faulted?): {e}"));
    // LEFT, RIGHT, UP, DOWN, A, B, A+DOWN, none — enough to drive every game's paths.
    let inputs: [u8; 9] = [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x18, 0x02];
    for f in 0..300u32 {
        let obs = c.run_frame(inputs[(f as usize) % inputs.len()]);
        assert!(
            obs.fault.is_none(),
            "{name}.lua faulted on frame {f}: {:?}",
            obs.fault
        );
        assert!(!obs.halted, "{name}.lua halted unexpectedly on frame {f}");
    }
}

/// Drive `sokoban.lua` through known solutions for all four stages, confirming
/// push mechanics, stage advancement, and the final wrap back to stage one.
#[test]
fn sokoban_solves_all_stages() {
    const LEFT: u8 = 0x01;
    const RIGHT: u8 = 0x02;
    const UP: u8 = 0x04;
    const DOWN: u8 = 0x08;

    let mut c = VmConsole::new();
    c.write_source("s.lua", include_str!("../../../games/sokoban.lua"));
    c.assemble("s.lua").unwrap();
    c.load_rom("s.lua").unwrap();

    // Press then release each step so btnp (edge input) fires exactly once.
    fn play(c: &mut VmConsole, steps: &[u8]) {
        for &step in steps {
            c.run_frame(step);
            c.run_frame(0);
        }
    }

    let solutions: [&[u8]; 4] = [
        &[LEFT, UP, DOWN, RIGHT, RIGHT, RIGHT, UP],
        &[LEFT, DOWN, DOWN, UP, RIGHT, RIGHT, RIGHT, DOWN],
        &[
            UP, RIGHT, RIGHT, RIGHT, LEFT, LEFT, LEFT, DOWN, DOWN, DOWN, RIGHT, RIGHT, RIGHT,
        ],
        &[
            UP, RIGHT, RIGHT, RIGHT, DOWN, RIGHT, RIGHT, DOWN, DOWN, LEFT, LEFT, LEFT, DOWN, LEFT,
            LEFT, UP, UP, RIGHT, RIGHT, RIGHT,
        ],
    ];
    let solved_positions = [(5, 3), (5, 4), (4, 5), (4, 4)];

    for (index, solution) in solutions.iter().enumerate() {
        play(&mut c, solution);
        let player = c.run_frame(0).entities[0];
        assert_eq!(player.tag, (index + 1) as u16, "unexpected active stage");
        assert_eq!(
            (player.x, player.y),
            solved_positions[index],
            "stage {} solution did not resolve",
            index + 1
        );

        play(&mut c, &[0x10]); // A advances after a clear (stage four wraps).
        let next = c.run_frame(0).entities[0];
        let expected_stage = if index == 3 { 1 } else { index + 2 };
        assert_eq!(next.tag, expected_stage as u16, "stage did not advance");
    }
}

macro_rules! games_ok {
    ($($test:ident => $file:literal),+ $(,)?) => {
        $(
            #[test]
            fn $test() {
                assert_game_ok($file, include_str!(concat!("../../../games/", $file, ".lua")));
            }
        )+
    };
}

games_ok! {
    bounce_ok => "bounce",
    mover_ok => "mover",
    sprite_ok => "sprite",
    platform_ok => "platform",
    snake_ok => "snake",
    brick_ok => "brick",
    shooter_ok => "shooter",
    rogue_ok => "rogue",
    tetris_ok => "tetris",
    wall_ok => "wall",
    sokoban_ok => "sokoban",
}
