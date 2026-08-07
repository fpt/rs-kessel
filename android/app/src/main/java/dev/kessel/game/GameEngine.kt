package dev.kessel.game

import android.graphics.Bitmap
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import dev.kessel.vm.Controls
import dev.kessel.vm.KesselVm
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.locks.LockSupport

/** What the UI needs to know about the running game. */
data class PlayState(
    val frame: ImageBitmap? = null,
    val controls: Controls = Controls(),
    val paused: Boolean = false,
    /** The machine halted or faulted — game over, or a crash. */
    val halted: Boolean = false,
    /** Compiler diagnostics; non-null means the game never started. */
    val error: String? = null,
)

/**
 * Runs one game at 60 Hz on its own thread.
 *
 * **The loop is not on the main thread**, for the same reason `kessel attach`
 * ticks off the UI thread: a frame is the VM running arbitrary game code, and a
 * game that takes 40 ms should drop a frame, not jank the whole app. The UI
 * only ever reads [state].
 *
 * Frames are double-buffered. The loop renders into whichever bitmap Compose is
 * *not* currently showing and then publishes it; mutating a single bitmap under
 * the compositor would tear. Two 128×128 bitmaps is 128 KiB, which is nothing
 * next to being wrong.
 */
class GameEngine(private val vm: KesselVm) : AutoCloseable {

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

    private var thread: Thread? = null

    private val bitmaps = Array(2) {
        Bitmap.createBitmap(vm.screenDim, vm.screenDim, Bitmap.Config.ARGB_8888)
    }
    private var back = 0

    /** Buttons currently held. Called from the UI thread on every touch. */
    fun setButtons(mask: Int) {
        buttons = mask
    }

    /** Press [mask] for exactly one frame, whatever the fingers are doing. */
    fun pulse(mask: Int) {
        pulse = pulse or mask
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
        _state.value = PlayState(controls = vm.controls())

        running = true
        thread = Thread({ loop() }, "kessel-game").apply {
            // Above default, below the UI: frames should keep pace under load,
            // but never at the cost of the touch handling that feeds them.
            priority = Thread.NORM_PRIORITY + 1
            start()
        }
    }

    /** Stop the loop and wait for it. Safe to call twice. */
    fun stop() {
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
     * [stop] comes first so the loop is not mid-tick, though the ordering is
     * belt-and-braces — [KesselVm] serialises `close` against `tick` and turns a
     * late tick into a no-op rather than a use-after-free.
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
            publishFrame()

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

    private fun publishFrame() {
        val target = bitmaps[back]
        if (!vm.readFrame(target)) return
        back = 1 - back
        _state.value = _state.value.copy(
            frame = target.asImageBitmap(),
            paused = vm.isPaused(),
            halted = vm.isHalted(),
        )
    }

    private companion object {
        /** The console is defined at 60 Hz — games assume it for their timing. */
        const val FRAME_NANOS = 1_000_000_000L / 60

        /** Frames of debt past which we stop trying to catch up. */
        const val MAX_LAG_FRAMES = 4
    }
}
