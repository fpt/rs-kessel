//! Differential play: run one state several ways, and report the difference.
//!
//! This is the gameplay half of what [`audio`](crate::audio) does for sound,
//! and it exists for the same reason. The agent cannot listen, so a render
//! returns numbers instead of a WAV; **the agent cannot play**, so a playtest
//! returns numbers instead of a screenshot.
//!
//! A screenshot is a spatial observation of a single instant. Whether a game
//! is worth playing is a property of how its state answers *input*, over
//! *time* — an axis that is not under-sampled by frames but absent from them.
//! So this module never returns a picture. See `docs/GAMEPLAY_METRICS.md`.
//!
//! The one question it is built around:
//!
//! > If playing well and not playing at all lead to the same place, the game is
//! > not responding to skill.
//!
//! That is decidable, and it is decidable *here* rather than in a host, because
//! deciding it needs the same starting state played several ways — which is
//! exactly what a deterministic, snapshotable machine is for. Nothing else in
//! the loop uses that property.
//!
//! **The tool does not know what a tag means, and must not learn.** `entity()`
//! is authored by the game precisely so the harness can be told what matters;
//! inferring which entity is the player would be guessing at something the ROM
//! already knows. So what is reported here is the *shape* of each tag's series —
//! how often it fires, how evenly, how it ends — and the game supplies the
//! meaning.

use std::collections::{BTreeMap, BTreeSet};

use crate::device::Input;
use crate::VmConsole;

/// Ceiling on one policy's run. Same number as `vm_run_frames`' batch cap: past
/// 30 seconds a blind run stops telling anyone anything.
pub const MAX_FRAMES: u64 = 1800;

/// A policy's name as a table column, and the width it will occupy.
///
/// Both are counted in **characters**, not bytes, and both have to be: a name
/// comes straight from the tool's JSON with no ASCII restriction, so
/// `&name[..11]` panics in the middle of a multibyte character and `name.len()`
/// as a column width over-counts one — a table that either crashes or comes out
/// ragged, from input the tool accepted without complaint. `Formatter::pad`
/// counts characters, so this counts them the same way.
const COL_MAX: usize = 11;

fn column(name: &str) -> String {
    name.chars().take(COL_MAX).collect()
}

fn column_width(name: &str) -> usize {
    name.chars().count().min(COL_MAX)
}

/// Ceiling on how many ways one call may play. Eight policies at the frame cap
/// is already four minutes of emulated play per call.
pub const MAX_POLICIES: usize = 8;

/// A habit, not a scenario.
///
/// `vm_run_frames` takes a script that is played once — "walk right 30 frames,
/// jump, wait" — because it is staging a specific situation. A policy is the
/// opposite thing: a way of holding the controller, which **loops** until the
/// frame budget is spent. That is what makes "mash A" two segments instead of
/// three hundred, and it is why this is a separate type rather than the same
/// `Vec<(Input, u64)>` under a name.
#[derive(Debug, Clone)]
pub struct Policy {
    pub name: String,
    pub segments: Vec<(Input, u64)>,
}

impl Policy {
    /// A segment longer than the run is clamped to [`MAX_FRAMES`], and that is
    /// an **identity**, not a safety clamp: `at` is only ever asked for frames
    /// below the run's own cap, so shortening a segment to that cap cannot
    /// change an answer anyone reads. Without it a script of
    /// `frames: 18446744073709551615` — which the tool's JSON accepts, since
    /// `as_u64` has no opinion — sums into an overflow, and the period a
    /// policy loops on is the one number that must not wrap.
    pub fn new(name: impl Into<String>, segments: Vec<(Input, u64)>) -> Self {
        Self {
            name: name.into(),
            segments: segments
                .into_iter()
                .filter(|(_, n)| *n > 0)
                .map(|(input, n)| (input, n.min(MAX_FRAMES)))
                .collect(),
        }
    }

