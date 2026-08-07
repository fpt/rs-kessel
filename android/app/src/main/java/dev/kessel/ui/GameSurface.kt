package dev.kessel.ui

import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import dev.kessel.game.GameEngine

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
 */
@Composable
fun GameSurface(
    engine: GameEngine,
    contentDescription: String,
    modifier: Modifier = Modifier,
) {
    AndroidView(
        modifier = modifier,
        factory = { context ->
            SurfaceView(context).apply {
                this.contentDescription = contentDescription
                holder.addCallback(object : SurfaceHolder.Callback {
                    override fun surfaceCreated(h: SurfaceHolder) = engine.setSurface(h)

                    override fun surfaceChanged(h: SurfaceHolder, f: Int, w: Int, ht: Int) =
                        engine.setSurface(h)

                    // The system can reclaim the surface at any time. Clearing
                    // it here is what stops the game thread drawing into a
                    // buffer that no longer exists.
                    override fun surfaceDestroyed(h: SurfaceHolder) = engine.setSurface(null)
                })
            }
        },
    )

    // Leaving the screen tears down the view, but the engine outlives this
    // composable by a moment — drop the reference so the loop cannot draw into
    // a dead surface in between.
    DisposableEffect(engine) {
        onDispose { engine.setSurface(null) }
    }
}
