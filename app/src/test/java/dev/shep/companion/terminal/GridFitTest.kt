package dev.shep.companion.terminal

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * How tall the terminal is once it has been fitted to a phone's width.
 *
 * The frame around the grid is sized from this, so getting it wrong is either a
 * border with empty space inside it or a border that clips the pane.
 */
class GridFitTest {
    @Test
    fun `a real pane fills a little over half a phone's height`() {
        // 167x54 is what a full-screen tab on a desktop terminal reports, and a
        // cell is about twice as tall as it is wide.
        val height = fittedGridHeight(cols = 167, rows = 54, cellW = 8f, cellH = 17.5f, width = 1000f)
        assertTrue("expected about 700px, got $height", height > 690f && height < 720f)
    }

    @Test
    fun `the fit is the grid's own shape, not the width it was given`() {
        val narrow = fittedGridHeight(80, 24, 8f, 16f, 500f)
        val wide = fittedGridHeight(80, 24, 8f, 16f, 1000f)
        assertEquals(2f, wide / narrow, 0.001f)
        // 80 columns of 8px is 640 wide and 24 rows of 16px is 384 tall, so at
        // 1000px across the height follows the same ratio.
        assertEquals(600f, wide, 0.001f)
    }

    @Test
    fun `nothing streamed yet is not a height`() {
        assertEquals(0f, fittedGridHeight(0, 0, 8f, 16f, 1000f), 0f)
        assertEquals(0f, fittedGridHeight(80, 24, 8f, 16f, 0f), 0f)
        assertEquals(0f, fittedGridHeight(80, 24, 0f, 16f, 1000f), 0f)
    }
}
