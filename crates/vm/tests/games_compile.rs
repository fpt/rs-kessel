//! The shipped sample games (`games/*.lua`) are the "adapt this" references the
//! model is pointed at, and the human-playable `kessel run` demos. Guard them
//! so a luax/assembler/VM change can't silently break the examples: each must
//! (1) run the full `.lua` → luax compile → assemble → ROM pipeline with no
//! diagnostics, and (2) execute 300 frames — under both idle and a rotating
//! button pattern — without ever faulting or halting.
//!
//! Sources are embedded with `include_str!` (compile-time, CWD-independent) so
//! the test binary itself fails to build if a game file is renamed or removed.

use kessel_vm::{assembler, device, luax, VmConsole};

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
    assert!(
        !built.rom.is_empty(),
        "{name}.lua assembled to an empty ROM"
    );

    // --- run (300 frames, cycling inputs to exercise move/fire/rotate/restart) ---
    let mut c = VmConsole::new();
    c.write_source("g.lua", src).unwrap();
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
    c.write_source("s.lua", include_str!("../../../games/sokoban.lua"))
        .unwrap();
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

#[test]
fn platform_has_clear_background_and_smooth_jump() {
    const A: u8 = 0x10;

    let peaceful = include_str!("../../../games/platform.lua")
        .replace("enemies[0].alive = 1", "enemies[0].alive = 0")
        .replace("enemies[1].alive = 1", "enemies[1].alive = 0")
        .replace("enemies[2].alive = 1", "enemies[2].alive = 0")
        .replace("enemies[3].alive = 1", "enemies[3].alive = 0");
    let mut c = VmConsole::new();
    c.write_source("p.lua", &peaceful).unwrap();
    c.assemble("p.lua").unwrap();
    c.load_rom("p.lua").unwrap();

    c.run_frame(0);
    let rgba = c.framebuffer_rgba();
    let background_pixel = 10 * 4; // This pixel was white when tile 0 was `hero`.
    assert_eq!(
        &rgba[background_pixel..background_pixel + 4],
        &[0x29, 0xad, 0xff, 0xff],
        "empty map cells should render as sky"
    );

    let mut player = c.run_frame(0).entities[0];
    for _ in 0..20 {
        player = c.run_frame(0).entities[0];
    }
    assert_eq!(player.y, 104, "player did not settle flush on the floor");

    player = c.run_frame(A).entities[0];
    let mut min_y = player.y;
    let mut airborne_frames = u32::from(player.y < 104);
    for _ in 0..44 {
        player = c.run_frame(0).entities[0];
        min_y = min_y.min(player.y);
        airborne_frames += u32::from(player.y < 104);
    }

    assert!(min_y <= 78, "jump was too low: apex y={min_y}");
    assert!(
        airborne_frames >= 24,
        "jump arc was too fast: {airborne_frames} airborne frames"
    );
    assert_eq!(player.y, 104, "player did not land after the jump");
}

#[test]
fn platform_camera_follows_player_across_stage() {
    const RIGHT: u8 = 0x02;

    fn white_x_bounds(rgba: &[u8]) -> Option<(usize, usize)> {
        let mut bounds: Option<(usize, usize)> = None;
        for (index, pixel) in rgba.chunks_exact(4).enumerate() {
            if pixel == [0xff, 0xf1, 0xe8, 0xff] {
                let x = index % 128;
                bounds = Some(match bounds {
                    Some((min_x, max_x)) => (min_x.min(x), max_x.max(x)),
                    None => (x, x),
                });
            }
        }
        bounds
    }

    let peaceful = include_str!("../../../games/platform.lua")
        .replace("enemies[0].alive = 1", "enemies[0].alive = 0")
        .replace("enemies[1].alive = 1", "enemies[1].alive = 0")
        .replace("enemies[2].alive = 1", "enemies[2].alive = 0")
        .replace("enemies[3].alive = 1", "enemies[3].alive = 0");
    let mut c = VmConsole::new();
    c.write_source("p.lua", &peaceful).unwrap();
    c.assemble("p.lua").unwrap();
    c.load_rom("p.lua").unwrap();

    for _ in 0..20 {
        c.run_frame(0);
    }

    let mut player = c.run_frame(0).entities[0];
    for _ in 0..160 {
        player = c.run_frame(RIGHT).entities[0];
    }
    assert!(player.x > 128, "player never entered the second screen");
    let (min_x, max_x) = white_x_bounds(&c.framebuffer_rgba()).expect("hero is visible");
    assert!(
        (50..=70).contains(&min_x),
        "camera did not centre hero: x={min_x}"
    );
    assert!(
        max_x < 80,
        "hero rendered too far right while camera followed"
    );

    for _ in 0..100 {
        player = c.run_frame(RIGHT).entities[0];
    }
    assert_eq!(player.x, 240, "right boundary did not stop the player");
    let (min_x, max_x) = white_x_bounds(&c.framebuffer_rgba()).expect("hero is visible");
    assert!(
        min_x >= 112 && max_x < 128,
        "hero disappeared at stage edge"
    );
}