    /// The input for frame `i` of this policy's run, looping the segments.
    fn at(&self, i: u64) -> Input {
        // Saturating, so the sum stays honest however many segments arrive.
        let period: u64 = self
            .segments
            .iter()
            .fold(0u64, |acc, (_, n)| acc.saturating_add(*n));
        if period == 0 {
            return Input::default();
        }
        let mut k = i % period;
        for (input, n) in &self.segments {
            if k < *n {
                return *input;
            }
            k -= n;
        }
        Input::default()
    }
}

fn held(names: &[&str]) -> Input {
    Input::from(crate::buttons_from_names(
        &names.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    ))
}

/// The default five, chosen so the comparison between them means something.
///
/// `idle` is the control: it is what the game does when nobody is there.
/// `random` is seeded and fixed rather than drawn from the clock, because a
/// playtest whose "random player" differed between two runs could not be used
/// to compare two builds — the whole point of running on this machine.
pub fn default_policies(frames: u64) -> Vec<Policy> {
    let none = Input::default();
    let mut rng: u32 = 0x1234_5678;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng
    };
    // A frame's worth of plausible flailing, changed every 6 frames — long
    // enough that a held direction actually travels, short enough to cover the
    // space in a few hundred frames.
    let combos = [
        vec![],
        vec!["LEFT"],
        vec!["RIGHT"],
        vec!["UP"],
        vec!["DOWN"],
        vec!["A"],
        vec!["LEFT", "A"],
        vec!["RIGHT", "A"],
        vec!["B"],
    ];
    let random: Vec<(Input, u64)> = (0..frames.div_ceil(6).max(1))
        .map(|_| {
            let c = &combos[(next() as usize) % combos.len()];
            (held(c), 6)
        })
        .collect();

    vec![
        Policy::new("idle", vec![(none, 1)]),
        Policy::new("mash-a", vec![(held(&["A"]), 2), (none, 4)]),
        Policy::new("hold-right", vec![(held(&["RIGHT"]), 1)]),
        Policy::new(
            "sweep",
            vec![(held(&["LEFT", "A"]), 40), (held(&["RIGHT", "A"]), 40)],
        ),
        Policy::new("random", random),
    ]
}

/// Everything one tag did during one policy's run.
#[derive(Debug, Clone, Default)]
struct TagTrace {
    /// Frames the tag appeared on, in order. The whole series, because the
    /// *spacing* is the thing being measured and a mean cannot be un-averaged.
    hits: Vec<u64>,
    /// How many records carried this tag on each of those frames.
    counts: Vec<u32>,
    total: u64,
    max_per_frame: u32,
    last: Option<(u16, u16)>,
}

impl TagTrace {
    /// Gaps between consecutive appearances, in frames.
    fn intervals(&self) -> Vec<u64> {
        self.hits.windows(2).map(|w| w[1] - w[0]).collect()
    }
}

/// What one named scalar did over one run.
#[derive(Debug, Clone)]
struct SigTrace {
    last: i32,
    lo: i32,
    hi: i32,
    /// Frames on which the value differed from the frame before. A signal that
    /// ends where it started but moved in between is a very different thing
    /// from one nothing ever touched, and only this tells them apart.
    moves: u64,
}

/// What one way of playing did.
#[derive(Debug, Clone)]
pub struct PolicyRun {
    pub name: String,
    pub frames: u64,
    pub stopped_early: Option<String>,
    pub screen_changes: u64,
    pub sound_triggers: u64,
    pub console: String,
    tags: BTreeMap<u16, TagTrace>,
    signals: BTreeMap<String, SigTrace>,
}

