//! A tiny fantasy-console VM for the "let the model write, run, observe, debug a
//! game" loop. Pure Rust, deterministic, and snapshotable.
//!
//! - [`isa`]  — the 34-opcode instruction set.
//! - [`vm`]   — the stack machine (memory, stacks, fetch/execute, frame runner).
//! - [`device`] — the Varvara-lite device layer (screen, gamepad, rng, storage, debug, console).
//! - [`assembler`] — a two-pass textual assembler → ROM + diagnostics.
//! - [`png`]  — dependency-free PNG + base64 for framebuffer output.
//! - [`tools`] — the `vm_*` [`crate::tool::ToolHandler`]s exposed to the agent.
//!
//! [`VmConsole`] holds all mutable state; the tools share one behind a
//! `Arc<Mutex<…>>`, and (in a later phase) the host window drives the same
//! console at 60 Hz for human play.

pub mod assembler;
pub mod device;
pub mod isa;
pub mod luax;
pub mod player;
pub mod png;
pub mod tools;
pub mod vm;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use device::SCREEN_DIM;
use vm::{RunOutcome, Vm};

/// The whole console: the machine plus the authoring workspace (sources, built
/// ROMs, snapshots) and the bookkeeping the observation JSON needs.
pub struct VmConsole {
    pub vm: Vm,
    pub rom_loaded: bool,
    pub frame: u64,
    /// Framebuffer at the end of the previous frame, for change detection.
    prev_fb: Vec<u8>,
    /// Project root, when a project is open. With one set, **the filesystem is
    /// the workspace**: sources are read from (and written to) disk, so whatever
    /// the backend's own file tools or a human editor put in `game.lua` is what
    /// gets compiled. Without one the console keeps sources in `sources` below
    /// (how `VmPlayer` and the tests use it).
    root: Option<PathBuf>,
    /// Source files the model has written, when no project root is set.
    sources: HashMap<String, String>,
    /// Assembled ROMs, keyed by source path.
    roms: HashMap<String, Vec<u8>>,
    /// Control-layout metadata, keyed by source path (see [`luax::Controls`]).
    controls: HashMap<String, luax::Controls>,
    /// Control metadata of the currently loaded ROM (default until a load).
    active_controls: luax::Controls,
    /// Host-play pause state (managed by [`play_tick`](Self::play_tick)).
    paused: bool,
    prev_pause_down: bool,
    /// Saved states, keyed by snapshot id.
    snapshots: HashMap<String, Snapshot>,
    snap_counter: u64,
}

#[derive(Clone)]
struct Snapshot {
    vm: Vm,
    frame: u64,
    prev_fb: Vec<u8>,
    rom_loaded: bool,
}

impl Default for VmConsole {
    fn default() -> Self {
        Self::new()
    }
}

impl VmConsole {
    pub fn new() -> Self {
        VmConsole {
            vm: Vm::new(),
            rom_loaded: false,
            frame: 0,
            prev_fb: vec![0u8; device::SCREEN_PIXELS],
            root: None,
            sources: HashMap::new(),
            roms: HashMap::new(),
            controls: HashMap::new(),
            active_controls: luax::Controls::default(),
            paused: false,
            prev_pause_down: false,
            snapshots: HashMap::new(),
            snap_counter: 0,
        }
    }

    /// Point the console at a project root (or clear it). Sources and ROMs from
    /// the previous workspace are dropped — they describe a different game.
    pub fn set_root(&mut self, root: Option<PathBuf>) {
        self.root = root;
        self.sources.clear();
        self.roms.clear();
        self.controls.clear();
    }

