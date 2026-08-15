//! The C ABI over [`VmPlayer`] — the console's play surface, reachable from a
//! language that is not Rust.
//!
//! This is the *run* surface only: load a game, tick it, read the pixels. The
//! authoring half of the console (`vm_write_source`, `vm_assemble`,
//! `vm_snapshot`, …) is deliberately absent — an in-app editor will want it, and
//! adding it then is additive, but shipping an unused FFI surface now would mean
//! carrying and testing bindings nothing calls.
//!
//! The C ABI is the portable layer: Swift calls it directly, so an iOS app needs
//! nothing from this crate but a header and a `staticlib`. Android cannot —
//! the JVM only speaks JNI — so [`android`] wraps these same functions in
//! `JNIEXPORT` entry points. Both sit on one implementation; neither knows
//! anything about a screen.
//!
//! ## Rules for callers
//!
//! - Every function tolerates a null handle. Passing one is a no-op returning a
//!   zero value, not a crash: a host that failed to construct a player should
//!   get a blank screen, not a SIGSEGV.
//! - Strings out of this module are heap-allocated by Rust and **must** be
//!   returned to [`kessel_string_free`]. `free()` from libc is undefined
//!   behaviour on them.
//! - A handle is not thread-safe to *free* concurrently with use, but is safe to
//!   use from several threads at once: `VmPlayer` holds its console behind a
//!   mutex. Android relies on this — the game thread ticks while the UI thread
//!   reads `is_paused`.

#[cfg(target_os = "android")]
mod android;

use std::ffi::{c_char, CStr, CString};
use std::sync::Arc;

use kessel_audio::{AudioEngine, AudioEvent, EventQueue, SynthConfig};
use kessel_vm::VmPlayer;
use parking_lot::Mutex;

/// Opaque handle. A pointer to this is what crosses the ABI; the layout is
/// nobody else's business.
pub struct KesselPlayer {
    player: VmPlayer,
    /// Sound the game has asked for, waiting for the audio thread.
    ///
    /// Lock-free on purpose: this is the one path that crosses from the game
    /// thread to the audio callback, and the callback must never wait on a
    /// frame that is running arbitrary game code.
    queue: Arc<EventQueue>,
    /// The synth, once a host has asked for one. `None` means this player is
    /// silent and costs nothing — no engine, no delay lines, no work in `tick`
    /// beyond an empty queue push.
    audio: Mutex<Option<AudioEngine>>,
}

/// Borrow the whole handle, or return `$default` if it is null.
///
/// The twin of [`player!`], for the entry points that need more than the
/// console — the audio ones, which also reach the queue and the synth. Both
/// exist as macros rather than functions so the null check and the deref stay
/// inside one `unsafe` block per call site.
macro_rules! handle {
    ($ptr:expr, $default:expr) => {
        match unsafe { $ptr.as_ref() } {
            Some(h) => h,
            None => return $default,
        }
    };
}

/// Borrow a handle, or return `$default` if it is null.
macro_rules! player {
    ($ptr:expr, $default:expr) => {
        match unsafe { $ptr.as_ref() } {
            Some(p) => &p.player,
            None => return $default,
        }
    };
}

/// Allocate a console. Never null in practice; free it with
/// [`kessel_player_free`].
#[no_mangle]
pub extern "C" fn kessel_player_new() -> *mut KesselPlayer {
    Box::into_raw(Box::new(KesselPlayer {
        player: VmPlayer::new(),
        queue: Arc::new(EventQueue::new()),
        audio: Mutex::new(None),
    }))
}

/// Destroy a console created by [`kessel_player_new`]. Null is a no-op;
/// double-freeing is not.
///
/// # Safety
/// `p` must be a pointer from [`kessel_player_new`] that has not already been
/// freed, and no other thread may be using it.
#[no_mangle]
pub unsafe extern "C" fn kessel_player_free(p: *mut KesselPlayer) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(p) });
    }
}

