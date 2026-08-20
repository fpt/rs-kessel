//! A tiny fantasy-console VM for the "let the model write, run, observe, debug a
//! game" loop. Pure Rust, deterministic, and snapshotable.
//!
//! - [`isa`]  — the 34-opcode instruction set.
//! - [`vm`]   — the stack machine (memory, stacks, fetch/execute, frame runner).
//! - [`device`] — the Varvara-lite device layer (screen, gamepad, rng, storage, debug, console).
//! - [`assembler`] — a two-pass textual assembler → ROM + diagnostics.
//! - [`png`]  — dependency-free PNG + base64 for framebuffer output.
//! - [`tools`] — the `vm_*` [`crate::tool::VmTool`]s exposed to an agent.
//! - [`player`] — [`VmPlayer`], a standalone handle for human play.
//!
//! [`VmConsole`] holds all mutable state; the tools share one behind a
//! `Arc<Mutex<…>>`, and a host window drives its own console at 60 Hz for human
//! play.
//!
//! The crate is deliberately **host-free**: no I/O beyond the source/ROM files
//! under an explicit working directory, no audio backend, no GPU. Drawing is a
//! software rasterizer into an indexed framebuffer and sound is an event log —
//! both are plain data the host presents however it likes. That is what keeps
//! the machine deterministic and snapshotable, which is the whole point of the
//! write → run → observe → debug loop.

pub mod assembler;
pub mod audio;
pub mod device;
pub mod isa;
pub mod luax;
pub mod player;
pub mod playtest;
pub mod png;
pub mod tool;
pub mod tools;
pub mod vm;

pub use player::VmPlayer;
pub use tool::{ImageContent, ToolResult, VmTool, VmToolError};
pub use tools::{Shared, VmToolSet};

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use device::VideoMode;
use vm::{RunOutcome, Vm};