#[test]
fn platform_wall_jump_launches_away_from_wall() {
    const A: u8 = 0x10;
    const LEFT: u8 = 0x01;
    const RIGHT: u8 = 0x02;

    let wall_jump = include_str!("../../../games/platform.lua")
        .replace(
            "p.x = 16  p.y = 96  p.y4 = 96 * 4",
            "p.x = 56  p.y = 72  p.y4 = 72 * 4",
        )
        .replace("enemies[0].alive = 1", "enemies[0].alive = 0")
        .replace("enemies[1].alive = 1", "enemies[1].alive = 0")
        .replace("enemies[2].alive = 1", "enemies[2].alive = 0")
        .replace("enemies[3].alive = 1", "enemies[3].alive = 0");
    let mut c = VmConsole::new();
    c.write_source("p.lua", &wall_jump).unwrap();
    c.assemble("p.lua").unwrap();
    c.load_rom("p.lua").unwrap();

    // The player starts flush against the right side of the first raised pillar.
    let launched = c.run_frame(A | LEFT);
    assert!(
        launched.entities[0].x > 56,
        "wall-jump did not detach from the wall"
    );
    assert!(
        launched.entities[0].y < 72,
        "wall-jump did not launch upward"
    );
    let launch_x = launched.entities[0].x;

    // Held input toward the wall can steer, but must not cancel the reflection.
    let toward_wall = c.run_frame(LEFT);
    assert!(
        toward_wall.entities[0].x > launch_x,
        "wall-jump reflection was cancelled by held input"
    );

    // Steering away from the wall has more influence than steering toward it.
    let away_start = c.run_frame(0).entities[0].x;
    let away = c.run_frame(RIGHT);
    assert!(
        away.entities[0].x - away_start > toward_wall.entities[0].x - launch_x,
        "wall-jump steering did not adjust horizontal movement"
    );
}

#[test]
fn platform_coins_patrols_stomps_and_knockback_work() {
    const RIGHT: u8 = 0x02;
    const PLATFORM: &str = include_str!("../../../games/platform.lua");

    // The enemy on the short raised platform walks to its edge, turns, and returns.
    let mut c = VmConsole::new();
    c.write_source("p.lua", PLATFORM).unwrap();
    c.assemble("p.lua").unwrap();
    c.load_rom("p.lua").unwrap();
    let first = c.run_frame(0);
    let solid_tiles = [
        (4, 11),
        (5, 11),
        (6, 11),
        (10, 9),
        (11, 9),
        (16, 11),
        (17, 11),
        (18, 11),
        (22, 8),
        (23, 8),
        (24, 8),
        (28, 10),
        (29, 10),
        (6, 8),
        (6, 9),
        (6, 10),
        (18, 8),
        (18, 9),
        (18, 10),
        (24, 5),
        (24, 6),
        (24, 7),
    ];
    for coin in first.entities.iter().filter(|e| e.tag == 3) {
        assert!(
            !solid_tiles.contains(&(coin.x / 8, coin.y / 8)),
            "coin at ({}, {}) overlaps a solid block",
            coin.x,
            coin.y
        );
    }
    let raised = first
        .entities
        .iter()
        .find(|e| e.tag == 2 && e.y == 80)
        .unwrap();
    assert_eq!(raised.x, 136, "enemy moved before its patrol tick");
    let mut raised_x = raised.x;
    for _ in 0..31 {
        let obs = c.run_frame(0);
        raised_x = obs
            .entities
            .iter()
            .find(|e| e.tag == 2 && e.y == 80)
            .unwrap()
            .x;
    }
    assert!(
        raised_x < 144,
        "raised enemy did not reverse at the platform edge"
    );

    let peaceful = PLATFORM
        .replace("enemies[0].alive = 1", "enemies[0].alive = 0")
        .replace("enemies[1].alive = 1", "enemies[1].alive = 0")
        .replace("enemies[2].alive = 1", "enemies[2].alive = 0")
        .replace("enemies[3].alive = 1", "enemies[3].alive = 0");
    let mut c = VmConsole::new();
    c.write_source("p.lua", &peaceful).unwrap();
    c.assemble("p.lua").unwrap();
    c.load_rom("p.lua").unwrap();
    for _ in 0..20 {
        c.run_frame(0);
    }
    let collected = c.run_frame(RIGHT);
    let state = collected.entities.iter().find(|e| e.tag == 30).unwrap();
    assert_eq!(state.x, 1, "coin overlap did not increment the counter");
    assert!(
        !collected
            .entities
            .iter()
            .any(|e| e.tag == 3 && (e.x, e.y) == (24, 104)),
        "collected coin remained visible"
    );

    let stomp = PLATFORM
        .replace("enemies[0].x = 64", "enemies[0].x = 16")
        .replace("enemies[0].dir = 1", "enemies[0].dir = 0")
        .replace("enemies[1].alive = 1", "enemies[1].alive = 0")
        .replace("enemies[2].alive = 1", "enemies[2].alive = 0")
        .replace("enemies[3].alive = 1", "enemies[3].alive = 0");
    let mut c = VmConsole::new();
    c.write_source("p.lua", &stomp).unwrap();
    c.assemble("p.lua").unwrap();
    c.load_rom("p.lua").unwrap();
    let mut stomped = false;
    let mut player_y = 0;
    for _ in 0..12 {
        let obs = c.run_frame(0);
        player_y = obs.entities[0].y;
        stomped = !obs.entities.iter().any(|e| e.tag == 2 && e.x == 16);
        if stomped {
            break;
        }
    }
    assert!(stomped, "falling onto an enemy did not defeat it");
    assert!(player_y <= 96, "stomp did not bounce the player upward");

    let side_hit = PLATFORM
        .replace("enemies[0].x = 64", "enemies[0].x = 32")
        .replace("enemies[0].dir = 1", "enemies[0].dir = 0")
        .replace("enemies[1].alive = 1", "enemies[1].alive = 0")
        .replace("enemies[2].alive = 1", "enemies[2].alive = 0")
        .replace("enemies[3].alive = 1", "enemies[3].alive = 0");
    let mut c = VmConsole::new();
    c.write_source("p.lua", &side_hit).unwrap();
    c.assemble("p.lua").unwrap();
    c.load_rom("p.lua").unwrap();
    for _ in 0..20 {
        c.run_frame(0);
    }
    let mut hit_player_x = 0;
    let mut hit = false;
    for _ in 0..20 {
        let obs = c.run_frame(RIGHT);
        let state = obs.entities.iter().find(|e| e.tag == 30).unwrap();
        if state.y > 0 {
            hit = true;
            hit_player_x = obs.entities[0].x;
            assert_eq!(state.y, 45, "side hit did not start invulnerability");
            break;
        }
    }
    assert!(hit, "horizontal enemy contact was not detected");
    assert!(
        hit_player_x <= 22,
        "side hit did not knock the player backward"
    );
    let after = c.run_frame(0);
    assert!(
        after.entities[0].x < hit_player_x,
        "knockback did not continue after impact"
    );
    assert!(
        after.entities.iter().any(|e| e.tag == 2 && e.x == 32),
        "enemy was defeated during horizontal knockback"
    );
    let mut enemy_survived = true;
    for _ in 0..20 {
        let obs = c.run_frame(0);
        enemy_survived = obs.entities.iter().any(|e| e.tag == 2 && e.x == 32);
    }
    assert!(
        enemy_survived,
        "enemy was stomped while the player was invulnerable"
    );
}