/// Compile and load `source` (named `name`, whose extension picks the dialect:
/// `.lua`/`.ux` for luax, `.asm` for assembly).
///
/// Returns **null on success**, or an owned C string of diagnostics that the
/// caller must pass to [`kessel_string_free`]. A failed load leaves the player
/// with no ROM — see [`VmPlayer::load`].
///
/// # Safety
/// `source` and `name` must be null-terminated UTF-8, valid for the call.
#[no_mangle]
pub unsafe extern "C" fn kessel_player_load(
    p: *mut KesselPlayer,
    source: *const c_char,
    name: *const c_char,
) -> *mut c_char {
    let player = player!(p, own_cstring("null player handle".into()));
    let (source, name) = match (unsafe { borrow_str(source) }, unsafe { borrow_str(name) }) {
        (Some(s), Some(n)) => (s, n),
        _ => return own_cstring("source and name must be valid UTF-8".into()),
    };
    let err = player.load(source.to_owned(), name.to_owned());
    if err.is_empty() {
        // A new game means new instruments, and the old game's voices and
        // reverb tail have no business ringing over it.
        if let Some(handle) = unsafe { p.as_ref() } {
            if let Some(engine) = handle.audio.lock().as_mut() {
                engine.handle(AudioEvent::Panic);
                engine.set_bank(handle.player.sound_bank());
            }
        }
        std::ptr::null_mut()
    } else {
        own_cstring(err)
    }
}

/// One touch point, in console pixels. Mirrors `kessel_vm::device::Touch`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct KesselTouch {
    pub x: u16,
    pub y: u16,
    pub down: bool,
}

/// How many touch slots [`KesselInput`] carries. Mirrors
/// `kessel_vm::device::MAX_TOUCHES`; the compile-time assert below is what keeps
/// the two from drifting into a struct the C side reads short.
pub const KESSEL_MAX_TOUCHES: usize = 4;
const _: () = assert!(KESSEL_MAX_TOUCHES == kessel_vm::device::MAX_TOUCHES);

/// Everything a host hands the console for one frame.
///
/// `#[repr(C)]` and passed by pointer, so adding a field later is a header
/// change rather than a different calling convention — and so a Swift host can
/// build one without a shim.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct KesselInput {
    /// The `KESSEL_BTN_*` bitfield.
    pub buttons: u8,
    /// Analog stick, signed 8.8 fixed: ±256 is full deflection, 0 centred.
    pub stick_x: i16,
    pub stick_y: i16,
    /// Touch points by slot. A slot is a finger's identity for its whole life —
    /// renumbering them between frames reports a release and a press the player
    /// never made.
    pub touches: [KesselTouch; KESSEL_MAX_TOUCHES],
}

impl From<&KesselInput> for kessel_vm::device::Input {
    fn from(i: &KesselInput) -> Self {
        let mut touches = [kessel_vm::device::Touch::default(); KESSEL_MAX_TOUCHES];
        for (dst, src) in touches.iter_mut().zip(i.touches.iter()) {
            *dst = kessel_vm::device::Touch {
                x: src.x,
                y: src.y,
                down: src.down,
            };
        }
        kessel_vm::device::Input {
            buttons: i.buttons,
            stick_x: i.stick_x,
            stick_y: i.stick_y,
            touches,
        }
    }
}

/// Advance one frame with `buttons` held (the `BTN_*` bitfield). A no-op until a
/// ROM is loaded.
///
/// When audio is enabled this also hands the frame's sound to the audio thread.
/// It never blocks on it: the queue is lock-free, and a full one drops the
/// sound rather than stalling the game.
///
/// The buttons-only shorthand for [`kessel_player_tick_input`]. It stays because
/// most games are digital, and making every host build a struct to press A would
/// be ceremony for nothing.
#[no_mangle]
pub extern "C" fn kessel_player_tick(p: *mut KesselPlayer, buttons: u8) {
    let handle = handle!(p, ());
    let queue = &handle.queue;
    handle.player.tick_collecting(buttons, &mut |ev| {
        queue.push(ev);
    });
}

