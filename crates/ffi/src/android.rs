//! JNI entry points for the Android app.
//!
//! Thin by design: every function here converts JVM types to plain Rust, calls
//! the same C ABI in [`crate`], and converts back. No console logic lives at
//! this layer — if you find yourself wanting to add some, it belongs in
//! `kessel-vm` where the Swift side gets it too.
//!
//! Symbols must match `dev.kessel.vm.KesselNative` exactly; renaming that class
//! renames every function below.
//!
//! ## Two things this layer owes the JVM
//!
//! **Frames go through a direct `ByteBuffer`.** The Kotlin side allocates one
//! `128*128*4` buffer at startup and we write straight into its memory. Handing
//! back a `byte[]` instead would allocate 64 KiB on the Java heap sixty times a
//! second, which is a GC problem, not a copy problem.
//!
//! **Panics stop here.** Unwinding across the JNI boundary is undefined
//! behaviour, and the functions that run *game* code — `playerLoad`,
//! `playerTick` — are exactly where a panic could come from. They run inside
//! [`catch_unwind`](std::panic::catch_unwind) and report a caught panic the same
//! way they report a compile error: as a message the app can show.

use std::panic::{catch_unwind, AssertUnwindSafe};

use jni::objects::{JByteBuffer, JClass, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;

use crate::KesselPlayer;

/// Reconstitute a handle from the `long` Kotlin holds, or `None` for 0.
///
/// Kotlin's `0L` is the "no player" sentinel — `KesselVm.close()` sets it — so
/// this is a routine state, not an error.
fn player<'a>(handle: jlong) -> Option<&'a KesselPlayer> {
    if handle == 0 {
        return None;
    }
    unsafe { (handle as *mut KesselPlayer).as_ref() }
}

#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerNew(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    crate::kessel_player_new() as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerFree(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        unsafe { crate::kessel_player_free(handle as *mut KesselPlayer) };
    }
}

/// Compile and load a game. Returns null on success, or the diagnostics to show.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerLoad<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    source: JString<'l>,
    name: JString<'l>,
) -> jstring {
    let fail = |env: &mut JNIEnv<'l>, msg: &str| -> jstring {
        env.new_string(msg)
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut())
    };

    let Some(p) = player(handle) else {
        return fail(&mut env, "no console (the player was already closed)");
    };
    let (Ok(source), Ok(name)) = (env.get_string(&source), env.get_string(&name)) else {
        return fail(&mut env, "source and name must be strings");
    };
    let (source, name): (String, String) = (source.into(), name.into());

    // A game that fails to *compile* already comes back as a message; a game
    // that manages to panic the compiler should reach the user the same way
    // rather than take the process down.
    let err = match catch_unwind(AssertUnwindSafe(|| p.player.load(source, name))) {
        Ok(e) => e,
        Err(_) => "internal error: the compiler panicked on this source".to_string(),
    };

    if err.is_empty() {
        std::ptr::null_mut()
    } else {
        fail(&mut env, &err)
    }
}

/// Advance one frame with `buttons` held. Silent no-op without a ROM, so the
/// caller's loop needs no guard of its own.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerTick(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    buttons: jint,
) {
    let Some(p) = player(handle) else { return };
    // The VM traps faults itself and reports them through `isHalted`, so a
    // panic here means a bug in the VM, not a bad game. Swallow it: the app
    // freezes on its last frame instead of the process dying under the user.
    let _ = catch_unwind(AssertUnwindSafe(|| p.player.tick(buttons as u8)));
}

/// Screen edge length. Valid only after `playerLoad` — the ROM chooses it.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerScreenDim(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    crate::kessel_player_screen_dim(handle as *mut KesselPlayer) as jint
}

/// Write the current frame into `dst`, which **must** be a direct `ByteBuffer`
/// of at least `screenDim()^2 * 4` bytes.
///
/// Returns false — leaving `dst` untouched — for a non-direct or undersized
/// buffer, or when no ROM is loaded. The app keeps showing its last frame.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerFramebuffer<'l>(
    env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    dst: JByteBuffer<'l>,
) -> jboolean {
    let Some(p) = player(handle) else {
        return false as jboolean;
    };
    // Both calls fail on a heap-allocated ByteBuffer, which is the mistake a
    // caller is most likely to make.
    let (Ok(addr), Ok(cap)) = (
        env.get_direct_buffer_address(&dst),
        env.get_direct_buffer_capacity(&dst),
    ) else {
        return false as jboolean;
    };
    if addr.is_null() {
        return false as jboolean;
    }
    let dst = unsafe { std::slice::from_raw_parts_mut(addr, cap) };
    p.player.framebuffer_rgba_into(dst) as jboolean
}

/// The loaded ROM's control metadata as JSON. Always a parseable object; `"{}"`
/// if there is no console.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerControlsJson<'l>(
    env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
) -> jstring {
    let json = match player(handle) {
        Some(p) => p.player.controls_json(),
        None => "{}".to_string(),
    };
    env.new_string(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerHasRom(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    player(handle).is_some_and(|p| p.player.has_rom()) as jboolean
}

#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerIsPaused(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    player(handle).is_some_and(|p| p.player.is_paused()) as jboolean
}

#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerIsHalted(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    player(handle).is_some_and(|p| p.player.is_halted()) as jboolean
}