    /// The active project root, if a project is open.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Write a source file. With a project open this writes through to disk, so
    /// the file the model authored is the same one the backend's file tools and
    /// `kessel --play` see; otherwise it is kept in memory.
    pub fn write_source(&mut self, path: &str, source: &str) -> Result<(), String> {
        if let Some(root) = &self.root {
            let full = crate::project::resolve_in_root(root, path)?;
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create '{}': {e}", parent.display()))?;
            }
            std::fs::write(&full, source)
                .map_err(|e| format!("write '{}': {e}", full.display()))?;
        } else {
            self.sources.insert(path.to_string(), source.to_string());
        }
        // Invalidate any previously built ROM for this path.
        self.roms.remove(path);
        Ok(())
    }

    /// Read a source file — from the project directory when one is open, else
    /// from the in-memory workspace.
    pub fn get_source(&self, path: &str) -> Option<String> {
        match &self.root {
            Some(root) => {
                let full = crate::project::resolve_in_root(root, path).ok()?;
                std::fs::read_to_string(full).ok()
            }
            None => self.sources.get(path).cloned(),
        }
    }

    /// Source files that *are* available, for "no source at 'x'" errors. Sorted;
    /// from the project directory when one is open, else the in-memory map.
    pub fn list_sources(&self) -> Vec<String> {
        let mut names: Vec<String> = match &self.root {
            Some(root) => std::fs::read_dir(root)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| is_lua(n) || n.to_ascii_lowercase().ends_with(".asm"))
                        .collect()
                })
                .unwrap_or_default(),
            None => self.sources.keys().cloned().collect(),
        };
        names.sort();
        names
    }

    /// Assemble a previously written source. On success the ROM is cached for
    /// [`load_rom`](Self::load_rom). Sources ending in `.lua` are first compiled
    /// from the luax front-end to assembler, then assembled.
    pub fn assemble(&mut self, path: &str) -> Result<assembler::Assembled, String> {
        let src = &self.get_source(path).ok_or_else(|| {
            let available = self.list_sources();
            let known = if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            };
            match self.root() {
                Some(root) => format!(
                    "no source at '{path}' in the project ({}) — available: {known}",
                    root.display()
                ),
                None => format!("no source written at '{path}' — available: {known}"),
            }
        })?;

        // luax (Lua-ish) dialect: compile to assembler first. Compiler
        // diagnostics are returned in an otherwise-empty `Assembled`. The
        // control-layout metadata rides along and is cached for `load_rom`.
        let (built, controls) = if is_lua(path) {
            let compiled = luax::compile(src);
            if !compiled.ok() {
                return Ok(assembler::Assembled {
                    rom: Vec::new(),
                    diagnostics: compiled.diagnostics,
                    labels: Default::default(),
                });
            }
            (assembler::assemble(&compiled.asm), compiled.controls)
        } else {
            // Raw assembly has no `controls` block; use the default layout.
            (assembler::assemble(src), luax::Controls::default())
        };

        if built.ok() {
            self.roms.insert(path.to_string(), built.rom.clone());
            self.controls.insert(path.to_string(), controls);
        }
        Ok(built)
    }

    /// Load a built ROM and run its reset vector.
    pub fn load_rom(&mut self, path: &str) -> Result<RunOutcome, String> {
        let rom = self
            .roms
            .get(path)
            .ok_or_else(|| format!("no assembled ROM for '{path}' — call vm_assemble first"))?
            .clone();
        let outcome = self.vm.load_rom(&rom);
        self.rom_loaded = true;
        self.frame = 0;
        self.prev_fb = self.vm.devices.framebuffer.clone();
        self.active_controls = self.controls.get(path).cloned().unwrap_or_default();
        self.paused = false;
        self.prev_pause_down = false;
        Ok(outcome)
    }

    /// The control-layout metadata of the currently loaded ROM.
    pub fn controls(&self) -> &luax::Controls {
        &self.active_controls
    }

    /// Whether host play is currently paused (see [`play_tick`](Self::play_tick)).
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// A host-play frame tick: toggle pause on the rising edge of the ROM's
    /// pause button, then advance one frame **unless** paused. The pause button
    /// (default START) comes from the ROM's `controls` metadata; it is a host
    /// control, so its bit is **masked out** of the buttons handed to the game —
    /// the game never sees it, not even on the frame play resumes (otherwise the
    /// resume press would leak in as a `btn`/`btnp` on the pause button). Used by
    /// the play window; the agent's `vm_run_frame` drives
    /// [`run_frame`](Self::run_frame) directly instead.
    pub fn play_tick(&mut self, buttons: u8) {
        let pause_bit = self.active_controls.pause_bit();
        let down = pause_bit != 0 && buttons & pause_bit != 0;
        if down && !self.prev_pause_down {
            self.paused = !self.paused;
        }
        self.prev_pause_down = down;
        if !self.paused {
            self.run_frame(buttons & !pause_bit);
        }
    }

    /// Advance one frame with `buttons` held; returns the observation record.
    pub fn run_frame(&mut self, buttons: u8) -> Observation {
        let outcome = self.vm.run_frame(buttons, vm::cap());
        self.frame += 1;
        let obs = self.observe(buttons, outcome);
        self.prev_fb = self.vm.devices.framebuffer.clone();
        obs
    }

    fn observe(&self, buttons: u8, outcome: RunOutcome) -> Observation {
        let fb = &self.vm.devices.framebuffer;
        let bbox = changed_bbox(&self.prev_fb, fb);
        let fault = match outcome {
            RunOutcome::CapExceeded => Some(format!("frame cycle cap ({}) exceeded", vm::cap())),
            _ => self.vm.fault.clone(),
        };
        Observation {
            frame: self.frame,
            cycles: self.vm.cycle,
            buttons: button_names(buttons),
            framebuffer_hash: fnv1a(fb),
            changed_pixels_bbox: bbox,
            console: String::from_utf8_lossy(&self.vm.devices.console).into_owned(),
            fault,
            pc: self.vm.pc,
            data_stack: self.vm.data_stack(),
            return_stack_depth: self.vm.return_stack_depth(),
            entities: self.vm.devices.entities.clone(),
            sound: self.vm.devices.sound.clone(),
            halted: self.vm.halted,
        }
    }

    /// The current framebuffer expanded to RGBA (for PNG / host window).
    pub fn framebuffer_rgba(&self) -> Vec<u8> {
        self.vm.devices.framebuffer_rgba()
    }

    /// Encode the current framebuffer as a base64 PNG.
    pub fn framebuffer_png_base64(&self) -> String {
        let rgba = self.framebuffer_rgba();
        let png = png::encode_rgba(SCREEN_DIM as u32, SCREEN_DIM as u32, &rgba);
        png::base64_encode(&png)
    }

    pub fn snapshot(&mut self) -> String {
        self.snap_counter += 1;
        let id = format!("snap{}", self.snap_counter);
        self.snapshots.insert(
            id.clone(),
            Snapshot {
                vm: self.vm.clone(),
                frame: self.frame,
                prev_fb: self.prev_fb.clone(),
                rom_loaded: self.rom_loaded,
            },
        );
        id
    }

    pub fn restore(&mut self, id: &str) -> Result<(), String> {
        let snap = self
            .snapshots
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no snapshot '{id}'"))?;
        self.vm = snap.vm;
        self.frame = snap.frame;
        self.prev_fb = snap.prev_fb;
        self.rom_loaded = snap.rom_loaded;
        Ok(())
    }

    pub fn reset(&mut self) {
        let keep_root = self.root.take();
        let keep_sources = std::mem::take(&mut self.sources);
        let keep_roms = std::mem::take(&mut self.roms);
        let keep_controls = std::mem::take(&mut self.controls);
        *self = VmConsole::new();
        self.root = keep_root;
        self.sources = keep_sources;
        self.roms = keep_roms;
        self.controls = keep_controls;
    }
}