/// Advance one frame with the full input — buttons, analog stick, touches.
///
/// A null `input` is treated as "nothing held", the same as passing 0 to
/// [`kessel_player_tick`], rather than skipping the frame: a game that stops
/// advancing is a far worse answer to a host bug than a frame with no input.
///
/// # Safety
/// `input` must be null or point to a readable `KesselInput` for the call. It is
/// only read, never retained — the caller keeps owning it, and is expected to
/// reuse one struct rather than build a fresh one sixty times a second.
#[no_mangle]
pub unsafe extern "C" fn kessel_player_tick_input(
    p: *mut KesselPlayer,
    input: *const KesselInput,
) {
    let handle = handle!(p, ());
    let input = match unsafe { input.as_ref() } {
        Some(i) => kessel_vm::device::Input::from(i),
        None => kessel_vm::device::Input::default(),
    };
    let queue = &handle.queue;
    handle.player.tick_collecting(input, &mut |ev| {
        queue.push(ev);
    });
}

// ---- audio ----

/// Give this player a synth, running at `sample_rate`.
///
/// Opt-in: a host that never calls this pays nothing at all — no engine, no
/// delay lines, and nothing in `tick` beyond a push onto an empty queue. Call
/// it once, before starting an audio thread, and call
/// [`kessel_player_audio_render`] from that thread.
///
/// Returns false only for a null handle. Enabling twice replaces the engine,
/// which is how a host changes sample rate.
#[no_mangle]
pub extern "C" fn kessel_player_audio_enable(p: *mut KesselPlayer, sample_rate: u32) -> bool {
    let handle = handle!(p, false);
    let mut engine = AudioEngine::new(SynthConfig {
        sample_rate: sample_rate.max(8_000),
        ..SynthConfig::default()
    });
    engine.set_bank(handle.player.sound_bank());
    *handle.audio.lock() = Some(engine);
    true
}

/// Render `frames` stereo frames into `out`, which must hold `frames * 2`
/// `f32`s. Returns the frames written, or 0.
///
/// **Call this from the audio thread and nowhere else.** It never touches the
/// console's lock, so it cannot be delayed by a frame of game code — that
/// separation is the whole point, and calling it from the game thread throws it
/// away.
///
/// It also never *waits* for the lock it does use: a contended engine yields
/// silence rather than a late buffer, because an audio callback that blocks is
/// an audible gap in everything, not just the sound it was waiting for.
///
/// # Safety
///
/// `out` must be valid for `frames * 2` `f32` writes.
#[no_mangle]
pub unsafe extern "C" fn kessel_player_audio_render(
    p: *mut KesselPlayer,
    out: *mut f32,
    frames: u32,
) -> u32 {
    if out.is_null() || frames == 0 {
        return 0;
    }
    let buf = unsafe { std::slice::from_raw_parts_mut(out, frames as usize * 2) };
    let Some(handle) = (unsafe { p.as_ref() }) else {
        buf.fill(0.0);
        return 0;
    };
    // Game code cannot reach this lock, so the only contention is another
    // audio thread — a host bug, and one that must not become a stall.
    let Some(mut guard) = handle.audio.try_lock() else {
        buf.fill(0.0);
        return 0;
    };
    let Some(engine) = guard.as_mut() else {
        buf.fill(0.0);
        return 0;
    };

    // Game code runs nowhere near here, but the synth is still Rust, and
    // unwinding into a JVM or an iOS audio thread is undefined behaviour.
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        while let Some(ev) = handle.queue.pop() {
            engine.handle(ev);
        }
        engine.render(buf);
    }))
    .is_ok();
    if !ok {
        buf.fill(0.0);
        return 0;
    }
    frames
}

/// Sounds dropped because the queue was full — a host can show it, or ignore it.
#[no_mangle]
pub extern "C" fn kessel_player_audio_dropped(p: *mut KesselPlayer) -> u64 {
    handle!(p, 0).queue.rejected()
}

/// Screen edge length in pixels; the framebuffer is `dim * dim * 4` bytes.
///
/// **Read this after [`kessel_player_load`], not before.** The resolution comes
/// from the ROM's `screen { … }` block, so a host that sizes its frame buffer
/// at start-up gets the 128 default and will tear a 240×240 game across it.
/// Without a ROM this reports the default.
#[no_mangle]
pub extern "C" fn kessel_player_screen_dim(p: *mut KesselPlayer) -> u32 {
    player!(p, kessel_vm::device::CLASSIC_DIM as u32).screen_dim()
}

