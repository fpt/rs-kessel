package dev.kessel.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
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

/**
 * The on-screen pad, drawn from the ROM's own control metadata.
 *
 * Two things drive the design:
 *
 * **Only real controls appear.** `bounce.lua` declares `dpad = false` and gets
 * no d-pad; `tetris.lua` labels A "rotate cw" and B "rotate ccw" and gets both,
 * captioned. A fixed eight-button pad would be advertising controls that do
 * nothing on most of the library.
 *
 * **Hit testing is by geometry, not by child views.** One [pointerInput] over
 * the whole pad resolves every active pointer against the button rectangles and
 * ORs the bits. That is what buys real multi-touch — holding A while steering,
 * and sliding a thumb from ⬅ to ⬆ without lifting — which per-button
 * `clickable` modifiers cannot express.
 */
@Composable
fun Gamepad(
    controls: Controls,
    modifier: Modifier = Modifier,
    onButtons: (Int) -> Unit,
) {
    var pressed by remember { mutableIntStateOf(0) }
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
                            for (change in event.changes) {
                                if (change.pressed) {
                                    mask = mask or layout.hit(change.position)
                                    change.consume()
                                }
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
            layout.draw(this, textMeasurer, pressed)
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
    )

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

    fun draw(scope: DrawScope, textMeasurer: TextMeasurer, pressed: Int) = with(scope) {
        dpad?.let { drawDpad(it, pressed) }
        for (a in actions) drawAction(a, textMeasurer, pressed and a.bit != 0)
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
        val name = textMeasurer.measure(
            a.bit.buttonName(),
            TextStyle(color = PAD_TEXT, fontSize = (a.radius * 0.62f).toSp()),
        )
        drawText(
            name,
            topLeft = a.center - Offset(name.size.width / 2f, name.size.height / 2f),
        )

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

        fun compute(size: Size, density: Density, controls: Controls): PadLayout {
            val margin = with(density) { 20.dp.toPx() }
            val actions = controls.actionButtons

            // The d-pad is sized off the pad's height so it stays thumb-sized on
            // a tall phone and a short one alike.
            val dpadSide = (size.height - margin * 2).coerceAtMost(size.width * 0.45f)
            val dpad = if (controls.dpad) {
                Rect(
                    offset = Offset(margin, (size.height - dpadSide) / 2f),
                    size = Size(dpadSide, dpadSide),
                )
            } else {
                null
            }

            val unit = dpadSide.takeIf { it > 0f } ?: (size.height - margin * 2)
            val radius = (unit * 0.22f).coerceAtMost(with(density) { 38.dp.toPx() })

            // Right-hand cluster, laid out right-to-left so A — the button every
            // game that has one leans on hardest — always lands under the thumb.
            // Two per row, so four buttons stack instead of running off-screen.
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
                )
            }
            return PadLayout(dpad, buttons, unit)
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
