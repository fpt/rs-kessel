//! `kessel run` / `kessel attach` — a window, a keyboard, and a 60 Hz tick.
//!
//! Presentation only. The console rasterizes into its own indexed framebuffer
//! and hands us RGBA; all this module does is nearest-neighbour upscale it to
//! the window and map keys onto the gamepad bitfield. That's why it uses
//! `softbuffer` (a CPU surface) rather than a GPU stack: there is no pipeline to
//! build for a 128×128 image, and keeping pixels on the CPU means what you see
//! is exactly the buffer an agent would get back from `vm_get_framebuffer`.
//!
//! Sound is silent by design — the VM records sound *events* rather than
//! synthesizing audio, so a game's `sfx()` calls show up in the observation
//! stream but nothing is played here yet.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kessel_vm::device::{
    BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_RIGHT, BTN_SELECT, BTN_START, BTN_UP,
};
use kessel_vm::VmPlayer;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Target frame time. The console is defined at 60 Hz — its frame counter and
/// any timing a game does assume it — so this paces the tick rather than
/// rendering as fast as the display allows.
const FRAME_TIME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Initial window scale. 128×128 is unusably small on a modern display.
const DEFAULT_SCALE: u32 = 5;

/// What the window is driving.
///
/// `Local` owns its VM outright. `Attached` drives the machine inside a running
/// `kessel mcp`, which the agent is driving too — see [`crate::attach`] for what
/// sharing one timeline means.
enum Source {
    Local {
        player: VmPlayer,
        /// Kept so `R` can recompile the file from disk — the edit-and-see loop
        /// a human wants while working on a game.
        path: PathBuf,
        name: String,
    },
    Attached(crate::attach::AttachClient),
}

impl Source {
    fn screen_dim(&self) -> u32 {
        match self {
            Source::Local { player, .. } => player.screen_dim(),
            Source::Attached(c) => c.screen_dim(),
        }
    }

    /// Advance one frame, handing any sound it asked for to `sink`. For
    /// `Attached` this only *records* the buttons — the client's worker thread
    /// owns the round trip, so the event loop never blocks on the agent, and
    /// the sound belongs to the agent's console rather than this process.
    fn tick(&self, buttons: u8, sink: &mut dyn FnMut(kessel_audio::AudioEvent)) {
        match self {
            Source::Local { player, .. } => player.tick_collecting(buttons, sink),
            Source::Attached(c) => c.set_buttons(buttons),
        }
    }

    /// The loaded ROM's sound bank, or an empty one when attached.
    #[cfg(feature = "audio")]
    fn sound_bank(&self) -> kessel_audio::SoundBank {
        match self {
            Source::Local { player, .. } => player.sound_bank(),
            Source::Attached(_) => kessel_audio::SoundBank::default(),
        }
    }

    /// See `VmConsole::audio_epoch` — changes when the timeline jumps.
    #[cfg(feature = "audio")]
    fn audio_epoch(&self) -> u64 {
        match self {
            Source::Local { player, .. } => player.audio_epoch(),
            Source::Attached(_) => 0,
        }
    }

    fn framebuffer_rgba(&self) -> Option<Vec<u8>> {
        match self {
            Source::Local { player, .. } => player.framebuffer_rgba(),
            Source::Attached(c) => c.framebuffer_rgba(),
        }
    }

    /// Window title, including live state so it's obvious when the agent has
    /// taken the game away from you.
    fn title(&self) -> String {
        match self {
            Source::Local { name, .. } => format!("kessel — {name}"),
            Source::Attached(c) => {
                if !c.is_connected() {
                    "kessel — session ended".to_string()
                } else if !c.has_rom() {
                    "kessel — attached (agent has no ROM loaded)".to_string()
                } else if c.is_paused() {
                    "kessel — attached (paused)".to_string()
                } else {
                    "kessel — attached to agent session".to_string()
                }
            }
        }
    }
}

