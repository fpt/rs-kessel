//! Every stage of `games/sokoban.lua` must be a *puzzle*: a sealed room, boxes
//! and goals that balance, and a solution.
//!
//! The levels are transcribed by hand from a text set into `data` blocks, and
//! the three ways that goes wrong are all silent. A wall typed as floor leaks
//! the room and the player walks out into the padding. A goal typed as floor
//! makes the stage unwinnable in a way that looks exactly like being stuck. A
//! box one cell off makes it unsolvable, which a player experiences as being bad
//! at Sokoban. None of these crash, and none of them fail the compile guard —
//! `games_compile.rs` is happy to run 300 frames of an impossible puzzle.
//!
//! So this reads the shipped source, decodes the blocks the game decodes, and
//! solves each one. A puzzle nobody has solved is not a level.

use kessel_vm::VmConsole;
use std::collections::{HashSet, VecDeque};

const SRC: &str = include_str!("../../../games/sokoban.lua");

/// The cell alphabet, which is the game's own — see the header comment there.
const VOID: u8 = 0;
const FLOOR: u8 = 1;
const WALL: u8 = 2;
const BOX: u8 = 3;
const GOAL: u8 = 4;
const BOX_ON_GOAL: u8 = 5;
const PLAYER: u8 = 6;
const PLAYER_ON_GOAL: u8 = 7;

struct Level {
    name: String,
    w: usize,
    h: usize,
    cells: Vec<u8>,
}

impl Level {
    fn at(&self, x: usize, y: usize) -> u8 {
        self.cells[y * self.w + x]
    }
    /// Stated as what a player *may* stand on rather than as "not a wall", so a
    /// cell character the alphabet does not cover is impassable rather than
    /// quietly walkable.
    fn walkable(&self, x: usize, y: usize) -> bool {
        matches!(
            self.at(x, y),
            FLOOR | BOX | GOAL | BOX_ON_GOAL | PLAYER | PLAYER_ON_GOAL
        )
    }
    fn is_wall(&self, x: usize, y: usize) -> bool {
        self.at(x, y) == WALL
    }
}

/// Pull every `data NAME { … }` block out of the game source, in order.
///
/// Reading the shipped file rather than a copy is the whole point: a fixture
/// would drift from the game the first time a level was edited, and the drift
/// would be invisible because both sides would still pass.
fn levels() -> Vec<Level> {
    let mut out = Vec::new();
    let mut lines = SRC.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim().strip_prefix("data ") else {
            continue;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            continue;
        };
        let mut rows: Vec<Vec<u8>> = Vec::new();
        for row in lines.by_ref() {
            let row = row.trim();
            if row == "}" {
                break;
            }
            rows.push(
                row.chars()
                    .map(|c| match c {
                        '.' => VOID,
                        '0'..='9' => c as u8 - b'0',
                        _ => panic!("{name}: bad cell char '{c}'"),
                    })
                    .collect(),
            );
        }
        let w = rows[0].len();
        assert!(
            rows.iter().all(|r| r.len() == w),
            "{name}: rows are not all the same length"
        );
        out.push(Level {
            name: name.to_string(),
            h: rows.len(),
            w,
            cells: rows.concat(),
        });
    }
    out
}

fn player_of(l: &Level) -> (usize, usize) {
    for y in 0..l.h {
        for x in 0..l.w {
            if matches!(l.at(x, y), PLAYER | PLAYER_ON_GOAL) {
                return (x, y);
            }
        }
    }
    panic!("{}: no player", l.name);
}

fn boxes_of(l: &Level) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    for y in 0..l.h {
        for x in 0..l.w {
            if matches!(l.at(x, y), BOX | BOX_ON_GOAL) {
                v.push((x, y));
            }
        }
    }
    v
}

fn goals_of(l: &Level) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    for y in 0..l.h {
        for x in 0..l.w {
            if matches!(l.at(x, y), GOAL | BOX_ON_GOAL | PLAYER_ON_GOAL) {
                v.push((x, y));
            }
        }
    }
    v
}

