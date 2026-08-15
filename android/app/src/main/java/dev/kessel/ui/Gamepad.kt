package dev.kessel.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import android.view.HapticFeedbackConstants
import dev.kessel.vm.Buttons
import dev.kessel.vm.Controls
import dev.kessel.vm.DirLayout
import dev.kessel.vm.STICK_FULL

/**
 * The on-screen pad, drawn from the ROM's own control metadata.
 *
 * Three things drive the design:
 *
 * **Only real controls appear.** `bounce.lua` declares `dpad = false` and gets
 * no d-pad; `tetris.lua` labels A "rotate cw" and B "rotate ccw" and gets both,
 * captioned. A fixed eight-button pad would be advertising controls that do
 * nothing on most of the library.
 *
 * **The direction bits are whatever the ROM says they are.** `dir_layout` picks
 * between a d-pad, a row of labelled buttons (`popn.lua` — nothing on that pad
 * means "up"), and nothing at all. The four bits are the same four bits either
 * way; only the picture changes.
 *
 * **Hit testing is by geometry, not by child views.** One [pointerInput] over
 * the whole pad resolves every active pointer against the button rectangles and
 * ORs the bits. That is what buys real multi-touch — holding A while steering,
 * six fingers on a six-button row, and sliding a thumb from ⬅ to ⬆ without
 * lifting — which per-button `clickable` modifiers cannot express.
 */
@Composable
fun Gamepad(
    controls: Controls,
    modifier: Modifier = Modifier,
    onButtons: (Int) -> Unit,
    onStick: (Int, Int) -> Unit = { _, _ -> },
) {
    var pressed by remember { mutableIntStateOf(0) }
    // The stick knob's offset from its centre, in pixels, or null when nothing
    // is on it. Drawn from this, so what the player sees is the deflection the
    // game was sent.
    var knob by remember { mutableStateOf<Offset?>(null) }
    val textMeasurer = rememberTextMeasurer()
    val view = LocalView.current

    BoxWithConstraints(modifier) {
        val density = LocalDensity.current
        val size = with(density) { Size(maxWidth.toPx(), maxHeight.toPx()) }
        val layout = remember(size, controls) { PadLayout.compute(size, density, controls) }

        Canvas(
            Modifier
                .fillMaxSize()
                .pointerInput(layout) {
                    awaitPointerEventScope {
                        while (true) {
                            // Main pass: the pad is the leaf here, and claiming
                            // pointers early keeps a vertical scroll gesture
                            // from stealing a thumb mid-game.
                            val event = awaitPointerEvent(PointerEventPass.Main)
                            var mask = 0
                            var onStickNow: Offset? = null
                            for (change in event.changes) {
                                if (change.pressed) {
                                    mask = mask or layout.hit(change.position)
                                    // A thumb that started on the stick keeps
                                    // steering after it slides off — lifting the
                                    // input the moment you overshoot the circle
                                    // is what makes a virtual stick feel broken.
                                    layout.stickOffset(change.position)?.let {
                                        onStickNow = it
                                    }
                                    change.consume()
                                }
                            }
                            if (onStickNow != knob) {
                                knob = onStickNow
                                val (sx, sy) = layout.deflection(onStickNow)
                                onStick(sx, sy)
                            }
                            if (mask != pressed) {
                                // Buzz on press only — a tick on every release
                                // turns a held direction into a stutter.
                                if (mask and pressed.inv() != 0) {
                                    view.performHapticFeedback(
                                        HapticFeedbackConstants.VIRTUAL_KEY
                                    )
                                }
                                pressed = mask
                                onButtons(mask)
                            }
                        }
                    }
                }
        ) {
            layout.draw(this, textMeasurer, pressed, knob)
        }
    }
}

/**
 * Where every button sits, in pixels, for the current pad size.
 *
 * Computed once per size/ROM and then used for both drawing and hit testing —
 * one source of geometry, so what the player sees is exactly what responds.
 */