impl PolicyRun {
    /// Where the run ended, as far as the game chose to describe it: the last
    /// value under every signal and every tag, rendered.
    ///
    /// One string-keyed map rather than two typed ones, because every question
    /// below is "did these two runs end in the same place" and a comparison
    /// that could see signals but not tags would answer it differently
    /// depending on which the game happened to use.
    pub fn outcome(&self) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = self
            .signals
            .iter()
            .map(|(n, t)| (n.clone(), t.last.to_string()))
            .collect();
        for (t, tr) in &self.tags {
            if let Some((x, y)) = tr.last {
                out.insert(format!("tag {t}"), format!("{x},{y}"));
            }
        }
        out
    }
}

/// Every policy's run, plus what they have in common.
#[derive(Debug, Clone)]
pub struct PlaytestSummary {
    pub frames: u64,
    pub runs: Vec<PolicyRun>,
}

/// Play the current state `frames` frames under each policy, restoring between
/// them so every run starts in the same place — and again at the end.
///
/// Restoring afterwards is the one place this deliberately differs from
/// `vm_render_audio`, which says loudly that it advances the machine. A render
/// is a *rendering*; a playtest is a *measurement*, and a measurement that moved
/// the thing it measured would make two consecutive calls disagree for no
/// reason the agent could see. It already needs a snapshot to run more than one
/// policy, so putting the state back costs nothing.
pub fn playtest(
    console: &mut VmConsole,
    policies: &[Policy],
    frames: u64,
) -> Result<PlaytestSummary, String> {
    if policies.is_empty() {
        return Err("no policies to run".into());
    }
    if policies.len() > MAX_POLICIES {
        return Err(format!("at most {MAX_POLICIES} policies per call"));
    }
    let frames = frames.clamp(1, MAX_FRAMES);

    let start = console.snapshot();
    let mut runs = Vec::new();

    for p in policies {
        console.restore(&start)?;
        let base = console.frame;
        let mut tags: BTreeMap<u16, TagTrace> = BTreeMap::new();
        let mut signals: BTreeMap<String, SigTrace> = BTreeMap::new();
        let mut screen_changes = 0;
        let mut sound_triggers = 0;
        let mut console_out = String::new();
        let mut stopped_early = None;
        let mut ran = 0;

        for i in 0..frames {
            let obs = console.run_frame(p.at(i));
            ran += 1;
            if obs.changed_pixels_bbox.is_some() {
                screen_changes += 1;
            }
            sound_triggers += obs.sound.len() as u64;
            console_out.push_str(&obs.console);

            // Relative to the run's own start, so two policies compared side by
            // side are talking about the same instant.
            let f = obs.frame - base;
            let mut per_frame: BTreeMap<u16, u32> = BTreeMap::new();
            for e in &obs.entities {
                *per_frame.entry(e.tag).or_default() += 1;
                let tr = tags.entry(e.tag).or_default();
                tr.total += 1;
                tr.last = Some((e.x, e.y));
            }
            for (tag, n) in per_frame {
                let tr = tags.entry(tag).or_default();
                tr.hits.push(f);
                tr.counts.push(n);
                tr.max_per_frame = tr.max_per_frame.max(n);
            }

            for (name, value, _) in &obs.signals {
                match signals.get_mut(name) {
                    Some(t) => {
                        if *value != t.last {
                            t.moves += 1;
                        }
                        t.last = *value;
                        t.lo = t.lo.min(*value);
                        t.hi = t.hi.max(*value);
                    }
                    None => {
                        signals.insert(
                            name.clone(),
                            SigTrace {
                                last: *value,
                                lo: *value,
                                hi: *value,
                                moves: 0,
                            },
                        );
                    }
                }
            }

            if obs.halted || obs.fault.is_some() {
                stopped_early = Some(match &obs.fault {
                    Some(x) => format!("faulted: {x}"),
                    None => "halted".to_string(),
                });
                break;
            }
        }

        runs.push(PolicyRun {
            name: p.name.clone(),
            frames: ran,
            stopped_early,
            screen_changes,
            sound_triggers,
            console: console_out,
            tags,
            signals,
        });
    }

    console.restore(&start)?;
    Ok(PlaytestSummary { frames, runs })
}

