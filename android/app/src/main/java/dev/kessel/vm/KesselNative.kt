package dev.kessel.vm

import java.nio.ByteBuffer

/**
 * The raw JNI surface of `libkessel_ffi.so`.
 *
 * Every declaration here is matched by a `Java_dev_kessel_vm_KesselNative_*`
 * symbol in `crates/ffi/src/android.rs`. Renaming this object or its package
 * renames those symbols, and the failure mode is an `UnsatisfiedLinkError` at
 * first call rather than a build error — so don't.
 *
 * Nothing outside this package should touch it. [KesselVm] is the safe handle:
 * it owns the pointer's lifetime, which raw `Long`s scattered through UI code
 * would not.
 */
internal object KesselNative {
    init {
        System.loadLibrary("kessel_ffi")
    }

    /** Allocate a console. Returns an opaque pointer, or 0 on failure. */
    external fun playerNew(): Long

    /** Destroy a console. The handle is dangling afterwards. */
    external fun playerFree(handle: Long)

    /** Compile and load a game. Returns null on success, diagnostics otherwise. */
    external fun playerLoad(handle: Long, source: String, name: String): String?

    /** Advance one frame with [buttons] held. No-op until a ROM is loaded. */
    external fun playerTick(handle: Long, buttons: Int)

    /**
     * Screen edge length in pixels — valid only after [playerLoad], since the
     * ROM's `screen { … }` block chooses it. Reports the 128 default before.
     */
    external fun playerScreenDim(handle: Long): Int

    /**
     * Write the current frame into [dst] as packed RGBA.
     *
     * [dst] **must** be direct ([ByteBuffer.allocateDirect]) and at least
     * `screenDim()^2 * 4` bytes; a heap buffer silently returns false, because
     * the native side cannot address it. False also means "no ROM yet", and in
     * every false case [dst] is left untouched.
     */
    external fun playerFramebuffer(handle: Long, dst: ByteBuffer): Boolean

    /** The loaded ROM's control metadata as JSON. Always a parseable object. */
    external fun playerControlsJson(handle: Long): String

    external fun playerHasRom(handle: Long): Boolean

    external fun playerIsPaused(handle: Long): Boolean

    external fun playerIsHalted(handle: Long): Boolean
}
