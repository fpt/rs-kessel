package dev.kessel.game

import dev.kessel.vm.MAX_TOUCHES

/**
 * Assigns each finger a console touch **slot** and keeps it there for that
 * finger's whole life.
 *
 * A slot is an identity, not a position in a list. The VM derives
 * `touch_pressed`/`touch_released` per slot, so if two fingers are in slots 0
 * and 1 and the first lifts, the second must *stay* in slot 1 — compacting it
 * down to slot 0 reports a release and a press that never happened, and hands
 * the game the wrong finger's coordinates in between.
 *
 * That is easy to get wrong and impossible to see: it produces a game that
 * works perfectly with one finger and misbehaves only during a specific
 * two-finger sequence. Hence a plain-typed class with no Compose in it, tested
 * off-device, rather than a loop inlined into the pointer handler.
 *
 * Usage, once per pointer event:
 * ```
 * tracker.begin(width, height, dim)
 * for (change in event.changes) tracker.offer(change.id.value, x, y, change.pressed)
 * engine.setTouches(tracker.finish())
 * ```
 */
class TouchTracker {

    /** Which pointer owns each slot, or [NO_POINTER] when the slot is free. */
    private val owner = LongArray(MAX_TOUCHES) { NO_POINTER }

    /** Slots that were offered a pointer during the event in progress. */
    private val seen = BooleanArray(MAX_TOUCHES)

    private val out = IntArray(MAX_TOUCHES * 3)
    private var anyDown = false
    private var width = 0
    private var height = 0
    private var dim = 0

    /** Start an event. [width]/[height] are the view's size in surface pixels. */
    fun begin(width: Int, height: Int, dim: Int) {
        this.width = width
        this.height = height
        this.dim = dim
        seen.fill(false)
        out.fill(0)
        anyDown = false
    }

    /**
     * Offer one pointer, at [x]/[y] in surface pixels.
     *
     * A pointer that has lifted is offered too — that is how its slot gets
     * freed. A pointer over the letterbox **keeps its slot** but reports up:
     * the game should see the finger leave its screen, not see some other
     * finger inherit the slot.
     */
    fun offer(id: Long, x: Float, y: Float, pressed: Boolean) {
        if (!pressed) return
        // More fingers than the console has slots: drop the extra rather than
        // evicting a finger the game is already tracking.
        val slot = slotFor(id) ?: return
        seen[slot] = true

        val packed = consoleTouch(x, y, width, height, dim)
        if (packed == OFF_SCREEN) return

        out[slot * 3] = packed shr 16
        out[slot * 3 + 1] = packed and 0xFFFF
        out[slot * 3 + 2] = 1
        anyDown = true
    }

    /**
     * End the event: free the slots of every finger that has gone, and return
     * the flat `[x, y, down] * MAX_TOUCHES` array the engine wants — or null
     * when no finger is on the screen.
     *
     * The array is a **copy**. The engine publishes what it is given to the
     * game thread, so handing out the buffer this tracker keeps mutating would
     * let that thread read a half-updated gesture.
     */
    fun finish(): IntArray? {
        for (slot in owner.indices) {
            if (!seen[slot]) owner[slot] = NO_POINTER
        }
        return if (anyDown) out.copyOf() else null
    }

    /** This pointer's slot, claiming a free one on its first press. */
    private fun slotFor(id: Long): Int? {
        for (slot in owner.indices) {
            if (owner[slot] == id) return slot
        }
        for (slot in owner.indices) {
            if (owner[slot] == NO_POINTER) {
                owner[slot] = id
                return slot
            }
        }
        return null
    }

    private companion object {
        /** No real `PointerId` is negative, so this cannot collide with one. */
        const val NO_POINTER = -1L
    }
}
