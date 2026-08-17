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

use crate::device::{Input, Touch, MAX_TOUCHES, STICK_FULL};

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
        Box::new(Reset(console.clone())),
        Box::new(RenderAudio(console)),
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
        r#"Write source for the fantasy-console VM to a named file. When a working directory is set the file is written ON DISK — the same file your own file-editing tools, a human editor, and `kessel run` see — so for a small change to an existing game, edit that file directly with your file tools and just call vm_assemble. Use vm_write_source for a first draft or a full rewrite. A '.asm' path is stack assembly; a '.lua' path is a small statically-typed Lua-ish dialect (NOT full PICO-8/Lua: no tables/metatables/closures/recursion). Overwrites any previous source at that path and invalidates its built ROM.

luax essentials (a '.lua' file):
- Entry points (vector-driven, no main loop): `function init()` runs once; `function update()` then `function draw()` run each frame. Names are bare — NOT `_update`/`_draw`.
- State: top-level `local x = 60` is a persistent global. `record Name { a, b: byte }` (fields default to `word`); `local es: array(8, Name)`.
- Sprites are DECLARATIONS, not table literals: `sprite hero { <8 rows of 8 chars, '.'=transparent else palette nibble 0-9a-f> }`. `hero` is then a tile id; draw with `spr(hero, x, y, flags)`.
- Builtins: `cls(c)` (colour REQUIRED), `pset(x,y,c)`, `spr(id,x,y,flags)`, `sprn(id,x,y,w,h,flags)` (w×h block of contiguous tiles, e.g. 16×16 = 2,2), `btn(LEFT|RIGHT|UP|DOWN|A|B)` (held), `btnp`/`btnr` (pressed/released THIS frame — use for jumps/menus), `frame_count()` (frames since start), `len(arr)` (array length), `clear(rec_or_arr)` (zero a record/array in place, e.g. reset a bullet pool), `text("LITERAL",x,y,color)` / `number(n,x,y,color)` (on-screen font: scores/titles/GAME OVER; `text` needs a string LITERAL), `sfx(id)` / `music(id)` / `music_stop()` (sound triggers), `entity(x,y,tag)` (report for observation), `rnd(n)`, `map/mget/mset/fset/solid` (tilemap).
- Sound is DECLARED like sprites, and the name is the id: `instrument kick { wave = sine  attack = 0  decay = 90  sustain = 0  pitch_env = 36  pitch_decay = 60 }` then `sfx boom { inst = kick  speed = 3  notes = "40 - 36" }`, played with `sfx(boom)`. instrument keys: wave (sine|triangle|saw|square|noise), attack/decay/release/pitch_decay (ms), sustain/cutoff/resonance/distortion/volume (0-255), pitch_env/pan (-127..127), filter (off|lpf|hpf), chorus/reverb (0-255 SENDS to the one shared chorus and reverb — there is one of each for the whole mix, not one per instrument; `fx { reverb_size = 190  reverb_damping = 90  chorus_rate = 50  chorus_depth = 140 }` says what they sound like). sfx keys: inst, speed (frames per row), vel, notes (a string of note numbers, `-` = hold the previous note, `.` = rest). MUSIC is a `track NAME { tempo = 8  vel = 150  loop = 1  <instrument> = "<rows>" ... }` block — each non-reserved key names an instrument and gives that channel's rows, same row syntax as sfx — played with `music(NAME)` and stopped with `music_stop()`. Tracks run on the AUDIO clock, so a slow frame does not stutter them. Start music in `init()`; it loops by default. For a note you decide at runtime (a rhythm game, a menu blip that follows the cursor), skip the bank: `play(instrument, note, vel, frames)` plays a MIDI note number for `frames` frames; `note_on(chan, instrument, note, vel)` / `note_off(chan)` hold one until you release it, where `chan` is any 0-255 label YOU choose and track (a voice is not — voices get stolen, channels don't). An out-of-range argument (channel/instrument >255, note >127, velocity >255) plays NOTHING rather than wrapping onto a channel another note is using; vm_render_audio reports how many were ignored. A note plus holds is ONE long note; a repeated number retriggers. Drums come from noise or sine + pitch_env, not a drum machine. You cannot hear the result, so check it with vm_render_audio: it reports every trigger with its frame, and warns when an id has no declaration or a sound fired but was silent.
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
                "path": {"type": "string", "description": "Workspace file name, e.g. 'game.lua' or 'game.asm'. An ABSOLUTE path (e.g. '/Users/me/games/tetris.lua') also SETS the working directory to its parent for the rest of the session, so pass one on the first write when you know where the project should live; every later call can then use the bare name. The reply says which file was written."},
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
         the Lua-ish dialect to assembly first. A game may span several files: \
         '#include \"lib/util.lua\"' splices another source's declarations in, \
         resolved against the same working directory (there is no 'require'). \
         Returns diagnostics with a file and line number on error, or the byte \
         size and labels on success."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string", "description": "The file to build: a workspace name, or an absolute path to compile a game that already exists on disk (which also makes its directory the working directory)."} },
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
                msg.push_str(&format!("  {}: {}\n", d.location(), d.message));
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
         DOWN, A, B, START, SELECT), plus an optional analog stick and touch \
         points. Returns the observation JSON: frame, cycles, framebuffer_hash, \
         changed_pixels_bbox, console, fault, vm{pc,data_stack,\
         return_stack_depth}, and game-reported entities."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": with_analog_props(json!({
                "buttons": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Buttons held this frame, e.g. [\"LEFT\"]"
                }
            }))
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let input = input_from_args(&args);
        let mut c = self.0.lock();
        if !c.rom_loaded {
            return Ok(ToolResult::text(
                "no ROM loaded — call vm_load_rom first".into(),
            ));
        }
        let obs = c.run_frame(input);
        Ok(ToolResult::text(obs.to_json().to_string()))
    }
}