/// The whole console: the machine plus the authoring workspace (sources, built
/// ROMs, snapshots) and the bookkeeping the observation JSON needs.
pub struct VmConsole {
    pub vm: Vm,
    pub rom_loaded: bool,
    pub frame: u64,
    /// Framebuffer at the end of the previous frame, for change detection.
    prev_fb: Vec<u8>,
    /// Working directory, when disk-backed. With one set, **the filesystem is
    /// the source of truth**: sources are read from (and written to) disk, so
    /// whatever the backend's own file-editing tools or a human editor put in
    /// `game.lua` is what gets compiled. Without one (`VmPlayer`, tests) sources
    /// stay in the `sources` map below.
    root: Option<PathBuf>,
    /// Directories the model may *move* the working directory to, by naming an
    /// absolute path in a tool call. Empty (the default) fixes the root at
    /// whatever [`set_root`](Self::set_root) was given. See
    /// [`set_adoptable_roots`](Self::set_adoptable_roots).
    adoptable: Vec<PathBuf>,
    /// Source files the model has written, when no working directory is set.
    sources: HashMap<String, String>,
    /// Assembled ROMs, keyed by source path.
    roms: HashMap<String, Vec<u8>>,
    /// Control-layout metadata, keyed by source path (see [`luax::Controls`]).
    controls: HashMap<String, luax::Controls>,
    /// Sound banks per source path, and the one the loaded ROM declared.
    banks: HashMap<String, kessel_audio::SoundBank>,
    /// Declared signal names per source path (see [`luax::SignalDef`]).
    signals: HashMap<String, Vec<luax::SignalDef>>,
    /// Sound the **reset vector** asked for, waiting for a host to collect it.
    ///
    /// `init()` runs at load, outside any frame, and the device's log is
    /// cleared at the start of the next one — so a game that starts its music
    /// in `init()` would have that trigger quietly dropped by every host. It is
    /// parked here instead and belongs to whatever frame comes first.
    reset_sound: Vec<kessel_audio::AudioEvent>,
    /// Bumped whenever the timeline jumps — reset, restore, or a ROM load.
    ///
    /// A host holding a live synth watches this and turns a change into
    /// `AudioEvent::Panic`: after a rewind, the previous timeline's voices and
    /// reverb tail are playing over a game that never made them. The VM cannot
    /// emit the event itself without knowing about a synth, and it is not going
    /// to start knowing about one.
    audio_epoch: u64,
    /// The screen each assembled ROM asked for, keyed the same way.
    modes: HashMap<String, VideoMode>,
    /// The loaded ROM's screen. Drives `screen_dim` and the framebuffer size.
    active_mode: VideoMode,
    /// Control metadata of the currently loaded ROM (default until a load).
    active_controls: luax::Controls,
    active_bank: kessel_audio::SoundBank,
    /// Signal metadata of the currently loaded ROM, so an observation can say
    /// `score` where the device only knows `id 0`.
    active_signals: Vec<luax::SignalDef>,
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
            prev_fb: vec![0u8; VideoMode::default().pixels()],
            root: None,
            adoptable: Vec::new(),
            sources: HashMap::new(),
            roms: HashMap::new(),
            controls: HashMap::new(),
            banks: HashMap::new(),
            signals: HashMap::new(),
            reset_sound: Vec::new(),
            audio_epoch: 0,
            modes: HashMap::new(),
            active_mode: VideoMode::default(),
            active_controls: luax::Controls::default(),
            active_bank: kessel_audio::SoundBank::default(),
            active_signals: Vec::new(),
            paused: false,
            prev_pause_down: false,
            snapshots: HashMap::new(),
            snap_counter: 0,
        }
    }

    /// Point the console at a working directory (or clear it). Cached sources and
    /// ROMs from the previous workspace are dropped — they describe a different
    /// game. With a directory set, sources live on disk.
    pub fn set_root(&mut self, root: Option<PathBuf>) {
        self.root = root;
        self.sources.clear();
        self.roms.clear();
        self.controls.clear();
        self.banks.clear();
        self.signals.clear();
        self.modes.clear();
    }

    /// The active working directory, if disk-backed.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Allow the model to move the working directory by naming an **absolute**
    /// path in a tool call, as long as it lands under one of `dirs`.
    ///
    /// This is what makes a per-session workspace possible over stdio MCP, where
    /// the server is launched from a static config and the cwd is wherever the
    /// host app happened to start. The model names its directory by writing a
    /// file into it, so nothing has to be configured per project.
    ///
    /// It takes a list of allowed parent directories rather than a `bool`
    /// because the *model* is choosing now, and "the model picks the workspace"
    /// must not widen into "the model may write anywhere": a host approves
    /// `vm_write_source` once by name, not once per path. Empty (the default)
    /// keeps the root fixed and an absolute path the error it has always been,
    /// which is what [`VmPlayer`] and the in-memory tests want.
    pub fn set_adoptable_roots(&mut self, dirs: Vec<PathBuf>) {
        self.adoptable = dirs;
    }

    /// Normalize a model-supplied path into a workspace key, adopting its parent
    /// directory as the working directory when the path is absolute.
    ///
    /// A relative path is returned unchanged and still resolves against the
    /// current root, so every existing caller is untouched — including the
    /// in-memory ones, where adoption is off entirely.
    ///
    /// `creating` separates a write (make the directory if missing) from a read
    /// (the file must already exist). Without that split a typo'd `vm_assemble`
    /// path would quietly move the workspace and, because a move drops the
    /// caches, throw away every ROM built so far.
    pub fn adopt_path(&mut self, path: &str, creating: bool) -> Result<String, String> {
        if path.trim().is_empty() {
            return Err("path is empty".to_string());
        }
        let p = Path::new(path);
        if !p.is_absolute() || self.adoptable.is_empty() {
            return Ok(path.to_string());
        }

        let full = normalize_abs(p)?;
        let (dir, name) = match (full.parent(), full.file_name().and_then(|n| n.to_str())) {
            (Some(d), Some(n)) => (d, n.to_string()),
            _ => return Err(format!("path '{path}' does not name a file")),
        };
        if !within_any(dir, &self.adoptable) {
            let allowed: Vec<String> = self
                .adoptable
                .iter()
                .map(|d| d.display().to_string())
                .collect();
            return Err(format!(
                "'{}' is outside the directories this console may use ({}) — \
                 write inside one of those",
                dir.display(),
                allowed.join(", ")
            ));
        }
        if creating {
            std::fs::create_dir_all(dir).map_err(|e| format!("create '{}': {e}", dir.display()))?;
        } else if !full.is_file() {
            return Err(format!("no file at '{}'", full.display()));
        }

        // Only *move* on a real change: `set_root` drops the built ROMs, and the
        // assemble → load_rom pair names the same file twice.
        if !self.root.as_deref().is_some_and(|cur| same_dir(cur, dir)) {
            self.set_root(Some(dir.to_path_buf()));
        }
        Ok(name)
    }

    /// Write a source file. With a working directory set this writes through to
    /// disk, so the file the model authored is the same one the backend's file
    /// tools and `kessel run` see; otherwise it is kept in memory.
    pub fn write_source(&mut self, path: &str, source: &str) -> Result<(), String> {
        let key = self.adopt_path(path, true)?;
        let path = key.as_str();
        if let Some(root) = &self.root {
            let full = resolve_in_root(root, path)?;
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create '{}': {e}", parent.display()))?;
            }
            std::fs::write(&full, source)
                .map_err(|e| format!("write '{}': {e}", full.display()))?;
        } else {
            self.sources.insert(path.to_string(), source.to_string());
        }
        // Invalidate every previously built ROM, not just this path's: with
        // `#include`, editing `util.lua` changes what `game.lua` compiles to,
        // and a stale cached ROM would have `vm_load_rom` silently run the
        // previous build. Tracking include edges would be tidier and buys
        // nothing at this size.
        self.roms.clear();
        Ok(())
    }

    /// Read a source file — from the working directory when disk-backed, else
    /// from the in-memory workspace.
    pub fn get_source(&self, path: &str) -> Option<String> {
        match &self.root {
            Some(root) => {
                let full = resolve_in_root(root, path).ok()?;
                std::fs::read_to_string(full).ok()
            }
            None => self.sources.get(path).cloned(),
        }
    }

    /// Source files that *are* available, for "no source at 'x'" errors. Sorted;
    /// from the working directory when disk-backed, else the in-memory map.
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
    /// from the luax front-end to assembler, then assembled. When disk-backed the
    /// source is re-read on every call, so an edit made by *any* tool is compiled.
    ///
    /// `#include "…"` resolves through [`get_source`](Self::get_source), so an
    /// include obeys the same rule as the file that named it: the working
    /// directory when disk-backed (and, through `resolve_in_root`, unable to
    /// escape it), the in-memory workspace otherwise.
    pub fn assemble(&mut self, path: &str) -> Result<assembler::Assembled, String> {
        let key = self.adopt_path(path, false)?;
        let path = key.as_str();
        let src = &self.get_source(path).ok_or_else(|| {
            let available = self.list_sources();
            let known = if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            };
            match self.root() {
                Some(root) => format!(
                    "no source at '{path}' in {} — available: {known}",
                    root.display()
                ),
                None => format!("no source written at '{path}' — available: {known}"),
            }
        })?;

        // luax (Lua-ish) dialect: compile to assembler first. Compiler
        // diagnostics are returned in an otherwise-empty `Assembled`. The
        // control-layout metadata rides along and is cached for `load_rom`.
        let (built, controls, mode, bank, signals) = if is_lua(path) {
            let compiled = luax::compile_with(src, &mut |inc: &str| self.get_source(inc));
            if !compiled.ok() {
                return Ok(assembler::Assembled {
                    rom: Vec::new(),
                    diagnostics: compiled.diagnostics,
                    labels: Default::default(),
                });
            }
            (
                assembler::assemble(&compiled.asm),
                compiled.controls,
                compiled.mode,
                compiled.bank,
                compiled.signals,
            )
        } else {
            // Raw assembly has no `controls`, `screen`, or `instrument` block;
            // it gets the default layout on the original console, and silence.
            (
                assembler::assemble(src),
                luax::Controls::default(),
                VideoMode::default(),
                kessel_audio::SoundBank::default(),
                Vec::new(),
            )
        };

        if built.ok() {
            self.roms.insert(path.to_string(), built.rom.clone());
            self.controls.insert(path.to_string(), controls);
            self.banks.insert(path.to_string(), bank);
            self.signals.insert(path.to_string(), signals);
            self.modes.insert(path.to_string(), mode);
        }
        Ok(built)
    }

    /// Load a built ROM and run its reset vector.
    pub fn load_rom(&mut self, path: &str) -> Result<RunOutcome, String> {
        let key = self.adopt_path(path, false)?;
        let path = key.as_str();
        let rom = self
            .roms
            .get(path)
            .ok_or_else(|| format!("no assembled ROM for '{path}' — call vm_assemble first"))?
            .clone();
        self.active_mode = self.modes.get(path).copied().unwrap_or_default();
        let outcome = self.vm.load_rom(&rom, self.active_mode);
        self.rom_loaded = true;
        self.frame = 0;
        self.prev_fb = self.vm.devices.framebuffer.clone();
        self.active_controls = self.controls.get(path).cloned().unwrap_or_default();
        self.active_bank = self.banks.get(path).cloned().unwrap_or_default();
        self.active_signals = self.signals.get(path).cloned().unwrap_or_default();
        self.reset_sound = self.vm.devices.sound.clone();
        self.audio_epoch += 1;
        self.paused = false;
        self.prev_pause_down = false;
        Ok(outcome)
    }

    /// The control-layout metadata of the currently loaded ROM.
    pub fn controls(&self) -> &luax::Controls {
        &self.active_controls
    }

    /// The loaded ROM's instruments and sound effects.
    ///
    /// The VM itself never reads this — it emits `sfx(id)` and stays silent.
    /// It is here so a host can hand the bank to `kessel-audio` at load time,
    /// the same way it reads [`controls`](Self::controls) for its button
    /// layout.
    pub fn sound_bank(&self) -> &kessel_audio::SoundBank {
        &self.active_bank
    }

    /// Take the sound the reset vector asked for, if any.
    ///
    /// A host calls this once after loading and treats the result as belonging
    /// to the first frame. See [`reset_sound`](Self::reset_sound) — without it,
    /// `music()` in `init()` is silent everywhere.
    pub fn take_reset_sound(&mut self) -> Vec<kessel_audio::AudioEvent> {
        std::mem::take(&mut self.reset_sound)
    }

    /// Counter that changes whenever the audio timeline is discontinuous.
    ///
    /// A host with a live synth should compare this each frame and, when it
    /// differs, submit [`kessel_audio::AudioEvent::Panic`]. See the field's
    /// documentation for why the VM signals rather than emits.
    pub fn audio_epoch(&self) -> u64 {
        self.audio_epoch
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
    pub fn play_tick(&mut self, input: impl Into<device::Input>) {
        let input = input.into();
        let pause_bit = self.active_controls.pause_bit();
        let down = pause_bit != 0 && input.buttons & pause_bit != 0;
        if down && !self.prev_pause_down {
            self.paused = !self.paused;
        }
        self.prev_pause_down = down;
        if !self.paused {
            self.run_frame(input.with_buttons(input.buttons & !pause_bit));
        }
    }

    /// Advance one frame with `input` applied; returns the observation record.
    ///
    /// Takes anything that converts into an [`Input`](device::Input), so a
    /// digital game is still `run_frame(BTN_A)` and only a game that reads the
    /// stick or the screen has to say more.
    pub fn run_frame(&mut self, input: impl Into<device::Input>) -> Observation {
        let input = input.into();
        let outcome = self.vm.run_frame(input, vm::cap());
        self.frame += 1;
        let obs = self.observe(input, outcome);
        self.prev_fb = self.vm.devices.framebuffer.clone();
        obs
    }

    fn observe(&self, input: device::Input, outcome: RunOutcome) -> Observation {
        let fb = &self.vm.devices.framebuffer;
        let bbox = changed_bbox(&self.prev_fb, fb, self.vm.devices.dim());
        let fault = match outcome {
            RunOutcome::CapExceeded => Some(format!("frame cycle cap ({}) exceeded", vm::cap())),
            _ => self.vm.fault.clone(),
        };
        Observation {
            frame: self.frame,
            cycles: self.vm.cycle,
            buttons: button_names(input.buttons),
            // Recorded only when something analog actually moved. An agent
            // reading a hundred frames of a d-pad game should not have to skim
            // past a hundred `"stick": [0,0]` lines to find what changed.
            analog: (!input.analog_is_at_rest()).then_some(input),
            framebuffer_hash: fnv1a(fb),
            changed_pixels_bbox: bbox,
            console: String::from_utf8_lossy(&self.vm.devices.console).into_owned(),
            fault,
            pc: self.vm.pc,
            data_stack: self.vm.data_stack(),
            return_stack_depth: self.vm.return_stack_depth(),
            entities: self.vm.devices.entities.clone(),
            // Resolved here, because this is the only layer that holds both the
            // device's ids and the ROM's names. A signal the ROM never declared
            // is dropped rather than reported as a number: the device cannot
            // produce one from luax, so it means hand-written assembly wrote a
            // port it did not declare, and inventing `signal 7` for it would put
            // a name in the report that is in no source file.
            signals: self
                .vm
                .devices
                .signals
                .iter()
                .filter_map(|s| {
                    let def = self.active_signals.get(s.id as usize)?;
                    let value = if def.signed {
                        s.value as i16 as i32
                    } else {
                        s.value as i32
                    };
                    Some((def.name.clone(), value, def.signed))
                })
                .collect(),
            sound: self.vm.devices.sound.clone(),
            halted: self.vm.halted,
        }
    }

    /// The current framebuffer expanded to RGBA (for PNG / host window).
    pub fn framebuffer_rgba(&self) -> Vec<u8> {
        self.vm.devices.framebuffer_rgba()
    }

    /// The same pixels written into a caller-owned buffer, for hosts that blit
    /// every frame. False if `dst` is too small.
    pub fn framebuffer_rgba_into(&self, dst: &mut [u8]) -> bool {
        self.vm.devices.framebuffer_rgba_into(dst)
    }

    /// Screen edge length in pixels (square). Mirrors [`VmPlayer::screen_dim`],
    /// for hosts that drive a console directly.
    pub fn screen_dim(&self) -> u32 {
        self.vm.devices.dim() as u32
    }

    /// The loaded ROM's screen mode.
    pub fn video_mode(&self) -> VideoMode {
        self.active_mode
    }

    /// Encode the current framebuffer as a base64 PNG.
    pub fn framebuffer_png_base64(&self) -> String {
        let rgba = self.framebuffer_rgba();
        let dim = self.screen_dim();
        let png = png::encode_rgba(dim, dim, &rgba);
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
        self.audio_epoch += 1;
        Ok(())
    }

    pub fn reset(&mut self) {
        let keep_root = self.root.take();
        let keep_sources = std::mem::take(&mut self.sources);
        let keep_roms = std::mem::take(&mut self.roms);
        let keep_controls = std::mem::take(&mut self.controls);
        // Banks are cached per source exactly like controls and modes: a reset
        // that kept the ROM but forgot its instruments would reload a game
        // that had gone silent.
        let keep_banks = std::mem::take(&mut self.banks);
        let keep_signals = std::mem::take(&mut self.signals);
        let keep_modes = std::mem::take(&mut self.modes);
        let epoch = self.audio_epoch;
        *self = VmConsole::new();
        self.root = keep_root;
        self.sources = keep_sources;
        self.roms = keep_roms;
        self.controls = keep_controls;
        self.banks = keep_banks;
        self.signals = keep_signals;
        self.modes = keep_modes;
        self.audio_epoch = epoch + 1;
    }
}

