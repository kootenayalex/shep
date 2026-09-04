package dev.shep.companion.terminal

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * How a 167-column pane is laid out on a phone that fits forty of them.
 *
 * Getting this wrong is either unreadable text, or a screen you have to scrub
 * sideways to read one line of.
 */
class GridFitTest {
    // Measured on a 1080x2400 phone: at the default size a cell is about
    // 13.2 x 28.8 px, so a 990px-wide pane view fits 75 columns; at a couple of
    // steps larger it fits about 45.
    private val wrapCols = 45

    /** Rows of plain text, one character a cell — a pane without the colors. */
    private fun cells(vararg text: String): Cells {
        val src = text.toList()
        return object : Cells {
            override val rows: Int get() = src.size
            override fun width(row: Int): Int = src[row].trimEnd().length
            override fun sym(row: Int, col: Int): String =
                src[row].getOrNull(col)?.toString() ?: " "
        }
    }

    /** The laid-out text, one string a display line, as it is drawn. */
    private fun render(cells: Cells, cols: Int): List<String> {
        val layout = wrapCells(cells, cols)
        return (0 until layout.size).map { line ->
            val out = StringBuilder()
            for (seg in layout.segmentsOf(line)) {
                while (out.length < layout.x(seg)) out.append(' ')
                for (c in layout.from(seg) until layout.to(seg)) {
                    out.append(cells.sym(layout.row(seg), c))
                }
            }
            out.toString()
        }
    }

    @Test
    fun `a long line breaks between words, not through them`() {
        val lines = render(cells("the quick brown fox jumps over the lazy dog"), 20)
        assertEquals(listOf("the quick brown fox", "jumps over the lazy", "dog"), lines)
    }

    @Test
    fun `a word longer than the screen is cut rather than lost`() {
        val lines = render(cells("supercalifragilistic"), 8)
        assertEquals(listOf("supercal", "ifragili", "stic"), lines)
    }

    @Test
    fun `rows the agent already wrapped are joined back into one paragraph`() {
        // Verbatim from a live 54-column pane: claude word-wraps its own prose
        // to the pane, so what arrives is a paragraph already in pieces.
        val pane = cells(
            "  latest 2026-08-17-035720), but it's the WorkMayt",
            "  repo and restic forget is repo-wide — joining it",
            "  couples the two products' retention. The alternative",
            "  is a separate repo plus a cipher passphrase in",
            "  ~/vault/secrets.",
        )
        val lines = render(pane, 30)
        // One paragraph laid out at 30 columns, not five re-wrapped pieces:
        // every line but the last is full, and the indent is kept.
        assertEquals(
            listOf(
                "  latest 2026-08-17-035720),",
                "  but it's the WorkMayt repo",
                "  and restic forget is",
                "  repo-wide — joining it",
                "  couples the two products'",
                "  retention. The alternative",
                "  is a separate repo plus a",
                "  cipher passphrase in",
                "  ~/vault/secrets.",
            ),
            lines,
        )
    }

    @Test
    fun `a paragraph that hangs its body under its first line is still one paragraph`() {
        // Verbatim from a live pane: claude sets a recap at the margin and
        // tucks the rest of it under, so the indents differ by two.
        val pane = cells(
            "※ recap: Orbit is finished: all twelve milestones",
            "  built and verified on your 12R with 134 real apps,",
            "  62 tests passing.",
        )
        val lines = render(pane, 34)
        assertEquals(
            listOf(
                "※ recap: Orbit is finished: all",
                "  twelve milestones built and",
                "  verified on your 12R with 134",
                "  real apps, 62 tests passing.",
            ),
            lines,
        )
    }


    @Test
    fun `a line that stopped short of the margin is left where it ended`() {
        // Both rows are far inside the margin the third row sets, so neither
        // break was forced and neither is a wrap to undo.
        val pane = cells(
            "  2 tasks (1 done, 1 open)",
            "  all of them are still open",
            "  a much longer line that establishes where this pane wraps",
        )
        val lines = render(pane, 40)
        assertEquals("  2 tasks (1 done, 1 open)", lines[0])
        assertEquals("  all of them are still open", lines[1])
    }

    @Test
    fun `drawn structure is never joined to the text around it`() {
        val pane = cells(
            "──────────────────────────── clear-chat-history ──",
            "❯ join the workmayt restic repo",
            "──────────────────────────────────────────────────",
            "  ⏵⏵ bypass permissions on (shift+tab to cycle) · ←",
        )
        val lines = render(pane, 52)
        // Each of these stands alone: a divider, a prompt, a divider, a hint.
        assertEquals(4, lines.size)
        assertEquals("❯ join the workmayt restic repo", lines[1])
    }

    @Test
    fun `a bullet starts a line of its own`() {
        val pane = cells(
            "  and this row runs right up against the margin ok",
            "  - a bullet that follows it is not its continuation",
        )
        val lines = render(pane, 50)
        assertEquals("  and this row runs right up against the margin ok", lines[0])
        assertTrue(lines[1].startsWith("  - a bullet"))
    }

