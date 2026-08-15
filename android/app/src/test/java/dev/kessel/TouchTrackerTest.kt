package dev.kessel

import dev.kessel.game.TouchTracker
import dev.kessel.vm.MAX_TOUCHES
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * A finger keeps its slot for its whole life.
 *
 * This is the contract the VM's per-slot `touch_pressed`/`touch_released` edges
 * are computed against, and breaking it does not crash: it produces a game that
 * is perfect with one finger and wrong only during a particular two-finger
 * sequence. So the sequences are spelled out here rather than left to a device.
 *
 * A 128 console on a 512×512 view is a clean 4×, so a surface coordinate is
 * simply a quarter of itself in console pixels.
 */
class TouchTrackerTest {

    private val view = 512
    private val dim = 128

    /** Run one pointer event; each triple is (id, x, y, pressed). */
    private fun event(
        tracker: TouchTracker,
        vararg pointers: Triple<Long, Pair<Float, Float>, Boolean>,
    ): IntArray? {
        tracker.begin(view, view, dim)
        for ((id, pos, pressed) in pointers) {
            tracker.offer(id, pos.first, pos.second, pressed)
        }
        return tracker.finish()
    }

    private fun down(id: Long, x: Float, y: Float) = Triple(id, x to y, true)
    private fun up(id: Long, x: Float, y: Float) = Triple(id, x to y, false)

    /** The (x, y, down) triple a slot reports. */
    private fun slot(a: IntArray, i: Int) =
        Triple(a[i * 3], a[i * 3 + 1], a[i * 3 + 2])

    @Test
    fun `a finger lands in slot zero and reports console pixels`() {
        val t = TouchTracker()
        val a = event(t, down(1L, 40f, 80f))!!
        assertEquals(Triple(10, 20, 1), slot(a, 0))
        assertEquals(Triple(0, 0, 0), slot(a, 1))
    }

    @Test
    fun `lifting the first finger does not renumber the second`() {
        // The bug this class exists to prevent. Two fingers, drop the first:
        // the second must stay in slot 1, or the VM sees slot 0 teleport and
        // slot 1 release — neither of which the player did.
        val t = TouchTracker()
        val both = event(t, down(1L, 40f, 40f), down(2L, 80f, 80f))!!
        assertEquals(Triple(10, 10, 1), slot(both, 0))
        assertEquals(Triple(20, 20, 1), slot(both, 1))

        val after = event(t, up(1L, 40f, 40f), down(2L, 80f, 80f))!!
        assertEquals("slot 0 must go empty, not inherit finger 2", Triple(0, 0, 0), slot(after, 0))
        assertEquals("finger 2 must keep slot 1", Triple(20, 20, 1), slot(after, 1))
    }

    @Test
    fun `a finger over the letterbox keeps its slot instead of yielding it`() {
        // 1080×640 for a 128 console letterboxes 220 px each side. A finger
        // that wanders into the margin reports up — but its slot stays reserved
        // so the finger behind it does not slide into it.
        val t = TouchTracker()
        t.begin(1080, 640, dim)
        t.offer(1L, 300f, 100f, true) // on screen
        t.offer(2L, 400f, 100f, true) // on screen
        val both = t.finish()!!
        assertEquals(1, slot(both, 0).third)
        assertEquals(1, slot(both, 1).third)

        t.begin(1080, 640, dim)
        t.offer(1L, 50f, 100f, true) // now in the left margin
        t.offer(2L, 400f, 100f, true)
        val after = t.finish()!!
        assertEquals("an off-screen finger reports up", 0, slot(after, 0).third)
        assertEquals("...and finger 2 stays put", 1, slot(after, 1).third)
        assertEquals(slot(both, 1), slot(after, 1))
    }

    @Test
    fun `a slot is reused only after its finger is gone`() {
        val t = TouchTracker()
        event(t, down(1L, 40f, 40f))
        event(t, up(1L, 40f, 40f))
        // Slot 0 is free again, so the next finger may have it.
        val a = event(t, down(9L, 200f, 200f))!!
        assertEquals(Triple(50, 50, 1), slot(a, 0))
    }

    @Test
    fun `every finger down at once fills every slot in order`() {
        val t = TouchTracker()
        t.begin(view, view, dim)
        for (i in 0 until MAX_TOUCHES) {
            t.offer(i.toLong(), (i * 40).toFloat(), 0f, true)
        }
        val a = t.finish()!!
        for (i in 0 until MAX_TOUCHES) {
            assertEquals("slot $i", Triple(i * 10, 0, 1), slot(a, i))
        }
    }

    @Test
    fun `a fifth finger is dropped rather than evicting one the game is tracking`() {
        val t = TouchTracker()
        t.begin(view, view, dim)
        for (i in 0 until MAX_TOUCHES) {
            t.offer(i.toLong(), (i * 40).toFloat(), 0f, true)
        }
        t.offer(99L, 400f, 400f, true)
        val a = t.finish()!!
        assertEquals(MAX_TOUCHES * 3, a.size)
        for (i in 0 until MAX_TOUCHES) {
            assertEquals("slot $i kept its finger", Triple(i * 10, 0, 1), slot(a, i))
        }
    }

    @Test
    fun `no fingers means null, so the engine clears rather than holding a stale set`() {
        val t = TouchTracker()
        assertNull(event(t))
        event(t, down(1L, 40f, 40f))
        assertNull("the last lift must clear", event(t, up(1L, 40f, 40f)))
        // Every finger in the letterbox is also "nothing on the screen".
        t.begin(1080, 640, dim)
        t.offer(1L, 10f, 10f, true)
        assertNull(t.finish())
    }

    @Test
    fun `each event returns its own array`() {
        // The engine publishes what it is handed to the game thread. Returning
        // the buffer this tracker keeps rewriting would let that thread read a
        // half-updated gesture.
        val t = TouchTracker()
        val first = event(t, down(1L, 40f, 40f))!!
        val snapshot = first.copyOf()
        event(t, down(1L, 200f, 200f))
        assertArrayEquals("the published array must not change underneath", snapshot, first)
    }
}
