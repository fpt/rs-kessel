package dev.kessel.game

import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Rect
import android.view.SurfaceHolder
import dev.kessel.vm.Controls
import dev.kessel.vm.KesselVm
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.locks.LockSupport

/**
 * What the UI needs to know about the running game.
 *
 * Note what is *not* here: the frame. Pixels go straight to a `Surface` from the
 * game thread and never enter the Compose world — see [render]. Publishing an
 * image sixty times a second would also recompose this screen sixty times a
 * second, to show something Compose is not the right tool to show.
 */
data class PlayState(
    val controls: Controls = Controls(),
    val paused: Boolean = false,
    /** The machine halted or faulted — game over, or a crash. */
    val halted: Boolean = false,
    /** Compiler diagnostics; non-null means the game never started. */
    val error: String? = null,
)

/**
 * Runs one game at 60 Hz on its own thread, drawing straight to a `Surface`.
 *
 * **The loop is not on the main thread**, for the same reason `kessel attach`
 * ticks off the UI thread: a frame is the VM running arbitrary game code, and a
 * game that takes 40 ms should drop a frame, not jank the app.
 *
 * **Frames are handed over through the surface, not through shared memory.**
 * An earlier version published alternating `Bitmap`s into a `StateFlow` and
 * assumed Compose would be finished with one before the loop came back around
 * to reuse it — an assumption nothing enforced, so a slow UI thread could be
 * reading a bitmap while this thread rewrote it. `lockCanvas` /
 * `unlockCanvasAndPost` is the acknowledgement protocol that was missing: the
 * consumer cannot be handed a buffer this thread still owns. The single
 * [scratch] bitmap below is safe precisely because nothing outside this class
 * ever sees it.
 */
class GameEngine(private val vm: KesselVm) : AutoCloseable {

    /**
     * Sound, on its own thread.
     *
     * Not driven from [loop]: the render call is the one that does not take the
     * console's lock, so keeping it off this thread is what stops a slow frame
     * from becoming a click. See [AudioPlayer].
     */
    private val audio = AudioPlayer(vm)

    private val _state = MutableStateFlow(PlayState())
    val state: StateFlow<PlayState> = _state.asStateFlow()

    /** Written by the UI thread, read by the game loop; hence volatile. */
    @Volatile
    private var buttons: Int = 0

    /**
     * A one-frame button pulse, for taps that must not depend on how long a
     * finger stayed down — the pause toggle. Consumed by the next tick.
     */
    @Volatile
    private var pulse: Int = 0

    @Volatile
    private var running = false

    /** The surface to draw on, or null while the view is away. */
    @Volatile
    private var holder: SurfaceHolder? = null

    private var thread: Thread? = null

    /**
     * The game thread's private staging bitmap. Never published, never touched
     * by another thread — `drawBitmap` copies out of it into the locked canvas
     * before the frame is posted.
     *
     * Created in [start] rather than here: the ROM's `screen { … }` block
     * decides the resolution, so there is nothing to size this from until a
     * game has been loaded.
     */
    private var scratch: Bitmap? = null

    /** Nearest-neighbour: smoothing a 128×128 image is not nicer, just blurrier. */
    private val paint = Paint().apply {
        isFilterBitmap = false
        isAntiAlias = false
        isDither = false
    }

    private val src = Rect()

    /** Reused so the 60 Hz path allocates nothing; only the game thread sees it. */
    private val dst = Rect()

    /** Buttons currently held. Called from the UI thread on every touch. */
    fun setButtons(mask: Int) {
        buttons = mask
    }

    /** Press [mask] for exactly one frame, whatever the fingers are doing. */
    fun pulse(mask: Int) {
        pulse = pulse or mask
    }

    /**
     * Point the loop at a surface, or pass null when the view goes away.
     *
     * Safe at any time: the loop re-reads this every frame and simply skips
     * drawing when there is nothing to draw on.
     */
    fun setSurface(target: SurfaceHolder?) {
        holder = target
    }