/// One frame's observation, per the harness spec. Serialized to JSON by the
/// `vm_run_frame` tool.
#[derive(Debug, Clone)]
pub struct Observation {
    pub frame: u64,
    pub cycles: u64,
    pub buttons: Vec<String>,
    /// The frame's stick and touches, present only when either was in play.
    pub analog: Option<device::Input>,
    pub framebuffer_hash: String,
    pub changed_pixels_bbox: Option<[u16; 4]>,
    pub console: String,
    pub fault: Option<String>,
    pub pc: u16,
    pub data_stack: Vec<u16>,
    pub return_stack_depth: usize,
    pub entities: Vec<device::Entity>,
    /// Named scalars the game reported, already resolved against the ROM's
    /// declarations. `(name, value, signed)` — the name is carried rather than
    /// the id because an id is not readable and the console is the only party
    /// that holds the mapping.
    pub signals: Vec<(String, i32, bool)>,
    pub sound: Vec<kessel_audio::AudioEvent>,
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
            "signals": self.signals.iter().map(|(n, v, _)| serde_json::json!({
                "name": n, "value": v,
            })).collect::<Vec<_>>(),
            "sound": self.sound.iter().map(audio::event_json).collect::<Vec<_>>(),
            "stick": self.analog.map(|i| serde_json::json!([i.stick_x, i.stick_y])),
            "touches": self.analog.map(|i| i.touches.iter()
                .enumerate()
                .filter(|(_, t)| t.down)
                .map(|(slot, t)| serde_json::json!({"slot": slot, "x": t.x, "y": t.y}))
                .collect::<Vec<_>>()),
        })
    }
}