// ---- vm_run_frames ----

/// Hard ceiling on a batched run. At 60 fps this is 30 seconds of play, which is
/// far past the point where a blind run tells the model anything useful — and it
/// bounds the work done while the console mutex is held.
const MAX_BATCH_FRAMES: u64 = 1800;

/// Read the `script` / `frames` + `buttons` input shape shared by
/// `vm_run_frames` and `vm_render_audio`.
///
/// One reader rather than two: a render whose inputs behaved differently from a
/// run would make "it sounded wrong" mean "you pressed something else".
fn script_segments(args: &Value, default_frames: u64) -> Vec<(Input, u64)> {
    match args.get("script").and_then(|v| v.as_array()) {
        Some(items) => items
            .iter()
            .map(|seg| {
                (
                    input_from_args(seg),
                    seg.get("frames").and_then(|v| v.as_u64()).unwrap_or(1),
                )
            })
            .collect(),
        None => vec![(
            input_from_args(args),
            u64_arg(args, "frames", default_frames),
        )],
    }
}

/// One segment's `buttons` / `stick` / `touch` arguments as an [`Input`].
///
/// The agent drives the same three surfaces a player does. Without this it
/// could write a stick-steered or touch-driven game and then have no way to
/// test it — which would make those inputs unreachable from the loop this
/// console exists to serve.
fn input_from_args(args: &Value) -> Input {
    let names: Vec<String> = args
        .get("buttons")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let full = STICK_FULL as i64;
    let axis = |v: Option<&Value>| -> i16 {
        v.and_then(|v| v.as_i64()).unwrap_or(0).clamp(-full, full) as i16
    };
    let stick = args.get("stick").and_then(|v| v.as_array());

    let mut touches = [Touch::default(); MAX_TOUCHES];
    if let Some(points) = args.get("touch").and_then(|v| v.as_array()) {
        // Extra points are dropped rather than wrapped around: a fifth finger
        // overwriting slot 0 would move a finger the caller never moved.
        for (slot, p) in points.iter().take(touches.len()).enumerate() {
            let pair = p.as_array();
            let coord = |i: usize| -> u16 {
                pair.and_then(|a| a.get(i))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .min(u16::MAX as u64) as u16
            };
            touches[slot] = Touch {
                x: coord(0),
                y: coord(1),
                down: true,
            };
        }
    }

    Input {
        buttons: buttons_from_names(&names),
        stick_x: axis(stick.and_then(|a| a.first())),
        stick_y: axis(stick.and_then(|a| a.get(1))),
        touches,
    }
}