/// Write the current frame as packed RGBA into `dst`.
///
/// Returns true if a frame was written. False means no ROM is loaded or `dst` is
/// smaller than `kessel_player_screen_dim(p)^2 * 4` — in both cases `dst` is untouched,
/// so a host can keep showing the last good frame.
///
/// This is the 60 Hz path, which is why it fills a caller-owned buffer rather
/// than returning one: the host allocates once and reuses it forever.
///
/// # Safety
/// `dst` must point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn kessel_player_framebuffer(
    p: *mut KesselPlayer,
    dst: *mut u8,
    len: usize,
) -> bool {
    let player = player!(p, false);
    if dst.is_null() {
        return false;
    }
    let dst = unsafe { std::slice::from_raw_parts_mut(dst, len) };
    player.framebuffer_rgba_into(dst)
}

/// The loaded ROM's control metadata as JSON — which buttons the game uses and
/// what they're called — so a host can lay out an on-screen pad that shows only
/// the controls that do something. Owned; free with [`kessel_string_free`].
#[no_mangle]
pub extern "C" fn kessel_player_controls_json(p: *mut KesselPlayer) -> *mut c_char {
    own_cstring(player!(p, own_cstring("{}".into())).controls_json())
}

/// Whether a ROM is loaded and rendering.
#[no_mangle]
pub extern "C" fn kessel_player_has_rom(p: *mut KesselPlayer) -> bool {
    player!(p, false).has_rom()
}

/// Whether the game is paused (its pause button was pressed). The framebuffer
/// stays frozen while this holds.
#[no_mangle]
pub extern "C" fn kessel_player_is_paused(p: *mut KesselPlayer) -> bool {
    player!(p, false).is_paused()
}

/// Whether the machine has halted or faulted — game over, or a crash.
#[no_mangle]
pub extern "C" fn kessel_player_is_halted(p: *mut KesselPlayer) -> bool {
    player!(p, false).is_halted()
}

/// Release a string returned by this module. Null is a no-op.
///
/// # Safety
/// `s` must have come from one of this module's functions and not been freed.
#[no_mangle]
pub unsafe extern "C" fn kessel_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Move a Rust `String` into a C string the caller owns.
///
/// An interior NUL cannot survive the C ABI, and diagnostics are the only
/// strings that cross here — truncating at the NUL keeps the message readable
/// rather than replacing it with an error about itself.
fn own_cstring(s: String) -> *mut c_char {
    let bytes = match s.find('\0') {
        Some(i) => s[..i].to_owned(),
        None => s,
    };
    match CString::new(bytes) {
        Ok(c) => c.into_raw(),
        Err(_) => CString::default().into_raw(),
    }
}

/// Borrow a C string as `&str`, or `None` if null or not UTF-8.
///
/// # Safety
/// `s` must be null or a valid null-terminated string living for the call.
unsafe fn borrow_str<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(s) }.to_str().ok()
}

#[cfg(test)]
mod tests {
    // The audio tests below drive the ABI exactly as a host does: enable, tick
    // on one thread, render on another.
    use super::*;
    use kessel_vm::device::{BTN_RIGHT, CLASSIC_DIM};

    /// The default screen, which every test here but the mode one uses.
    const SCREEN_PIXELS: usize = CLASSIC_DIM * CLASSIC_DIM;

    const MOVER: &str = r#"
        local x = 32
        function update() if btn(RIGHT) then x = x + 1 end end
        function draw() cls(0)  pset(x, 60, 7)  entity(x, 60, 1) end
    "#;

    const BEEPER: &str = r#"
        instrument blip { wave = square  attack = 0  decay = 80  sustain = 0 }
        sfx ping { inst = blip  notes = "72" }
        local t: word
        function update() t = t + 1  if t == 2 then sfx(ping) end end
        function draw() cls(0) end
    "#;

    /// Load `src` through the ABI, returning the diagnostics string (empty on
    /// success) so tests read like the C caller's view.
    unsafe fn load(p: *mut KesselPlayer, src: &str, name: &str) -> String {
        let (src, name) = (CString::new(src).unwrap(), CString::new(name).unwrap());
        let err = unsafe { kessel_player_load(p, src.as_ptr(), name.as_ptr()) };
        if err.is_null() {
            return String::new();
        }
        let s = unsafe { CStr::from_ptr(err) }
            .to_string_lossy()
            .into_owned();
        unsafe { kessel_string_free(err) };
        s
    }