/// One frame's observation, per the harness spec. Serialized to JSON by the
/// `vm_run_frame` tool.
#[derive(Debug, Clone)]
pub struct Observation {
    pub frame: u64,
    pub cycles: u64,
    pub buttons: Vec<String>,
    pub framebuffer_hash: String,
    pub changed_pixels_bbox: Option<[u16; 4]>,
    pub console: String,
    pub fault: Option<String>,
    pub pc: u16,
    pub data_stack: Vec<u16>,
    pub return_stack_depth: usize,
    pub entities: Vec<device::Entity>,
    pub sound: Vec<device::SoundEvent>,
    pub halted: bool,
}

impl Observation {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "frame": self.frame,
            "cycles": self.cycles,
            "buttons": self.buttons,
            "framebuffer_hash": self.framebuffer_hash,
            "changed_pixels_bbox": self.changed_pixels_bbox.map(|b| b.to_vec()),
            "console": self.console,
            "fault": self.fault,
            "halted": self.halted,
            "vm": {
                "pc": self.pc,
                "data_stack": self.data_stack,
                "return_stack_depth": self.return_stack_depth,
            },
            "entities": self.entities.iter().map(|e| serde_json::json!({
                "tag": e.tag, "x": e.x, "y": e.y,
            })).collect::<Vec<_>>(),
            "sound": self.sound.iter().map(|s| serde_json::json!({
                "kind": match s.kind {
                    device::SoundKind::Sfx => "sfx",
                    device::SoundKind::Music => "music",
                    device::SoundKind::MusicStop => "music_stop",
                },
                "id": s.id,
            })).collect::<Vec<_>>(),
        })
    }
}