    @Test
    fun `a blank row stays a blank line`() {
        val lines = render(cells("first", "", "second"), wrapCols)
        assertEquals(listOf("first", "", "second"), lines)
    }

    @Test
    fun `padding does not become empty lines`() {
        // What a terminal row really looks like: 167 cells, 12 of them content.
        // Wrapping the padding too would turn a screen of short lines into four
        // times as many, nearly all blank.
        val row = "hello world".padEnd(167)
        assertEquals(1, wrapCells(cells(row), wrapCols).size)
    }

    @Test
    fun `the cursor is found where its cell was drawn`() {
        val pane = cells("the quick brown fox jumps over the lazy dog")
        val layout = wrapCells(pane, 20)
        // "the quick brown fox" / "jumps over the lazy" / "dog"
        assertEquals(0, layout.lineOf(0, 0))
        assertEquals(0, layout.xOf(0, 0))
        assertEquals(1, layout.lineOf(0, 20))
        assertEquals(0, layout.xOf(0, 20))
        assertEquals(2, layout.lineOf(0, 40))
        // A cursor parked past the end of the text — where an agent leaves it —
        // lands just after it rather than off the end of the layout.
        assertEquals(2, layout.lineOf(0, 60))
        assertEquals(3, layout.xOf(0, 43))
    }

    @Test
    fun `the cursor on a rejoined row is on the line that row ended up on`() {
        val pane = cells(
            "  aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj",
            "  kkkk llll",
        )
        val layout = wrapCells(pane, 30)
        // Row 1's text was pulled up into the paragraph; the cursor follows it.
        assertEquals(layout.lineOf(1, 2), layout.lineOf(1, 6))
        assertTrue(layout.lineOf(1, 2) < layout.size)
    }

    @Test
    fun `nothing measured yet is left alone`() {
        assertEquals(0, wrapCells(cells(), wrapCols).size)
        assertEquals(1, wrapCells(cells("hello"), 0).size.coerceAtMost(1))
    }

    @Test
    fun `text size steps stay inside the legible range`() {
        assertEquals(14f, stepFontSize(13f, 1), 0.001f)
        assertEquals(12f, stepFontSize(13f, -1), 0.001f)
        assertEquals(TERMINAL_MAX_SP, stepFontSize(TERMINAL_MAX_SP, 1), 0.001f)
        assertEquals(TERMINAL_MIN_SP, stepFontSize(TERMINAL_MIN_SP, -1), 0.001f)
    }

    @Test
    fun `a pinch is only worth a step once it has travelled`() {
        assertEquals(0, pinchSteps(1.05f))
        assertEquals(0, pinchSteps(0.95f))
        assertEquals(1, pinchSteps(1.2f))
        assertEquals(-1, pinchSteps(0.8f))
    }

    @Test
    fun `the view never leaves the wrapped image`() {
        // 200 lines of content in a 1300px window: the top is 0, the bottom is
        // as far up as the last line, and nothing goes past either.
        val contentH = 200 * 28.8f
        assertEquals(0f, clampY(50f, contentH, 1300f), 0.001f)
        assertEquals(-(contentH - 1300f), clampY(-99999f, contentH, 1300f), 0.001f)
    }

    @Test
    fun `text shorter than the window sits on the bottom of it`() {
        // The complaint this exists for: at a small size the text stops filling
        // the window, and the last line should not end up halfway up the screen
        // with blank underneath it.
        assertEquals(900f, clampY(0f, contentH = 400f, viewH = 1300f), 0.001f)
        assertEquals(900f, clampY(-500f, 400f, 1300f), 0.001f)
        assertEquals(900f, clampY(99999f, 400f, 1300f), 0.001f)
    }

    @Test
    fun `the pane rests with its newest line against the bottom`() {
        val cellH = 28.8f
        val contentH = 200 * cellH
        // Long content: scrolled all the way down, last line flush with the
        // bottom edge.
        assertEquals(1300f - contentH, restY(199, cellH, contentH, 1300f), 0.001f)
        // Short content: still on the bottom, not clinging to the top.
        assertEquals(1300f - 400f, restY(13, cellH, 400f, 1300f), 0.001f)
        // A cursor the anchor would push off the top wins instead — a
        // full-screen editor rather than an agent's input line.
        assertEquals(0f, restY(0, cellH, contentH, 1300f), 0.001f)
    }

    @Test
    fun `the blank bottom of an agent's screen is not laid out`() {
        // A pane is 54 rows whatever is in it; the empty ones below the last
        // line are not something to scroll to.
        val pane = cells("hello", "world", "", "", "", "")
        assertEquals(2, wrapCells(pane, wrapCols).size)
        // Blank lines *between* content are spacing the agent drew: those stay.
        assertEquals(3, wrapCells(cells("hello", "", "world", "", ""), wrapCols).size)
    }
}