    /**
     * Compile [source] and start the loop. Diagnostics land in [state] rather
     * than being thrown: a game that won't compile is something to show the
     * player, not a crash.
     */
    fun start(source: String, name: String) {
        check(thread == null) { "engine already started" }

        val error = vm.load(source, name)
        if (error != null) {
            _state.value = PlayState(error = error)
            return
        }
        // Only now is the resolution known.
        val dim = vm.screenDim()
        scratch = Bitmap.createBitmap(dim, dim, Bitmap.Config.ARGB_8888)
        src.set(0, 0, dim, dim)
        _state.value = PlayState(controls = vm.controls())

        // After the load, because the synth needs the ROM's instruments — and
        // before the game thread, so the first frame's sound has somewhere to
        // go. A device with no audio simply plays the game silently.
        audio.start()

        running = true
        thread = Thread({ loop() }, "kessel-game").apply {
            // Above default, below the UI: frames should keep pace under load,
            // but never at the cost of the touch handling that feeds them.
            priority = Thread.NORM_PRIORITY + 1
            start()
        }
    }

    /**
     * Stop the loop and wait for it. Safe to call twice.
     *
     * The audio thread goes first and is joined, because it is the one caller
     * that reaches the console without the lock guarding `close` — see
     * [dev.kessel.vm.KesselVm.renderAudio].
     */
    fun stop() {
        audio.stop()
        running = false
        thread?.let {
            it.interrupt()
            it.join(FRAME_NANOS / 1_000_000 * 10)
        }
        thread = null
    }

    /**
     * Stop the loop and free the native console. Safe to call twice.
     *
     * Not optional: the console lives in native memory, which no GC will
     * reclaim, so an engine that is merely stopped leaks a whole `VmConsole`
     * every time the player backs out of a game.
     *
     * [stop] comes first so the loop is not mid-tick. For the game thread that
     * is belt-and-braces — [KesselVm] serialises `close` against `tick` and
     * turns a late tick into a no-op. For the *audio* thread it is not: that
     * one renders without the lock, so joining it before the console is freed
     * is the only thing making it safe.
     */
    override fun close() {
        stop()
        vm.close()
    }

    private fun loop() {
        var deadline = System.nanoTime()
        while (running) {
            val held = buttons or consumePulse()
            vm.tick(held)
            render()
            publishState()

            deadline += FRAME_NANOS
            val remaining = deadline - System.nanoTime()
            if (remaining > 0) {
                LockSupport.parkNanos(remaining)
            } else if (-remaining > FRAME_NANOS * MAX_LAG_FRAMES) {
                // Far enough behind that catching up would fast-forward the game
                // in the player's face. Drop the debt and resume at real time.
                deadline = System.nanoTime()
            }
        }
    }

    /** Take the pending pulse and clear it, so it lasts exactly one frame. */
    private fun consumePulse(): Int {
        val p = pulse
        if (p != 0) pulse = 0
        return p
    }

    /**
     * Draw the current frame, if there is one and somewhere to put it.
     *
     * Everything here is best-effort: a surface can be torn down by the system
     * at any moment, and the honest response to that is to skip a frame, not to
     * take the app down.
     */
    private fun render() {
        val h = holder ?: return
        val bmp = scratch ?: return
        if (!vm.readFrame(bmp)) return

        val canvas = try {
            h.lockCanvas() ?: return
        } catch (_: IllegalStateException) {
            return // Surface went away between the null check and the lock.
        }
        try {
            // Letterbox rather than leave whatever the previous owner of this
            // buffer left behind.
            canvas.drawColor(Color.BLACK)
            val r = destRect(bmp.width, canvas.width, canvas.height)
            if (!r.isEmpty) {
                dst.set(r.left, r.top, r.right, r.bottom)
                canvas.drawBitmap(bmp, src, dst, paint)
            }
        } finally {
            try {
                h.unlockCanvasAndPost(canvas)
            } catch (_: IllegalStateException) {
                // Surface died mid-frame; the next loop sees holder == null.
            }
        }
    }

    /**
     * Push pause/halt changes to Compose — but only on a change.
     *
     * `MutableStateFlow` conflates equal values, so assigning an unchanged
     * `PlayState` every frame would be harmless; building one to throw away
     * sixty times a second would not be.
     */
    private fun publishState() {
        val paused = vm.isPaused()
        val halted = vm.isHalted()
        val current = _state.value
        if (paused != current.paused || halted != current.halted) {
            _state.value = current.copy(paused = paused, halted = halted)
        }
    }

    private companion object {
        /** The console is defined at 60 Hz — games assume it for their timing. */
        const val FRAME_NANOS = 1_000_000_000L / 60

        /** Frames of debt past which we stop trying to catch up. */
        const val MAX_LAG_FRAMES = 4
    }
}