/// True if a source path selects the luax (Lua-ish) dialect (`.lua`).
fn is_lua(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".lua")
}

/// FNV-1a (64-bit) of the framebuffer, as a hex string.
fn fnv1a(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Bounding box (x0,y0,x1,y1 inclusive) of pixels that differ between two
/// framebuffers, or `None` if identical.
fn changed_bbox(prev: &[u8], cur: &[u8]) -> Option<[u16; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (u16::MAX, u16::MAX, 0u16, 0u16);
    let mut any = false;
    for (i, (&a, &b)) in prev.iter().zip(cur.iter()).enumerate() {
        if a != b {
            any = true;
            let x = (i % SCREEN_DIM) as u16;
            let y = (i / SCREEN_DIM) as u16;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    any.then_some([x0, y0, x1, y1])
}

/// Map a button bitfield to human-readable names (stable order).
pub fn button_names(bits: u8) -> Vec<String> {
    use device::*;
    let table = [
        (BTN_LEFT, "LEFT"),
        (BTN_RIGHT, "RIGHT"),
        (BTN_UP, "UP"),
        (BTN_DOWN, "DOWN"),
        (BTN_A, "A"),
        (BTN_B, "B"),
        (BTN_START, "START"),
        (BTN_SELECT, "SELECT"),
    ];
    table
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| name.to_string())
        .collect()
}

/// Parse button names (case-insensitive) into a bitfield. Unknown names are ignored.
pub fn buttons_from_names(names: &[String]) -> u8 {
    use device::*;
    let mut bits = 0u8;
    for n in names {
        bits |= match n.to_ascii_uppercase().as_str() {
            "LEFT" => BTN_LEFT,
            "RIGHT" => BTN_RIGHT,
            "UP" => BTN_UP,
            "DOWN" => BTN_DOWN,
            "A" => BTN_A,
            "B" => BTN_B,
            "START" => BTN_START,
            "SELECT" => BTN_SELECT,
            _ => 0,
        };
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_assemble_load_runframe_loop() {
        let mut c = VmConsole::new();
        // A game: reset installs the frame vector and sets player-x = 32. Each
        // frame, LEFT decrements player-x, then a pixel is drawn at (player-x, 60)
        // and an entity (tag 1) is reported there for observation.
        let clean = r#"
            on-frame #10 DEO
            #20 player-x STORE16
            RET

            @on-frame
                #20 DEI #01 AND draw JZ    ( if LEFT not pressed, jump to draw )
                player-x LOAD16 #01 SUB player-x STORE16
                @draw
                player-x LOAD16 #11 DEO
                60 #12 DEO
                #07 #13 DEO
                #00 #14 DEO
                player-x LOAD16 #50 DEO
                60 #51 DEO
                #01 #52 DEO
                RET

            @player-x .res 2
        "#;

        c.write_source("game.asm", clean).unwrap();
        let built = c.assemble("game.asm").expect("assemble call");
        assert!(built.ok(), "assemble errors: {:?}", built.diagnostics);
        let outcome = c.load_rom("game.asm").expect("load");
        assert_eq!(
            outcome,
            RunOutcome::Completed,
            "reset fault: {:?}",
            c.vm.fault
        );

        // Frame 1: no buttons -> player stays at 32, entity reported at x=32.
        let o1 = c.run_frame(0);
        assert!(o1.fault.is_none(), "frame1 fault: {:?}", o1.fault);
        assert_eq!(o1.entities.len(), 1);
        assert_eq!(o1.entities[0].x, 32);
        assert!(o1.changed_pixels_bbox.is_some());

        // Frame 2: hold LEFT -> player-x decreases to 31.
        let o2 = c.run_frame(device::BTN_LEFT);
        assert_eq!(o2.buttons, vec!["LEFT"]);
        assert_eq!(o2.entities[0].x, 31);
    }

    #[test]
    fn play_tick_pauses_and_masks_the_pause_button() {
        // A game that (a) advances a counter each frame and (b) reacts to the
        // pause button (START) itself via btnp — reporting the counter as an
        // entity so we can read it. Pause must freeze the counter AND the game
        // must never observe a START press (default pause button), even on the
        // frame play resumes.
        let src = r#"
            local n = 0
            local hits = 0
            function update()
              n = n + 1
              if btnp(START) then hits = hits + 1 end
            end
            function draw() cls(0)  entity(n, hits, 1) end
        "#;
        let mut c = VmConsole::new();
        c.write_source("p.lua", src).unwrap();
        assert!(c.assemble("p.lua").unwrap().ok());
        c.load_rom("p.lua").unwrap();

        // The last entity reported: (x = n, y = hits).
        let read = |c: &VmConsole| {
            let e = c.vm.devices.entities.last().copied().unwrap();
            (e.x, e.y)
        };

        c.play_tick(0); // n=1
        c.play_tick(0); // n=2
        assert_eq!(read(&c), (2, 0));
        assert!(!c.is_paused());

        c.play_tick(device::BTN_START); // pause (down edge): frame skipped
        assert!(c.is_paused());
        assert_eq!(read(&c), (2, 0), "frozen while paused");
        c.play_tick(0); // release, still paused
        c.play_tick(device::BTN_START); // resume (down edge)
        assert!(!c.is_paused());
        // n advanced to 3, but hits is STILL 0: the pause button was masked out,
        // so btnp(START) never fired despite the game watching for it.
        assert_eq!(read(&c), (3, 0), "pause button leaked into the game");
    }

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut c = VmConsole::new();
        c.write_source(
            "s.asm",
            "on-frame #10 DEO RET @on-frame player-x LOAD16 #01 ADD player-x STORE16 RET @player-x .res 2",
        ).unwrap();
        assert!(c.assemble("s.asm").unwrap().ok());
        c.load_rom("s.asm").unwrap();
        c.run_frame(0); // player-x: 0 -> 1
        let id = c.snapshot();
        let x_at_snap = read_u16(&c, "s.asm");
        c.run_frame(0); // player-x: 1 -> 2
        assert_ne!(read_u16(&c, "s.asm"), x_at_snap);
        c.restore(&id).unwrap();
        assert_eq!(read_u16(&c, "s.asm"), x_at_snap);
    }

    // Helper: read the 16-bit variable at label player-x from the loaded ROM.
    fn read_u16(c: &VmConsole, path: &str) -> u16 {
        let built = assembler::assemble(&c.get_source(path).unwrap());
        let addr = *built.labels.get("player-x").unwrap();
        let hi = c.vm.mem[addr as usize];
        let lo = c.vm.mem[addr as usize + 1];
        u16::from_be_bytes([hi, lo])
    }

    /// The exact program printed in docs/VM.md must assemble and behave.
    #[test]
    fn doc_example_move_pixel() {
        let src = r#"
            ( reset: install the frame vector, put the player at x=32 )
            on-frame #10 DEO
            #20 player-x STORE16
            RET

            @on-frame
                #20 DEI #01 AND  skip-left JZ
                player-x LOAD16 #01 SUB player-x STORE16
                @skip-left

                #20 DEI #02 AND  skip-right JZ
                player-x LOAD16 #01 ADD player-x STORE16
                @skip-right

                player-x LOAD16 #11 DEO
                60 #12 DEO
                #07 #13 DEO
                #00 #14 DEO

                player-x LOAD16 #50 DEO
                60 #51 DEO
                #01 #52 DEO
                RET

            @player-x .res 2
        "#;
        let mut c = VmConsole::new();
        c.write_source("doc.asm", src).unwrap();
        let built = c.assemble("doc.asm").expect("assemble");
        assert!(built.ok(), "doc example errors: {:?}", built.diagnostics);
        assert_eq!(c.load_rom("doc.asm").unwrap(), RunOutcome::Completed);

        assert_eq!(c.run_frame(0).entities[0].x, 32); // idle
        assert_eq!(c.run_frame(device::BTN_LEFT).entities[0].x, 31); // left
        assert_eq!(c.run_frame(device::BTN_RIGHT).entities[0].x, 32); // right back
        assert_eq!(c.run_frame(device::BTN_RIGHT).entities[0].x, 33); // right again
    }

    #[test]
    fn button_name_roundtrip() {
        let bits = buttons_from_names(&["left".into(), "A".into()]);
        assert_eq!(bits, device::BTN_LEFT | device::BTN_A);
        let names = button_names(bits);
        assert_eq!(names, vec!["LEFT", "A"]);
    }
}
