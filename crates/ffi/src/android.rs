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

use jni::objects::{JByteBuffer, JClass, JIntArray, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;

use crate::{KesselInput, KesselPlayer, KesselTouch, KESSEL_MAX_TOUCHES};

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

/// Hand over a source the game may `#include`, without loading it. Returns null
/// on success, or the reason it could not be stored.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerWriteSource<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    path: JString<'l>,
    source: JString<'l>,
) -> jstring {
    let fail = |env: &mut JNIEnv<'l>, msg: &str| -> jstring {
        env.new_string(msg)
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut())
    };

    let Some(p) = player(handle) else {
        return fail(&mut env, "no console (the player was already closed)");
    };
    let (Ok(path), Ok(source)) = (env.get_string(&path), env.get_string(&source)) else {
        return fail(&mut env, "path and source must be strings");
    };
    let (path, source): (String, String) = (path.into(), source.into());

    let err = p.player.write_source(&path, &source);
    if err.is_empty() {
        std::ptr::null_mut()
    } else {
        fail(&mut env, &err)
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
    // Through the C ABI rather than `player.tick`, so this frame's sound
    // reaches the audio queue. Calling the inner player directly is how Android
    // would end up silent while every other host played.
    //
    // The VM traps faults itself and reports them through `isHalted`, so a
    // panic here means a bug in the VM, not a bad game. Swallow it: the app
    // freezes on its last frame instead of the process dying under the user.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        crate::kessel_player_tick(handle as *mut KesselPlayer, buttons as u8)
    }));
}

/// Advance one frame with buttons, an analog stick, and touch points.
///
/// `touches` is a flat `[x, y, down] * MAX_TOUCHES` array the caller **owns and
/// reuses**. A `KesselTouch[]` would be a JNI object array — an allocation and a
/// per-element call, sixty times a second, to move twelve integers.
///
/// A **short** array is fine and leaves the remaining slots empty: that is a
/// well-formed call saying less, and a host with one finger down should not have
/// to pad. A **null or unreadable** array is not — it is a call this side cannot
/// interpret, so the frame is skipped rather than advanced with every finger
/// apparently lifted. Turning a broken host call into a valid-looking input
/// frame is how a JNI bug reaches the player as a game bug.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerTickInput(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    buttons: jint,
    stick_x: jint,
    stick_y: jint,
    touches: JIntArray,
) {
    // Read the array before entering `catch_unwind`: the JNI calls need `env`,
    // and they are not what should be running inside an unwind guard.
    //
    // Every failure path below clears the pending JNI exception first. Leaving
    // one pending is worse than the call that raised it: the *next* JNI call
    // the JVM makes, anywhere, is the one that misbehaves.
    let mut raw = [0i32; KESSEL_MAX_TOUCHES * 3];
    if touches.is_null() {
        return;
    }
    let len = match env.get_array_length(&touches) {
        Ok(n) => (n.max(0) as usize).min(raw.len()),
        Err(_) => {
            let _ = env.exception_clear();
            return;
        }
    };
    if len > 0
        && env
            .get_int_array_region(&touches, 0, &mut raw[..len])
            .is_err()
    {
        let _ = env.exception_clear();
        return;
    }

    let mut input = KesselInput {
        buttons: buttons as u8,
        stick_x: stick_x as i16,
        stick_y: stick_y as i16,
        touches: [KesselTouch::default(); KESSEL_MAX_TOUCHES],
    };
    for (slot, t) in input.touches.iter_mut().enumerate() {
        let at = slot * 3;
        if at + 2 < len {
            *t = KesselTouch {
                x: raw[at].clamp(0, u16::MAX as i32) as u16,
                y: raw[at + 1].clamp(0, u16::MAX as i32) as u16,
                down: raw[at + 2] != 0,
            };
        }
    }

    // Same reasoning as `playerTick`: unwinding into the JVM is UB, and a VM bug
    // should freeze the game on its last frame rather than take the app down.
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        crate::kessel_player_tick_input(handle as *mut KesselPlayer, &input)
    }));
}

/// Give this console a synth at `sampleRate`. Call once, before starting an
/// audio thread.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerAudioEnable(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    sample_rate: jint,
) -> jboolean {
    crate::kessel_player_audio_enable(handle as *mut KesselPlayer, sample_rate.max(0) as u32)
        as jboolean
}

/// Render `frames` stereo frames of `f32` into `dst`, which **must** be a
/// direct `ByteBuffer` of at least `frames * 2 * 4` bytes.
///
/// Called from the audio thread. It reaches the synth without touching the
/// console's lock, so a slow frame of game code cannot delay it — and it is
/// wrapped in `catch_unwind` for the same reason every other entry point here
/// is: unwinding into the JVM is undefined behaviour, and this one runs on a
/// thread the platform will kill the process over.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerAudioRender<'l>(
    env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    dst: JByteBuffer<'l>,
    frames: jint,
) -> jint {
    if frames <= 0 {
        return 0;
    }
    let frames = frames as u32;
    let (Ok(addr), Ok(cap)) = (
        env.get_direct_buffer_address(&dst),
        env.get_direct_buffer_capacity(&dst),
    ) else {
        return 0;
    };
    if addr.is_null() || cap < frames as usize * 2 * std::mem::size_of::<f32>() {
        return 0;
    }
    let out = addr as *mut f32;
    catch_unwind(AssertUnwindSafe(|| unsafe {
        crate::kessel_player_audio_render(handle as *mut KesselPlayer, out, frames) as jint
    }))
    .unwrap_or(0)
}

/// Sounds dropped because the queue was full.
#[no_mangle]
pub extern "system" fn Java_dev_kessel_vm_KesselNative_playerAudioDropped(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    crate::kessel_player_audio_dropped(handle as *mut KesselPlayer) as jlong
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
