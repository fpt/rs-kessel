//! The `vm_*` tools the agent uses to drive the fantasy console: the full
//! write → assemble → load → run → observe → debug loop.
//!
//! All tools share one [`VmConsole`] behind an `Arc<Mutex<…>>`, so the set must
//! be built together — see [`vm_tool_handlers_rooted`]. Hosts adapt the returned
//! [`VmTool`]s to their own tool framework; `kessel mcp` serves them over MCP.

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::tool::{ImageContent, ToolResult, VmTool, VmToolError};

use super::{buttons_from_names, VmConsole};

/// A console shared between everything that drives it. Exported because a host
/// that wants an attached player needs to hold one alongside the tools.
pub type Shared = Arc<Mutex<VmConsole>>;

/// Construct one shared [`VmConsole`] and build every `vm_*` tool over it.
///
/// The tools share the console, so their order does not matter but the set must
/// stay together (they drive one VM).
///
/// This builds an **in-memory** workspace, which is really only right for tests —
/// prefer [`vm_tool_handlers_rooted`] with a real directory so the model's own
/// file-editing tools and the VM agree on what `game.lua` contains.
pub fn vm_tool_handlers() -> Vec<Box<dyn VmTool>> {
    vm_tool_handlers_rooted(None)
}

/// Build every `vm_*` tool over a console rooted at `root`. With a working
/// directory set, `vm_write_source`/`vm_assemble` read and write actual files
/// there — the same files the host agent's own file-editing tools touch — so a
/// game the model wrote to disk is what the VM compiles. `None` keeps the
/// in-memory workspace (tests).
pub fn vm_tool_handlers_rooted(root: Option<std::path::PathBuf>) -> Vec<Box<dyn VmTool>> {
    let mut console = VmConsole::new();
    console.set_root(root);
    vm_tool_handlers_on(Arc::new(Mutex::new(console)))
}

/// Build every `vm_*` tool over a console the **caller** owns a handle to.
///
/// Use this when something other than the tools also drives the machine — an
/// attached play window, say. Everyone sharing the `Arc` shares one timeline:
/// the mutex serializes access, but the interleaving is real, so a `vm_restore`
/// from one holder rewinds the game the other is watching. That is the intended
/// semantics for an attached player, and the reason it is a separate,
/// explicitly-named constructor rather than the default.
pub fn vm_tool_handlers_on(console: Shared) -> Vec<Box<dyn VmTool>> {
    vec![
        Box::new(WriteSource(console.clone())),
        Box::new(Assemble(console.clone())),
        Box::new(LoadRom(console.clone())),
        Box::new(RunCycles(console.clone())),
        Box::new(RunFrame(console.clone())),
        Box::new(RunFrames(console.clone())),
        Box::new(InspectMemory(console.clone())),
        Box::new(InspectStacks(console.clone())),
        Box::new(GetFramebuffer(console.clone())),
        Box::new(Snapshot(console.clone())),
        Box::new(Restore(console.clone())),
        Box::new(Reset(console)),
    ]
}

/// The whole `vm_*` set behind a name lookup — what a host that dispatches by
/// tool name (an MCP server, a test) wants instead of a bare `Vec`.
pub struct VmToolSet {
    tools: Vec<Box<dyn VmTool>>,
    console: Shared,
}

impl VmToolSet {
    /// Build the set over a console rooted at `root`; see
    /// [`vm_tool_handlers_rooted`] for what a root buys you.
    pub fn new(root: Option<std::path::PathBuf>) -> Self {
        let mut console = VmConsole::new();
        console.set_root(root);
        Self::with_console(Arc::new(Mutex::new(console)))
    }

    /// Build the set over a console the caller keeps a handle to — see
    /// [`vm_tool_handlers_on`] for what sharing it means.
    pub fn with_console(console: Shared) -> Self {
        Self {
            tools: vm_tool_handlers_on(console.clone()),
            console,
        }
    }