/// Mean and population standard deviation of a set of gaps.
fn stats(v: &[u64]) -> (f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let n = v.len() as f32;
    let mean = v.iter().map(|x| *x as f32).sum::<f32>() / n;
    let var = v.iter().map(|x| (*x as f32 - mean).powi(2)).sum::<f32>() / n;
    (mean, var.sqrt())
}

impl PlaytestSummary {
    fn tags(&self) -> BTreeSet<u16> {
        self.runs
            .iter()
            .flat_map(|r| r.tags.keys().copied())
            .collect()
    }

    /// A tag is an **event** if it fires on a minority of frames and never twice
    /// in one — a spawn, a kill, a pickup. Anything else is a **population**: a
    /// thing that is simply there, whose count is the interesting number.
    ///
    /// The distinction is not cosmetic. Spacing is only meaningful for the
    /// first kind, and a tag reported every frame can be counted but never
    /// timed — which is why `games/shooter.lua` reports its spawns separately
    /// from its foes rather than letting the population stand in for both.
    fn is_event(&self, tag: u16) -> bool {
        let mut seen = false;
        for r in &self.runs {
            if let Some(tr) = r.tags.get(&tag) {
                seen = true;
                if tr.max_per_frame > 1 || tr.hits.len() as u64 * 4 > r.frames {
                    return false;
                }
                // …and it must fire on *isolated* frames. A tag that appears on
                // consecutive frames has persisted, however small a slice of
                // the run it covers — `games/shooter.lua` reports its game-over
                // state that way, and read as an event it came out as a
                // 1-frame "metronome", which is a confident piece of nonsense.
                let iv = tr.intervals();
                if !iv.is_empty() && iv.iter().filter(|g| **g > 1).count() * 2 < iv.len() {
                    return false;
                }
            }
        }
        seen
    }