/// True if a source path selects the luax (Lua-ish) dialect (`.lua`).
fn is_lua(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".lua")
}

/// Resolve a model-supplied source path against the working directory, refusing
/// anything that would escape it (absolute paths, `..`). Keeps `vm_write_source`
/// from writing arbitrary files outside the game's directory.
pub(crate) fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.trim().is_empty() {
        return Err("path is empty".to_string());
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(format!(
            "path '{rel}' must be relative to the working directory"
        ));
    }
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(format!(
                    "path '{rel}' must stay inside the working directory"
                ))
            }
        }
    }
    let full = root.join(p);

    // The component checks above cannot see a **symlink**, and both `fs::write`
    // and `read_to_string` follow one — so a link planted in the workspace would
    // turn a confined write into an arbitrary one, which is exactly the property
    // this function exists to provide. Resolve what already exists and require
    // the result to stay put.
    //
    // Both sides go through `canonical_prefix`, so a workspace reached *through* a
    // link (`/tmp` on macOS, a symlinked home) still matches itself, and a file
    // that does not exist yet is checked by its deepest existing ancestor.
    //
    // This is a check-then-use, and deliberately: closing the race would need the
    // path opened once with `O_NOFOLLOW` per component, and anyone who can plant a
    // link inside the workspace between these two lines can already write the file
    // directly.
    if !canonical_prefix(&full).starts_with(canonical_prefix(root)) {
        return Err(format!(
            "path '{rel}' must stay inside the working directory \
             (it leads outside through a link)"
        ));
    }
    Ok(full)
}

