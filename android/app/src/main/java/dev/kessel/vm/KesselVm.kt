package dev.kessel.vm

import android.graphics.Bitmap
import java.nio.ByteBuffer

/** Gamepad bits. Must match `BTN_*` in `crates/vm/src/device.rs`. */
object Buttons {
    const val LEFT = 0x01
    const val RIGHT = 0x02
    const val UP = 0x04
    const val DOWN = 0x08
    const val A = 0x10
    const val B = 0x20
    const val START = 0x40
    const val SELECT = 0x80

    /** The bit named by the ROM's `pause = …` metadata, or 0 if it names nothing. */
    fun byName(name: String): Int = when (name.uppercase()) {
        "LEFT" -> LEFT
        "RIGHT" -> RIGHT
        "UP" -> UP
        "DOWN" -> DOWN
        "A" -> A
        "B" -> B
        "START" -> START
        "SELECT" -> SELECT
        else -> 0
    }
}

/** Screen edge length of [dev.kessel.vm.KesselNative]'s default mode. */
const val CLASSIC_DIM = 128

/** Touch slots the console reports. Must match `MAX_TOUCHES` in `device.rs`. */
const val MAX_TOUCHES = 4

/** Full analog deflection, signed 8.8 fixed. Must match `STICK_FULL`. */
const val STICK_FULL = 256

/**
 * One frame's input: buttons, an analog stick, and touch points.
 *
 * The touch array is flat `[x, y, down] * MAX_TOUCHES` because that is what
 * crosses JNI without allocating — see [KesselNative.playerTickInput]. It is
 * owned by whoever builds the input and must not be shared across threads
 * without the caller's own ordering.
 */
class VmInput {
    var buttons: Int = 0
    var stickX: Int = 0
    var stickY: Int = 0
    val touches = IntArray(MAX_TOUCHES * 3)

    /** Clear every touch slot. Cheaper than allocating a fresh array a frame. */
    fun clearTouches() = touches.fill(0)

    /** Put a finger in [slot]; out-of-range slots are dropped, not wrapped. */
    fun setTouch(slot: Int, x: Int, y: Int) {
        if (slot !in 0 until MAX_TOUCHES) return
        touches[slot * 3] = x
        touches[slot * 3 + 1] = y
        touches[slot * 3 + 2] = 1
    }
}

/**
 * A console, with a lifetime Kotlin can be trusted with.
 *
 * Wraps the raw handle from [KesselNative] so the pointer cannot outlive this
 * object or be used after [close]. That matters more than it looks: the native
 * side tolerates a *null* handle, but nothing can save it from a freed one.
 *
 * Every method is `synchronized` **except [renderAudio]**. The native console
 * is internally locked and would be safe to call concurrently, but `close`
 * racing a `tick` would not be, and one lock here removes the whole question.
 * The cost is bounded — a frame of a 128×128 machine is microseconds, so a UI
 * thread asking [isPaused] never waits long enough to notice.
 *
 * The audio thread is the exception, and has to be: it would be waiting on a
 * whole frame of game code, and the result is a click in everything. See
 * [renderAudio] for what the caller owes in exchange.
 */
class KesselVm : AutoCloseable {

    /**
     * Volatile because [renderAudio] reads it off the audio thread without
     * taking this object's lock — see that method for why it must not.
     */
    @Volatile
    private var handle: Long = KesselNative.playerNew()

    /**
     * Screen edge length; the framebuffer is `screenDim()²` pixels.
     *
     * **Only meaningful after [load].** The ROM picks the resolution through
     * its `screen { … }` block, so anything sized from this before a game is
     * loaded gets the 128 default — and would tear a 240×240 game across it.
     */
    fun screenDim(): Int = if (handle == 0L) CLASSIC_DIM else KesselNative.playerScreenDim(handle)

    /**
     * The frame staging buffer. Direct, so the native side can write into it
     * without a copy, and reused rather than reallocated per frame — sixty
     * 64 KiB allocations a second is a GC problem nobody needs.
     *
     * Grown on demand instead of at construction, because the size is not known
     * until a ROM has been loaded.
     */
    private var frame: ByteBuffer = ByteBuffer.allocateDirect(CLASSIC_DIM * CLASSIC_DIM * 4)

    /**
     * Make [source] available at [path] for a later [load] to `#include`.
     * Returns null on success. Call before [load]; a game whose library is
     * missing fails to load and says which file it wanted.
     */
    @Synchronized
    fun writeSource(path: String, source: String): String? =
        if (handle == 0L) "console closed" else KesselNative.playerWriteSource(handle, path, source)

