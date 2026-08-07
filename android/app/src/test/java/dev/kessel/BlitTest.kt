package dev.kessel

import dev.kessel.game.ScreenRect
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
