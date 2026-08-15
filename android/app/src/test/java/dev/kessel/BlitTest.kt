package dev.kessel

import dev.kessel.game.ScreenRect
import dev.kessel.game.OFF_SCREEN
import dev.kessel.game.consoleTouch
import dev.kessel.game.destRect
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Where the frame lands on the surface.
 *
 * Tested on its own for the reason the desktop `blit` is: a mistake here draws
 * a plausible-but-wrong picture — smeared, off-centre, subtly non-square —
 * rather than crashing, so nothing else would catch it.
 */
class BlitTest {

    @Test
    fun `scales by a whole number and centres the result`() {
        // 400 wide / 128 = 3.125 -> 3×, so 384 px of picture and 8 px of letterbox
        // on each side. A fractional scale would put uneven pixel sizes side by
        // side, which is exactly what 128×128 art shows up.
        assertEquals(ScreenRect(8, 8, 392, 392), destRect(128, 400, 400))
    }

    @Test
    fun `an exact multiple fills the surface with no letterbox`() {
        assertEquals(ScreenRect(0, 0, 512, 512), destRect(128, 512, 512))
    }

    @Test
    fun `a non-square surface stays square and centres on the long axis`() {
        // The console screen is square; a wide surface must letterbox, not
        // stretch. 1080/128 = 8, 640/128 = 5 -> 5× = 640.
        val r = destRect(128, 1080, 640)
        assertEquals(640, r.width)
        assertEquals(640, r.height)
        assertEquals(0, r.top)
        assertEquals((1080 - 640) / 2, r.left)
    }

    @Test
    fun `a surface smaller than one console pixel per pixel still draws`() {
        // Below 1× we clamp rather than compute a zero-or-negative scale, and
        // the origin stays non-negative so the draw is in-bounds.
        val r = destRect(128, 64, 64)
        assertEquals(128, r.width)
        assertTrue("origin must not go negative", r.left >= 0 && r.top >= 0)
    }

    @Test
    fun `a touch unprojects to the console pixel under the finger`() {
        // 512-wide surface, 128 console: 4×, no letterbox horizontally.
        assertEquals(2 to 3, unpack(consoleTouch(9f, 13f, 512, 512, 128)))
        assertEquals(0 to 0, unpack(consoleTouch(0f, 0f, 512, 512, 128)))
        assertEquals(127 to 127, unpack(consoleTouch(511f, 511f, 512, 512, 128)))
    }

    @Test
    fun `a touch is unprojected through the letterbox, not around it`() {
        // 1080×640 for a 128 console: 5× = 640, so 220 px of margin each side.
        // The exact inverse of `destRect`, which is the point.
        val r = destRect(128, 1080, 640)
        assertEquals(220, r.left)
        assertEquals(0 to 0, unpack(consoleTouch(220f, 0f, 1080, 640, 128)))
        assertEquals(1 to 0, unpack(consoleTouch(225f, 0f, 1080, 640, 128)))
        // Inside the margin is not the screen. Clamping instead would make a
        // game react to a tap that missed it.
        assertEquals(OFF_SCREEN, consoleTouch(219f, 0f, 1080, 640, 128))
        assertEquals(OFF_SCREEN, consoleTouch(861f, 0f, 1080, 640, 128))
    }

    @Test
    fun `a finger dragged off the view is off screen, not wrapped inside it`() {
        // Compose reports negative coordinates during a drag past the edge, and
        // an unsigned-looking conversion would land them back in the picture.
        assertEquals(OFF_SCREEN, consoleTouch(-5f, 10f, 512, 512, 128))
        assertEquals(OFF_SCREEN, consoleTouch(10f, -5f, 512, 512, 128))
        assertEquals(OFF_SCREEN, consoleTouch(10f, 10f, 512, 512, 0))
    }

    /** Undo `consoleTouch`'s packing, for readable assertions. */
    private fun unpack(packed: Int): Pair<Int, Int> =
        if (packed == OFF_SCREEN) -1 to -1 else (packed shr 16) to (packed and 0xFFFF)

    @Test
    fun `degenerate inputs produce an empty rect rather than an exception`() {
        // A surface can be reported at zero size while it is being torn down.
        for (r in listOf(
            destRect(0, 400, 400),
            destRect(128, 0, 400),
            destRect(128, 400, 0),
            destRect(-1, 400, 400),
        )) {
            assertTrue("expected empty, got $r", r.isEmpty)
        }
    }
}