#[test]
fn shooter_centres_bullets_and_player_can_die() {
    const A: u8 = 0x10;
    const SHOOTER: &str = include_str!("../../../games/shooter.lua");

    let mut c = VmConsole::new();
    c.write_source("s.lua", SHOOTER).unwrap();
    c.assemble("s.lua").unwrap();
    c.load_rom("s.lua").unwrap();
    c.run_frame(0);
    c.run_frame(A);

    let rgba = c.framebuffer_rgba();
    let mut yellow_x = Vec::new();
    for (index, pixel) in rgba.chunks_exact(4).enumerate() {
        if pixel == [0xff, 0xec, 0x27, 0xff] {
            yellow_x.push(index % 128);
        }
    }
    assert!(!yellow_x.is_empty(), "fired bullet was not rendered");
    assert_eq!(
        (
            *yellow_x.iter().min().unwrap(),
            *yellow_x.iter().max().unwrap()
        ),
        (63, 64),
        "bullet was not aligned to the ship centreline"
    );

    // Make spawned enemies track the initial ship x so collision is deterministic.
    let targeted = SHOOTER.replace("foes[i].x = rnd(120)", "foes[i].x = px");
    let mut c = VmConsole::new();
    c.write_source("s.lua", &targeted).unwrap();
    c.assemble("s.lua").unwrap();
    c.load_rom("s.lua").unwrap();

    let mut player = c.run_frame(0).entities[0];
    for _ in 0..150 {
        player = c.run_frame(0).entities[0];
    }
    assert_eq!(player.tag, 2, "enemy collision did not kill the player");

    player = c.run_frame(A).entities[0];
    assert_eq!(player.tag, 1, "A did not restart after game over");
    assert_eq!(player.x, 60, "restart did not reset the ship position");
}