    /**
     * Compile and load a game. Returns null on success, or diagnostics to show
     * the user. A failed load leaves the console with no ROM, so the caller
     * must not assume the previous game is still running.
     */
    @Synchronized
    fun load(source: String, name: String): String? =
        if (handle == 0L) "console closed" else KesselNative.playerLoad(handle, source, name)

    /** Advance one frame with [buttons] held. No-op until a ROM is loaded. */
    @Synchronized
    fun tick(buttons: Int) {
        if (handle != 0L) KesselNative.playerTick(handle, buttons)
    }

    /**
     * Advance one frame with the full input — buttons, stick and touches.
     *
     * [input] is read, not retained: the caller keeps owning it and is expected
     * to reuse one instance so the 60 Hz path allocates nothing.
     */
    @Synchronized
    fun tick(input: VmInput) {
        if (handle != 0L) {
            KesselNative.playerTickInput(
                handle,
                input.buttons,
                input.stickX,
                input.stickY,
                input.touches,
            )
        }
    }

    /**
     * Copy the current frame into [bitmap], which must be [screenDim]² and
     * [Bitmap.Config.ARGB_8888]. Returns false — leaving [bitmap] untouched —
     * when there is no ROM yet, so a caller can keep presenting its last frame.
     *
     * The byte orders line up exactly: the VM hands out packed RGBA, and
     * `ARGB_8888` despite its name stores R,G,B,A ascending in memory. Getting
     * this wrong yields a plausible-looking picture in the wrong colours rather
     * than a crash, which is why it is stated here and tested in `BlitTest`.
     */
    @Synchronized
    fun readFrame(bitmap: Bitmap): Boolean {
        if (handle == 0L) return false
        val need = screenDim() * screenDim() * 4
        if (frame.capacity() < need) {
            frame = ByteBuffer.allocateDirect(need)
        }
        frame.rewind()
        if (!KesselNative.playerFramebuffer(handle, frame)) return false
        frame.rewind()
        bitmap.copyPixelsFromBuffer(frame)
        return true
    }

    /** The loaded ROM's control metadata. Defaults until a ROM is loaded. */
    @Synchronized
    fun controls(): Controls =
        if (handle == 0L) Controls() else Controls.parse(KesselNative.playerControlsJson(handle))

    @Synchronized
    fun hasRom(): Boolean = handle != 0L && KesselNative.playerHasRom(handle)

    @Synchronized
    fun isPaused(): Boolean = handle != 0L && KesselNative.playerIsPaused(handle)

    /** Halted or faulted — game over, or a crash the game didn't handle. */
    @Synchronized
    fun isHalted(): Boolean = handle != 0L && KesselNative.playerIsHalted(handle)

    /**
     * Give the console a synth. Call before starting an audio thread.
     *
     * Returns false if the console is closed.
     */
    @Synchronized
    fun enableAudio(sampleRate: Int): Boolean =
        handle != 0L && KesselNative.playerAudioEnable(handle, sampleRate)

    /**
     * Render [frames] stereo frames into [dst]. Returns the frames written.
     *
     * **Deliberately not `@Synchronized`.** Every other method here shares one
     * lock so that `close` cannot race a `tick`; this one must not join them,
     * because the lock is held for the length of a frame — the VM running
     * arbitrary game code — and an audio callback that waits that long is an
     * audible gap in *everything*, not just this sound. The native side is
     * built for it: this path never touches the console's own lock.
     *
     * The price is that the caller owns the ordering against [close]. Stop the
     * audio thread and join it before closing the console; [AudioPlayer] does,
     * and [handle] being volatile turns a late call into a no-op rather than a
     * use-after-free in the common case — but only a joined thread is a
     * guarantee.
     */
    fun renderAudio(dst: ByteBuffer, frames: Int): Int {
        val h = handle
        return if (h == 0L) 0 else KesselNative.playerAudioRender(h, dst, frames)
    }

    /** Sounds dropped because the game got ahead of the audio thread. */
    fun audioDropped(): Long {
        val h = handle
        return if (h == 0L) 0 else KesselNative.playerAudioDropped(h)
    }

    /** Free the console. Idempotent; every method is a no-op afterwards. */
    @Synchronized
    override fun close() {
        if (handle != 0L) {
            KesselNative.playerFree(handle)
            handle = 0L
        }
    }
}
