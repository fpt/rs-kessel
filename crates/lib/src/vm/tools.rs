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
        "Write assembly source for the fantasy-console VM to a named file in the VM workspace. \
         Overwrites any previous source at that path and invalidates its built ROM."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Workspace file name, e.g. 'game.asm'"},
                "source": {"type": "string", "description": "Assembly source text"}
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
        "Assemble a previously written source file into a ROM. Returns diagnostics \
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