#[test]
fn rogue_sword_hearts_and_invulnerability_work() {
    const A: u8 = 0x10;
    const ROGUE: &str = include_str!("../../../games/rogue.lua");

    // Put the first orc directly to the hero's right, which is the initial facing.
    let adjacent = ROGUE.replace(
        "enemies[0].x = 12  enemies[0].y = 3",
        "enemies[0].x = 3   enemies[0].y = 2",
    );
    let mut c = VmConsole::new();
    c.write_source("r.lua", &adjacent).unwrap();
    c.assemble("r.lua").unwrap();
    c.load_rom("r.lua").unwrap();

    let before = c.run_frame(0);
    assert!(
        before
            .entities
            .iter()
            .any(|e| e.tag == 10 && (e.x, e.y) == (24, 16)),
        "adjacent test orc was not present"
    );
    let after = c.run_frame(A);
    assert!(
        !after
            .entities
            .iter()
            .any(|e| e.tag == 10 && (e.x, e.y) == (24, 16)),
        "sword did not defeat the adjacent orc"
    );
    let rgba = c.framebuffer_rgba();
    let sword_tip = (19 * 128 + 31) * 4;
    assert_eq!(
        &rgba[sword_tip..sword_tip + 4],
        &[0xff, 0xec, 0x27, 0xff],
        "sword attack was not rendered"
    );

    // Keep one adjacent orc alive to exercise repeated contact attempts.
    let contact = adjacent
        .replace("enemies[1].alive = 1", "enemies[1].alive = 0")
        .replace("enemies[2].alive = 1", "enemies[2].alive = 0");
    let mut c = VmConsole::new();
    c.write_source("r.lua", &contact).unwrap();
    c.assemble("r.lua").unwrap();
    c.load_rom("r.lua").unwrap();

    c.run_frame(0);
    let rgba = c.framebuffer_rgba();
    let fifth_heart = (3 * 128 + 38) * 4;
    assert_eq!(
        &rgba[fifth_heart..fifth_heart + 4],
        &[0xff, 0x00, 0x4d, 0xff]
    );

    let mut obs = c.run_frame(0);
    let mut player = *obs.entities.iter().find(|e| e.tag <= 5).unwrap();
    for _ in 0..18 {
        obs = c.run_frame(0);
        player = *obs.entities.iter().find(|e| e.tag <= 5).unwrap();
    }
    assert_eq!(player.tag, 4, "contact did not remove exactly one heart");
    let rgba = c.framebuffer_rgba();
    assert_eq!(
        &rgba[fifth_heart..fifth_heart + 4],
        &[0xc2, 0xc3, 0xc7, 0xff]
    );

    let hero_pixel = (16 * 128 + 18) * 4;
    assert_eq!(
        &rgba[hero_pixel..hero_pixel + 4],
        &[0x5f, 0x57, 0x4f, 0xff],
        "hero should begin the blink hidden"
    );
    obs = c.run_frame(0);
    player = *obs.entities.iter().find(|e| e.tag <= 5).unwrap();
    let rgba = c.framebuffer_rgba();
    assert_eq!(
        &rgba[hero_pixel..hero_pixel + 4],
        &[0xff, 0xf1, 0xe8, 0xff],
        "hero should alternate visible during invulnerability"
    );

    for _ in 0..39 {
        obs = c.run_frame(0);
        player = *obs.entities.iter().find(|e| e.tag <= 5).unwrap();
    }
    assert_eq!(player.tag, 4, "invulnerability allowed damage too soon");
    for _ in 0..20 {
        obs = c.run_frame(0);
        player = *obs.entities.iter().find(|e| e.tag <= 5).unwrap();
    }
    assert_eq!(player.tag, 3, "damage did not resume after invulnerability");
}

#[test]
fn rogue_chests_and_stairs_advance_stages() {
    const LEFT: u8 = 0x01;
    const RIGHT: u8 = 0x02;
    const UP: u8 = 0x04;
    const DOWN: u8 = 0x08;

    fn walk(c: &mut VmConsole, route: &[(u8, usize)]) {
        for &(direction, count) in route {
            for _ in 0..count {
                c.run_frame(direction);
                for _ in 0..8 {
                    c.run_frame(0);
                }
            }
        }
    }

    let peaceful = include_str!("../../../games/rogue.lua")
        .replace("enemies[0].alive = 1", "enemies[0].alive = 0")
        .replace("enemies[1].alive = 1", "enemies[1].alive = 0")
        .replace("enemies[2].alive = 1", "enemies[2].alive = 0");
    let mut c = VmConsole::new();
    c.write_source("r.lua", &peaceful).unwrap();
    c.assemble("r.lua").unwrap();
    c.load_rom("r.lua").unwrap();

    let chest_routes: [&[(u8, usize)]; 4] = [
        &[(RIGHT, 5)],
        &[(LEFT, 5), (DOWN, 4)],
        &[(UP, 5), (RIGHT, 5)],
        &[(UP, 7), (LEFT, 8)],
    ];
    let stair_routes: [&[(u8, usize)]; 4] = [
        &[(RIGHT, 6), (DOWN, 11)],
        &[(LEFT, 6), (DOWN, 7)],
        &[(UP, 6), (RIGHT, 6)],
        &[(LEFT, 3), (UP, 4)],
    ];

    for index in 0..4 {
        walk(&mut c, chest_routes[index]);
        let obs = c.run_frame(0);
        let state = obs.entities.iter().find(|e| e.tag == 30).unwrap();
        assert_eq!((state.x, state.y), ((index + 1) as u16, (index + 1) as u16));
        assert!(
            obs.entities.iter().any(|e| e.tag == 22),
            "stage {} chest did not open",
            index + 1
        );

        walk(&mut c, stair_routes[index]);
        let obs = c.run_frame(0);
        let state = obs.entities.iter().find(|e| e.tag == 30).unwrap();
        assert_eq!(
            (state.x, state.y),
            ((index + 2) as u16, (index + 1) as u16),
            "stage {} staircase did not advance",
            index + 1
        );
        assert!(
            obs.entities.iter().any(|e| e.tag == 20),
            "next chest was not present"
        );
    }
}