/// Every cell the player can stand on, ignoring boxes.
fn reachable(l: &Level) -> HashSet<(usize, usize)> {
    let mut seen = HashSet::new();
    let mut q = VecDeque::from([player_of(l)]);
    seen.insert(player_of(l));
    while let Some((x, y)) = q.pop_front() {
        for (nx, ny) in neighbours(l, x, y) {
            if l.walkable(nx, ny) && seen.insert((nx, ny)) {
                q.push_back((nx, ny));
            }
        }
    }
    seen
}

fn neighbours(l: &Level, x: usize, y: usize) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
        if nx >= 0 && ny >= 0 && (nx as usize) < l.w && (ny as usize) < l.h {
            v.push((nx as usize, ny as usize));
        }
    }
    v
}

/// Breadth-first over (player, boxes), which finds the fewest *moves* rather
/// than the fewest pushes — the same count the game's own HUD shows, so a
/// reported length is a number a player can compare themselves against.
///
/// Deadlock pruning is one rule: a box on a non-goal cell with walls on two
/// adjacent sides can never move again. That is enough to keep these twelve
/// inside a second; a full corral/freeze analysis would be a solver, and this
/// is a guard.
fn solve(l: &Level) -> Option<Vec<(i32, i32)>> {
    let start_boxes: Vec<(usize, usize)> = {
        let mut b = boxes_of(l);
        b.sort_unstable();
        b
    };
    let goals: HashSet<(usize, usize)> = goals_of(l).into_iter().collect();
    let start = (player_of(l), start_boxes);
    let mut seen = HashSet::new();
    seen.insert(start.clone());
    let mut q = VecDeque::from([(start, Vec::new())]);

    while let Some((((px, py), boxes), path)) = q.pop_front() {
        if boxes.iter().all(|b| goals.contains(b)) {
            return Some(path);
        }
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (px as i32 + dx, py as i32 + dy);
            if nx < 0 || ny < 0 || nx as usize >= l.w || ny as usize >= l.h {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if !l.walkable(nx, ny) {
                continue;
            }
            let mut next = boxes.clone();
            if let Some(i) = boxes.iter().position(|&b| b == (nx, ny)) {
                let (tx, ty) = (nx as i32 + dx, ny as i32 + dy);
                if tx < 0 || ty < 0 || tx as usize >= l.w || ty as usize >= l.h {
                    continue;
                }
                let (tx, ty) = (tx as usize, ty as usize);
                if !l.walkable(tx, ty) || boxes.contains(&(tx, ty)) {
                    continue;
                }
                if !goals.contains(&(tx, ty)) && cornered(l, tx, ty) {
                    continue; // dead box: nothing after this can win
                }
                next[i] = (tx, ty);
                next.sort_unstable();
            }
            let state = ((nx, ny), next);
            if seen.insert(state.clone()) {
                let mut path = path.clone();
                path.push((dx, dy));
                q.push_back((state, path));
            }
        }
    }
    None
}

/// A cell with blocked cells on two perpendicular sides: a box pushed here can
/// never leave it.
fn cornered(l: &Level, x: usize, y: usize) -> bool {
    let blocked = |dx: i32, dy: i32| {
        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
        nx < 0
            || ny < 0
            || nx as usize >= l.w
            || ny as usize >= l.h
            || !l.walkable(nx as usize, ny as usize)
    };
    (blocked(-1, 0) || blocked(1, 0)) && (blocked(0, -1) || blocked(0, 1))
}

#[test]
fn every_stage_is_a_sealed_room() {
    for l in levels() {
        for &(x, y) in &reachable(&l) {
            assert!(
                x > 0 && y > 0 && x < l.w - 1 && y < l.h - 1,
                "{}: the player reaches ({x},{y}) on the grid edge — the room leaks",
                l.name
            );
            for (nx, ny) in neighbours(&l, x, y) {
                assert!(
                    l.walkable(nx, ny) || l.is_wall(nx, ny),
                    "{}: ({x},{y}) is reachable and touches the outside at ({nx},{ny}) — \
                     a wall was typed as floor, or floor as outside",
                    l.name
                );
            }
        }
    }
}

