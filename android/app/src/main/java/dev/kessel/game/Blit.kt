package dev.kessel.game

/**
 * A destination rectangle in surface pixels.
 *
 * Deliberately *not* `android.graphics.Rect`: that class is a stub on the unit
 * test classpath that throws on every call, so using it here would make the one
 * piece of geometry in this app the one piece that cannot be tested off-device.
 */
data class ScreenRect(val left: Int, val top: Int, val right: Int, val bottom: Int) {
    val width: Int get() = right - left
    val height: Int get() = bottom - top
    val isEmpty: Boolean get() = width <= 0 || height <= 0
}

/**
 * Where a `dim`×`dim` console frame lands on a `width`×`height` surface.
 *
 * Integer upscale, centred, never below 1× — the same rule as `blit` in
 * `crates/cli/src/play.rs`, so a game looks identical on a phone and in the
 * desktop window. A fractional scale would put uneven pixel sizes next to each
 * other, which on 128×128 pixel art is immediately visible.
 *
 * Pulled out as a pure function for the reason the desktop `blit` is tested
 * separately: getting this wrong produces a plausible-but-wrong picture rather
 * than a crash, so nothing else would catch it.
 */
fun destRect(dim: Int, width: Int, height: Int): ScreenRect {
    if (dim <= 0 || width <= 0 || height <= 0) return ScreenRect(0, 0, 0, 0)

    val scale = maxOf(1, minOf(width / dim, height / dim))
    val side = dim * scale
    // A surface smaller than the frame still gets its top-left corner rather
    // than a negative offset.
    val left = maxOf(0, (width - side) / 2)
    val top = maxOf(0, (height - side) / 2)
    return ScreenRect(left, top, left + side, top + side)
}

/** [consoleTouch]'s answer for a point that is not on the drawn frame. */
const val OFF_SCREEN = -1

/**
 * Undo [destRect]: a point in surface pixels as one in console pixels, packed
 * `(x shl 16) or y`, or [OFF_SCREEN] when it lands in the letterbox.
 *
 * The exact inverse of the blit, which is why it lives beside it and is tested
 * with it. A margin left out here does not crash: it puts the game's cursor a
 * few pixels away from the player's finger, on every phone whose aspect ratio
 * differs from the one it was tried on.
 *
 * Packed into an Int rather than returning a pair, because this runs inside a
 * pointer loop and a per-finger allocation there is exactly the kind of garbage
 * that shows up as a stutter mid-gesture.
 */
fun consoleTouch(x: Float, y: Float, width: Int, height: Int, dim: Int): Int {
    val r = destRect(dim, width, height)
    if (r.isEmpty) return OFF_SCREEN
    val scale = maxOf(1, r.width / dim)
    // Compose reports pointers outside the view during a drag, and a negative
    // coordinate divided into the frame would land back inside it.
    if (x < r.left || y < r.top) return OFF_SCREEN
    val cx = (x.toInt() - r.left) / scale
    val cy = (y.toInt() - r.top) / scale
    return if (cx < dim && cy < dim) (cx shl 16) or cy else OFF_SCREEN
}
