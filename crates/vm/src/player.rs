//! `VmPlayer` — a standalone handle that drives a [`VmConsole`] for **human
//! play**: load a game, advance a frame with the current gamepad state, and read
//! back the framebuffer as RGBA.
//!
//! `kessel run` renders `framebuffer_rgba()` scaled up and calls `tick()` on a
//! 60 Hz timer with the keyboard-derived button bitfield. The window layer is
//! deliberately on the other side of this API: the player hands out plain pixels
//! and knows nothing about how they reach a screen.

use parking_lot::Mutex;

use super::vm::RunOutcome;
use super::VmConsole;

/// A self-contained console for playing a ROM. Cheap to construct; holds one
/// [`VmConsole`] behind a mutex so the render timer and any loader can share it.
pub struct VmPlayer {
    inner: Mutex<VmConsole>,
}

impl Default for VmPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl VmPlayer {
    pub fn new() -> Self {
        VmPlayer {
            inner: Mutex::new(VmConsole::new()),
        }
    }

    /// Compile (`.ux`) or assemble (`.asm`) `source`, load the ROM, and run its
    /// reset vector. Returns an empty string on success, or a human-readable
    /// error / diagnostics listing.
    ///
    /// On **any** failure the previously active ROM (if any) is deactivated, so
    /// `has_rom` reports false and the render loop won't keep showing a stale
    /// game. A reset that halts/faults/exceeds the instruction cap is reported
    /// as a load error rather than silently opening a dead game.
    /// `#include` resolves against the sources this player has been handed —
    /// nothing else, since a caller that passes a bare string has told us
    /// nothing about a directory. A host with several files hands each of them
    /// over with [`write_source`](Self::write_source) before loading;
    /// [`load_file`](Self::load_file) is the one for a host that has a real
    /// directory.
    pub fn load(&self, source: String, path: String) -> String {
        let mut c = self.inner.lock();
        // VmPlayer is in-memory (no root), so this never touches disk — but a
        // write can still fail if a project were ever set, so surface it.
        if let Err(e) = c.write_source(&path, &source) {
            c.rom_loaded = false;
            return e;
        }
        drop(c);
        self.build(path)
    }

    /// Add a source to the player's in-memory workspace without loading it —
    /// how a host with no filesystem (Android's `AssetManager`, say) makes a
    /// file available to `#include` before calling [`load`](Self::load).
    pub fn write_source(&self, path: &str, source: &str) -> String {
        match self.inner.lock().write_source(path, source) {
            Ok(()) => String::new(),
            Err(e) => e,
        }
    }

    /// Load `name` from the directory `root`, reading it (and anything it
    /// `#include`s) from disk.
    ///
    /// This is [`load`](Self::load) for a host that has a real directory: the
    /// console reads the file itself, so a reload picks up edits to *included*
    /// files too, and nothing is written back over the game the player is
    /// running.
    pub fn load_file(&self, root: std::path::PathBuf, name: String) -> String {
        self.inner.lock().set_root(Some(root));
        self.build(name)
    }

    /// Assemble a source already in the workspace, load it, and report what a
    /// person needs to see. Shared by both load paths.
    fn build(&self, path: String) -> String {
        let mut c = self.inner.lock();
        let built = match c.assemble(&path) {
            Ok(b) => b,
            Err(e) => {
                c.rom_loaded = false;
                return e;
            }
        };
        if !built.ok() {
            c.rom_loaded = false;
            return built
                .diagnostics
                .iter()
                .map(|d| format!("{}: {}", d.location(), d.message))
                .collect::<Vec<_>>()
                .join("\n");
        }
        match c.load_rom(&path) {
            Ok(RunOutcome::Completed) => String::new(),
            Ok(RunOutcome::Halted) => {
                c.rom_loaded = false;
                match c.vm.fault.clone() {
                    Some(f) => format!("reset faulted: {f}"),
                    None => "reset halted before installing a frame vector".to_string(),
                }
            }
            Ok(RunOutcome::CapExceeded) => {
                c.rom_loaded = false;
                "reset exceeded the instruction cap (possible infinite loop)".to_string()
            }
            Err(e) => {
                c.rom_loaded = false;
                e
            }
        }
    }