/// Load `path` into a player and run the game window until the user quits.
pub fn run(path: PathBuf) -> Result<(), String> {
    let source =
        std::fs::read_to_string(&path).map_err(|e| format!("read '{}': {e}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let player = VmPlayer::new();
    // A game that won't load is a hard error with the compiler's diagnostics —
    // opening a blank window and leaving the user to guess would be worse.
    let err = player.load(source, name.clone());
    if !err.is_empty() {
        return Err(format!("{}:\n{err}", path.display()));
    }

    run_window(Source::Local { player, path, name })
}

/// Attach to a running `kessel mcp` and play its console.
pub fn run_attached(root: Option<&std::path::Path>) -> Result<(), String> {
    run_window(Source::Attached(crate::attach::attach(root)?))
}

fn run_window(source: Source) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("create event loop: {e}"))?;
    // Poll rather than Wait: the console advances on its own clock, not only
    // when input arrives.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        buttons: 0,
        window: None,
        surface: None,
        next_frame: Instant::now(),
        dim: 0,
        shown_title: String::new(),
        #[cfg(feature = "audio")]
        audio: start_audio(&source),
        #[cfg(feature = "audio")]
        audio_epoch: source.audio_epoch(),
        source,
    };
    let result = event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop: {e}"));

    #[cfg(feature = "audio")]
    if let Some(a) = &app.audio {
        // Worth one line: a game that fires more sounds than the queue can hold
        // is losing them, and nothing else would ever say so.
        let lost = a.dropped();
        if lost > 0 {
            eprintln!("kessel run: dropped {lost} sound event(s) — the audio queue was full");
        }
    }
    result
}

/// Open the audio device, or explain why there is no sound and carry on.
///
/// Sound is never a reason not to show the game: a machine with no sound card,
/// a busy device, or a format we don't handle all end up here.
#[cfg(feature = "audio")]
fn start_audio(source: &Source) -> Option<crate::audio::AudioHost> {
    match crate::audio::start(source.sound_bank()) {
        Ok(host) => {
            // Say what came up. Otherwise "no sound" and "sound at a rate your
            // speakers dislike" look identical from the outside.
            eprintln!("kessel run: audio at {} Hz", host.sample_rate());
            Some(host)
        }
        Err(e) => {
            eprintln!("kessel run: no sound ({e})");
            None
        }
    }
}

struct App {
    source: Source,
    /// The output stream, when there is one. `None` means the game plays in
    /// silence — a missing sound card is not a reason to refuse to run.
    #[cfg(feature = "audio")]
    audio: Option<crate::audio::AudioHost>,
    /// Last epoch seen from the console. A change means the timeline jumped
    /// (reload/reset), and the synth has to be told before the old timeline's
    /// notes ring over the new one.
    #[cfg(feature = "audio")]
    audio_epoch: u64,
    buttons: u8,
    window: Option<std::sync::Arc<Window>>,
    surface: Option<softbuffer::Surface<std::sync::Arc<Window>, std::sync::Arc<Window>>>,
    next_frame: Instant,
    dim: u32,
    /// Last title pushed to the window, so we only call into the window system
    /// when it actually changes.
    shown_title: String,
}