    unsafe fn take_string(s: *mut c_char) -> String {
        let out = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
        unsafe { kessel_string_free(s) };
        out
    }

    #[test]
    fn load_tick_and_read_pixels() {
        unsafe {
            let p = kessel_player_new();
            assert!(!kessel_player_has_rom(p));

            assert_eq!(load(p, MOVER, "mover.lua"), "");
            assert!(kessel_player_has_rom(p));

            let n = (kessel_player_screen_dim(p) * kessel_player_screen_dim(p) * 4) as usize;
            assert_eq!(n, SCREEN_PIXELS * 4);
            let mut fb = vec![0u8; n];

            kessel_player_tick(p, 0);
            assert!(kessel_player_framebuffer(p, fb.as_mut_ptr(), fb.len()));
            // (32,60) drawn in colour 7 — opaque.
            let idx = (60 * kessel_player_screen_dim(p) as usize + 32) * 4;
            assert_eq!(fb[idx + 3], 0xff);

            // Holding RIGHT moves the pixel, so successive frames differ.
            let before = fb.clone();
            kessel_player_tick(p, BTN_RIGHT);
            kessel_player_tick(p, BTN_RIGHT);
            assert!(kessel_player_framebuffer(p, fb.as_mut_ptr(), fb.len()));
            assert_ne!(before, fb, "frame did not advance");

            kessel_player_free(p);
        }
    }

    /// The full-input entry point has to reach the same ports the Rust hosts
    /// do, or an iOS/Android game would read a permanently centred stick.
    #[test]
    fn tick_input_carries_the_stick_and_touches() {
        // Paints where the finger is, and marks x=1 for a negative stick or
        // x=2 for a non-negative one. The framebuffer is the only state this
        // ABI exposes, so the probe writes its answers into pixels.
        const PROBE: &str = r#"
            function update() end
            function draw()
              cls(0)
              local x: int = stick_x()
              if x < 0 then pset(1, 0, 7) else pset(2, 0, 7) end
              if touch_down(0) then pset(touch_x(0), touch_y(0), 7) end
            end
        "#;
        unsafe {
            let p = kessel_player_new();
            assert_eq!(load(p, PROBE, "probe.lua"), "");
            let dim = kessel_player_screen_dim(p) as usize;
            let mut fb = vec![0u8; dim * dim * 4];
            // Colour 7 is (0xFF,0xF1,0xE8); index 0 is black. Comparing the red
            // channel is enough to tell them apart.
            let lit = |fb: &[u8], x: usize, y: usize| fb[(y * dim + x) * 4] == 0xFF;

            let mut input = KesselInput {
                stick_x: -256,
                ..KesselInput::default()
            };
            input.touches[0] = KesselTouch {
                x: 40,
                y: 90,
                down: true,
            };
            kessel_player_tick_input(p, &input);
            assert!(kessel_player_framebuffer(p, fb.as_mut_ptr(), fb.len()));
            assert!(lit(&fb, 1, 0), "a full-left stick did not read as negative");
            assert!(lit(&fb, 40, 90), "the touch never reached the ROM");

            // A null input is "nothing held" — the frame still has to run, and
            // the previous frame's finger must be gone.
            kessel_player_tick_input(p, std::ptr::null());
            assert!(kessel_player_framebuffer(p, fb.as_mut_ptr(), fb.len()));
            assert!(lit(&fb, 2, 0), "a null input must centre the stick");
            assert!(!lit(&fb, 40, 90), "the finger outlived its frame");

            kessel_player_free(p);
        }
    }

    /// A null handle must be as safe here as everywhere else on this surface.
    #[test]
    fn tick_input_tolerates_a_null_handle() {
        let input = KesselInput::default();
        unsafe {
            kessel_player_tick_input(std::ptr::null_mut(), &input);
            kessel_player_tick_input(std::ptr::null_mut(), std::ptr::null());
        }
    }

