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

/**
 * A console, with a lifetime Kotlin can be trusted with.
 *
 * Wraps the raw handle from [KesselNative] so the pointer cannot outlive this
 * object or be used after [close]. That matters more than it looks: the native
 * side tolerates a *null* handle, but nothing can save it from a freed one.
 *
 * Every method is `synchronized`. The native console is internally locked and
 * would be safe to call concurrently, but `close` racing a `tick` would not be,
 * and one lock here removes the whole question. The cost is bounded — a frame of
 * a 128×128 machine is microseconds, so a UI thread asking [isPaused] never
 * waits long enough to notice.
 */
class KesselVm : AutoCloseable {

    private var handle: Long = KesselNative.playerNew()

    /** Screen edge length; the framebuffer is [screenDim]² pixels. */
    val screenDim: Int = KesselNative.screenDim()

    /**
     * The frame staging buffer. Direct, so the native side can write into it
     * without a copy, and allocated once rather than per frame — sixty 64 KiB
     * allocations a second is a GC problem nobody needs.
     */
    private val frame: ByteBuffer =
        ByteBuffer.allocateDirect(screenDim * screenDim * 4)

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

    /** Free the console. Idempotent; every method is a no-op afterwards. */
    @Synchronized
    override fun close() {
        if (handle != 0L) {
            KesselNative.playerFree(handle)
            handle = 0L
        }
    }
}
