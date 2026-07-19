//! The `vm_*` tools the agent uses to drive the fantasy console: the full
//! write → assemble → load → run → observe → debug loop.
//!
//! All tools share one [`VmConsole`] behind an `Arc<Mutex<…>>`. They are
//! registered only by `agent_new` (the standalone `kessel` app), so they never
//! appear in `kessel-cli`/app-server. Register the whole set with
//! [`register_vm_tools`].

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::llm::ImageContent;
use crate::tool::{ToolHandler, ToolRegistry, ToolResult};
use crate::AgentError;

use super::{buttons_from_names, VmConsole};

type Shared = Arc<Mutex<VmConsole>>;

/// Construct one shared [`VmConsole`] and register every `vm_*` tool onto
/// `registry`.
pub fn register_vm_tools(registry: &mut ToolRegistry) {
    let console: Shared = Arc::new(Mutex::new(VmConsole::new()));
    registry.register(Box::new(WriteSource(console.clone())));
    registry.register(Box::new(Assemble(console.clone())));
    registry.register(Box::new(LoadRom(console.clone())));
    registry.register(Box::new(RunCycles(console.clone())));
    registry.register(Box::new(RunFrame(console.clone())));
    registry.register(Box::new(InspectMemory(console.clone())));
    registry.register(Box::new(InspectStacks(console.clone())));
    registry.register(Box::new(GetFramebuffer(console.clone())));
    registry.register(Box::new(Snapshot(console.clone())));
    registry.register(Box::new(Restore(console.clone())));
    registry.register(Box::new(Reset(console)));
}

// ---- helpers ----

fn str_arg(args: &Value, key: &str) -> Result<String, AgentError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AgentError::InternalError(format!("missing string argument '{key}'")))
}

fn u64_arg(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(|v| v.as_u64()).unwrap_or(default)
}

// ---- vm_write_source ----

struct WriteSource(Shared);
impl ToolHandler for WriteSource {
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
        r#"Write source for the fantasy-console VM to a named file in the VM workspace. A '.asm' path is stack assembly; a '.lua' path is a small statically-typed Lua-ish dialect (NOT full PICO-8/Lua: no tables/metatables/closures/recursion). Overwrites any previous source at that path and invalidates its built ROM.

luax essentials (a '.lua' file):
- Entry points (vector-driven, no main loop): `function init()` runs once; `function update()` then `function draw()` run each frame. Names are bare — NOT `_update`/`_draw`.
- State: top-level `local x = 60` is a persistent global. `record Name { a, b: byte }` (fields default to `word`); `local es: array(8, Name)`.
- Sprites are DECLARATIONS, not table literals: `sprite hero { <8 rows of 8 chars, '.'=transparent else palette nibble 0-9a-f> }`. `hero` is then a tile id; draw with `spr(hero, x, y, flags)`.
- Builtins: `cls(c)` (colour REQUIRED), `pset(x,y,c)`, `spr(id,x,y,flags)`, `btn(LEFT|RIGHT|UP|DOWN|A|B)` (held), `btnp`/`btnr` (pressed/released THIS frame — use for jumps/menus), `frame_count()` (frames since start), `len(arr)` (array length), `entity(x,y,tag)` (report for observation), `rnd(n)`, `map/mget/mset/fset/solid` (tilemap).
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
    fn call(&self, args: Value) -> Result<ToolResult, AgentError> {
        let path = str_arg(&args, "path")?;
        let source = str_arg(&args, "source")?;
        let bytes = source.len();
        self.0.lock().write_source(&path, &source);
        Ok(ToolResult::text(format!("wrote {bytes} bytes to '{path}'")))
    }
}

// ---- vm_assemble ----

struct Assemble(Shared);
impl ToolHandler for Assemble {
    fn name(&self) -> &str {
        "vm_assemble"
    }
    fn description(&self) -> &str {
        "Assemble a previously written source file into a ROM. A '.lua' file is \
         compiled from the Lua-ish dialect to assembly first. Returns diagnostics \
         with line numbers on error, or the byte size and labels on success."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string"} },
            "required": ["path"]
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, AgentError> {
        let path = str_arg(&args, "path")?;
        let built = self
            .0
            .lock()
            .assemble(&path)
            .map_err(AgentError::InternalError)?;
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
            let mut msg = format!("assemble failed with {} error(s):\n", built.diagnostics.len());
            for d in &built.diagnostics {
                msg.push_str(&format!("  line {}: {}\n", d.line, d.message));
            }
            Ok(ToolResult::text(msg))
        }
    }
}