/// The `stick` / `touch` half of an input schema, shared by every tool that
/// takes one. One definition, so the agent never sees two spellings of the same
/// argument.
fn analog_schema_props() -> Vec<(&'static str, Value)> {
    vec![
        (
            "stick",
            json!({
                "type": "array",
                "items": {"type": "integer"},
                "description": "Analog stick as [x, y], signed 8.8 fixed point: -256 is full \
                                left/up, 0 centred, 256 full right/down. Omit for a game that \
                                does not read it."
            }),
        ),
        (
            "touch",
            json!({
                "type": "array",
                "description": "Touch points held, as [[x, y], …] in console pixels — slot 0 \
                                first, up to 4. A point's slot is its identity: keep one finger \
                                at one index across frames or the game sees a release and a \
                                press that never happened.",
                "items": {"type": "array", "items": {"type": "integer"}}
            }),
        ),
    ]
}

/// Merge [`analog_schema_props`] into an existing `properties` object.
fn with_analog_props(mut properties: Value) -> Value {
    if let Some(map) = properties.as_object_mut() {
        for (k, v) in analog_schema_props() {
            map.insert(k.to_string(), v);
        }
    }
    properties
}

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
            "properties": with_analog_props(json!({
                "script": {
                    "type": "array",
                    "description": "Input segments played in order, e.g. \
                                    [{\"buttons\":[\"RIGHT\"],\"frames\":30},{\"buttons\":[\"A\"],\"frames\":2}]",
                    "items": {
                        "type": "object",
                        "properties": with_analog_props(json!({
                            "buttons": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Buttons held for this segment (LEFT, RIGHT, UP, DOWN, A, B, START, SELECT)"
                            },
                            "frames": {"type": "integer", "description": "Frames to hold them for (default 1)"}
                        }))
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
            }))
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let segments = script_segments(&args, 60);

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
                for ev in &obs.sound {
                    let mut j = crate::audio::event_json(ev);
                    if let Some(o) = j.as_object_mut() {
                        o.insert("frame".into(), json!(obs.frame));
                    }
                    sounds.push(j);
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

// ---- vm_render_audio ----

struct RenderAudio(Shared);
impl VmTool for RenderAudio {
    fn name(&self) -> &str {
        "vm_render_audio"
    }
    fn description(&self) -> &str {
        "Run the game forward and render its SOUND, returning a report you can \
         read: every trigger with the frame it fired on, peak/RMS level, voices \
         started and stolen, and a warning for each specific way audio goes \
         wrong (an sfx id with no declaration, an instrument that isn't there, \
         too many sounds at once, or triggers that fired but produced silence). \
         With a working directory set it also writes a .wav you or a human can \
         play. THIS ADVANCES THE MACHINE exactly like vm_run_frames — snapshot \
         first if you want the state back. Use it to check that a sound you \
         added actually fires; you cannot hear the WAV, but the report says \
         what happened."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": {
                    "type": "array",
                    "description": "Input segments played in order, same shape as vm_run_frames — a sound behind a button needs the button pressed.",
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
                    "description": "Shorthand for a single segment: render this many frames (default 180 = 3 seconds). Ignored when `script` is given."
                },
                "buttons": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Buttons held for the whole render. Ignored when `script` is given."
                },
                "path": {
                    "type": "string",
                    "description": "Where to write the .wav, relative to the working directory (default 'render.wav'). Ignored when no working directory is set."
                }
            }
        })
    }
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError> {
        let segments = script_segments(&args, 180);
        let mut c = self.0.lock();
        if !c.rom_loaded {
            return Ok(ToolResult::text(
                "no ROM loaded — call vm_load_rom first".into(),
            ));
        }
        let render = match c.render_audio(&segments) {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::text(e)),
        };

        let mut text = render.summary.report();
        // The WAV is for a human (or a later listening test); the report above
        // is what the agent actually reads. Failing to write it is worth
        // saying, but it does not make the render a failure.
        match c.root() {
            Some(root) => {
                let rel = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("render.wav");
                match crate::resolve_in_root(root, rel) {
                    Ok(full) => {
                        let bytes = render.to_wav();
                        let len = bytes.len();
                        match std::fs::write(&full, bytes) {
                            Ok(()) => text.push_str(&format!(
                                "wrote {} ({} bytes)\n",
                                full.display(),
                                len
                            )),
                            Err(e) => text.push_str(&format!("could not write {rel}: {e}\n")),
                        }
                    }
                    Err(e) => text.push_str(&format!("bad path '{rel}': {e}\n")),
                }
            }
            None => text.push_str(
                "no working directory set, so no .wav was written — the report above \
                 is the whole result.\n",
            ),
        }
        Ok(ToolResult::text(text))
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
            "vm_render_audio",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    const SOUND_GAME: &str = r#"
