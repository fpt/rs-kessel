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