// ---- vm_load_rom ----

struct LoadRom(Shared);
impl ToolHandler for LoadRom {
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
    fn call(&self, args: Value) -> Result<ToolResult, AgentError> {
        let path = str_arg(&args, "path")?;
        let mut c = self.0.lock();
        let outcome = c.load_rom(&path).map_err(AgentError::InternalError)?;
        Ok(ToolResult::text(format!(
            "loaded '{path}'. reset: {:?}. pc=0x{:04X}, fault={:?}",
            outcome, c.vm.pc, c.vm.fault
        )))
    }
}

// ---- vm_run_cycles ----

struct RunCycles(Shared);
impl ToolHandler for RunCycles {
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
    fn call(&self, args: Value) -> Result<ToolResult, AgentError> {
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
impl ToolHandler for RunFrame {
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
    fn call(&self, args: Value) -> Result<ToolResult, AgentError> {
        let names: Vec<String> = args
            .get("buttons")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let bits = buttons_from_names(&names);
        let mut c = self.0.lock();
        if !c.rom_loaded {
            return Ok(ToolResult::text("no ROM loaded — call vm_load_rom first".into()));
        }
        let obs = c.run_frame(bits);
        Ok(ToolResult::text(obs.to_json().to_string()))
    }
}

// ---- vm_inspect_memory ----

struct InspectMemory(Shared);
impl ToolHandler for InspectMemory {
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
    fn call(&self, args: Value) -> Result<ToolResult, AgentError> {
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
                ascii.push(if (0x20..0x7f).contains(&b) { b as char } else { '.' });
            }
            out.push_str(&format!("{a:04x}: {hex:<48} {ascii}\n"));
            a = row_end;
        }
        Ok(ToolResult::text(out))
    }
}

// ---- vm_inspect_stacks ----

struct InspectStacks(Shared);
impl ToolHandler for InspectStacks {
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
    fn call(&self, _args: Value) -> Result<ToolResult, AgentError> {
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
impl ToolHandler for GetFramebuffer {
    fn name(&self) -> &str {
        "vm_get_framebuffer"
    }
    fn description(&self) -> &str {
        "Return the current 128×128 screen as a PNG image for visual inspection."
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    fn call(&self, _args: Value) -> Result<ToolResult, AgentError> {
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
impl ToolHandler for Snapshot {
    fn name(&self) -> &str {
        "vm_snapshot"
    }
    fn description(&self) -> &str {
        "Save the entire VM state and return a snapshot id to restore later."
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    fn call(&self, _args: Value) -> Result<ToolResult, AgentError> {
        let id = self.0.lock().snapshot();
        Ok(ToolResult::text(format!("snapshot saved: {id}")))
    }
}

struct Restore(Shared);
impl ToolHandler for Restore {
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
    fn call(&self, args: Value) -> Result<ToolResult, AgentError> {
        let id = str_arg(&args, "id")?;
        self.0
            .lock()
            .restore(&id)
            .map_err(AgentError::InternalError)?;
        Ok(ToolResult::text(format!("restored snapshot {id}")))
    }
}

// ---- vm_reset ----

struct Reset(Shared);
impl ToolHandler for Reset {
    fn name(&self) -> &str {
        "vm_reset"
    }
    fn description(&self) -> &str {
        "Reset the VM to power-on state (keeps written sources and built ROMs)."
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    fn call(&self, _args: Value) -> Result<ToolResult, AgentError> {
        self.0.lock().reset();
        Ok(ToolResult::text("VM reset".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        register_vm_tools(&mut r);
        r
    }

    #[test]
    fn all_vm_tools_registered() {
        use crate::tool::ToolAccess;
        let r = shared_registry();
        let names: Vec<String> = r.get_definitions().into_iter().map(|d| d.name).collect();
        for expected in [
            "vm_write_source",
            "vm_assemble",
            "vm_load_rom",
            "vm_run_cycles",
            "vm_run_frame",
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
        use crate::tool::ToolAccess;
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
}
