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
     * Advance one frame with buttons, an analog stick, and touch points.
     *
     * [stickX]/[stickY] are signed 8.8 fixed point: ±256 is full deflection,
     * 0 centred. [touches] is a flat `[x, y, down] * MAX_TOUCHES` array the
     * caller allocates **once** and reuses — an object array of touch points
     * would be an allocation and a JNI call per finger, sixty times a second.
     *
     * A touch's *index* is its identity: the console derives press and release
     * edges per slot, so a finger must keep the same slot for its whole life or
     * the game sees a release and a press that never happened.
     */
    external fun playerTickInput(
        handle: Long,
        buttons: Int,
        stickX: Int,
        stickY: Int,
        touches: IntArray,
    )

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

    /**
     * Give the console a synth at [sampleRate]. Call once, before starting an
     * audio thread. A console that never gets one stays silent and costs
     * nothing.
     */
    external fun playerAudioEnable(handle: Long, sampleRate: Int): Boolean

    /**
     * Render [frames] stereo frames of little-endian f32 into [dst], returning
     * the frames written.
     *
     * [dst] **must** be direct and hold at least `frames * 2 * 4` bytes.
     *
     * Called from the audio thread. Unlike every other function here it does
     * not reach the console's lock, so a slow frame of game code cannot delay
     * it — which is the entire reason sound on this platform is not simply
     * rendered inside [playerTick].
     */
    external fun playerAudioRender(handle: Long, dst: ByteBuffer, frames: Int): Int

    /** Sounds dropped because the game got ahead of the audio thread. */
    external fun playerAudioDropped(handle: Long): Long

    external fun playerHasRom(handle: Long): Boolean

    external fun playerIsPaused(handle: Long): Boolean

    external fun playerIsHalted(handle: Long): Boolean
}