    /// The report. Prose with numbers in it, for the same reason
    /// [`crate::audio::AudioSummary::report`] is: it is read by a model deciding
    /// what to change next, and a verdict it can act on beats a field it has to
    /// interpret. Every line here is willing to be an opinion.
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "playtest: {} policies x {} frames, each from the same state\n\n",
            self.runs.len(),
            self.frames
        ));

        // --- what each way of playing did ---
        out.push_str(&format!(
            "{:<12} {:>6} {:>8} {:>7}  {}\n",
            "policy", "frames", "redraws", "sounds", "ended"
        ));
        for r in &self.runs {
            out.push_str(&format!(
                "{:<12} {:>6} {:>8} {:>7}  {}\n",
                r.name,
                r.frames,
                r.screen_changes,
                r.sound_triggers,
                r.stopped_early.as_deref().unwrap_or("ran to the end")
            ));
        }

        // --- signals lead, because a name reads and a tag does not ---------
        let names: BTreeSet<&String> = self.runs.iter().flat_map(|r| r.signals.keys()).collect();
        if !names.is_empty() {
            let cw = self
                .runs
                .iter()
                .map(|r| column_width(&r.name))
                .max()
                .unwrap_or(8)
                .max(6);
            let label = names.iter().map(|n| n.len()).max().unwrap_or(6).max(6);
            out.push_str("\nsignals — where each way of playing left each named scalar:\n");
            out.push_str(&format!("  {:<label$}", "signal", label = label));
            for r in &self.runs {
                out.push_str(&format!(" {:>cw$}", column(&r.name), cw = cw));
            }
            out.push('\n');
            for n in &names {
                out.push_str(&format!("  {:<label$}", n, label = label));
                for r in &self.runs {
                    match r.signals.get(*n) {
                        Some(t) => out.push_str(&format!(" {:>cw$}", t.last, cw = cw)),
                        None => out.push_str(&format!(" {:>cw$}", "-", cw = cw)),
                    }
                }
                out.push('\n');
            }
            // The extremes, not just the ending. A score that peaked at 130 and
            // a run that has 0 hit points left both happened *during* the run,
            // and a final value alone cannot show either.
            let ranges: Vec<String> = names
                .iter()
                .filter_map(|n| {
                    let (lo, hi) = self
                        .runs
                        .iter()
                        .filter_map(|r| r.signals.get(*n))
                        .fold((i32::MAX, i32::MIN), |(a, b), t| (a.min(t.lo), b.max(t.hi)));
                    (lo < hi).then(|| format!("{n} {lo}..{hi}"))
                })
                .collect();
            if !ranges.is_empty() {
                out.push_str(&format!("  over the run: {}\n", ranges.join(", ")));
            }
            // A readout nothing can move is a readout nobody needs — either the
            // game never changes it, or it changes somewhere this run never got
            // to. Both are worth being told rather than squinting at a column.
            let dead: Vec<&str> = names
                .iter()
                .filter(|n| {
                    self.runs
                        .iter()
                        .all(|r| r.signals.get(**n).map(|t| t.moves) == Some(0))
                })
                .map(|n| n.as_str())
                .collect();
            if !dead.is_empty() {
                out.push_str(&format!(
                    "  NOTE: {} never moved under any policy — nothing anyone did touched it.\n",
                    dead.join(", ")
                ));
            }
            let stuck: Vec<&str> = names
                .iter()
                .filter(|n| {
                    let vals: BTreeSet<i32> = self
                        .runs
                        .iter()
                        .filter_map(|r| r.signals.get(**n))
                        .map(|t| t.last)
                        .collect();
                    vals.len() == 1
                        && self
                            .runs
                            .iter()
                            .any(|r| r.signals.get(**n).is_some_and(|t| t.moves > 0))
                })
                .map(|n| n.as_str())
                .collect();
            if !stuck.is_empty() {
                out.push_str(&format!(
                    "  NOTE: {} moved during the run but every policy left it in the same\n\
                     \x20       place. Playing well does not change where it ends up.\n",
                    stuck.join(", ")
                ));
            }
        }

        let tags = self.tags();
        if tags.is_empty() {
            out.push_str(
                "\nno entities reported — this game calls entity() nowhere, so a \
                 playtest can see\nonly whether the screen changed. Report the \
                 player, the hazards, and the score;\nwithout them nothing below \
                 can be measured.\n",
            );
            return out;
        }

        let events: Vec<u16> = tags.iter().copied().filter(|t| self.is_event(*t)).collect();
        let pops: Vec<u16> = tags
            .iter()
            .copied()
            .filter(|t| !self.is_event(*t))
            .collect();

        // --- rhythm: the shape of the game's pacing ---
        if !events.is_empty() {
            out.push_str("\nrhythm — tags that fire as events, and how evenly:\n");
            for t in &events {
                let mut lines = Vec::new();
                let mut signatures = BTreeSet::new();
                for r in &self.runs {
                    let Some(tr) = r.tags.get(t) else {
                        lines.push(format!("  tag {t:<4} {:<12} never fired", r.name));
                        signatures.insert("never".to_string());
                        continue;
                    };
                    let iv = tr.intervals();
                    let (mean, sd) = stats(&iv);
                    let lo = iv.iter().min().copied().unwrap_or(0);
                    let hi = iv.iter().max().copied().unwrap_or(0);
                    // The signature is the *interval*, deliberately not the
                    // number of fires. A policy that dies early fires fewer
                    // times while keeping the identical rhythm, and it is the
                    // rhythm that is the claim about the design.
                    signatures.insert(format!("{lo}/{hi}"));
                    if iv.is_empty() {
                        lines.push(format!(
                            "  tag {t:<4} {:<12} fired once, at frame {}",
                            r.name, tr.hits[0]
                        ));
                    } else {
                        lines.push(format!(
                            "  tag {t:<4} {:<12} {:>4} fires, every {:.0} frames ({lo}..{hi}, sd {sd:.1})",
                            r.name,
                            tr.hits.len(),
                            mean
                        ));
                    }
                }
                out.push_str(&lines.join("\n"));
                out.push('\n');

                // A perfectly even interval is a metronome. Say so: it is the
                // difference between pacing a player can learn and a timer.
                let flat = self.runs.iter().filter_map(|r| r.tags.get(t)).any(|tr| {
                    let iv = tr.intervals();
                    iv.len() >= 3 && stats(&iv).1 == 0.0
                });
                if flat {
                    out.push_str(
                        "           NOTE: the interval never varies. That is a metronome, not a\n\
                         \x20          rhythm — there is no wave for the player's hands to learn,\n\
                         \x20          and no rest to make the busy stretch feel busy.\n",
                    );
                }
                if signatures.len() == 1 && self.runs.len() > 1 {
                    out.push_str(
                        "           NOTE: identical under every policy. This tag's timing does not\n\
                         \x20          depend on how the game is played at all.\n",
                    );
                }
            }
        }

        // --- population: how much is on the screen, and when ---
        if !pops.is_empty() {
            out.push_str("\npopulation — tags that persist, and how many are up at once:\n");
            for t in &pops {
                let mut lo = u32::MAX;
                let mut hi = 0;
                let mut means: Vec<f32> = Vec::new();
                for r in &self.runs {
                    let Some(tr) = r.tags.get(t) else { continue };
                    lo = lo.min(tr.counts.iter().copied().min().unwrap_or(0));
                    hi = hi.max(tr.counts.iter().copied().max().unwrap_or(0));
                    means.push(tr.total as f32 / r.frames.max(1) as f32);
                }
                let mlo = means.iter().copied().fold(f32::MAX, f32::min);
                let mhi = means.iter().copied().fold(0.0f32, f32::max);
                out.push_str(&format!(
                    "  tag {t:<4} {lo}..{hi} at once, averaging {mlo:.1}..{mhi:.1} across policies\n"
                ));
            }
        }

        // --- the differential ---
        //
        // Two tables, both rows-are-tags, because the question is always "what
        // did this tag do under each way of playing" and a policy per column
        // keeps that on one line however many tags a game reports.
        let w = self
            .runs
            .iter()
            .map(|r| column_width(&r.name))
            .max()
            .unwrap_or(8)
            .max(6);
        let header = |out: &mut String| {
            out.push_str(&format!("  {:<8}", "tag"));
            for r in &self.runs {
                out.push_str(&format!(" {:>w$}", column(&r.name), w = w));
            }
            out.push('\n');
        };

        out.push_str("\nresponse — events: times fired; populations: frames present:\n");
        header(&mut out);
        let mut deaf = Vec::new();
        for t in &tags {
            out.push_str(&format!("  {:<8}", format!("tag {t}")));
            let mut cells = BTreeSet::new();
            for r in &self.runs {
                let n = r.tags.get(t).map(|tr| tr.hits.len()).unwrap_or(0);
                cells.insert((n, r.outcome().get(&format!("tag {t}")).cloned()));
                out.push_str(&format!(" {:>w$}", n, w = w));
            }
            out.push('\n');
            if cells.len() == 1 && self.runs.len() > 1 {
                deaf.push(*t);
            }
        }

        out.push_str("\nfinal reported value under each tag:\n");
        header(&mut out);
        for t in &tags {
            out.push_str(&format!("  {:<8}", format!("tag {t}")));
            for r in &self.runs {
                match r.outcome().get(&format!("tag {t}")) {
                    Some(v) => out.push_str(&format!(" {:>w$}", v, w = w)),
                    None => out.push_str(&format!(" {:>w$}", "-", w = w)),
                }
            }
            out.push('\n');
        }

        out.push('\n');
        if !deaf.is_empty() && self.runs.len() > 1 {
            let names: Vec<String> = deaf.iter().map(|t| format!("tag {t}")).collect();
            out.push_str(&format!(
                "NOTE: {} came out the same under every policy. Whatever those describe,\n\
                 \x20     it happens to the player rather than because of them.\n",
                names.join(", ")
            ));
        }

        // `idle` is the control, so measuring against it is the measurement.
        // A policy that barely moves away from doing nothing is the finding —
        // and it is invisible in any single run, however long.
        if let Some(base) = self.runs.iter().find(|r| r.name == "idle") {
            let bf = base.outcome();
            out.push_str("\nagainst `idle`, the control:\n");
            for r in &self.runs {
                if r.name == base.name {
                    continue;
                }
                let f = r.outcome();
                let keys: BTreeSet<&String> = bf.keys().chain(f.keys()).collect();
                let diff: Vec<String> = keys
                    .iter()
                    .filter(|k| f.get(**k) != bf.get(**k))
                    .map(|k| (*k).clone())
                    .collect();
                if diff.is_empty() {
                    out.push_str(&format!(
                        "  {:<12} identical to doing nothing, on every reported value.\n",
                        r.name
                    ));
                } else {
                    out.push_str(&format!(
                        "  {:<12} differs on {} of {} reported values: {}\n",
                        r.name,
                        diff.len(),
                        keys.len(),
                        diff.join(", ")
                    ));
                }
            }
            let faint: Vec<&str> = self
                .runs
                .iter()
                .filter(|r| {
                    r.name != base.name && {
                        let f = r.outcome();
                        let keys: BTreeSet<&String> = bf.keys().chain(f.keys()).collect();
                        let d = keys.iter().filter(|k| f.get(**k) != bf.get(**k)).count();
                        d > 0 && d * 3 <= keys.len().max(1)
                    }
                })
                .map(|r| r.name.as_str())
                .collect();
            if !faint.is_empty() {
                out.push_str(&format!(
                    "\nNOTE: {} moved the game hardly further than doing nothing did. An\n\
                     \x20     input the game answers this faintly is one the player will stop\n\
                     \x20     using.\n",
                    faint.join(", ")
                ));
            }
            let flat: Vec<&str> = self
                .runs
                .iter()
                .filter(|r| r.name != base.name && r.outcome() == bf)
                .map(|r| r.name.as_str())
                .collect();
            if !flat.is_empty() {
                out.push_str(&format!(
                    "\nWARNING: {} reached the same place as doing nothing. Playing and not\n\
                     \x20        playing are the same move here — fix that before tuning\n\
                     \x20        anything else.\n",
                    flat.join(", ")
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(src: &str) -> VmConsole {
        let mut c = VmConsole::new();
        c.write_source("t.lua", src).unwrap();
        let a = c.assemble("t.lua").unwrap();
        assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
        c.load_rom("t.lua").unwrap();
        c
    }

    /// A game that never reads a button. Every policy has to land in the same
    /// place, and saying so is the whole reason this module exists.
    const DEAF: &str = r#"
local t = 0
function init() t = 0 end
function update() t = t + 1 end
function draw()
  cls(0)
  entity(t, 0, 1)
  if t % 7 == 0 then entity(t, 0, 2) end
end
"#;

    /// The same shape, but the controls do something.
    ///
    /// It answers `RIGHT` as well as `A` on purpose. An earlier version read
    /// only `A`, which made `hold-right` genuinely identical to `idle` — the
    /// report was right and the fixture was wrong, which is the failure this
    /// whole module exists to make visible.
    const ALIVE: &str = r#"
local x = 0
local t = 0
function init() x = 0  t = 0 end
function update()
  t = t + 1
  if btn(A) then x = x + 1 end
  if btn(RIGHT) then x = x + 2 end
  if btn(LEFT) then x = x + 3 end
end
function draw()
  cls(0)
  entity(x, 0, 1)
  if t % 7 == 0 then entity(t, 0, 2) end
end
"#;

    /// A policy name is agent-supplied JSON with no ASCII restriction, and the
    /// report puts it in a fixed-width column. Truncating by byte offset split
    /// a multibyte character and panicked — turning a valid call into a failed
    /// one, and only for names nobody types while testing.
    #[test]
    fn a_non_ascii_policy_name_does_not_split_a_character() {
        let mut c = console(ALIVE);
        let policies = vec![
            Policy::new("ししししししししし", vec![(Input::default(), 4)]),
            Policy::new("mash-a", vec![(held(&["A"]), 2), (Input::default(), 2)]),
        ];
        let text = playtest(&mut c, &policies, 40).unwrap().report();
        assert!(
            text.contains("ししししししししし"),
            "the name was mangled out of the report:\n{text}"
        );
    }

    /// `frames` arrives as an unbounded `as_u64`, so a script may name a segment
    /// longer than any run. Summing two of those overflowed the period a policy
    /// loops on — a panic in debug, a wrapped period in release.
    #[test]
    fn an_absurd_segment_length_does_not_overflow_the_period() {
        let huge = u64::MAX;
        let p = Policy::new("huge", vec![(held(&["A"]), huge), (Input::default(), huge)]);
        // Clamping is an identity here: every frame of any run still reads the
        // first segment, exactly as an unbounded period would have given.
        for i in [0u64, 1, MAX_FRAMES - 1] {
            assert_eq!(p.at(i), held(&["A"]), "frame {i} left the first segment");
        }

        let mut c = console(ALIVE);
        let text = playtest(&mut c, &[p], 30).unwrap().report();
        assert!(text.contains("huge"), "the policy never ran:\n{text}");
    }

    #[test]
    fn a_game_that_ignores_input_is_named_as_one() {
        let mut c = console(DEAF);
        let r = playtest(&mut c, &default_policies(120), 120).unwrap();
        let text = r.report();
        assert!(
            text.contains("the same place as doing nothing"),
            "no verdict on a deaf game:\n{text}"
        );
    }

    #[test]
    fn a_game_that_answers_input_is_not_accused() {
        let mut c = console(ALIVE);
        let r = playtest(&mut c, &default_policies(120), 120).unwrap();
        let text = r.report();
        assert!(
            !text.contains("the same place as doing nothing"),
            "a responsive game was accused of ignoring the player:\n{text}"
        );
    }

    /// An event on a fixed period is the finding this was built to catch.
    #[test]
    fn a_constant_interval_is_called_a_metronome() {
        let mut c = console(ALIVE);
        let r = playtest(&mut c, &default_policies(120), 120).unwrap();
        let text = r.report();
        assert!(
            text.contains("metronome"),
            "constant interval missed:\n{text}"
        );
    }

    /// A measurement must not move the thing it measures — two calls in a row
    /// have to agree, and the state after one has to be the state before it.
    #[test]
    fn a_playtest_leaves_the_machine_where_it_found_it() {
        let mut c = console(ALIVE);
        for _ in 0..30 {
            c.run_frame(0u8);
        }
        let before = c.frame;
        let first = playtest(&mut c, &default_policies(60), 60)
            .unwrap()
            .report();
        assert_eq!(c.frame, before, "playtest advanced the console");
        let second = playtest(&mut c, &default_policies(60), 60)
            .unwrap()
            .report();
        assert_eq!(first, second, "two playtests of one state disagreed");
    }

    /// A policy is a habit: two segments have to cover a whole run.
    #[test]
    fn a_policy_script_loops_to_fill_the_run() {
        let p = Policy::new("mash", vec![(Input::from(1u8), 2), (Input::default(), 3)]);
        let seen: Vec<u8> = (0..12).map(|i| p.at(i).buttons).collect();
        assert_eq!(seen, vec![1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1]);
    }
}