    /// The console these tools drive. Locking it drives the *same* machine the
    /// tools do; hold the lock for as short a time as possible, since a tool
    /// call in flight blocks on it.
    pub fn console(&self) -> &Shared {
        &self.console
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn VmTool> {
        self.tools.iter().map(|t| t.as_ref())
    }

    pub fn get(&self, name: &str) -> Option<&dyn VmTool> {
        self.iter().find(|t| t.name() == name)
    }

    /// Dispatch by name. An unknown name is a [`VmToolError`], not a panic —
    /// hosts forward arbitrary strings from the model.
    pub fn call(&self, name: &str, args: Value) -> Result<ToolResult, VmToolError> {
        self.get(name)
            .ok_or_else(|| VmToolError(format!("unknown tool '{name}'")))?
            .call(args)
    }
}

// ---- helpers ----

fn str_arg(args: &Value, key: &str) -> Result<String, VmToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| VmToolError(format!("missing string argument '{key}'")))
}

fn u64_arg(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(|v| v.as_u64()).unwrap_or(default)
}

// ---- vm_write_source ----

struct WriteSource(Shared);
impl VmTool for WriteSource {
    fn name(&self) -> &str {
        "vm_write_source"
    }
    fn description(&self) -> &str {
        // A canonical luax snippet is embedded so the model writes the real
        // dialect on the first try instead of falling back to raw PICO-8 (which
        // only fails at assemble time). Covers the three most common priors that
        // DON'T port: sprites are `sprite NAME { rows }` declarations (not table
        // literals), entry points are `update`/`draw` (NOT `_update`/`_draw`),
        // and `cls` requires a colour argument.
        r#"Write source for the fantasy-console VM to a named file. When a working directory is set the file is written ON DISK — the same file your own file-editing tools, a human editor, and `kessel play` see — so for a small change to an existing game, edit that file directly with your file tools and just call vm_assemble. Use vm_write_source for a first draft or a full rewrite. A '.asm' path is stack assembly; a '.lua' path is a small statically-typed Lua-ish dialect (NOT full PICO-8/Lua: no tables/metatables/closures/recursion). Overwrites any previous source at that path and invalidates its built ROM.

luax essentials (a '.lua' file):
- Entry points (vector-driven, no main loop): `function init()` runs once; `function update()` then `function draw()` run each frame. Names are bare — NOT `_update`/`_draw`.
- State: top-level `local x = 60` is a persistent global. `record Name { a, b: byte }` (fields default to `word`); `local es: array(8, Name)`.
- Sprites are DECLARATIONS, not table literals: `sprite hero { <8 rows of 8 chars, '.'=transparent else palette nibble 0-9a-f> }`. `hero` is then a tile id; draw with `spr(hero, x, y, flags)`.
- Builtins: `cls(c)` (colour REQUIRED), `pset(x,y,c)`, `spr(id,x,y,flags)`, `sprn(id,x,y,w,h,flags)` (w×h block of contiguous tiles, e.g. 16×16 = 2,2), `btn(LEFT|RIGHT|UP|DOWN|A|B)` (held), `btnp`/`btnr` (pressed/released THIS frame — use for jumps/menus), `frame_count()` (frames since start), `len(arr)` (array length), `clear(rec_or_arr)` (zero a record/array in place, e.g. reset a bullet pool), `text("LITERAL",x,y,color)` / `number(n,x,y,color)` (on-screen font: scores/titles/GAME OVER; `text` needs a string LITERAL), `sfx(id)` / `music(id)` / `music_stop()` (sound triggers — recorded, silent for now), `entity(x,y,tag)` (report for observation), `rnd(n)`, `map/mget/mset/fset/solid` (tilemap).
- Collision (need a `tilemap`): `map_rect_overlap(x,y,w,h,flag)` (rect hits a flagged tile?); `collide_x(x,y,w,h,dx,flag)`/`collide_y(...,dy,flag)` MOVE a box by dx/dy and return the new coord snapped out of solid tiles — resolve X then Y each frame; `touching_left|right|floor|ceiling(x,y,w,h,flag)` (is a flagged tile against that edge?). Prefer these over hand-writing collision.

Canonical example:
  sprite hero {
    ..7777..
    .777777.
    77777777
    77.77.77
    77777777
    .777777.
    ..7777..
    .77..77.
  }
  local x = 60
  local y = 60
  function update()
    if btn(LEFT)  then x = x - 1 end
    if btn(RIGHT) then x = x + 1 end
  end
  function draw()
    cls(0)
    spr(hero, x, y, 0)
    entity(x, y, 1)
  end
"#
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Workspace file name, e.g. 'game.lua' or 'game.asm'"},
                "source": {"type": "string", "description": "Source text: luax (.lua) or stack assembly (.asm)"}
            },
            "required": ["path", "source"]
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let path = str_arg(&args, "path")?;
        let source = str_arg(&args, "source")?;
        let bytes = source.len();
        let mut console = self.0.lock();
        if let Err(e) = console.write_source(&path, &source) {
            return Ok(ToolResult::text(e));
        }
        // Report the on-disk location when disk-backed, so the model knows the
        // exact file its own file tools can edit next time.
        let where_ = match console.root() {
            Some(root) => root.join(&path).display().to_string(),
            None => path.clone(),
        };
        Ok(ToolResult::text(format!(
            "wrote {bytes} bytes to '{where_}'"
        )))
    }
}

