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

use kessel_vm::VmPlayer;

/// Opaque handle. A pointer to this is what crosses the ABI; the layout is
/// nobody else's business.
pub struct KesselPlayer {
    player: VmPlayer,
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
        std::ptr::null_mut()
    } else {
        own_cstring(err)
    }
}

/// Advance one frame with `buttons` held (the `BTN_*` bitfield). A no-op until a
/// ROM is loaded.
#[no_mangle]
pub extern "C" fn kessel_player_tick(p: *mut KesselPlayer, buttons: u8) {
    player!(p, ()).tick(buttons);
}

/// Screen edge length in pixels; the framebuffer is `dim * dim * 4` bytes.
/// Constant for the machine, so a host may call it once.
#[no_mangle]
pub extern "C" fn kessel_screen_dim() -> u32 {
    kessel_vm::device::SCREEN_DIM as u32
}

/// Write the current frame as packed RGBA into `dst`.
///
/// Returns true if a frame was written. False means no ROM is loaded or `dst` is
/// smaller than `kessel_screen_dim()^2 * 4` — in both cases `dst` is untouched,
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
    use super::*;
    use kessel_vm::device::{BTN_RIGHT, SCREEN_PIXELS};

    const MOVER: &str = r#"
        local x = 32
        function update() if btn(RIGHT) then x = x + 1 end end
        function draw() cls(0)  pset(x, 60, 7)  entity(x, 60, 1) end
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

            let n = (kessel_screen_dim() * kessel_screen_dim() * 4) as usize;
            assert_eq!(n, SCREEN_PIXELS * 4);
            let mut fb = vec![0u8; n];

            kessel_player_tick(p, 0);
            assert!(kessel_player_framebuffer(p, fb.as_mut_ptr(), fb.len()));
            // (32,60) drawn in colour 7 — opaque.
            let idx = (60 * kessel_screen_dim() as usize + 32) * 4;
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
}