/// Lexically normalize an absolute path, dropping `.` and refusing `..`.
///
/// `..` is refused rather than resolved because [`within_any`] is what keeps an
/// adopted root inside the directories the host allowed, and a component that
/// climbs back out *after* that check would defeat it.
fn normalize_abs(p: &Path) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("path '{}' must not contain '..'", p.display()))
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

/// Canonicalize as much of `dir` as already exists, keeping the rest lexically.
///
/// A directory the model is about to create has no canonical form yet, and a
/// plain `canonicalize().unwrap_or(lexical)` compares those two kinds of path
/// against each other: on macOS `/tmp/newgame` would then fail a bounds check
/// that the very same, existing `/tmp/game` passes, because only one of them
/// resolves to `/private/tmp`. Refusing `..` upstream is what makes keeping the
/// unresolved tail sound.
fn canonical_prefix(dir: &Path) -> PathBuf {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = dir;
    loop {
        if let Ok(resolved) = cur.canonicalize() {
            let mut out = resolved;
            out.extend(tail.iter().rev());
            return out;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                cur = parent;
            }
            // Nothing along the path exists — compare it as written.
            _ => return dir.to_path_buf(),
        }
    }
}

/// True if `dir` is inside one of `allowed`.
fn within_any(dir: &Path, allowed: &[PathBuf]) -> bool {
    let target = canonical_prefix(dir);
    allowed
        .iter()
        .any(|a| target.starts_with(canonical_prefix(a)))
}