// ---- vm_assemble ----

struct Assemble(Shared);
impl VmTool for Assemble {
    fn name(&self) -> &str {
        "vm_assemble"
    }
    fn description(&self) -> &str {
        "Assemble a source file into a ROM, reading it fresh each time — from the \
         working directory when one is set, so edits made with any editing tool \
         (yours or vm_write_source) are picked up. A '.lua' file is compiled from \
         the Lua-ish dialect to assembly first. Returns diagnostics with line \
         numbers on error, or the byte size and labels on success."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string"} },
            "required": ["path"]
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let path = str_arg(&args, "path")?;
        let built = self.0.lock().assemble(&path).map_err(VmToolError)?;
        if built.ok() {
            let labels: Vec<String> = built
                .labels
                .iter()
                .map(|(n, a)| format!("{n}=0x{a:04X}"))
                .collect();
            Ok(ToolResult::text(format!(
                "assembled '{path}': {} bytes ok.\nlabels: {}",
                built.rom.len(),
                if labels.is_empty() {
                    "(none)".into()
                } else {
                    labels.join(", ")
                }
            )))
        } else {
            let mut msg = format!(
                "assemble failed with {} error(s):\n",
                built.diagnostics.len()
            );
            for d in &built.diagnostics {
                msg.push_str(&format!("  line {}: {}\n", d.line, d.message));
            }
            Ok(ToolResult::text(msg))
        }
    }
}

// ---- vm_load_rom ----

struct LoadRom(Shared);
impl VmTool for LoadRom {
    fn name(&self) -> &str {
        "vm_load_rom"
    }
    fn description(&self) -> &str {
        "Load an assembled ROM into the VM and run its reset vector once (init). \
         Reports the reset outcome and any fault."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string"} },
            "required": ["path"]
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let path = str_arg(&args, "path")?;
        let mut c = self.0.lock();
        let outcome = c.load_rom(&path).map_err(VmToolError)?;
        Ok(ToolResult::text(format!(
            "loaded '{path}'. reset: {:?}. pc=0x{:04X}, fault={:?}",
            outcome, c.vm.pc, c.vm.fault
        )))
    }
}

// ---- vm_run_cycles ----

struct RunCycles(Shared);
impl VmTool for RunCycles {
    fn name(&self) -> &str {
        "vm_run_cycles"
    }
    fn description(&self) -> &str {
        "Free-run up to N instructions for sub-frame debugging (stops on halt). \
         Returns pc, total cycles, halted flag, and any fault."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "n": {"type": "integer", "description": "Max instructions to run"} },
            "required": ["n"]
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let n = u64_arg(&args, "n", 1);
        let mut c = self.0.lock();
        let ran = c.vm.run_cycles(n);
        Ok(ToolResult::text(
            json!({
                "ran": ran,
                "pc": c.vm.pc,
                "cycles": c.vm.cycle,
                "halted": c.vm.halted,
                "fault": c.vm.fault,
            })
            .to_string(),
        ))
    }
}