/// The whole shipped corpus. Kept as one list so a guard can cover every game
/// rather than only the ones a given test happens to embed.
const GAMES: &[(&str, &str)] = &[
    ("2048.lua", include_str!("../../../games/2048.lua")),
    ("bounce.lua", include_str!("../../../games/bounce.lua")),
    ("brick.lua", include_str!("../../../games/brick.lua")),
    ("mover.lua", include_str!("../../../games/mover.lua")),
    ("outrun.lua", include_str!("../../../games/outrun.lua")),
    ("paint.lua", include_str!("../../../games/paint.lua")),
    ("piano.lua", include_str!("../../../games/piano.lua")),
    ("platform.lua", include_str!("../../../games/platform.lua")),
    ("popn.lua", include_str!("../../../games/popn.lua")),
    ("rogue.lua", include_str!("../../../games/rogue.lua")),
    ("shooter.lua", include_str!("../../../games/shooter.lua")),
    ("snake.lua", include_str!("../../../games/snake.lua")),
    ("sokoban.lua", include_str!("../../../games/sokoban.lua")),
    ("spectrum.lua", include_str!("../../../games/spectrum.lua")),
    ("sprite.lua", include_str!("../../../games/sprite.lua")),
    ("tetris.lua", include_str!("../../../games/tetris.lua")),
];

/// Every embedded game must use LF endings.
///
/// The fixtures below splice deterministic starting state into a game with
/// `str::replace` on multi-line patterns. Under a CRLF checkout those patterns
/// silently fail to match, `replace` returns the text unchanged, and the test
/// runs the *unmodified* game — surfacing as an unrelated gameplay assertion
/// ("first swipe scored incorrectly") with no hint at the real cause. `.gitattributes`
/// enforces the checkout; this names the problem if that ever stops working.
#[test]
fn corpus_is_lf_only() {
    for (name, src) in GAMES {
        assert!(
            !src.contains('\r'),
            "{name} has CR in it — check .gitattributes; multi-line test fixtures \
             will silently stop matching"
        );
    }
}

