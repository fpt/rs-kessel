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
    /**
     * Labels for the four direction bits when the game uses them as plain
     * buttons — the pop'n-music shape. Non-null here means [dirLayout] is
     * [DirLayout.BUTTONS].
     */
    val left: String? = null,
    val right: String? = null,
    val up: String? = null,
    val down: String? = null,
    /** What the analog stick does, or null when the game never reads it. */
    val stick: String? = null,
    /** What touching the screen does, or null when the game never reads it. */
    val touch: String? = null,
    /** Physical button that pauses, by name (e.g. `"START"`). */
    val pause: String = "START",
) {
    /** The gamepad bit that pauses, or 0 if the ROM names no known button. */
    val pauseBit: Int get() = Buttons.byName(pause)

    /**
     * How to present the four direction bits.
     *
     * The VM decides this and sends it as `dir_layout`, rather than each host
     * re-deriving it from `dpad` plus four labels — three hosts guessing
     * separately is three chances to disagree about one ROM.
     */
    val dirLayout: DirLayout
        get() = when {
            directionButtons.isNotEmpty() -> DirLayout.BUTTONS
            dpad -> DirLayout.DPAD
            else -> DirLayout.NONE
        }

    /**
     * The labelled direction bits, in pad order, as (bit, label) pairs. Empty
     * unless the ROM labelled at least one.
     */
    val directionButtons: List<Pair<Int, String>>
        get() = buildList {
            left?.let { add(Buttons.LEFT to it) }
            down?.let { add(Buttons.DOWN to it) }
            up?.let { add(Buttons.UP to it) }
            right?.let { add(Buttons.RIGHT to it) }
        }

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
                left = o.label("left"),
                right = o.label("right"),
                up = o.label("up"),
                down = o.label("down"),
                stick = o.label("stick"),
                touch = o.label("touch"),
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

/** How a host should present the four direction bits. Mirrors `luax::DirLayout`. */
enum class DirLayout {
    /** A d-pad — the default, and what every directional game gets. */
    DPAD,

    /** Four plain buttons in a row, each with its own label. */
    BUTTONS,

    /** The game ignores those bits; draw nothing. */
    NONE,
}