#[test]
fn every_stage_balances_its_boxes_and_goals() {
    for l in levels() {
        let (boxes, goals) = (boxes_of(&l).len(), goals_of(&l).len());
        assert!(boxes > 0, "{}: no boxes", l.name);
        assert_eq!(
            boxes, goals,
            "{}: {boxes} box(es) and {goals} goal(s) can never balance",
            l.name
        );
        for b in boxes_of(&l) {
            assert!(
                l.walkable(b.0, b.1),
                "{}: a box at {b:?} is inside a wall",
                l.name
            );
        }
    }
}

/// The one that matters, end to end: solve each stage against an independent
/// model, then **play the solution into the real ROM** and require the game to
/// agree that it cleared.
///
/// Solving alone would only prove the transcription is a puzzle. Playing it
/// proves the game's own push rules are the rules the puzzle was solved under —
/// a box that the ROM refuses to push into a goal, or a wall it walks through,
/// shows up here and nowhere else. The previous version of this guard was in
/// `games_compile.rs` with four hand-typed key sequences, which is why it could
/// only ever cover four hand-made stages.
///
/// It also prints the optimal move count per stage, which is how the difficulty
/// curve gets checked by eye when a level is swapped.
#[test]
fn every_stage_is_solvable_and_the_rom_agrees() {
    const BUTTONS: [(i32, i32, u8); 4] = [
        (-1, 0, 0x01), // LEFT
        (1, 0, 0x02),  // RIGHT
        (0, -1, 0x04), // UP
        (0, 1, 0x08),  // DOWN
    ];
    const A: u8 = 0x10;

    let all = levels();
    assert_eq!(all.len(), 12, "the game declares twelve stages");

    let mut c = VmConsole::new();
    c.write_source("s.lua", SRC).unwrap();
    assert!(c.assemble("s.lua").unwrap().ok(), "sokoban did not compile");
    c.load_rom("s.lua").unwrap();

    let mut hardest = 0;
    for (i, l) in all.iter().enumerate() {
        let stage = (i + 1) as u16;
        assert_eq!(
            c.run_frame(0u8).entities[0].tag,
            stage,
            "the ROM is not on stage {stage}"
        );

        let path = solve(l).unwrap_or_else(|| panic!("{}: no solution exists", l.name));
        println!("{:>8}: {} moves", l.name, path.len());
        hardest = hardest.max(path.len());

        // Press then release each step, so the game's `btnp` edge fires once.
        for (dx, dy) in &path {
            let btn = BUTTONS
                .iter()
                .find(|(bx, by, _)| bx == dx && by == dy)
                .map(|(_, _, b)| *b)
                .unwrap();
            c.run_frame(btn);
            c.run_frame(0u8);
        }

        let obs = c.run_frame(0u8);
        let cleared = obs
            .signals
            .iter()
            .find(|(n, _, _)| n == "cleared")
            .expect("the game reports a `cleared` signal")
            .1;
        assert_eq!(
            cleared,
            1,
            "{}: the ROM does not agree the stage is solved after {} moves",
            l.name,
            path.len()
        );

        // A advances after a clear — and *restarts* when not cleared, so the
        // stage number moving on is itself a second confirmation.
        c.run_frame(A);
        c.run_frame(0u8);
        let next = c.run_frame(0u8).entities[0].tag;
        let want = if i + 1 == all.len() { 1 } else { stage + 1 };
        assert_eq!(next, want, "{}: stage did not advance", l.name);
    }
    assert!(
        hardest > 60,
        "the set never gets beyond a warm-up (hardest is {hardest} moves)"
    );
}