// ---- vm_run_frame ----

struct RunFrame(Shared);
impl VmTool for RunFrame {
    fn name(&self) -> &str {
        "vm_run_frame"
    }
    fn description(&self) -> &str {
        "Advance the game one frame with the given buttons held (LEFT, RIGHT, UP, \
         DOWN, A, B, START, SELECT). Returns the observation JSON: frame, cycles, \
         framebuffer_hash, changed_pixels_bbox, console, fault, vm{pc,data_stack,\
         return_stack_depth}, and game-reported entities."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "buttons": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Buttons held this frame, e.g. [\"LEFT\"]"
                }
            }
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let names: Vec<String> = args
            .get("buttons")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let bits = buttons_from_names(&names);
        let mut c = self.0.lock();
        if !c.rom_loaded {
            return Ok(ToolResult::text(
                "no ROM loaded — call vm_load_rom first".into(),
            ));
        }
        let obs = c.run_frame(bits);
        Ok(ToolResult::text(obs.to_json().to_string()))
    }
}

// ---- vm_run_frames ----

/// Hard ceiling on a batched run. At 60 fps this is 30 seconds of play, which is
/// far past the point where a blind run tells the model anything useful — and it
/// bounds the work done while the console mutex is held.
const MAX_BATCH_FRAMES: u64 = 1800;

struct RunFrames(Shared);
impl VmTool for RunFrames {
    fn name(&self) -> &str {
        "vm_run_frames"
    }
    fn description(&self) -> &str {
        "Advance many frames in one call, following an input script. Prefer this \
         over repeated vm_run_frame: one call can play a whole scenario (walk \
         right 30 frames, jump, wait) instead of a round trip per frame. \
         Returns the final observation, a run summary (frames actually run, \
         whether it stopped early on a fault/halt, every sound trigger emitted, \
         and how many distinct frames the screen changed on), and optionally the \
         final screen as a PNG."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": {
                    "type": "array",
                    "description": "Input segments played in order, e.g. \
                                    [{\"buttons\":[\"RIGHT\"],\"frames\":30},{\"buttons\":[\"A\"],\"frames\":2}]",
                    "items": {
                        "type": "object",
                        "properties": {
                            "buttons": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Buttons held for this segment (LEFT, RIGHT, UP, DOWN, A, B, START, SELECT)"
                            },
                            "frames": {"type": "integer", "description": "Frames to hold them for (default 1)"}
                        }
                    }
                },
                "frames": {
                    "type": "integer",
                    "description": "Shorthand for a single segment: run this many frames with `buttons` held (default 60). Ignored when `script` is given."
                },
                "buttons": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Buttons held for the whole run. Ignored when `script` is given."
                },
                "image": {
                    "type": "boolean",
                    "description": "Also return the final screen as a PNG (default false — ask for it when you need to SEE the result, not just its numbers)."
                }
            }
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        // Either an explicit script, or the `frames`/`buttons` shorthand as a
        // single segment.
        let segments: Vec<(u8, u64)> = match args.get("script").and_then(|v| v.as_array()) {
            Some(items) => items
                .iter()
                .map(|seg| {
                    let names: Vec<String> = seg
                        .get("buttons")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    (
                        buttons_from_names(&names),
                        seg.get("frames").and_then(|v| v.as_u64()).unwrap_or(1),
                    )
                })
                .collect(),
            None => {
                let names: Vec<String> = args
                    .get("buttons")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                vec![(buttons_from_names(&names), u64_arg(&args, "frames", 60))]
            }
        };

        let mut c = self.0.lock();
        if !c.rom_loaded {
            return Ok(ToolResult::text(
                "no ROM loaded — call vm_load_rom first".into(),
            ));
        }

        let mut ran = 0u64;
        let mut sounds: Vec<Value> = Vec::new();
        let mut changed_frames = 0u64;
        let mut last: Option<crate::Observation> = None;
        let mut stopped_early: Option<String> = None;

        'outer: for (bits, count) in segments {
            for _ in 0..count {
                if ran >= MAX_BATCH_FRAMES {
                    stopped_early = Some(format!("frame cap ({MAX_BATCH_FRAMES}) reached"));
                    break 'outer;
                }
                let obs = c.run_frame(bits);
                ran += 1;
                if obs.changed_pixels_bbox.is_some() {
                    changed_frames += 1;
                }
                // The per-frame sound log is cleared each frame, so collect it as
                // we go — this stream is the only way the model can tell that
                // audio "happened" (the console is deliberately silent).
                for s in &obs.sound {
                    sounds.push(json!({
                        "frame": obs.frame,
                        "kind": match s.kind {
                            crate::device::SoundKind::Sfx => "sfx",
                            crate::device::SoundKind::Music => "music",
                            crate::device::SoundKind::MusicStop => "music_stop",
                        },
                        "id": s.id,
                    }));
                }
                // A fault or halt mid-run is the interesting event: stop there so
                // the returned observation is the one that shows the failure,
                // rather than burying it under hundreds of dead frames.
                if obs.halted || obs.fault.is_some() {
                    stopped_early = Some(match &obs.fault {
                        Some(f) => format!("faulted: {f}"),
                        None => "halted".to_string(),
                    });
                    last = Some(obs);
                    break 'outer;
                }
                last = Some(obs);
            }
        }

        let final_obs = match last {
            Some(o) => o.to_json(),
            None => return Ok(ToolResult::text("ran 0 frames (empty script)".into())),
        };
        let text = json!({
            "frames_run": ran,
            "stopped_early": stopped_early,
            "frames_with_screen_change": changed_frames,
            "sound": sounds,
            "final": final_obs,
        })
        .to_string();

        if args.get("image").and_then(|v| v.as_bool()).unwrap_or(false) {
            let base64 = c.framebuffer_png_base64();
            return Ok(ToolResult::with_images(
                text,
                vec![ImageContent {
                    base64,
                    media_type: "image/png".to_string(),
                }],
            ));
        }
        Ok(ToolResult::text(text))
    }
}