#[test]
fn game_2048_merges_wins_loses_and_restarts() {
    const LEFT: u8 = 0x01;
    const A: u8 = 0x10;
    const GAME: &str = include_str!("../../../games/2048.lua");
    const INITIAL_SPAWNS: &str = "  spawn_tile()\n  spawn_tile()";

    let merge_board = GAME.replace(
        INITIAL_SPAWNS,
        "  cells[0] = 2  cells[1] = 2  cells[2] = 2  cells[3] = 2",
    );
    let mut c = VmConsole::new();
    c.write_source("2048.lua", &merge_board).unwrap();
    c.assemble("2048.lua").unwrap();
    c.load_rom("2048.lua").unwrap();

    c.run_frame(0);
    let first = c.run_frame(LEFT);
    let state = first.entities.iter().find(|e| e.tag == 30).unwrap();
    assert_eq!((state.x, state.y), (8, 0), "first swipe scored incorrectly");
    let animation = first.entities.iter().find(|e| e.tag == 31).unwrap();
    assert_eq!(
        (animation.x, animation.y),
        (1, 4),
        "left nudge did not start"
    );
    assert!(first
        .entities
        .iter()
        .any(|e| e.tag == 4 && (e.x, e.y) == (32, 29)));
    assert!(first
        .entities
        .iter()
        .any(|e| e.tag == 4 && (e.x, e.y) == (48, 29)));
    let rgba = c.framebuffer_rgba();
    let nudged_edge = (29 * 128 + 30) * 4;
    assert_eq!(
        &rgba[nudged_edge..nudged_edge + 4],
        &[0xc2, 0xc3, 0xc7, 0xff],
        "matrix did not move toward the swipe"
    );

    let held = c.run_frame(LEFT);
    let state = held.entities.iter().find(|e| e.tag == 30).unwrap();
    assert_eq!(state.x, 8, "held direction repeated a move");
    let animation = held.entities.iter().find(|e| e.tag == 31).unwrap();
    assert_eq!(animation.y, 3, "nudge animation did not advance");
    let rgba = c.framebuffer_rgba();
    assert_eq!(
        &rgba[nudged_edge..nudged_edge + 4],
        &[0x1d, 0x2b, 0x53, 0xff],
        "matrix did not ease back after the initial nudge"
    );
    c.run_frame(0);
    let second = c.run_frame(LEFT);
    let state = second.entities.iter().find(|e| e.tag == 30).unwrap();
    assert_eq!(state.x, 16, "second swipe did not merge the two fours");
    assert!(second
        .entities
        .iter()
        .any(|e| e.tag == 8 && (e.x, e.y) == (32, 29)));

    c.run_frame(0);
    let restarted = c.run_frame(A);
    let state = restarted.entities.iter().find(|e| e.tag == 30).unwrap();
    assert_eq!(
        (state.x, state.y),
        (0, 0),
        "restart did not reset score and state"
    );
    assert_eq!(
        restarted
            .entities
            .iter()
            .filter(|e| e.tag == 2 || e.tag == 4)
            .count(),
        4,
        "restart did not restore the deterministic initial board"
    );

    let win_board = GAME.replace(INITIAL_SPAWNS, "  cells[0] = 1024  cells[1] = 1024");
    let mut c = VmConsole::new();
    c.write_source("2048.lua", &win_board).unwrap();
    c.assemble("2048.lua").unwrap();
    c.load_rom("2048.lua").unwrap();
    let won = c.run_frame(LEFT);
    let state = won.entities.iter().find(|e| e.tag == 30).unwrap();
    assert_eq!((state.x, state.y), (2048, 1), "2048 merge did not win");

    let stuck_board = GAME.replace(
        INITIAL_SPAWNS,
        "  cells[0] = 2  cells[1] = 4  cells[2] = 2  cells[3] = 4\n  cells[4] = 4  cells[5] = 2  cells[6] = 4  cells[7] = 2\n  cells[8] = 2  cells[9] = 4  cells[10] = 2  cells[11] = 4\n  cells[12] = 4  cells[13] = 2  cells[14] = 4  cells[15] = 2",
    );
    let mut c = VmConsole::new();
    c.write_source("2048.lua", &stuck_board).unwrap();
    c.assemble("2048.lua").unwrap();
    c.load_rom("2048.lua").unwrap();
    let lost = c.run_frame(LEFT);
    let state = lost.entities.iter().find(|e| e.tag == 30).unwrap();
    assert_eq!((state.x, state.y), (0, 2), "stuck board did not game over");
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
    game_2048_ok => "2048",
    outrun_ok => "outrun",
    bounce_ok => "bounce",
    mover_ok => "mover",
    sprite_ok => "sprite",
    platform_ok => "platform",
    snake_ok => "snake",
    brick_ok => "brick",
    shooter_ok => "shooter",
    rogue_ok => "rogue",
    tetris_ok => "tetris",
    sokoban_ok => "sokoban",
    piano_ok => "piano",
    popn_ok => "popn",
    paint_ok => "paint",
}

/// `popn.lua` is the reference for a pad with **no directions**: the four
/// direction bits are labelled keys, and a host must lay them out as a row.
///
/// The guard is that the metadata and the game agree. A ROM whose `controls`
/// said "d-pad" while `update` treated LEFT as a piano key would compile, run,
/// and be unplayable on the one host that draws its pad from the metadata.
#[test]
fn popn_declares_a_button_row_and_scores_on_those_keys() {
    use kessel_vm::luax::DirLayout;

    const GAME: &str = include_str!("../../../games/popn.lua");
    let compiled = luax::compile(GAME);
    assert!(compiled.ok(), "{:?}", compiled.diagnostics);
    assert_eq!(
        compiled.controls.dir_layout(),
        DirLayout::Buttons,
        "popn must present its direction bits as plain keys"
    );
    assert_eq!(compiled.controls.left.as_deref(), Some("red"));
    assert!(
        compiled.controls.a.is_some() && compiled.controls.b.is_some(),
        "popn's two action keys must be labelled too"
    );

    let mut c = VmConsole::new();
    c.write_source("popn.lua", GAME).unwrap();
    c.assemble("popn.lua").unwrap();
    c.load_rom("popn.lua").unwrap();

    // Mash every key every other frame. A press with nothing under it is a
    // whiff, so over 300 frames of a chart this has to land some hits — and
    // `score` is reported as tag 2 nowhere, so read it through the sound
    // instead: a hit plays a note, a whiff plays an sfx.
    let all = device::BTN_LEFT
        | device::BTN_DOWN
        | device::BTN_UP
        | device::BTN_RIGHT
        | device::BTN_A
        | device::BTN_B;
    let mut notes = 0;
    for f in 0..300u32 {
        let obs = c.run_frame(if f % 2 == 0 { all } else { 0 });
        assert!(obs.fault.is_none(), "popn faulted on frame {f}");
        notes += obs
            .sound
            .iter()
            .filter(|e| matches!(e, kessel_audio::AudioEvent::Play { .. }))
            .count();
    }
    assert!(
        notes > 0,
        "mashing every key for 300 frames never caught a single note"
    );
}

