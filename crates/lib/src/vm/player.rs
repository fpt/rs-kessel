//! `VmPlayer` — a standalone, UniFFI-exported handle that drives a [`VmConsole`]
//! for **human play**: load a game, advance a frame with the current gamepad
//! state, and read back the framebuffer as RGBA. It has no LLM/agent dependency,
//! so the host can open a playable window without any model configured.
//!
//! The macOS/Windows frontend renders `framebuffer_rgba()` scaled up and calls
//! `tick()` on a 60 Hz timer with the keyboard-derived button bitfield.

use parking_lot::Mutex;

use super::device::SCREEN_DIM;
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
    pub fn load(&self, source: String, path: String) -> String {
        let mut c = self.inner.lock();
        c.write_source(&path, &source);
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
                .map(|d| format!("line {}: {}", d.line, d.message))
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
    /// no-op until a ROM is loaded.
    pub fn tick(&self, buttons: u8) {
        let mut c = self.inner.lock();
        if c.rom_loaded {
            c.run_frame(buttons);
        }
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

    /// Screen edge length in pixels (square).
    pub fn screen_dim(&self) -> u32 {
        SCREEN_DIM as u32
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
    use crate::vm::device::{BTN_RIGHT, SCREEN_PIXELS};

    const MOVER: &str = r#"
        var player_x: word = 32;
        proc update() {
            if button(RIGHT) { player_x = player_x + 1; }
        }
        proc draw() {
            clear(0);
            pixel(player_x, 60, 7);
            entity(player_x, 60, 1);
        }
    "#;

    #[test]
    fn load_tick_and_render() {
        let p = VmPlayer::new();
        assert!(!p.has_rom());
        assert!(p.framebuffer_rgba().is_none());

        let err = p.load(MOVER.to_string(), "mover.ux".to_string());
        assert!(err.is_empty(), "load error: {err}");
        assert!(p.has_rom());
        assert_eq!(p.screen_dim(), SCREEN_DIM as u32);

        // Tick a frame; framebuffer should now be the right size and drawable.
        p.tick(0);
        let fb = p.framebuffer_rgba().expect("has rom");
        assert_eq!(fb.len(), SCREEN_PIXELS * 4);
        // Pixel (32,60) drawn in colour 7 (opaque). Alpha byte is 0xff.
        let idx = (60 * SCREEN_DIM + 32) * 4;
        assert_eq!(fb[idx + 3], 0xff);

        // Hold RIGHT: the player pixel advances one column each tick.
        p.tick(BTN_RIGHT);
        p.tick(BTN_RIGHT);
        // The pixel is now at x=34; the old column (32) should be background.
        let fb = p.framebuffer_rgba().unwrap();
        let old = (60 * SCREEN_DIM + 32) * 4;
        let new = (60 * SCREEN_DIM + 34) * 4;
        assert_ne!(&fb[new..new + 3], &fb[old..old + 3], "pixel should have moved");
    }

    #[test]
    fn load_reports_diagnostics() {
        let p = VmPlayer::new();
        let err = p.load("proc draw() { x = 1; }".to_string(), "bad.ux".to_string());
        assert!(err.contains("unknown variable"), "got: {err}");
        assert!(!p.has_rom());
    }

    #[test]
    fn failed_reload_deactivates_previous_rom() {
        let p = VmPlayer::new();
        assert!(p.load(MOVER.to_string(), "mover.ux".to_string()).is_empty());
        p.tick(0);
        assert!(p.has_rom());
        // A subsequent bad load must not leave the old ROM active/rendering.
        let err = p.load("proc draw() { nope(); }".to_string(), "bad.ux".to_string());
        assert!(!err.is_empty());
        assert!(!p.has_rom(), "stale ROM stayed active after a failed reload");
        assert!(p.framebuffer_rgba().is_none());
    }

    #[test]
    fn reset_fault_is_a_load_error() {
        // Reset vector that immediately HALTs never installs a frame vector.
        let p = VmPlayer::new();
        let err = p.load("HALT".to_string(), "halt.asm".to_string());
        assert!(err.contains("reset halted") || err.contains("faulted"), "got: {err}");
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

    #[test]
    fn shipped_sample_games_load() {
        // The games/ assets shipped for `kessel --play` must stay valid.
        for (src, name) in [
            (include_str!("../../../../games/bounce.ux"), "bounce.ux"),
            (include_str!("../../../../games/mover.ux"), "mover.ux"),
        ] {
            let p = VmPlayer::new();
            let err = p.load(src.to_string(), name.to_string());
            assert!(err.is_empty(), "{name} failed to load: {err}");
            p.tick(0);
            p.tick(BTN_RIGHT);
            assert!(p.framebuffer_rgba().is_some());
        }
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