// ---- vm_inspect_memory ----

struct InspectMemory(Shared);
impl VmTool for InspectMemory {
    fn name(&self) -> &str {
        "vm_inspect_memory"
    }
    fn description(&self) -> &str {
        "Hex+ASCII dump of a VM memory range for debugging (address and length \
         are clamped to the 64 KiB space)."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "address": {"type": "integer"},
                "length": {"type": "integer"}
            },
            "required": ["address", "length"]
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let addr = u64_arg(&args, "address", 0).min(0xffff) as usize;
        let len = u64_arg(&args, "length", 16).min(0x1000) as usize;
        let c = self.0.lock();
        let end = (addr + len).min(0x1_0000);
        let mut out = String::new();
        let mut a = addr;
        while a < end {
            let row_end = (a + 16).min(end);
            let mut hex = String::new();
            let mut ascii = String::new();
            for &b in &c.vm.mem[a..row_end] {
                hex.push_str(&format!("{b:02x} "));
                ascii.push(if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '.'
                });
            }
            out.push_str(&format!("{a:04x}: {hex:<48} {ascii}\n"));
            a = row_end;
        }
        Ok(ToolResult::text(out))
    }
}

// ---- vm_inspect_stacks ----

struct InspectStacks(Shared);
impl VmTool for InspectStacks {
    fn name(&self) -> &str {
        "vm_inspect_stacks"
    }
    fn description(&self) -> &str {
        "Return the current data stack (bottom→top), return-stack depth, pc, and \
         halt/fault state."
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    fn call(&self, _args: Value) -> Result<ToolResult, VmToolError> {
        let c = self.0.lock();
        Ok(ToolResult::text(
            json!({
                "pc": c.vm.pc,
                "data_stack": c.vm.data_stack(),
                "return_stack_depth": c.vm.return_stack_depth(),
                "halted": c.vm.halted,
                "fault": c.vm.fault,
            })
            .to_string(),
        ))
    }
}

// ---- vm_get_framebuffer ----

struct GetFramebuffer(Shared);
impl VmTool for GetFramebuffer {
    fn name(&self) -> &str {
        "vm_get_framebuffer"
    }
    fn description(&self) -> &str {
        "Return the current 128×128 screen as a PNG image for visual inspection."
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    fn call(&self, _args: Value) -> Result<ToolResult, VmToolError> {
        let c = self.0.lock();
        let base64 = c.framebuffer_png_base64();
        Ok(ToolResult::with_images(
            "128x128 framebuffer (PNG)".into(),
            vec![ImageContent {
                base64,
                media_type: "image/png".to_string(),
            }],
        ))
    }
}

// ---- vm_snapshot / vm_restore ----

struct Snapshot(Shared);
impl VmTool for Snapshot {
    fn name(&self) -> &str {
        "vm_snapshot"
    }
    fn description(&self) -> &str {
        "Save the entire VM state and return a snapshot id to restore later."
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    fn call(&self, _args: Value) -> Result<ToolResult, VmToolError> {
        let id = self.0.lock().snapshot();
        Ok(ToolResult::text(format!("snapshot saved: {id}")))
    }
}

struct Restore(Shared);
impl VmTool for Restore {
    fn name(&self) -> &str {
        "vm_restore"
    }
    fn description(&self) -> &str {
        "Restore a previously saved VM snapshot by id."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": {"type": "string"} },
            "required": ["id"]
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let id = str_arg(&args, "id")?;
        self.0.lock().restore(&id).map_err(VmToolError)?;
        Ok(ToolResult::text(format!("restored snapshot {id}")))
    }
}

// ---- vm_reset ----

struct Reset(Shared);
impl VmTool for Reset {
    fn name(&self) -> &str {
        "vm_reset"
    }
    fn description(&self) -> &str {
        "Reset the VM to power-on state (keeps written sources and built ROMs)."
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    fn call(&self, _args: Value) -> Result<ToolResult, VmToolError> {
        self.0.lock().reset();
        Ok(ToolResult::text("VM reset".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_registry() -> VmToolSet {
        VmToolSet::new(None)
    }

    #[test]
    fn all_vm_tools_registered() {
        let r = shared_registry();
        let names: Vec<String> = r.iter().map(|t| t.name().to_string()).collect();
        for expected in [
            "vm_write_source",
            "vm_assemble",
            "vm_load_rom",
            "vm_run_cycles",
            "vm_run_frame",
            "vm_run_frames",
            "vm_inspect_memory",
            "vm_inspect_stacks",
            "vm_get_framebuffer",
            "vm_snapshot",
            "vm_restore",
            "vm_reset",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn end_to_end_via_tools() {
        let r = shared_registry();
        let src = r#"
            on-frame #10 DEO
            #20 player-x STORE16
            RET
            @on-frame
                #20 DEI #01 AND draw JZ
                player-x LOAD16 #01 SUB player-x STORE16
                @draw
                player-x LOAD16 #50 DEO
                60 #51 DEO
                #01 #52 DEO
                RET
            @player-x .res 2
        "#;
        r.call("vm_write_source", json!({"path": "g.asm", "source": src}))
            .unwrap();
        let asm = r.call("vm_assemble", json!({"path": "g.asm"})).unwrap();
        assert!(asm.text.contains("ok"), "assemble said: {}", asm.text);
        r.call("vm_load_rom", json!({"path": "g.asm"})).unwrap();

        let f1 = r.call("vm_run_frame", json!({"buttons": []})).unwrap();
        let v1: Value = serde_json::from_str(&f1.text).unwrap();
        assert_eq!(v1["entities"][0]["x"], 32);

        let f2 = r
            .call("vm_run_frame", json!({"buttons": ["LEFT"]}))
            .unwrap();
        let v2: Value = serde_json::from_str(&f2.text).unwrap();
        assert_eq!(v2["entities"][0]["x"], 31);

        // Framebuffer tool returns a PNG image.
        let fb = r.call("vm_get_framebuffer", json!({})).unwrap();
        assert_eq!(fb.images.len(), 1);
        assert_eq!(fb.images[0].media_type, "image/png");
        assert!(!fb.images[0].base64.is_empty());
    }

    /// One `vm_run_frames` call must be equivalent to the same inputs fed
    /// frame-by-frame — that equivalence is the whole reason it's safe to batch.
    #[test]
    fn run_frames_matches_frame_by_frame() {
        // The test ROM decrements player-x on every other frame while LEFT is
        // irrelevant, so position is a pure function of the frame count.
        let src = r#"
            on-frame #10 DEO
            #20 player-x STORE16
            RET
            @on-frame
                #20 DEI #01 AND draw JZ
                player-x LOAD16 #01 SUB player-x STORE16
                @draw
                player-x LOAD16 #50 DEO
                60 #51 DEO
                #01 #52 DEO
                RET
            @player-x .res 2
        "#;

        let stepwise = shared_registry();
        stepwise
            .call("vm_write_source", json!({"path": "g.asm", "source": src}))
            .unwrap();
        stepwise
            .call("vm_assemble", json!({"path": "g.asm"}))
            .unwrap();
        stepwise
            .call("vm_load_rom", json!({"path": "g.asm"}))
            .unwrap();
        let mut stepwise_last = Value::Null;
        for _ in 0..10 {
            let out = stepwise
                .call("vm_run_frame", json!({"buttons": []}))
                .unwrap();
            stepwise_last = serde_json::from_str(&out.text).unwrap();
        }

        let batched = shared_registry();
        batched
            .call("vm_write_source", json!({"path": "g.asm", "source": src}))
            .unwrap();
        batched
            .call("vm_assemble", json!({"path": "g.asm"}))
            .unwrap();
        batched
            .call("vm_load_rom", json!({"path": "g.asm"}))
            .unwrap();
        let out = batched
            .call("vm_run_frames", json!({"frames": 10, "image": true}))
            .unwrap();
        let v: Value = serde_json::from_str(&out.text).unwrap();

        assert_eq!(v["frames_run"], 10);
        assert!(v["stopped_early"].is_null(), "clean run: {}", out.text);
        assert_eq!(
            v["final"]["entities"][0]["x"], stepwise_last["entities"][0]["x"],
            "batched run diverged from frame-by-frame"
        );
        assert_eq!(v["final"]["frame"], stepwise_last["frame"]);
        // `image: true` attaches the final screen.
        assert_eq!(out.images.len(), 1);
        assert_eq!(out.images[0].media_type, "image/png");
    }

    /// A script's segments are played in order, and each holds its buttons for
    /// its own frame count.
    #[test]
    fn run_frames_follows_a_script() {
        let r = shared_registry();
        // Moves left while LEFT is held, otherwise stands still.
        let src = r#"
            on-frame #10 DEO
            #40 player-x STORE16
            RET
            @on-frame
                #20 DEI #01 AND skip JZ
                player-x LOAD16 #01 SUB player-x STORE16
                @skip
                player-x LOAD16 #50 DEO
                60 #51 DEO
                #01 #52 DEO
                RET
            @player-x .res 2
        "#;
        r.call("vm_write_source", json!({"path": "g.asm", "source": src}))
            .unwrap();
        r.call("vm_assemble", json!({"path": "g.asm"})).unwrap();
        r.call("vm_load_rom", json!({"path": "g.asm"})).unwrap();

        let out = r
            .call(
                "vm_run_frames",
                json!({"script": [
                    {"buttons": ["LEFT"], "frames": 5},
                    {"buttons": [], "frames": 3},
                ]}),
            )
            .unwrap();
        let v: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(v["frames_run"], 8);
        // 5 frames of LEFT from x=64, then 3 idle frames that must not move it.
        assert_eq!(v["final"]["entities"][0]["x"], 64 - 5);
        // No image unless asked for.
        assert!(out.images.is_empty());
    }

    #[test]
    fn run_frames_needs_a_rom() {
        let r = shared_registry();
        let out = r.call("vm_run_frames", json!({"frames": 5})).unwrap();
        assert!(out.text.contains("no ROM loaded"), "{}", out.text);
    }

    #[test]
    fn unknown_tool_name_is_an_error_not_a_panic() {
        let r = shared_registry();
        let err = r.call("vm_nope", json!({})).unwrap_err();
        assert!(err.to_string().contains("vm_nope"), "{err}");
    }
}