    #[test]
    fn a_short_buffer_is_refused_and_left_alone() {
        unsafe {
            let p = kessel_player_new();
            assert_eq!(load(p, MOVER, "mover.lua"), "");
            kessel_player_tick(p, 0);

            // One byte short of a full frame: refuse rather than write partway
            // and leave the host rendering a torn picture.
            let mut small = vec![0xABu8; SCREEN_PIXELS * 4 - 1];
            assert!(!kessel_player_framebuffer(
                p,
                small.as_mut_ptr(),
                small.len()
            ));
            assert!(small.iter().all(|&b| b == 0xAB), "buffer was written into");

            kessel_player_free(p);
        }
    }

    #[test]
    fn diagnostics_come_back_as_an_owned_string() {
        unsafe {
            let p = kessel_player_new();
            let err = load(p, "function draw() x = 1 end", "bad.lua");
            assert!(err.contains("unknown variable"), "got: {err}");
            assert!(!kessel_player_has_rom(p));
            // No ROM -> no frame, so the host keeps its last good picture.
            let mut fb = vec![0u8; SCREEN_PIXELS * 4];
            assert!(!kessel_player_framebuffer(p, fb.as_mut_ptr(), fb.len()));
            kessel_player_free(p);
        }
    }

    #[test]
    fn controls_json_describes_the_loaded_rom() {
        unsafe {
            let p = kessel_player_new();
            // Default before any ROM: valid JSON, so the host can always parse.
            assert!(take_string(kessel_player_controls_json(p)).contains("dpad"));

            let src = "controls { a = \"fire\"  pause = SELECT } function draw() cls(0) end";
            assert_eq!(load(p, src, "c.lua"), "");
            let j = take_string(kessel_player_controls_json(p));
            assert!(j.contains("\"fire\""), "got: {j}");
            assert!(j.contains("SELECT"), "got: {j}");
            kessel_player_free(p);
        }
    }

    #[test]
    fn pause_freezes_the_frame() {
        unsafe {
            let p = kessel_player_new();
            let src = "local n = 0
                       function update() n = n + 1 end
                       function draw() cls(0)  pset(n, 0, 7)  entity(n, 0, 1) end";
            assert_eq!(load(p, src, "p.lua"), "");
            kessel_player_tick(p, 0);

            kessel_player_tick(p, kessel_vm::device::BTN_START);
            assert!(kessel_player_is_paused(p));

            let mut a = vec![0u8; SCREEN_PIXELS * 4];
            let mut b = vec![0u8; SCREEN_PIXELS * 4];
            kessel_player_framebuffer(p, a.as_mut_ptr(), a.len());
            kessel_player_tick(p, 0);
            kessel_player_framebuffer(p, b.as_mut_ptr(), b.len());
            assert_eq!(a, b, "frame advanced while paused");

            kessel_player_free(p);
        }
    }

    /// A host that failed to construct a player must get blanks, not a crash —
    /// this is the contract every entry point is written to.
    #[test]
    fn every_entry_point_tolerates_a_null_handle() {
        unsafe {
            let null = std::ptr::null_mut();
            kessel_player_tick(null, 0xff);
            kessel_player_free(null);
            kessel_string_free(std::ptr::null_mut());

            assert!(!kessel_player_has_rom(null));
            assert!(!kessel_player_is_paused(null));
            assert!(!kessel_player_is_halted(null));

            let mut fb = vec![0u8; SCREEN_PIXELS * 4];
            assert!(!kessel_player_framebuffer(null, fb.as_mut_ptr(), fb.len()));
            assert_eq!(take_string(kessel_player_controls_json(null)), "{}");
            assert!(!load(null, MOVER, "mover.lua").is_empty());

            // A null destination is refused even with a live player.
            let p = kessel_player_new();
            assert!(!kessel_player_framebuffer(p, std::ptr::null_mut(), 0));
            kessel_player_free(p);
        }
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }

