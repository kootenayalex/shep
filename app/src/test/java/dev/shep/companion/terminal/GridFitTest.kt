package dev.shep.companion.terminal

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The scale a pane opens at.
 *
 * Getting it wrong is either unreadable text under a half-empty window, or a
 * pane cropped so far that the part you wanted is off the side.
 */
class GridFitTest {
    // Measured on a 1080x2400 phone: a cell is about 13.2 x 28.8 px at the
    // terminal's base size, and the pane view gets roughly 990 x 1300 of it
    // once the frame and the key bar have taken theirs.
    private val cellW = 13.2f
    private val cellH = 28.8f
    private val viewW = 990f
    private val viewH = 1300f

    private fun scaleFor(cols: Int, rows: Int) =
        coverScale(cols * cellW, rows * cellH, viewW, viewH)

    @Test
    fun `a real pane opens with the window full, not just its width`() {
        // 167x54 is what a full-screen tab on a desktop terminal reports.
        val cover = scaleFor(167, 54)
        val widthFit = viewW / (167 * cellW)
        assertEquals(viewH / (54 * cellH), cover, 0.001f)
        // Which is the whole point: that much more text, that much bigger.
        assertTrue("expected about 1.9x the width fit, got ${cover / widthFit}",
            cover / widthFit > 1.7f && cover / widthFit < 2.0f)
    }

    @Test
    fun `a short pane stops at its width, with room to spare`() {
        // 60x21 — a shell in a split. Its width fit is already above native, so
        // the text is large enough and there is nothing to buy by cropping.
        val cover = scaleFor(60, 21)
        assertEquals(viewW / (60 * cellW), cover, 0.001f)
        assertTrue("must not crop a pane that fits", cover * 60 * cellW <= viewW + 0.001f)
    }

    @Test
    fun `never grown past the size the font was measured at`() {
        // A five-row pane in a tall window would otherwise reach 9x.
        assertEquals(1f, coverScale(gridW = 200f, gridH = 145f, viewW = 190f, viewH = 1300f), 0.001f)
    }

    @Test
    fun `nothing measured yet is left alone`() {
        assertEquals(1f, coverScale(0f, 0f, viewW, viewH), 0f)
        assertEquals(1f, coverScale(1000f, 1000f, 0f, 0f), 0f)
    }
}