    /// Advance one frame with `buttons` held (see the `BTN_*` bit values). A
    /// no-op until a ROM is loaded. The ROM's pause button (from its `controls`
    /// metadata, default START) freezes and resumes the game — see
    /// [`is_paused`](Self::is_paused).
    pub fn tick(&self, input: impl Into<crate::device::Input>) {
        self.tick_collecting(input, &mut |_| {});
    }

    /// Advance one frame and hand each sound it asked for to `sink`.
    ///
    /// The sink is a callback rather than a returned `Vec` because a host calls
    /// this sixty times a second and usually forwards straight into a fixed
    /// queue; allocating a vector per frame to carry zero or one event would be
    /// pure waste.
    ///
    /// Nothing is emitted for a frame that did not run — while paused the
    /// device's log still holds the *last* frame's triggers, and replaying them
    /// every frame would turn a pause into a stuck note.
    pub fn tick_collecting(
        &self,
        input: impl Into<crate::device::Input>,
        sink: &mut dyn FnMut(kessel_audio::AudioEvent),
    ) {
        let mut c = self.inner.lock();
        if !c.rom_loaded {
            return;
        }
        // `init()` ran at load, outside any frame. Its triggers belong to the
        // first frame that runs, or they are lost.
        for ev in c.take_reset_sound() {
            sink(ev);
        }
        let before = c.frame;
        c.play_tick(input);
        if c.frame == before {
            return; // paused
        }
        for ev in &c.vm.devices.sound {
            sink(*ev);
        }
    }

    /// The loaded ROM's instruments and sound effects, for a host to hand to
    /// `kessel-audio`.
    pub fn sound_bank(&self) -> kessel_audio::SoundBank {
        self.inner.lock().sound_bank().clone()
    }

    /// See [`VmConsole::audio_epoch`]. A host with a live synth panics it when
    /// this changes.
    pub fn audio_epoch(&self) -> u64 {
        self.inner.lock().audio_epoch()
    }

    /// Whether the game is currently paused (the pause button was pressed). The
    /// framebuffer stays frozen on the last frame while paused.
    pub fn is_paused(&self) -> bool {
        self.inner.lock().is_paused()
    }

    /// The loaded ROM's control-layout metadata as a JSON string, for a host UI
    /// to label buttons / lay out an on-screen pad. Default layout until a ROM
    /// is loaded.
    pub fn controls_json(&self) -> String {
        self.inner.lock().controls().to_json().to_string()
    }

    /// The current framebuffer expanded to `dim*dim*4` RGBA bytes, or `None`
    /// when no ROM is loaded.
    pub fn framebuffer_rgba(&self) -> Option<Vec<u8>> {
        let c = self.inner.lock();
        if c.rom_loaded {
            Some(c.framebuffer_rgba())
        } else {
            None
        }
    }

    /// The current framebuffer written into a caller-owned RGBA buffer. True if
    /// a frame was written; false if no ROM is loaded or `dst` is too small.
    ///
    /// The zero-allocation counterpart to [`framebuffer_rgba`](Self::framebuffer_rgba),
    /// for hosts blitting at 60 Hz into a buffer they already own.
    pub fn framebuffer_rgba_into(&self, dst: &mut [u8]) -> bool {
        let c = self.inner.lock();
        c.rom_loaded && c.framebuffer_rgba_into(dst)
    }

    /// Screen edge length in pixels (square).
    ///
    /// Set by the loaded ROM's `screen { … }` block, so a host must read it
    /// *after* [`load`](Self::load) — sizing a frame buffer before then gets
    /// the default 128, and a 240×240 game would tear across it.
    pub fn screen_dim(&self) -> u32 {
        self.inner.lock().screen_dim()
    }