instrument blip {
  wave = square
  attack = 0  decay = 60  sustain = 0
}
sfx ping { inst = blip  speed = 2  notes = "72 79" }
local t: word
function update() t = t + 1  if t == 3 then sfx(ping) end end
function draw() cls(0) end
"#;

    fn loaded_sound_game(r: &VmToolSet) {
        r.call(
            "vm_write_source",
            json!({"path": "g.lua", "source": SOUND_GAME}),
        )
        .unwrap();
        r.call("vm_assemble", json!({"path": "g.lua"})).unwrap();
        r.call("vm_load_rom", json!({"path": "g.lua"})).unwrap();
    }

    #[test]
    fn render_audio_reports_what_fired() {
        let r = shared_registry();
        loaded_sound_game(&r);
        let out = r.call("vm_render_audio", json!({"frames": 30})).unwrap();
        let text = out.text;
        // The whole point: an agent that cannot listen can still tell that the
        // sound it added fired, and when.
        assert!(text.contains("frame 3"), "{text}");
        assert!(text.contains("ping"), "{text}");
        assert!(text.contains("2 started"), "{text}");
        assert!(!text.contains("WARNING"), "{text}");
        // No root in this registry, so it says so rather than silently not
        // writing a file.
        assert!(text.contains("no working directory set"), "{text}");
    }

    #[test]
    fn render_audio_names_the_reason_for_silence() {
        let r = shared_registry();
        r.call(
            "vm_write_source",
            json!({"path": "q.lua", "source": "function update() end\nfunction draw() cls(0) end"}),
        )
        .unwrap();
        r.call("vm_assemble", json!({"path": "q.lua"})).unwrap();
        r.call("vm_load_rom", json!({"path": "q.lua"})).unwrap();
        let out = r.call("vm_render_audio", json!({"frames": 10})).unwrap();
        assert!(out.text.contains("never called sfx()"), "{}", out.text);
    }

    #[test]
    fn render_audio_needs_a_rom() {
        let r = shared_registry();
        let out = r.call("vm_render_audio", json!({"frames": 10})).unwrap();
        assert!(out.text.contains("no ROM loaded"), "{}", out.text);
    }

    #[test]
    fn render_audio_follows_the_same_input_script_as_run_frames() {
        // Both tools read the script the same way, so "it sounded wrong" can
        // never mean "you pressed something else".
        let src = r#"
            instrument i { wave = square  attack = 0  decay = 40  sustain = 0 }
            sfx shoot { inst = i  notes = "72" }
            function update() if btnp(A) then sfx(shoot) end end
            function draw() cls(0) end
        "#;
        let r = shared_registry();
        r.call("vm_write_source", json!({"path": "s.lua", "source": src}))
            .unwrap();
        r.call("vm_assemble", json!({"path": "s.lua"})).unwrap();
        r.call("vm_load_rom", json!({"path": "s.lua"})).unwrap();

        let script = json!({"script": [
            {"buttons": [], "frames": 3},
            {"buttons": ["A"], "frames": 3},
            {"buttons": [], "frames": 10},
        ]});
        let audio = r.call("vm_render_audio", script.clone()).unwrap();
        assert!(audio.text.contains("shoot"), "{}", audio.text);

        r.call("vm_reset", json!({})).unwrap();
        r.call("vm_load_rom", json!({"path": "s.lua"})).unwrap();
        let run = r.call("vm_run_frames", script).unwrap();
        // Both tools name the same frame for the same trigger.
        assert!(run.text.contains("\"frame\":4"), "{}", run.text);
        assert!(audio.text.contains("frame 4"), "{}", audio.text);
    }

    #[test]
    fn reset_keeps_the_sound_bank_with_the_rom() {
        // vm_reset preserves sources and built ROMs; the bank is cached beside
        // them and has to survive too, or a reset silently mutes the game.
        let r = shared_registry();
        loaded_sound_game(&r);
        r.call("vm_reset", json!({})).unwrap();
        r.call("vm_load_rom", json!({"path": "g.lua"})).unwrap();
        let out = r.call("vm_render_audio", json!({"frames": 30})).unwrap();
        assert!(out.text.contains("ping"), "{}", out.text);
        assert!(!out.text.contains("WARNING"), "{}", out.text);
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
