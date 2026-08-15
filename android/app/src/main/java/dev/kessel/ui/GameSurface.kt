package dev.kessel.ui

import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.viewinterop.AndroidView
import dev.kessel.game.GameEngine
import dev.kessel.game.TouchTracker

/**
 * The console's screen: a [SurfaceView] the game thread draws into directly.
 *
 * This is the one piece of the app that is deliberately *not* Compose. The
 * engine produces a frame every 16 ms on its own thread, and a `Surface` is the
 * only handoff on Android where the producer cannot scribble on a buffer the
 * consumer is still reading — `lockCanvas` will not hand back a buffer that is
 * in flight. Publishing `ImageBitmap`s into Compose state instead would mean
 * either a 64 KiB allocation per frame or a shared bitmap with no proof of when
 * the compositor is done with it.
 *
 * It also keeps sixty frames a second from turning into sixty recompositions a
 * second: only pause/halt changes reach Compose at all.
 *
 * When [touchable] the surface also reports fingers to the game, in console
 * pixels. The pointer handling is a Compose overlay rather than an
 * `OnTouchListener` on the view: Compose is already resolving pointers for the
 * pad below, and one input model for both halves is what keeps a finger on the
 * screen and a thumb on a button from fighting over the same gesture.
 */
@Composable
fun GameSurface(
    engine: GameEngine,
    contentDescription: String,
    modifier: Modifier = Modifier,
    touchable: Boolean = false,
    screenDim: Int = 0,
) {
    Box(modifier) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { context ->
                SurfaceView(context).apply {
                    this.contentDescription = contentDescription
                    holder.addCallback(object : SurfaceHolder.Callback {
                        override fun surfaceCreated(h: SurfaceHolder) = engine.setSurface(h)

                        override fun surfaceChanged(h: SurfaceHolder, f: Int, w: Int, ht: Int) =
                            engine.setSurface(h)

                        // The system can reclaim the surface at any time.
                        // Clearing it here is what stops the game thread drawing
                        // into a buffer that no longer exists.
                        override fun surfaceDestroyed(h: SurfaceHolder) = engine.setSurface(null)
                    })
                }
            },
        )

        if (touchable && screenDim > 0) {
            // Slot assignment is state that outlives one event — a finger has
            // to keep its slot across every event of its life. See TouchTracker.
            val tracker = remember(screenDim) { TouchTracker() }
            Box(
                Modifier
                    .fillMaxSize()
                    .pointerInput(screenDim) {
                        awaitPointerEventScope {
                            while (true) {
                                val event = awaitPointerEvent(PointerEventPass.Main)
                                tracker.begin(size.width, size.height, screenDim)
                                for (change in event.changes) {
                                    // Lifted pointers are offered too: that is
                                    // what frees their slot for the next finger.
                                    tracker.offer(
                                        change.id.value,
                                        change.position.x,
                                        change.position.y,
                                        change.pressed,
                                    )
                                    if (change.pressed) change.consume()
                                }
                                engine.setTouches(tracker.finish())
                            }
                        }
                    }
            )
        }
    }

    // Leaving the screen tears down the view, but the engine outlives this
    // composable by a moment — drop the reference so the loop cannot draw into
    // a dead surface in between.
    DisposableEffect(engine) {
        onDispose {
            engine.setSurface(null)
            engine.setTouches(null)
        }
    }
}