    pub fn has_rom(&self) -> bool {
        self.inner.lock().rom_loaded
    }

    /// Whether the machine has halted or faulted (game over / crash).
    pub fn is_halted(&self) -> bool {
        self.inner.lock().vm.halted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{BTN_RIGHT, CLASSIC_DIM};

    const MOVER: &str = r#"
        local player_x = 32
        function update()
          if btn(RIGHT) then player_x = player_x + 1 end
        end
        function draw()
          cls(0)
          pset(player_x, 60, 7)
          entity(player_x, 60, 1)
        end
    "#;

    #[test]
    fn load_tick_and_render() {
        let p = VmPlayer::new();
        assert!(!p.has_rom());
        assert!(p.framebuffer_rgba().is_none());

        let err = p.load(MOVER.to_string(), "mover.lua".to_string());
        assert!(err.is_empty(), "load error: {err}");
        assert!(p.has_rom());
        assert_eq!(p.screen_dim(), CLASSIC_DIM as u32);

        // Tick a frame; framebuffer should now be the right size and drawable.
        p.tick(0);
        let fb = p.framebuffer_rgba().expect("has rom");
        assert_eq!(fb.len(), (CLASSIC_DIM * CLASSIC_DIM) * 4);
        // Pixel (32,60) drawn in colour 7 (opaque). Alpha byte is 0xff.
        let idx = (60 * CLASSIC_DIM + 32) * 4;
        assert_eq!(fb[idx + 3], 0xff);

        // Hold RIGHT: the player pixel advances one column each tick.
        p.tick(BTN_RIGHT);
        p.tick(BTN_RIGHT);
        // The pixel is now at x=34; the old column (32) should be background.
        let fb = p.framebuffer_rgba().unwrap();
        let old = (60 * CLASSIC_DIM + 32) * 4;
        let new = (60 * CLASSIC_DIM + 34) * 4;
        assert_ne!(
            &fb[new..new + 3],
            &fb[old..old + 3],
            "pixel should have moved"
        );
    }

    #[test]
    fn pause_button_freezes_and_resumes() {
        // A game that advances a global each frame; START pauses by default.
        let src = r#"
            local n = 0
            function update() n = n + 1 end
            function draw() cls(0)  pset(n, 0, 7)  entity(n, 0, 1) end
        "#;
        let p = VmPlayer::new();
        assert!(p.load(src.to_string(), "p.lua".to_string()).is_empty());
        assert!(!p.is_paused());
        p.tick(0); // n -> 1
        p.tick(0); // n -> 2

        // Press START (bit 0x40): toggles pause, so this frame does NOT advance.
        p.tick(super::super::device::BTN_START);
        assert!(p.is_paused());
        // Holding / re-ticking while paused does not advance the game.
        p.tick(0);
        p.tick(0);
        let fb_paused = p.framebuffer_rgba().unwrap();
        p.tick(0);
        assert_eq!(
            fb_paused,
            p.framebuffer_rgba().unwrap(),
            "frozen while paused"
        );

        // Press START again: resume.
        p.tick(super::super::device::BTN_START);
        assert!(!p.is_paused());
        p.tick(0); // advances again
        assert_ne!(
            fb_paused,
            p.framebuffer_rgba().unwrap(),
            "resumed after pause"
        );
    }

    #[test]
    fn controls_json_reflects_the_rom() {
        let p = VmPlayer::new();
        let src = "controls { a = \"fire\"  pause = SELECT } function draw() cls(0) end";
        assert!(p.load(src.to_string(), "c.lua".to_string()).is_empty());
        let j = p.controls_json();
        assert!(j.contains("\"fire\""), "got: {j}");
        assert!(j.contains("SELECT"), "got: {j}");
    }

    #[test]
    fn load_reports_diagnostics() {
        let p = VmPlayer::new();
        let err = p.load(
            "function draw() x = 1 end".to_string(),
            "bad.lua".to_string(),
        );
        assert!(err.contains("unknown variable"), "got: {err}");
        assert!(!p.has_rom());
    }

    #[test]
    fn failed_reload_deactivates_previous_rom() {
        let p = VmPlayer::new();
        assert!(p
            .load(MOVER.to_string(), "mover.lua".to_string())
            .is_empty());
        p.tick(0);
        assert!(p.has_rom());
        // A subsequent bad load must not leave the old ROM active/rendering.
        let err = p.load(
            "function draw() nope() end".to_string(),
            "bad.lua".to_string(),
        );
        assert!(!err.is_empty());
        assert!(
            !p.has_rom(),
            "stale ROM stayed active after a failed reload"
        );
        assert!(p.framebuffer_rgba().is_none());
    }

    #[test]
    fn reset_fault_is_a_load_error() {
        // Reset vector that immediately HALTs never installs a frame vector.
        let p = VmPlayer::new();
        let err = p.load("HALT".to_string(), "halt.asm".to_string());
        assert!(
            err.contains("reset halted") || err.contains("faulted"),
            "got: {err}"
        );
        assert!(!p.has_rom());
    }

    #[test]
    fn reset_infinite_loop_is_a_load_error() {
        // Reset spins forever -> CapExceeded, reported as a load error.
        let p = VmPlayer::new();
        let err = p.load("@spin spin JMP".to_string(), "spin.asm".to_string());
        assert!(err.contains("instruction cap"), "got: {err}");
        assert!(!p.has_rom());
    }

    /// The path a host with no filesystem takes: hand over each library file,
    /// then load the game. This is what Android does with its assets.
    #[test]
    fn a_game_loads_once_its_libraries_have_been_handed_over() {
        let p = VmPlayer::new();
        assert!(p
            .write_source(
                "lib/motion.lua",
                include_str!("../../../games/lib/motion.lua")
            )
            .is_empty());

        let err = p.load(
            include_str!("../../../games/swarm.lua").to_string(),
            "swarm.lua".to_string(),
        );
        assert!(err.is_empty(), "swarm.lua failed to load: {err}");
        p.tick(0);
        p.tick(BTN_RIGHT);
        assert!(p.framebuffer_rgba().is_some());
    }

    /// `kessel run games/swarm.lua` — the console reads the game *and* its
    /// includes off the disk, out of the directory the file sits in.
    #[test]
    fn a_game_loads_from_a_directory_with_its_includes() {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games");
        let p = VmPlayer::new();
        let err = p.load_file(games, "swarm.lua".to_string());
        assert!(err.is_empty(), "swarm.lua failed to load: {err}");
        p.tick(0);
        p.tick(BTN_RIGHT);
        assert!(p.framebuffer_rgba().is_some());
    }

    /// Forgetting the library is the shape of bug a host hits, so it has to say
    /// which file is missing rather than fail somewhere in the compiler.
    #[test]
    fn a_missing_library_names_itself() {
        let p = VmPlayer::new();
        let err = p.load(
            include_str!("../../../games/swarm.lua").to_string(),
            "swarm.lua".to_string(),
        );
        assert!(
            err.contains("cannot find include 'lib/motion.lua'"),
            "{err}"
        );
    }

    #[test]
    fn assembly_dialect_also_plays() {
        let p = VmPlayer::new();
        let asm = "on-frame #10 DEO RET @on-frame #07 #16 DEO RET";
        assert!(p.load(asm.to_string(), "x.asm".to_string()).is_empty());
        p.tick(0);
        let fb = p.framebuffer_rgba().unwrap();
        // cls to colour 7 -> every pixel opaque and equal.
        assert_eq!(fb[3], 0xff);
        assert_eq!(&fb[0..4], &fb[4..8]);
    }
}