impl App {
    /// Reload the source from disk and restart the game. Reports failures to
    /// stderr and keeps the previous ROM unloaded, matching `VmPlayer::load`.
    fn reload(&mut self) {
        let Source::Local { player, path, name } = &self.source else {
            // Attached: the agent owns what's loaded. Reloading from here would
            // yank its ROM out mid-run, which is a step past sharing a timeline.
            eprintln!("kessel attach: the agent owns what's loaded — reload does nothing here");
            return;
        };
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("kessel run: reload failed: {e}");
                return;
            }
        };
        let err = player.load(source, name.clone());
        if err.is_empty() {
            eprintln!("kessel run: reloaded {}", path.display());
        } else {
            eprintln!("kessel run: reload failed:\n{err}");
        }
        // The reloaded game may declare different instruments, and the stream
        // holds the bank it was built with. Restarting it is the whole fix:
        // swapping a bank under a live callback would need a lock in exactly
        // the place that must not have one, to handle a keypress.
        #[cfg(feature = "audio")]
        {
            self.audio = start_audio(&self.source);
            self.audio_epoch = self.source.audio_epoch();
        }
    }

    /// Tell the synth when the timeline jumps.
    ///
    /// After a reload the previous game's voices and release tails are still
    /// sounding over a game that never made them, so a discontinuity has to
    /// reach the audio side as a `Panic`. The VM signals with a counter rather
    /// than emitting the event itself — it does not know a synth exists.
    #[cfg(feature = "audio")]
    fn check_audio_epoch(&mut self) {
        let epoch = self.source.audio_epoch();
        if epoch != self.audio_epoch {
            self.audio_epoch = epoch;
            if let Some(a) = &self.audio {
                a.send(kessel_audio::AudioEvent::Panic);
            }
        }
    }

    /// Keep the title in step with the session state, without hammering the
    /// window system every frame.
    fn sync_title(&mut self) {
        let title = self.source.title();
        if title != self.shown_title {
            if let Some(w) = &self.window {
                w.set_title(&title);
            }
            self.shown_title = title;
        }
    }

    /// Blit the console's framebuffer into the window, scaled to fit with
    /// nearest-neighbour so pixels stay crisp rather than blurred.
    fn redraw(&mut self) {
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return; // minimised
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };

        match self.source.framebuffer_rgba() {
            Some(fb) => blit(&mut buffer, size.width, size.height, &fb, self.dim),
            // No ROM (a failed reload, or an agent that hasn't loaded one yet) —
            // clear rather than leaving a stale frame on screen.
            None => buffer.fill(0),
        }
        let _ = buffer.present();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.dim = self.source.screen_dim();
        self.shown_title = self.source.title();
        let side = self.dim * DEFAULT_SCALE;
        let attrs = Window::default_attributes()
            .with_title(&self.shown_title)
            .with_inner_size(winit::dpi::LogicalSize::new(side, side));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => std::sync::Arc::new(w),
            Err(e) => {
                eprintln!("kessel: create window: {e}");
                event_loop.exit();
                return;
            }
        };
        match softbuffer::Context::new(window.clone())
            .and_then(|ctx| softbuffer::Surface::new(&ctx, window.clone()))
        {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                eprintln!("kessel: create surface: {e}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;

                if pressed && code == KeyCode::Escape {
                    event_loop.exit();
                    return;
                }
                // Reload on the key *release* so holding R doesn't recompile
                // dozens of times a second.
                if !pressed && code == KeyCode::KeyR {
                    self.reload();
                    return;
                }

                if let Some(bit) = button_for(code) {
                    if pressed {
                        self.buttons |= bit;
                    } else {
                        self.buttons &= !bit;
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Advance at most one frame per 1/60s of wall clock. Catching up on a
        // backlog after a stall would fast-forward the game, so a late frame is
        // simply dropped.
        let now = Instant::now();
        if now < self.next_frame {
            return;
        }
        self.next_frame = now + FRAME_TIME;

        // Forward this frame's sound to the device. The closure runs on the
        // game thread and only pushes into a lock-free queue, so a busy audio
        // callback can never stall the tick (or the window).
        #[cfg(feature = "audio")]
        {
            let audio = self.audio.as_ref();
            self.source.tick(self.buttons, &mut |ev| {
                if let Some(a) = audio {
                    a.send(ev);
                }
            });
            self.check_audio_epoch();
        }
        #[cfg(not(feature = "audio"))]
        self.source.tick(self.buttons, &mut |_| {});

        self.sync_title();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// Nearest-neighbour upscale `src` (a `dim`×`dim` RGBA image) into `dst` (a
/// `dst_w`×`dst_h` buffer of `0RGB` u32s), centred and letterboxed.
///
/// The scale is an integer so pixels stay square and crisp: a fractional scale
/// would make some source pixels one output pixel wider than their neighbours,
/// which on 8×8 sprite art is glaring.
fn blit(dst: &mut [u32], dst_w: u32, dst_h: u32, src: &[u8], dim: u32) {
    dst.fill(0);
    if dim == 0 {
        return;
    }
    let scale = (dst_w / dim).min(dst_h / dim).max(1);
    let draw = dim * scale;
    // A window smaller than one console pixel per pixel still gets the
    // top-left corner rather than an out-of-bounds write.
    let ox = dst_w.saturating_sub(draw) / 2;
    let oy = dst_h.saturating_sub(draw) / 2;

    for y in 0..draw.min(dst_h) {
        let src_row = (y / scale) * dim;
        let dst_row = ((oy + y) * dst_w) as usize;
        for x in 0..draw.min(dst_w) {
            let s = ((src_row + x / scale) * 4) as usize;
            // softbuffer wants 0RGB packed into a u32; the console's alpha is
            // always opaque, so it is simply dropped.
            dst[dst_row + (ox + x) as usize] =
                (src[s] as u32) << 16 | (src[s + 1] as u32) << 8 | (src[s + 2] as u32);
        }
    }
}

/// Map a physical key onto a gamepad bit. Arrows and WASD both drive the d-pad;
/// Z/J and X/K cover the two common right-hand layouts.
fn button_for(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::ArrowLeft | KeyCode::KeyA => BTN_LEFT,
        KeyCode::ArrowRight | KeyCode::KeyD => BTN_RIGHT,
        KeyCode::ArrowUp | KeyCode::KeyW => BTN_UP,
        KeyCode::ArrowDown | KeyCode::KeyS => BTN_DOWN,
        KeyCode::KeyZ | KeyCode::KeyJ | KeyCode::Space => BTN_A,
        KeyCode::KeyX | KeyCode::KeyK => BTN_B,
        KeyCode::Enter => BTN_START,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => BTN_SELECT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_key_layouts_reach_the_same_buttons() {
        assert_eq!(button_for(KeyCode::ArrowLeft), button_for(KeyCode::KeyA));
        assert_eq!(button_for(KeyCode::KeyZ), button_for(KeyCode::KeyJ));
        assert_eq!(button_for(KeyCode::KeyX), button_for(KeyCode::KeyK));
        assert_eq!(button_for(KeyCode::Enter), Some(BTN_START));
        assert!(button_for(KeyCode::F1).is_none());
    }

    /// Every d-pad and face button must be reachable, or a game becomes
    /// unplayable in a way no test of the VM itself would catch.
    #[test]
    fn every_console_button_is_bound() {
        let bound: u8 = [
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::KeyZ,
            KeyCode::KeyX,
            KeyCode::Enter,
            KeyCode::ShiftLeft,
        ]
        .into_iter()
        .filter_map(button_for)
        .fold(0, |acc, b| acc | b);

        let all = BTN_LEFT | BTN_RIGHT | BTN_UP | BTN_DOWN | BTN_A | BTN_B | BTN_START | BTN_SELECT;
        assert_eq!(bound, all, "some console button has no key");
    }

    /// Channel order is the bug that would show as a plausible-but-wrong picture
    /// (a blue game instead of a red one), so pin it: RGBA in, 0RGB out.
    #[test]
    fn blit_packs_rgb_in_the_right_order() {
        let src = vec![0xAA, 0xBB, 0xCC, 0xFF]; // 1×1 RGBA
        let mut dst = vec![0u32; 1];
        blit(&mut dst, 1, 1, &src, 1);
        assert_eq!(dst[0], 0x00AA_BBCC);
    }

    /// A 2× window must repeat each source pixel in a 2×2 block — not smear or
    /// skip, which a stride mistake would do.
    #[test]
    fn blit_upscales_by_an_integer_factor() {
        // 2×2 source: red, green / blue, white.
        let src = vec![
            255, 0, 0, 255, 0, 255, 0, 255, // row 0
            0, 0, 255, 255, 255, 255, 255, 255, // row 1
        ];
        let mut dst = vec![0u32; 4 * 4];
        blit(&mut dst, 4, 4, &src, 2);

        let at = |x: usize, y: usize| dst[y * 4 + x];
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(at(x, y), 0x00FF_0000, "top-left block should be red");
        }
        assert_eq!(at(2, 0), 0x0000_FF00, "top-right green");
        assert_eq!(at(0, 2), 0x0000_00FF, "bottom-left blue");
        assert_eq!(at(3, 3), 0x00FF_FFFF, "bottom-right white");
    }

    /// A non-multiple window letterboxes: the image is centred and the margin
    /// stays black rather than the picture stretching.
    #[test]
    fn blit_centres_and_letterboxes() {
        let src = vec![255, 255, 255, 255]; // 1×1 white
        let mut dst = vec![0u32; 5 * 3];
        blit(&mut dst, 5, 3, &src, 1);

        // scale = min(5,3) = 3 → a 3×3 block centred in 5 wide, 3 tall.
        let at = |x: usize, y: usize| dst[y * 5 + x];
        assert_eq!(at(0, 0), 0, "left margin stays black");
        assert_eq!(at(4, 0), 0, "right margin stays black");
        for y in 0..3 {
            for x in 1..4 {
                assert_eq!(at(x, y), 0x00FF_FFFF, "({x},{y}) should be drawn");
            }
        }
    }

    /// A window smaller than the console must not write out of bounds — the
    /// case a naive scale calculation panics on.
    #[test]
    fn blit_survives_a_window_smaller_than_the_console() {
        let src = vec![0u8; 128 * 128 * 4];
        let mut dst = vec![0u32; 10 * 10];
        blit(&mut dst, 10, 10, &src, 128); // would overflow if unclamped
    }

    /// A missing file is a clean error, not a panic or a blank window.
    #[test]
    fn missing_file_is_reported() {
        let err = run(PathBuf::from("/definitely/not/here.lua")).unwrap_err();
        assert!(err.contains("read"), "{err}");
    }

    /// A source that doesn't compile fails before any window is created, with
    /// the diagnostics attached.
    #[test]
    fn broken_source_fails_with_diagnostics() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("kessel-broken-{}.lua", std::process::id()));
        std::fs::write(&path, "function draw() this is not luax end").unwrap();
        let err = run(path.clone()).unwrap_err();
        assert!(
            err.contains(&path.display().to_string()),
            "names the file: {err}"
        );
        std::fs::remove_file(&path).ok();
    }
}