private class PadLayout(
    /** The d-pad's bounding box, or null when the game ignores direction. */
    val dpad: Rect?,
    val actions: List<ActionButton>,
    /** The analog stick's well, or null when the game never reads it. */
    val stick: Stick?,
    val unit: Float,
) {
    class ActionButton(
        val bit: Int,
        val label: String,
        val center: Offset,
        val radius: Float,
        /** How much width the caption may use before wrapping — the column
         *  this button owns, so two captions can never collide. */
        val captionWidth: Float,
        /** What to write inside the circle. The gamepad letter for an action
         *  button; nothing for a labelled direction key, whose colour name is
         *  already the caption and whose bit means nothing to the player. */
        val glyph: String,
    )

    /** The virtual thumbstick: a well the knob travels inside. */
    class Stick(val center: Offset, val radius: Float, val label: String)

    /**
     * The buttons under [p].
     *
     * The d-pad resolves through a 3×3 grid over its box, so the corner cells
     * report two directions at once. Games in this library steer on diagonals —
     * `outrun`, `platform`, `rogue` — and a pad that can only report one axis at
     * a time makes them feel broken.
     */
    fun hit(p: Offset): Int {
        var mask = 0
        dpad?.let { d ->
            if (d.contains(p)) {
                val col = (((p.x - d.left) / d.width) * 3).toInt().coerceIn(0, 2)
                val row = (((p.y - d.top) / d.height) * 3).toInt().coerceIn(0, 2)
                if (col == 0) mask = mask or Buttons.LEFT
                if (col == 2) mask = mask or Buttons.RIGHT
                if (row == 0) mask = mask or Buttons.UP
                if (row == 2) mask = mask or Buttons.DOWN
            }
        }
        for (a in actions) {
            // Slightly generous: a thumb that lands just off a round button
            // meant to press it.
            if ((p - a.center).getDistance() <= a.radius * 1.15f) mask = mask or a.bit
        }
        return mask
    }

    /**
     * Where [p] sits relative to the stick's centre, clamped to the well, or
     * null when there is no stick or the pointer is nowhere near it.
     *
     * The catch radius is generous for the same reason the buttons' is, and the
     * *result* is clamped rather than the input rejected: past the edge of the
     * well is full deflection, which is what a physical stick does.
     */
    fun stickOffset(p: Offset): Offset? {
        val s = stick ?: return null
        val d = p - s.center
        val dist = d.getDistance()
        if (dist > s.radius * 1.6f) return null
        return if (dist <= s.radius || dist == 0f) d else d * (s.radius / dist)
    }

    /**
     * A knob offset as signed 8.8 fixed deflection, for the VM. Null (nothing
     * on the stick) is centred — a released stick springs back.
     */
    fun deflection(knob: Offset?): Pair<Int, Int> {
        val s = stick ?: return 0 to 0
        val o = knob ?: return 0 to 0
        fun axis(v: Float) = (v / s.radius * STICK_FULL).toInt().coerceIn(-STICK_FULL, STICK_FULL)
        return axis(o.x) to axis(o.y)
    }

    fun draw(scope: DrawScope, textMeasurer: TextMeasurer, pressed: Int, knob: Offset?) =
        with(scope) {
            dpad?.let { drawDpad(it, pressed) }
            stick?.let { drawStick(it, textMeasurer, knob) }
            for (a in actions) drawAction(a, textMeasurer, pressed and a.bit != 0)
        }

    private fun DrawScope.drawStick(s: Stick, textMeasurer: TextMeasurer, knob: Offset?) {
        drawCircle(PAD_WELL, s.radius, s.center)
        drawCircle(
            if (knob == null) PAD_FACE else PAD_PRESSED,
            s.radius * 0.44f,
            s.center + (knob ?: Offset.Zero),
        )
        val caption = textMeasurer.measure(
            s.label,
            style = TextStyle(
                color = PAD_LABEL,
                fontSize = (s.radius * 0.24f).toSp(),
                textAlign = TextAlign.Center,
            ),
            overflow = TextOverflow.Ellipsis,
            maxLines = 1,
            constraints = Constraints(maxWidth = (s.radius * 2.4f).toInt()),
        )
        drawText(
            caption,
            topLeft = Offset(
                s.center.x - caption.size.width / 2f,
                s.center.y + s.radius + s.radius * 0.12f,
            ),
        )
    }

    private fun DrawScope.drawDpad(d: Rect, pressed: Int) {
        val arm = d.width / 3f
        // A plus sign, as two overlapping bars; the arms tint when held.
        drawRoundedBox(Rect(d.left + arm, d.top, d.left + 2 * arm, d.bottom), PAD_FACE)
        drawRoundedBox(Rect(d.left, d.top + arm, d.right, d.top + 2 * arm), PAD_FACE)

        fun arm(bit: Int, r: Rect) {
            if (pressed and bit != 0) drawRoundedBox(r, PAD_PRESSED)
        }
        arm(Buttons.UP, Rect(d.left + arm, d.top, d.left + 2 * arm, d.top + arm))
        arm(Buttons.DOWN, Rect(d.left + arm, d.bottom - arm, d.left + 2 * arm, d.bottom))
        arm(Buttons.LEFT, Rect(d.left, d.top + arm, d.left + arm, d.top + 2 * arm))
        arm(Buttons.RIGHT, Rect(d.right - arm, d.top + arm, d.right, d.top + 2 * arm))
    }

    private fun DrawScope.drawAction(
        a: ActionButton,
        textMeasurer: TextMeasurer,
        held: Boolean,
    ) {
        drawCircle(if (held) PAD_PRESSED else PAD_FACE, a.radius, a.center)

        // The button's letter, centred. Sizes derive from the button's own
        // radius and go through the DrawScope's density, so they track the pad
        // rather than the system font scale — a player who runs large text
        // wants a readable phone, not a gamepad that has outgrown its screen.
        if (a.glyph.isNotEmpty()) {
            val name = textMeasurer.measure(
                a.glyph,
                TextStyle(color = PAD_TEXT, fontSize = (a.radius * 0.62f).toSp()),
            )
            drawText(
                name,
                topLeft = a.center - Offset(name.size.width / 2f, name.size.height / 2f),
            )
        }

        // What the button does, under it — the whole reason for the metadata.
        // Constrained to its own column and wrapped: "rotate ccw" is wider than
        // the circle it belongs to, and unconstrained it would run into the
        // neighbouring button's caption.
        val caption = textMeasurer.measure(
            a.label,
            style = TextStyle(
                color = PAD_LABEL,
                fontSize = (a.radius * 0.34f).toSp(),
                textAlign = TextAlign.Center,
            ),
            overflow = TextOverflow.Ellipsis,
            maxLines = 2,
            constraints = Constraints(maxWidth = a.captionWidth.toInt()),
        )
        drawText(
            caption,
            topLeft = Offset(
                a.center.x - caption.size.width / 2f,
                a.center.y + a.radius + a.radius * 0.22f,
            ),
        )
    }

    private fun DrawScope.drawRoundedBox(r: Rect, color: Color) =
        drawRoundRect(
            color,
            topLeft = r.topLeft,
            size = r.size,
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(unit * 0.14f),
        )

    companion object {
        private val PAD_FACE = Color(0xFF2C3038)
        private val PAD_PRESSED = Color(0xFF5A9BE8)
        private val PAD_TEXT = Color(0xFFE8EAED)
        private val PAD_LABEL = Color(0xFF9AA0A6)

        /** The stick's well — darker than a button face, so the knob reads as
         *  sitting *in* something rather than on top of it. */
        private val PAD_WELL = Color(0xFF1B1E24)

        fun compute(size: Size, density: Density, controls: Controls): PadLayout {
            val margin = with(density) { 20.dp.toPx() }
            // A row of labelled directions is a *different pad*, not a d-pad
            // with extra buttons: every key gets equal weight across the full
            // width, because on a pop'n-style game none of them is "the main
            // one" the way A is.
            if (controls.dirLayout == DirLayout.BUTTONS) {
                return buttonRow(size, density, controls, margin)
            }

            // The d-pad is sized off the pad's height so it stays thumb-sized on
            // a tall phone and a short one alike.
            val dpadSide = (size.height - margin * 2).coerceAtMost(size.width * 0.45f)
            val well = Rect(
                offset = Offset(margin, (size.height - dpadSide) / 2f),
                size = Size(dpadSide, dpadSide),
            )
            // One control under the left thumb. A ROM that declares both a
            // stick and a d-pad gets the stick: two thumb-sized controls in the
            // same place would make neither usable, and the stick is the one
            // that can express what the d-pad can't.
            val stick = controls.stick?.let { Stick(well.center, well.width / 2f * 0.86f, it) }
            val dpad = if (stick == null && controls.dirLayout == DirLayout.DPAD) well else null

            val unit = dpadSide.takeIf { it > 0f } ?: (size.height - margin * 2)
            val radius = (unit * 0.22f).coerceAtMost(with(density) { 38.dp.toPx() })

            // Right-hand cluster, laid out right-to-left so A — the button every
            // game that has one leans on hardest — always lands under the thumb.
            // Two per row, so four buttons stack instead of running off-screen.
            val actions = controls.actionButtons
            val gap = radius * 2.6f
            val rows = if (actions.size > 2) 2 else 1
            val buttons = actions.mapIndexed { i, (bit, label) ->
                val row = i / 2
                ActionButton(
                    bit = bit,
                    label = label,
                    center = Offset(
                        x = size.width - margin - radius - (i % 2) * gap,
                        y = size.height / 2f + (row - (rows - 1) / 2f) * gap,
                    ),
                    radius = radius,
                    captionWidth = gap,
                    glyph = bit.buttonName(),
                )
            }
            return PadLayout(dpad, buttons, stick, unit)
        }

        /**
         * Every button in one evenly spaced row across the pad — the
         * pop'n-music shape.
         *
         * The radius comes from the *count*, so six keys shrink to fit rather
         * than the last two sliding off the right edge. Labels are the whole
         * point of this layout, so the glyph inside each circle is dropped: on
         * a key called "red", the letter `LEFT` would be noise.
         */
        private fun buttonRow(
            size: Size,
            density: Density,
            controls: Controls,
            margin: Float,
        ): PadLayout {
            val keys = controls.directionButtons + controls.actionButtons
            if (keys.isEmpty()) return PadLayout(null, emptyList(), null, size.height)

            val slot = (size.width - margin * 2) / keys.size
            val radius = minOf(
                slot * 0.42f,
                size.height * 0.30f,
                with(density) { 44.dp.toPx() },
            )
            val buttons = keys.mapIndexed { i, (bit, label) ->
                ActionButton(
                    bit = bit,
                    label = label,
                    center = Offset(margin + slot * (i + 0.5f), size.height * 0.44f),
                    radius = radius,
                    captionWidth = slot,
                    glyph = "",
                )
            }
            return PadLayout(null, buttons, null, size.height)
        }

        private fun Int.buttonName() = when (this) {
            Buttons.A -> "A"
            Buttons.B -> "B"
            Buttons.START -> "ST"
            Buttons.SELECT -> "SE"
            else -> "?"
        }
    }
}
