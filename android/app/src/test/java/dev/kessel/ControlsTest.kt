package dev.kessel

import dev.kessel.vm.Buttons
import dev.kessel.vm.Controls
import dev.kessel.vm.DirLayout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Parsing the ROM's control metadata.
 *
 * Worth testing on its own because a mistake here does not crash — it draws a
 * pad with the wrong buttons on it, which looks like a broken *game*.
 *
 * The JSON strings are the real output of `luax::Controls::to_json`.
 */
class ControlsTest {

    @Test
    fun `parses a tetris-shaped rom`() {
        val c = Controls.parse(
            """{"dpad":true,"a":"rotate cw","b":"rotate ccw","start":null,"select":null,"pause":"START"}"""
        )
        assertTrue(c.dpad)
        assertEquals("rotate cw", c.a)
        assertEquals("rotate ccw", c.b)
        assertNull(c.start)
        assertEquals(Buttons.START, c.pauseBit)
        assertEquals(
            listOf(Buttons.A to "rotate cw", Buttons.B to "rotate ccw"),
            c.actionButtons,
        )
    }

    @Test
    fun `a game that ignores input gets no dpad`() {
        // bounce.lua declares `dpad = false` — it is a self-animating demo.
        val c = Controls.parse("""{"dpad":false,"a":null,"b":null,"start":null,"select":null,"pause":"START"}""")
        assertFalse(c.dpad)
        assertTrue(c.actionButtons.isEmpty())
    }

    @Test
    fun `the pause button is not also drawn on the pad`() {
        // A ROM can label START and also pause with it. The top bar owns pause,
        // so drawing it again on the pad would imply two different actions.
        val c = Controls.parse("""{"dpad":true,"a":"fire","b":null,"start":"menu","select":null,"pause":"START"}""")
        assertEquals(listOf(Buttons.A to "fire"), c.actionButtons)
    }

    @Test
    fun `a rom can pause with a button other than start`() {
        val c = Controls.parse("""{"dpad":true,"a":"fire","b":null,"start":null,"select":null,"pause":"SELECT"}""")
        assertEquals(Buttons.SELECT, c.pauseBit)
        assertEquals(listOf(Buttons.A to "fire"), c.actionButtons)
    }

    @Test
    fun `malformed metadata costs labels, never the game`() {
        for (junk in listOf("", "not json", "[]", "{")) {
            val c = Controls.parse(junk)
            assertEquals(Controls(), c)
            assertEquals(Buttons.START, c.pauseBit)
        }
    }

    @Test
    fun `an unknown pause button yields no bit rather than a wrong one`() {
        val c = Controls.parse("""{"dpad":true,"pause":"TURBO"}""")
        assertEquals(0, c.pauseBit)
        // …and with no pause bit, nothing gets filtered off the pad by accident.
        assertTrue(c.actionButtons.isEmpty())
    }

    @Test
    fun `a labelled direction turns the pad into a button row`() {
        // popn.lua: six coloured keys, nothing meaning "up".
        val c = Controls.parse(
            """{"dpad":false,"dir_layout":"buttons","a":"white","b":"pink","start":null,
               "select":null,"left":"red","right":"yellow","up":"blue","down":"green",
               "stick":null,"touch":null,"pause":"START"}"""
        )
        assertEquals(DirLayout.BUTTONS, c.dirLayout)
        // Left to right as a player sees them, and as popn.lua's `lane_bit`
        // orders its lanes — the pad and the screen have to agree.
        assertEquals(
            listOf(
                Buttons.LEFT to "red",
                Buttons.DOWN to "green",
                Buttons.UP to "blue",
                Buttons.RIGHT to "yellow",
            ),
            c.directionButtons,
        )
        assertEquals(listOf(Buttons.A to "white", Buttons.B to "pink"), c.actionButtons)
    }

    @Test
    fun `dir layout covers the three cases a rom can ask for`() {
        val dpad = Controls.parse("""{"dpad":true,"pause":"START"}""")
        assertEquals(DirLayout.DPAD, dpad.dirLayout)
        assertTrue(dpad.directionButtons.isEmpty())

        val none = Controls.parse("""{"dpad":false,"pause":"START"}""")
        assertEquals(DirLayout.NONE, none.dirLayout)

        // One label is enough — a game may use a single key.
        val one = Controls.parse("""{"dpad":false,"left":"red","pause":"START"}""")
        assertEquals(DirLayout.BUTTONS, one.dirLayout)
        assertEquals(listOf(Buttons.LEFT to "red"), one.directionButtons)
    }

    @Test
    fun `the analog surfaces are opt-in`() {
        val plain = Controls.parse("""{"dpad":true,"pause":"START"}""")
        assertNull(plain.stick)
        assertNull(plain.touch)

        // paint.lua declares both.
        val analog = Controls.parse(
            """{"dpad":false,"stick":"move brush","touch":"draw","a":"clear","pause":"START"}"""
        )
        assertEquals("move brush", analog.stick)
        assertEquals("draw", analog.touch)
    }

    @Test
    fun `button names map to the bits in device rs`() {
        // These must match BTN_* in crates/vm/src/device.rs exactly; a wrong bit
        // here silently presses a different button.
        assertEquals(0x01, Buttons.byName("LEFT"))
        assertEquals(0x02, Buttons.byName("RIGHT"))
        assertEquals(0x04, Buttons.byName("UP"))
        assertEquals(0x08, Buttons.byName("DOWN"))
        assertEquals(0x10, Buttons.byName("A"))
        assertEquals(0x20, Buttons.byName("B"))
        assertEquals(0x40, Buttons.byName("START"))
        assertEquals(0x80, Buttons.byName("SELECT"))
        assertEquals(0, Buttons.byName("nonsense"))
    }
}