/// True if two paths name the same directory, resolving symlinks where they
/// exist.
///
/// Compared this way rather than lexically because the starting root may have
/// arrived relative or through a symlink (`$KESSEL_ROOT=.`, a linked home): a
/// spurious mismatch here would re-adopt on every call, and re-adopting clears
/// the ROM cache between `vm_assemble` and `vm_load_rom`.
fn same_dir(a: &Path, b: &Path) -> bool {
    canonical_prefix(a) == canonical_prefix(b)
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
fn changed_bbox(prev: &[u8], cur: &[u8], dim: usize) -> Option<[u16; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (u16::MAX, u16::MAX, 0u16, 0u16);
    let mut any = false;
    for (i, (&a, &b)) in prev.iter().zip(cur.iter()).enumerate() {
        if a != b {
            any = true;
            let x = (i % dim) as u16;
            let y = (i / dim) as u16;
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

    /// A luax source written to the working dir by *any* means is what
    /// `assemble` compiles — the file write → vm run path.
    #[test]
    fn disk_backed_source_is_read_from_the_working_dir() {
        let dir = tempfile::tempdir().unwrap();
        let src = "local x = 0\nfunction update() x = x + 1 end\nfunction draw() cls(0) end\n";

        // Simulate the backend's own file tools writing game.lua to disk —
        // the console never saw a write_source call for it.
        std::fs::write(dir.path().join("game.lua"), src).unwrap();

        let mut c = VmConsole::new();
        c.set_root(Some(dir.path().to_path_buf()));

        assert_eq!(c.get_source("game.lua").as_deref(), Some(src));
        assert!(
            c.assemble("game.lua").unwrap().ok(),
            "the on-disk source should assemble"
        );
    }

    /// `write_source` writes through to disk when rooted, and a later edit on
    /// disk is picked up on the next assemble (fresh read every call).
    #[test]
    fn write_source_writes_through_to_disk_and_rereads() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = VmConsole::new();
        c.set_root(Some(dir.path().to_path_buf()));

        c.write_source("game.lua", "function draw() cls(0) end\n")
            .unwrap();
        let on_disk = std::fs::read_to_string(dir.path().join("game.lua")).unwrap();
        assert!(
            on_disk.contains("cls(0)"),
            "wrote through to the actual file"
        );

        // External edit; assemble must compile the new bytes, not a cached copy.
        std::fs::write(dir.path().join("game.lua"), "function draw() cls(1) end\n").unwrap();
        assert_eq!(
            c.get_source("game.lua").as_deref(),
            Some("function draw() cls(1) end\n")
        );
    }

    /// `#include` reads through the same working directory as the file that
    /// named it — the whole point of resolving it here rather than in luax.
    #[test]
    fn an_include_resolves_against_the_working_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("util.lua"),
            "record Point { x, y }\nfunction sum(p: Point) return p.x + p.y end\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("game.lua"),
            "#include \"util.lua\"\nlocal p: Point\nfunction draw() pset(sum(p), 0, 7) end\n",
        )
        .unwrap();

        let mut c = VmConsole::new();
        c.set_root(Some(dir.path().to_path_buf()));
        let built = c.assemble("game.lua").unwrap();
        assert!(built.ok(), "{:?}", built.diagnostics);
    }

    /// An adopted directory is where that game's `#include`s live, so following
    /// an absolute path has to move the include search with it. Adoption runs
    /// before the source is read for exactly this reason.
    #[test]
    fn an_include_follows_the_adopted_working_dir() {
        let base = tempfile::tempdir().unwrap();
        let proj = base.path().join("with-lib");
        std::fs::create_dir(&proj).unwrap();
        std::fs::write(
            proj.join("util.lua"),
            "record Point { x, y }\nfunction sum(p: Point) return p.x + p.y end\n",
        )
        .unwrap();
        let game = proj.join("game.lua");
        std::fs::write(
            &game,
            "#include \"util.lua\"\nlocal p: Point\nfunction draw() pset(sum(p), 0, 7) end\n",
        )
        .unwrap();

        // Rooted at the parent, where neither file is: only adoption makes the
        // include resolvable.
        let mut c = adopting(base.path());
        let built = c.assemble(game.to_str().unwrap()).unwrap();
        assert!(built.ok(), "{:?}", built.diagnostics);
    }

    /// An include cannot reach outside the working directory, because it goes
    /// through the same `resolve_in_root` as everything else.
    #[test]
    fn an_include_cannot_escape_the_working_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("outside.lua"), "record Point { x, y }\n").unwrap();
        let inner = dir.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(
            inner.join("game.lua"),
            "#include \"../outside.lua\"\nfunction draw() cls(0) end\n",
        )
        .unwrap();

        let mut c = VmConsole::new();
        c.set_root(Some(inner));
        let built = c.assemble("game.lua").unwrap();
        assert!(!built.ok());
        assert!(
            built.diagnostics[0].message.contains("cannot find include"),
            "{:?}",
            built.diagnostics
        );
    }

    /// Editing an included file must invalidate the *including* file's cached
    /// ROM — otherwise `load_rom` silently runs the previous build.
    #[test]
    fn editing_an_include_invalidates_the_cached_rom() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = VmConsole::new();
        c.set_root(Some(dir.path().to_path_buf()));
        c.write_source("util.lua", "function tint() return 3 end\n")
            .unwrap();
        c.write_source(
            "game.lua",
            "#include \"util.lua\"\nfunction draw() cls(tint()) end\n",
        )
        .unwrap();
        assert!(c.assemble("game.lua").unwrap().ok());

        // The include no longer compiles; game.lua's cached ROM must not
        // survive to be loaded.
        c.write_source("util.lua", "function tint() return nope end\n")
            .unwrap();
        assert!(
            c.load_rom("game.lua").is_err(),
            "a stale ROM was loaded after its include changed"
        );
        assert!(!c.assemble("game.lua").unwrap().ok());
    }

    /// Model-supplied paths can't escape the working directory.
    #[test]
    fn rooted_write_refuses_paths_outside_the_working_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = VmConsole::new();
        c.set_root(Some(dir.path().to_path_buf()));

        assert!(c.write_source("../escape.lua", "x").is_err());
        assert!(c.write_source("/etc/passwd", "x").is_err());
        assert!(!dir.path().join("../escape.lua").exists());
    }

    /// A relative path may not follow a **symlink** out of the working directory.
    /// The component checks are lexical and cannot see one, while `fs::write` and
    /// `read_to_string` both follow it — so a link planted in the workspace would
    /// otherwise defeat the confinement for writes *and* reads.
    #[cfg(unix)]
    #[test]
    fn a_symlink_may_not_lead_out_of_the_working_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        let root = tmp.path().join("workspace");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let secret = outside.join("secret.lua");
        std::fs::write(&secret, "-- private\n").unwrap();

        // One link to the directory, one straight to the file.
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        std::os::unix::fs::symlink(&secret, root.join("alias.lua")).unwrap();

        let mut c = VmConsole::new();
        c.set_root(Some(root));

        for path in ["link/secret.lua", "alias.lua", "link/new.lua"] {
            assert!(
                c.write_source(path, "-- overwritten\n").is_err(),
                "wrote through '{path}'"
            );
            assert!(c.get_source(path).is_none(), "read through '{path}'");
        }
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "-- private\n");
        assert!(!outside.join("new.lua").exists());
    }

    /// A game, small enough to assemble and load in the adoption tests.
    const TINY: &str = "function draw() cls(0) end\n";

    /// A console that may follow the model into `dir` — what `kessel mcp` builds.
    fn adopting(dir: &Path) -> VmConsole {
        let mut c = VmConsole::new();
        c.set_root(Some(dir.to_path_buf()));
        c.set_adoptable_roots(vec![dir.to_path_buf()]);
        c
    }

    /// The point of adoption: an absolute path names the workspace, so a server
    /// launched from a static config still gets a per-session directory.
    #[test]
    fn an_absolute_path_moves_the_workspace_to_its_directory() {
        let base = tempfile::tempdir().unwrap();
        let mut c = adopting(base.path());

        let proj = base.path().join("my-game");
        let file = proj.join("game.lua");
        c.write_source(file.to_str().unwrap(), TINY).unwrap();

        assert!(same_dir(c.root().unwrap(), &proj), "root should have moved");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), TINY);
        // ...and the bare name now resolves there, so the rest of the session
        // does not have to repeat the path.
        assert_eq!(c.get_source("game.lua").as_deref(), Some(TINY));
    }

    /// The trap this feature could easily walk into: adopting re-runs `set_root`,
    /// which drops built ROMs — and `assemble` then `load_rom` names one file
    /// twice. Naming the same directory again must not move anything.
    #[test]
    fn naming_the_same_directory_twice_keeps_the_built_rom() {
        let base = tempfile::tempdir().unwrap();
        let mut c = adopting(base.path());
        let file = base.path().join("proj").join("game.lua");
        let abs = file.to_str().unwrap().to_string();

        c.write_source(&abs, TINY).unwrap();
        assert!(c.assemble(&abs).unwrap().ok());
        c.load_rom(&abs)
            .expect("the ROM built one call ago is gone");
        assert!(c.rom_loaded);
    }

    /// A typo'd read must not throw the workspace away: the fix is worth a test
    /// because the failure is invisible — the call errors either way, and only
    /// the *next* one reveals that every built ROM went with it.
    #[test]
    fn a_missing_absolute_path_does_not_move_the_workspace() {
        let base = tempfile::tempdir().unwrap();
        let mut c = adopting(base.path());
        let good = base.path().join("proj").join("game.lua");
        c.write_source(good.to_str().unwrap(), TINY).unwrap();
        assert!(c.assemble("game.lua").unwrap().ok());

        let typo = base.path().join("prj").join("game.lua");
        assert!(c.assemble(typo.to_str().unwrap()).is_err());

        assert!(same_dir(c.root().unwrap(), good.parent().unwrap()));
        c.load_rom("game.lua")
            .expect("a failed read discarded the built ROM");
    }

    /// Moving to a different project *does* drop the previous one's ROMs — they
    /// describe a different game.
    #[test]
    fn moving_to_another_directory_drops_the_previous_rom() {
        let base = tempfile::tempdir().unwrap();
        let mut c = adopting(base.path());

        let first = base.path().join("one").join("game.lua");
        c.write_source(first.to_str().unwrap(), TINY).unwrap();
        assert!(c.assemble(first.to_str().unwrap()).unwrap().ok());

        let second = base.path().join("two").join("other.lua");
        c.write_source(second.to_str().unwrap(), TINY).unwrap();

        assert!(same_dir(c.root().unwrap(), second.parent().unwrap()));
        assert!(
            c.load_rom("game.lua").is_err(),
            "a ROM from the previous workspace is still loadable"
        );
    }

    /// Adoption is opt-in. Without it an absolute path stays the error it has
    /// always been — which is what keeps `VmPlayer` and the FFI hosts, where the
    /// path is whatever file the user opened, from silently becoming disk-backed.
    #[test]
    fn adoption_is_off_until_a_host_allows_it() {
        let base = tempfile::tempdir().unwrap();
        let mut c = VmConsole::new();
        c.set_root(Some(base.path().to_path_buf()));

        let abs = base.path().join("proj").join("game.lua");
        assert!(c.write_source(abs.to_str().unwrap(), TINY).is_err());
        assert!(same_dir(c.root().unwrap(), base.path()));

        // In-memory (no root at all): an absolute path is just a key, as before.
        let mut mem = VmConsole::new();
        mem.write_source(abs.to_str().unwrap(), TINY).unwrap();
        assert!(mem.root().is_none());
        assert!(!abs.exists(), "an in-memory write must not touch disk");
    }

    /// The model picks the directory now, so the host's list is the whole
    /// boundary: a tool a host approves once by name must not become a licence to
    /// write anywhere on the disk.
    #[test]
    fn adopting_outside_the_allowed_directories_is_refused() {
        let allowed = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let mut c = adopting(allowed.path());

        let outside = elsewhere.path().join("game.lua");
        let err = c
            .write_source(outside.to_str().unwrap(), TINY)
            .expect_err("wrote outside the allowed directories");
        assert!(err.contains("outside"), "{err}");
        assert!(!outside.exists());
        assert!(same_dir(c.root().unwrap(), allowed.path()));
    }

    /// `..` is refused rather than resolved: a path that climbs out after the
    /// prefix check would make the check decorative.
    #[test]
    fn an_absolute_path_may_not_climb_out_with_dotdot() {
        let allowed = tempfile::tempdir().unwrap();
        let mut c = adopting(allowed.path());

        let sneaky = format!("{}/../escaped/game.lua", allowed.path().display());
        let err = c
            .write_source(&sneaky, TINY)
            .expect_err("'..' escaped the allowed directories");
        assert!(err.contains(".."), "{err}");
        assert!(same_dir(c.root().unwrap(), allowed.path()));
    }

    /// A path whose prefix does not exist yet still has to compare against a
    /// canonicalized bound — the `/tmp` → `/private/tmp` case on macOS, which
    /// would otherwise refuse every new project directory.
    #[test]
    fn a_directory_that_does_not_exist_yet_is_still_inside_its_bound() {
        let base = tempfile::tempdir().unwrap();
        let nested = base.path().join("a").join("b").join("c");
        assert!(!nested.exists());
        assert!(within_any(&nested, &[base.path().to_path_buf()]));
        assert!(!within_any(
            Path::new("/somewhere/else"),
            &[base.path().to_path_buf()]
        ));
    }

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
