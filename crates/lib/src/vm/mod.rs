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
pub mod png;
pub mod tools;
pub mod uxlang;
pub mod vm;

use std::collections::HashMap;

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
    /// Source files the model has written (keyed by path).
    sources: HashMap<String, String>,
    /// Assembled ROMs, keyed by source path.
    roms: HashMap<String, Vec<u8>>,
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
            sources: HashMap::new(),
            roms: HashMap::new(),
            snapshots: HashMap::new(),
            snap_counter: 0,
        }
    }

    pub fn write_source(&mut self, path: &str, source: &str) {
        self.sources.insert(path.to_string(), source.to_string());
        // Invalidate any previously built ROM for this path.
        self.roms.remove(path);
    }

    pub fn get_source(&self, path: &str) -> Option<&String> {
        self.sources.get(path)
    }

    /// Assemble a previously written source. On success the ROM is cached for
    /// [`load_rom`](Self::load_rom). Sources ending in `.ux` are first compiled
    /// from the uxlang front-end to assembler, then assembled.
    pub fn assemble(&mut self, path: &str) -> Result<assembler::Assembled, String> {
        let src = self
            .sources
            .get(path)
            .ok_or_else(|| format!("no source written at '{path}'"))?;

        // uxlang dialect: compile to assembler first. Compiler diagnostics are
        // returned in an otherwise-empty `Assembled` so the tool can report them.
        let built = if is_uxlang(path) {
            let compiled = uxlang::compile(src);
            if !compiled.ok() {
                return Ok(assembler::Assembled {
                    rom: Vec::new(),
                    diagnostics: compiled.diagnostics,
                    labels: Default::default(),
                });
            }
            assembler::assemble(&compiled.asm)
        } else {
            assembler::assemble(src)
        };

        if built.ok() {
            self.roms.insert(path.to_string(), built.rom.clone());
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
        Ok(outcome)
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
        let keep_sources = std::mem::take(&mut self.sources);
        let keep_roms = std::mem::take(&mut self.roms);
        *self = VmConsole::new();
        self.sources = keep_sources;
        self.roms = keep_roms;
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
        })
    }
}

/// True if a source path selects the uxlang dialect (`.ux`).
fn is_uxlang(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".ux")
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

        c.write_source("game.asm", clean);
        let built = c.assemble("game.asm").expect("assemble call");
        assert!(built.ok(), "assemble errors: {:?}", built.diagnostics);
        let outcome = c.load_rom("game.asm").expect("load");
        assert_eq!(outcome, RunOutcome::Completed, "reset fault: {:?}", c.vm.fault);

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
    fn snapshot_restore_roundtrip() {
        let mut c = VmConsole::new();
        c.write_source(
            "s.asm",
            "on-frame #10 DEO RET @on-frame player-x LOAD16 #01 ADD player-x STORE16 RET @player-x .res 2",
        );
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
        let built = assembler::assemble(c.get_source(path).unwrap());
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
        c.write_source("doc.asm", src);
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
