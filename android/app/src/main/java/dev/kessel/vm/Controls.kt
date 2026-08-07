package dev.kessel.vm

import org.json.JSONObject

/**
 * What a ROM says about its own controls, from its `controls { … }` block.
 *
 * The point is an on-screen pad that shows only the buttons that do something,
 * labelled with what they do — `bounce.lua` declares `dpad = false` and gets no
 * d-pad; `tetris.lua` labels A "rotate cw" and B "rotate ccw". A fixed eight-
 * button pad would be lying to the player on most games.
 *
 * Mirrors `luax::Controls` in `crates/vm/src/luax.rs`.
 */
data class Controls(
    /** Whether the game reads the d-pad at all. */
    val dpad: Boolean = true,
    /** Action labels; null means the game never reads that button. */
    val a: String? = null,
    val b: String? = null,
    val start: String? = null,
    val select: String? = null,
    /** Physical button that pauses, by name (e.g. `"START"`). */
    val pause: String = "START",
) {
    /** The gamepad bit that pauses, or 0 if the ROM names no known button. */
    val pauseBit: Int get() = Buttons.byName(pause)

    /**
     * The action buttons to draw, in pad order, as (bit, label) pairs.
     *
     * A button used *only* to pause is excluded: the pause control lives in the
     * top bar, and drawing it twice would suggest two different things.
     */
    val actionButtons: List<Pair<Int, String>>
        get() = buildList {
            a?.let { add(Buttons.A to it) }
            b?.let { add(Buttons.B to it) }
            start?.let { add(Buttons.START to it) }
            select?.let { add(Buttons.SELECT to it) }
        }.filter { (bit, _) -> bit != pauseBit }

    companion object {
        /**
         * Parse the JSON from `KesselNative.playerControlsJson`.
         *
         * Falls back to defaults on anything unexpected. The metadata only
         * decides how the pad is drawn, so a parse failure should cost the
         * player some labels, never the game.
         */
        fun parse(json: String): Controls = try {
            val o = JSONObject(json)
            Controls(
                dpad = o.optBoolean("dpad", true),
                a = o.label("a"),
                b = o.label("b"),
                start = o.label("start"),
                select = o.label("select"),
                pause = o.optString("pause", "START").ifBlank { "START" },
            )
        } catch (_: Exception) {
            Controls()
        }

        /** A label, or null for JSON null / absent / empty. */
        private fun JSONObject.label(key: String): String? =
            if (isNull(key)) null else optString(key).takeIf { it.isNotBlank() }
    }
}