/// `2048.lua` is the swipe reference: a drag must move the board exactly as an
/// arrow press does, and must do it **once** per gesture.
///
/// The recognizer lives in the VM rather than in each host, so this test is
/// also what proves `kessel run`'s mouse, Android's fingers and an agent's
/// scripted `touch:` argument all get the same gesture — they are the same
/// three lines of touch state underneath.
#[test]
fn game_2048_slides_on_a_swipe_exactly_once() {
    use kessel_vm::device::{Input, Touch};

    const GAME: &str = include_str!("../../../games/2048.lua");
    const INITIAL_SPAWNS: &str = "  spawn_tile()\n  spawn_tile()";

    // A row of four 2s: one leftward move merges them into two 4s, so the
    // board's response to a swipe is unambiguous.
    let merge_board = GAME.replace(
        INITIAL_SPAWNS,
        "  cells[0] = 2  cells[1] = 2  cells[2] = 2  cells[3] = 2",
    );
    let mut c = VmConsole::new();
    c.write_source("2048.lua", &merge_board).unwrap();
    c.assemble("2048.lua").unwrap();
    c.load_rom("2048.lua").unwrap();

    let finger = |x: u16| {
        let mut i = Input::default();
        i.touches[0] = Touch {
            x,
            y: 64,
            down: true,
        };
        i
    };
    // Tiles are reported as entities, so the count is how the board is read.
    let tiles = |c: &mut VmConsole, i: Input| c.run_frame(i).entities.len();

    let before = tiles(&mut c, Input::default());
    c.run_frame(finger(90)); // press — cannot have travelled yet
    let after = tiles(&mut c, finger(50)); // 40 px left, past the 16 px threshold
    assert!(
        after < before,
        "a leftward swipe did not merge the row: {before} tiles -> {after}"
    );

    // Dragging further in the same press must not slide the board again — one
    // press is one move, or a slow drag would run the whole game.
    let settled = tiles(&mut c, finger(20));
    for x in [16u16, 12, 8] {
        assert_eq!(
            tiles(&mut c, finger(x)),
            settled,
            "the board moved twice in one gesture"
        );
    }
}

/// `paint.lua` is the reference for the analog surfaces, and the only game that
/// reads them — so this is the one test that proves a touch and a stick
/// deflection reach a running ROM at all.
#[test]
fn paint_reads_the_stick_and_the_touchscreen() {
    use kessel_vm::device::{Input, Touch};

    const GAME: &str = include_str!("../../../games/paint.lua");
    let mut c = VmConsole::new();
    c.write_source("paint.lua", GAME).unwrap();
    c.assemble("paint.lua").unwrap();
    c.load_rom("paint.lua").unwrap();

    // The brush starts centred and reports itself as entity tag 1.
    let start = c.run_frame(Input::default()).entities[0];

    // Full deflection right and down for a few frames must move it.
    let push = |x, y| Input {
        stick_x: x,
        stick_y: y,
        ..Input::default()
    };
    let mut moved = start;
    for _ in 0..5 {
        moved = c
            .run_frame(push(device::STICK_FULL, device::STICK_FULL))
            .entities[0];
    }
    assert!(
        moved.x > start.x && moved.y > start.y,
        "the stick did not move the brush: {start:?} -> {moved:?}"
    );

    // ...and the other way, which is the direction that breaks when a signed
    // deflection is read as an unsigned word: -256 arrives as 0xFF00, and a
    // game that divides it unsigned lurches 255 pixels instead of stepping one.
    let mut back = moved;
    for _ in 0..5 {
        back = c
            .run_frame(push(-device::STICK_FULL, -device::STICK_FULL))
            .entities[0];
    }
    assert!(
        back.x < moved.x && back.y < moved.y,
        "a negative deflection did not move the brush back: {moved:?} -> {back:?}"
    );
    assert!(
        moved.x - back.x <= 40 && moved.y - back.y <= 40,
        "five frames of full deflection moved the brush {} px — a sign was read \
         as a magnitude somewhere",
        moved.x - back.x
    );

    // A finger paints where it lands. Take the hash before and after: the game
    // draws onto a canvas it never clears, so a touch that reached the ROM has
    // to change pixels.
    let before = c.run_frame(Input::default()).framebuffer_hash;
    let finger = Input {
        touches: [
            Touch {
                x: 30,
                y: 200,
                down: true,
            },
            Touch::default(),
            Touch::default(),
            Touch::default(),
        ],
        ..Input::default()
    };
    let after = c.run_frame(finger);
    assert_ne!(
        before, after.framebuffer_hash,
        "a touch at (30,200) painted nothing"
    );
    // ...and it is reported back, so an agent can see what it pressed.
    let json = after.to_json();
    assert_eq!(json["touches"][0]["x"], 30, "observation: {json}");
}