    /// Render `frames` stereo frames through the ABI.
    unsafe fn render(p: *mut KesselPlayer, frames: u32) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames as usize * 2];
        let n = unsafe { kessel_player_audio_render(p, buf.as_mut_ptr(), frames) };
        assert_eq!(n, frames);
        buf
    }

    #[test]
    fn a_player_is_silent_until_audio_is_enabled() {
        // Opt-in: a host that never asks for sound gets none, and pays for none.
        unsafe {
            let p = kessel_player_new();
            assert!(load(p, BEEPER, "g.lua").is_empty());
            for _ in 0..10 {
                kessel_player_tick(p, 0);
            }
            let mut buf = [0.0f32; 64];
            assert_eq!(kessel_player_audio_render(p, buf.as_mut_ptr(), 32), 0);
            assert_eq!(peak(&buf), 0.0);
            kessel_player_free(p);
        }
    }

    #[test]
    fn ticking_then_rendering_produces_the_games_sound() {
        unsafe {
            let p = kessel_player_new();
            assert!(load(p, BEEPER, "g.lua").is_empty());
            assert!(kessel_player_audio_enable(p, 48_000));

            // Silence before the game asks for anything.
            assert_eq!(peak(&render(p, 800)), 0.0);

            for _ in 0..3 {
                kessel_player_tick(p, 0);
            }
            let out = render(p, 4_000);
            assert!(peak(&out) > 0.1, "the game's sound never arrived");
            assert!(out.iter().all(|v| v.is_finite()));
            kessel_player_free(p);
        }
    }

    #[test]
    fn loading_a_new_game_reinstalls_the_bank_and_cuts_the_old_one_off() {
        unsafe {
            let p = kessel_player_new();
            assert!(load(p, BEEPER, "g.lua").is_empty());
            assert!(kessel_player_audio_enable(p, 48_000));
            for _ in 0..3 {
                kessel_player_tick(p, 0);
            }
            assert!(peak(&render(p, 400)) > 0.0);

            // A game with no sound at all: the previous game's ringing note
            // must not carry over into it.
            assert!(load(p, MOVER, "m.lua").is_empty());
            for _ in 0..3 {
                kessel_player_tick(p, 0);
            }
            assert_eq!(peak(&render(p, 4_000)), 0.0, "the old game kept playing");
            kessel_player_free(p);
        }
    }

    #[test]
    fn every_audio_entry_point_tolerates_a_null_handle() {
        unsafe {
            assert!(!kessel_player_audio_enable(std::ptr::null_mut(), 48_000));
            let mut buf = [1.0f32; 8];
            assert_eq!(
                kessel_player_audio_render(std::ptr::null_mut(), buf.as_mut_ptr(), 4),
                0
            );
            assert_eq!(peak(&buf), 0.0, "a null handle must still clear the buffer");
            assert_eq!(kessel_player_audio_dropped(std::ptr::null_mut()), 0);

            // A null output buffer is a caller bug, not a crash.
            let p = kessel_player_new();
            assert_eq!(kessel_player_audio_render(p, std::ptr::null_mut(), 4), 0);
            kessel_player_free(p);
        }
    }

    /// The arrangement every host actually uses: the game ticks on one thread
    /// while the audio callback renders on another.
    ///
    /// The render path must not touch the console's lock — if it did, this
    /// would still pass, but a slow frame would become an audible gap. What it
    /// does catch is the shape being wrong: a deadlock, a panic across the
    /// boundary, or nothing ever reaching the synth.
    #[test]
    fn the_game_thread_and_the_audio_thread_can_run_at_once() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Handle(*mut KesselPlayer);
        // Safe for the same reason the C ABI documents: the console is behind a
        // mutex and the queue is lock-free. Only `free` may not race.
        unsafe impl Send for Handle {}

        unsafe {
            let p = kessel_player_new();
            assert!(load(p, BEEPER, "g.lua").is_empty());
            assert!(kessel_player_audio_enable(p, 48_000));

            let stop = Arc::new(AtomicBool::new(false));
            let ticker_stop = Arc::clone(&stop);
            let ticker_handle = Handle(p);
            let ticker = std::thread::spawn(move || {
                let h = ticker_handle;
                while !ticker_stop.load(Ordering::Relaxed) {
                    kessel_player_tick(h.0, 0);
                    std::thread::yield_now();
                }
            });

            let mut heard = false;
            for _ in 0..200 {
                if peak(&render(p, 256)) > 0.0 {
                    heard = true;
                }
            }
            stop.store(true, Ordering::Relaxed);
            ticker.join().unwrap();
            assert!(heard, "nothing reached the synth from the other thread");
            kessel_player_free(p);
        }
    }
}
