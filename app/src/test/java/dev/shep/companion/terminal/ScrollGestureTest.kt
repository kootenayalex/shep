package dev.shep.companion.terminal

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The drag-to-rows conversion behind scrolling a pane.
 *
 * A pane on the alternate screen has no terminal scrollback to pan through, so
 * the gesture is not a pan at all — it is a wheel, measured in rows, and these
 * are the cases where measuring it wrong is invisible until you try to use it.
 */
class ScrollGestureTest {
    @Test
    fun `a drag shorter than a row scrolls nothing yet`() {
        assertEquals(0, wholeRows(carry = 12f, rowSpan = 30f))
        assertEquals(0, wholeRows(carry = -12f, rowSpan = 30f))
    }

    @Test
    fun `the leftover carries so a slow drag still gets there`() {
        val rowSpan = 30f
        var carry = 0f
        var rows = 0
        // Ten frames of twelve pixels: four rows, and never a frame that on its
        // own would have counted as one.
        repeat(10) {
            carry += 12f
            val step = wholeRows(carry, rowSpan)
            carry -= step * rowSpan
            rows += step
        }
        assertEquals(4, rows)
    }

    @Test
    fun `dragging down goes back into history`() {
        assertEquals(3, wholeRows(carry = 95f, rowSpan = 30f))
        assertEquals(-3, wholeRows(carry = -95f, rowSpan = 30f))
    }

    @Test
    fun `a grid with no measured row height is not divided by zero`() {
        assertEquals(0, wholeRows(carry = 500f, rowSpan = 0f))
    }
}