/// piano.lua is the reference for the held-note API (`note_on`/`note_off`),
/// so a finger landing on a key must start a note on that finger's own
/// channel, sliding onto a different key must retune rather than stack a
/// second note, and lifting must release it — all without ever touching the
/// fire-and-forget `play()` path.
#[test]
fn piano_holds_and_releases_notes_under_a_finger() {
    use kessel_audio::AudioEvent;
    use kessel_vm::device::{Input, Touch};

    const PIANO: &str = include_str!("../../../games/piano.lua");
    let mut c = VmConsole::new();
    c.write_source("piano.lua", PIANO).unwrap();
    assert!(c.assemble("piano.lua").unwrap().ok());
    c.load_rom("piano.lua").unwrap();

    // No music, no autoplay — this instrument only ever sounds because a
    // finger asked it to.
    assert_eq!(c.take_reset_sound(), []);

    fn touch_at(x: u16, y: u16) -> Input {
        Input {
            touches: [
                Touch { x, y, down: true },
                Touch::default(),
                Touch::default(),
                Touch::default(),
            ],
            ..Input::default()
        }
    }

    // The leftmost white key (C4 = 60). y = 200 sits below the black keys'
    // reach (BLACK_H = 90 from KB_Y = 70, i.e. below 160), so a touch there
    // can only ever land on a white key. `vel_for_y`'s own formula, away
    // from a magic number: 90 + (200-70) * 130 / 150 = 202.
    const VEL: u8 = 202;
    let obs = c.run_frame(touch_at(4, 200));
    assert_eq!(
        obs.sound,
        [AudioEvent::NoteOn {
            chan: 0,
            inst: 0,
            note: 60,
            vel: VEL,
        }],
        "landing on the C key did not start it on channel 0"
    );

    // Holding still must not retrigger the note.
    let held = c.run_frame(touch_at(4, 200));
    assert!(
        held.sound.is_empty(),
        "a held key retriggered: {:?}",
        held.sound
    );

    // Sliding onto the next white key (D4 = 62) retunes: the old note off,
    // the new one on, both on the same channel.
    let slid = c.run_frame(touch_at(4 + 24, 200));
    assert_eq!(
        slid.sound,
        [
            AudioEvent::NoteOff { chan: 0 },
            AudioEvent::NoteOn {
                chan: 0,
                inst: 0,
                note: 62,
                vel: VEL,
            },
        ],
        "sliding onto D did not retune channel 0: {:?}",
        slid.sound
    );

    // Lifting the finger releases it.
    let lifted = c.run_frame(Input::default());
    assert_eq!(
        lifted.sound,
        [AudioEvent::NoteOff { chan: 0 }],
        "lifting the finger did not release the note"
    );
}

/// A and B shift the whole keybed an octave, and — since a key shown at one
/// pitch must never keep sounding at another — the shift has to release
/// whatever was held first.
#[test]
fn piano_octave_shift_releases_held_notes_and_retunes_the_keybed() {
    use kessel_audio::AudioEvent;
    use kessel_vm::device;
    use kessel_vm::device::{Input, Touch};

    const PIANO: &str = include_str!("../../../games/piano.lua");
    let mut c = VmConsole::new();
    c.write_source("piano.lua", PIANO).unwrap();
    assert!(c.assemble("piano.lua").unwrap().ok());
    c.load_rom("piano.lua").unwrap();

    fn touch_and_button(x: u16, y: u16, btn: u8) -> Input {
        let mut i: Input = btn.into();
        i.touches[0] = Touch { x, y, down: true };
        i
    }

    // Press the C key and hold it through the octave-up press.
    c.run_frame(touch_and_button(4, 150, 0));
    let shifted = c.run_frame(touch_and_button(4, 150, device::BTN_B));
    assert!(
        shifted
            .sound
            .iter()
            .any(|e| matches!(e, AudioEvent::NoteOff { chan: 0 })),
        "octave up did not release the held note: {:?}",
        shifted.sound
    );
    assert!(
        shifted.sound.iter().any(|e| matches!(
            e,
            AudioEvent::NoteOn {
                chan: 0,
                note: 72,
                ..
            }
        )),
        "octave up did not restart the same key a fifth-octave up (C5 = 72): {:?}",
        shifted.sound
    );
}

/// spectrum.lua is the reference for the video features, so its ROM must
/// actually select the wide screen and paint colours a 16-entry palette could
/// not name. A silent fallback to 128×128 would still "work" and still be wrong.
#[test]
fn spectrum_uses_the_extended_screen_and_high_colours() {
    use kessel_vm::device::{VideoMode, EXTENDED_DIM};

    let mut c = VmConsole::new();
    c.write_source("s.lua", include_str!("../../../games/spectrum.lua"))
        .unwrap();
    assert!(c.assemble("s.lua").unwrap().ok());
    c.load_rom("s.lua").unwrap();

    assert_eq!(c.video_mode(), VideoMode::Extended240);
    assert_eq!(c.screen_dim(), EXTENDED_DIM as u32);

    c.run_frame(0);
    let fb = &c.vm.devices.framebuffer;
    assert_eq!(fb.len(), EXTENDED_DIM * EXTENDED_DIM);
    assert!(
        fb.iter().any(|&p| p > 15),
        "nothing drawn above index 15 — the deep palette is unused"
    );
    assert!(
        fb.iter().any(|&p| p != 0),
        "the extended framebuffer is blank"
    );
}
