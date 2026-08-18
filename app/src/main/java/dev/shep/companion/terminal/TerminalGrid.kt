package dev.shep.companion.terminal

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.sp
import dev.shep.companion.ui.theme.JetBrainsMono
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepType
import kotlin.math.max

/** One horizontal stretch of cells sharing a style, drawn in a single call. */
private class Run(
    val x: Int,
    val text: StringBuilder,
    val fg: Color,
    val bg: Color,
    val mod: Int,
)

/** Smallest and largest terminal text, in sp, and the step the controls move in. */
const val TERMINAL_MIN_SP = 7f
const val TERMINAL_MAX_SP = 26f
const val TERMINAL_STEP_SP = 1f

/** How far a pinch has to travel before it is worth a whole step. */
private const val PINCH_STEP = 1.12f

/**
 * Renders a streamed pane's cell grid, **wrapped to the screen's width**.
 *
 * Drawn on a Canvas as style runs rather than as composables per cell: a row is
 * typically 3-8 runs, so a full screen is a few hundred draw calls instead of
 * thousands of layout nodes.
 *
 * A real agent pane is 167 columns and a phone fits about forty of them at a
 * readable size, so *something* has to give. Shrinking the text until 167
 * columns fit draws about 6px a cell — legible to nobody. Drawing it at full
 * size and panning sideways means every single line needs a horizontal scrub to
 * read. So the grid is wrapped instead: each terminal row is broken into as
 * many display lines as it needs, and the pane only ever scrolls up and down.
 * The text size is chosen explicitly (the controls in the title bar, or a
 * pinch), and the wrap is recomputed from it — a bigger size simply means
 * fewer columns per line.
 *
 * Wrapping is a *view*, not a resize: `pane.stream` never reflows the real
 * terminal ("a phone attaching must never reflow the user's terminal"), so the
 * pane stays 167 columns and the desktop is untouched. The cost is that box
 * drawing an agent wraps too — the border around Claude's input box becomes a
 * few lines of dashes. Readable text was worth more than an intact border.
 *
 * The text rests against the *bottom* of the window unless the user has panned
 * away — see [clampY]. A pane is read from its newest line, and its screen is
 * 54 rows whatever is in it, so anything else leaves the line you care about
 * stranded above a half-window of blank. That is also what makes the pane usable
 * with the keyboard up: the IME takes half the height, and the anchor is
 * recomputed from what is left rather than kept where it was, so the answer to
 * "where did my prompt go" is always "on screen".
 */
@OptIn(ExperimentalTextApi::class)
@Composable
fun TerminalGrid(
    grid: GridState,
    modifier: Modifier = Modifier,
    fontSizeSp: Float = ShepType.TERMINAL_BASE_SP,
    onFontSizeSp: (Float) -> Unit = {},
    onTap: () -> Unit = {},
    onScrollRows: (Int) -> Unit = {},
) {
    // `pointerInput(Unit)` captures its lambda once, so a callback read
    // directly inside it is the one from the first composition — which is how
    // tapping the grid kept opening the keyboard after the input mode changed.
    val tap by rememberUpdatedState(onTap)
    val scrollRows by rememberUpdatedState(onScrollRows)
    val setFontSize by rememberUpdatedState(onFontSizeSp)
    val fontSize by rememberUpdatedState(fontSizeSp)
    val measurer = rememberTextMeasurer()
    val style = remember(fontSizeSp) {
        TextStyle(fontFamily = JetBrainsMono, fontSize = fontSizeSp.sp)
    }
    // Monospace: one measurement generalizes to every cell.
    val cell = remember(style) { measurer.measure("M", style) }
    val cellW = cell.size.width.toFloat().coerceAtLeast(1f)
    val cellH = cell.size.height.toFloat().coerceAtLeast(1f)

    // Null means "following the cursor". A pan pins it; the viewport changing
    // size (the keyboard) releases it again.
    var pannedY by remember { mutableStateOf<Float?>(null) }
    // Drag the clamp could not absorb, in pixels, waiting to add up to a whole
    // row. Without it a slow drag is a run of sub-row movements that each
    // truncate to zero and the pane never scrolls at all.
    var scrollCarry by remember { mutableFloatStateOf(0f) }
    // A pinch is continuous and a text size is not; this is how far the current
    // pinch has travelled toward being worth a step.
    var pinch by remember { mutableFloatStateOf(1f) }
    // Written by the draw pass, read by the gesture handler. Both run outside
    // composition, and snapshot state here would invalidate the tree on every
    // frame — which is exactly what the flat-array design exists to avoid.
    val drawn = remember { DrawnWrap() }

    BoxWithConstraints(modifier) {
        val viewW = constraints.maxWidth.toFloat()
        val viewH = constraints.maxHeight.toFloat()
        // How many of the pane's columns fit on a line at this size. Never
        // zero: a wrap width of nothing has no smallest piece to wrap into.
        val wrapCols = max(1, (viewW / cellW).toInt())

        // The IME opening or closing is the case this whole mechanism exists
        // for, and it arrives as a height change. A new text size re-wraps
        // everything under the viewport, so where it was parked means nothing
        // any more — go back to the anchor, which is where the newest line is.
        LaunchedEffect(viewH, fontSizeSp) { pannedY = null }

        val description = remember(grid.revision.intValue) { grid.plainText() }

        Canvas(
            modifier = Modifier
                .fillMaxSize()
                // A line only partly on screen is still drawn whole, and a
                // Canvas does not clip: without this the bottom line spills
                // past the pane's border and over whatever is under it.
                .clipToBounds()
                .semantics { contentDescription = description }
                .pointerInput(Unit) {
                    detectTapGestures(
                        onTap = { tap() },
                        // Back to the size it was shipped at, for when a pinch
                        // has wandered somewhere unreadable.
                        onDoubleTap = {
                            setFontSize(ShepType.TERMINAL_BASE_SP)
                            pannedY = null
                        },
                    )
                }
                .pointerInput(Unit) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        if (zoom != 1f) {
                            pinch *= zoom
                            val steps = pinchSteps(pinch)
                            if (steps != 0) {
                                pinch = 1f
                                setFontSize(stepFontSize(fontSize, steps))
                            }
                            return@detectTransformGestures
                        }
                        // Start from wherever the follower had us, so grabbing
                        // the grid never makes it jump.
                        val from = pannedY ?: restY(drawn.cursorLine, cellH, drawn.contentH, viewH)
                        val wanted = from + pan.y
                        val to = clampY(wanted, drawn.contentH, viewH)
                        // Drag the wrapped image had nowhere to go with: the
                        // gesture asking for content that is not on the screen,
                        // which is what scrolling is.
                        scrollCarry += wanted - to
                        // In *terminal* rows, which is what `pane.scroll` takes
                        // — one of those is however many display lines it wraps
                        // into, or the pane would scroll several times too fast.
                        val rowSpan = cellH * drawn.wrapFactor
                        val rows = wholeRows(scrollCarry, rowSpan)
                        if (rows != 0) {
                            scrollCarry -= rows * rowSpan
                            scrollRows(rows)
                        }
                        pannedY = to
                    }
                },
        ) {
            // Reading revision here (not in composition) means a new frame
            // repaints without recomposing the tree.
            @Suppress("UNUSED_EXPRESSION")
            grid.revision.intValue
            if (grid.isEmpty) return@Canvas

            val layout = wrapGrid(grid, wrapCols)
            drawn.contentH = layout.size * cellH
            drawn.wrapFactor = layout.size.toFloat() / max(grid.rows, 1)
            drawn.cursorLine = grid.cursor?.let { layout.lineOf(it.y, it.x) } ?: 0

            val y = pannedY?.let { clampY(it, drawn.contentH, viewH) }
                ?: restY(drawn.cursorLine, cellH, drawn.contentH, viewH)

            // Only draw lines that can actually land on screen. Beyond the perf
            // win, drawing a line far below the viewport asks Compose for a
            // negative height constraint, which throws.
            val lastLine = max(layout.size - 1, 0)
            val first = ((-y) / cellH).toInt().coerceIn(0, lastLine)
            val last = (((-y) + viewH) / cellH).toInt().coerceIn(first, lastLine)

            translate(top = y) {
                drawWrapped(grid, layout, measurer, style, cellW, cellH, first, last)
            }
        }
    }
}

/**
 * What the last draw worked out, for the gesture handler to reuse.
 *
 * Plain fields on purpose — see the note at the call site.
 */
private class DrawnWrap {
    var contentH: Float = 0f
    var cursorLine: Int = 0
    var wrapFactor: Float = 1f
}

/**
 * The pane's cells, as much of them as laying the text out needs to know.
 *
 * An interface so the layout can be exercised on plain strings: everything
 * below this line is pure, and the only thing it needs from a real frame is
 * where each row's content ends and what is in it.
 */
internal interface Cells {
    val rows: Int

    /**
     * Cells up to and including the last one with anything drawn in it.
     *
     * A cell counts as content if it has a glyph, a background, or a style — a
     * run of blanks carrying a highlight is a drawn thing, and trimming it
     * would erase the highlight rather than the padding. Terminal rows are
     * otherwise padded to the full width, and wrapping *that* would turn a
     * 167-column pane of short lines into four times as many display lines,
     * nearly all of them empty.
     */
    fun width(row: Int): Int

    fun sym(row: Int, col: Int): String
}

/**
 * Where each piece of the grid landed: which display line, and how far across.
 *
 * Flat segment arrays rather than nested lists. A *segment* is one unbroken
 * stretch of a source row drawn at one place on one display line, and a display
 * line is a slice of them. Most lines are a single segment; a line has two when
 * a rejoined paragraph runs across the row boundary the agent's own wrapping
 * left behind.
 */
internal class WrapLayout(
    val size: Int,
    private val lineSeg: IntArray,
    private val segRow: IntArray,
    private val segFrom: IntArray,
    private val segTo: IntArray,
    private val segX: IntArray,
    private val segLine: IntArray,
    private val rowSeg: IntArray,
    private val rowLine: IntArray,
    private val wrapCols: Int,
) {
    /** The segments making up one display line, in the order they are drawn. */
    fun segmentsOf(line: Int): IntRange = lineSeg[line] until lineSeg[line + 1]

    fun row(seg: Int): Int = segRow[seg]
    fun from(seg: Int): Int = segFrom[seg]
    fun to(seg: Int): Int = segTo[seg]
    fun x(seg: Int): Int = segX[seg]

    /** The display line holding cell (`row`, `col`) — where the cursor went. */
    fun lineOf(row: Int, col: Int): Int {
        val seg = seek(row, col)
        val line = if (seg >= 0) segLine[seg] else rowLine.getOrElse(row) { 0 }
        // A cursor left on the blank bottom of the screen belongs to the last
        // line there is, since those blanks are not laid out.
        return line.coerceIn(0, max(size - 1, 0))
    }

    /** How far across that line the cell sits. */
    fun xOf(row: Int, col: Int): Int {
        val seg = seek(row, col)
        if (seg < 0) return 0
        return (segX[seg] + col - segFrom[seg]).coerceIn(0, max(wrapCols - 1, 0))
    }

    /**
     * The segment holding `col` in `row`, or -1 if the row drew nothing.
     *
     * Falls off the end onto the row's last segment on purpose: an agent parks
     * its cursor past the end of the input line, which is a column no segment
     * covers, and the answer wanted there is "just after the text".
     */
    private fun seek(row: Int, col: Int): Int {
        if (row < 0 || row + 1 >= rowSeg.size) return -1
        val first = rowSeg[row]
        val end = rowSeg[row + 1]
        if (first >= end) return -1
        for (s in first until end) if (col < segTo[s]) return s
        return end - 1
    }
}

/**
 * Lay the pane's text out across [wrapCols] columns.
 *
 * Two things happen here, and the second is the reason the first is not enough.
 *
 * **Words are kept whole.** Breaking a row every [wrapCols] cells cuts through
 * the middle of words, which is the difference between reading a pane and
 * decoding it.
 *
 * **Rows the agent had already wrapped are joined back up.** An agent word-wraps
 * its own prose to the width of the pane, so what arrives is not paragraphs but
 * paragraphs already broken into ~130-column pieces. Re-wrapping each piece on
 * its own at phone width gives a full line and then a stub, over and over — text
 * wrapped twice, which reads far worse than text wrapped once. So a run of rows
 * that was one line of prose is put back together before being laid out.
 *
 * Rejoining has to be *conservative*, because joining two rows that were always
 * separate is worse than not joining ones that weren't: see [joinsNext].
 */
internal fun wrapCells(cells: Cells, wrapCols: Int): WrapLayout {
    val cols = max(wrapCols, 1)
    val n = cells.rows
    val width = IntArray(n)
    val textWidth = IntArray(n)
    val indent = IntArray(n)
    val wordLen = IntArray(n)
    val structural = BooleanArray(n)
    val opensBlock = BooleanArray(n)
    for (r in 0 until n) {
        val w = cells.width(r)
        width[r] = w
        var tw = 0
        for (x in 0 until w) if (!blank(cells.sym(r, x))) tw = x + 1
        textWidth[r] = tw
        var i = 0
        while (i < w && blank(cells.sym(r, i))) i++
        indent[r] = i
        var j = i
        val head = StringBuilder()
        while (j < w && !blank(cells.sym(r, j))) {
            if (head.length < 4) head.append(cells.sym(r, j))
            j++
        }
        wordLen[r] = j - i
        opensBlock[r] = opensBlock(head.toString())
        var boxy = false
        for (x in 0 until w) if (boxDrawing(cells.sym(r, x))) { boxy = true; break }
        structural[r] = boxy
    }
    // The width the agent wrapped its own text at. Taken from the widest row
    // that is text — the pane's own column count is no use, because an agent
    // wraps well short of it and a margin guessed too wide never fires.
    var margin = 0
    for (r in 0 until n) if (!structural[r] && textWidth[r] > margin) margin = textWidth[r]

    val segRow = IntBuf(); val segFrom = IntBuf(); val segTo = IntBuf()
    val segX = IntBuf(); val segLine = IntBuf(); val lineSeg = IntBuf()
    val rowLine = IntArray(n) { -1 }
    var lines = 0

    val lRow = IntBuf(); val lCol = IntBuf()
    fun spaceAt(k: Int): Boolean = lRow[k] < 0 || blank(cells.sym(lRow[k], lCol[k]))

    var r = 0
    while (r < n) {
        var last = r
        while (last + 1 < n &&
            joinsNext(last, last == r, textWidth, indent, wordLen, structural, opensBlock, margin)
        ) last++

        // Each row's indent is stripped here and re-applied as its line's
        // starting column, which is what keeps a wrapped line under its
        // paragraph instead of sending it back to the left margin. The first
        // line can be indented differently from the rest — `※ recap:` and
        // friends hang their continuations — so the two are tracked apart.
        // Neither may take more than half the screen, or a deeply indented
        // paragraph would wrap one word to a line.
        val firstInd = indent[r].coerceAtMost(cols / 2)
        val bodyInd = (if (last > r) indent[r + 1] else indent[r]).coerceAtMost(cols / 2)
        lRow.clear(); lCol.clear()
        for (row in r..last) {
            // The space the agent's own line break stood for.
            if (row > r) { lRow.add(-1); lCol.add(-1) }
            // A joined row's trailing blanks are padding the line break used to
            // hide; carrying them through would put a gap mid-sentence. The
            // paragraph's last row keeps them, because a styled blank run at
            // the end of a block is something drawn.
            val end = if (row < last) textWidth[row] else width[row]
            for (c in indent[row] until end) { lRow.add(row); lCol.add(c) }
        }

        val total = lRow.size
        var i = 0
        var paraLine = 0
        if (total == 0) {
            // A blank row is a blank line the agent drew; keep it.
            lineSeg.add(segRow.size)
            rowLine[r] = lines
            lines++
        }
        while (i < total) {
            while (i < total && spaceAt(i)) i++
            if (i >= total) break
            val ind = if (paraLine == 0) firstInd else bodyInd
            val room = max(1, cols - ind)
            var end = (i + room).coerceAtMost(total)
            if (end < total) {
                var k = end
                while (k > i && !spaceAt(k)) k--
                // No break to be had: a single word longer than the screen has
                // to be cut, and cutting it is still better than losing it.
                if (k > i) end = k
            }
            var stop = end
            while (stop > i && spaceAt(stop - 1)) stop--

            lineSeg.add(segRow.size)
            var k = i
            while (k < stop) {
                if (lRow[k] < 0) { k++; continue }
                val row = lRow[k]
                val from = lCol[k]
                var to = from
                val x = ind + (k - i)
                while (k < stop && lRow[k] == row && lCol[k] == to) { to++; k++ }
                if (rowLine[row] < 0) rowLine[row] = lines
                segRow.add(row); segFrom.add(from); segTo.add(to)
                segX.add(x); segLine.add(lines)
            }
            lines++
            paraLine++
            i = end
        }
        r = last + 1
    }
    lineSeg.add(segRow.size)
    // Trailing blank lines are the unused bottom of the agent's screen, not
    // something to scroll to. A pane is 54 rows whatever is in it, so keeping
    // them would leave the last line of real text stranded halfway up the
    // window with nothing under it.
    var visible = lines
    while (visible > 0 && lineSeg[visible - 1] == lineSeg[visible]) visible--

    // Segments are appended in row order, so a row's are contiguous and its
    // range is a running count away.
    val rowSeg = IntArray(n + 1)
    for (s in 0 until segRow.size) rowSeg[segRow[s] + 1]++
    for (i in 1..n) rowSeg[i] += rowSeg[i - 1]
    // A row that drew nothing still has to answer "which line are you on".
    for (i in 0 until n) if (rowLine[i] < 0) rowLine[i] = if (i > 0) rowLine[i - 1] else 0

    return WrapLayout(
        size = visible,
        lineSeg = lineSeg.toArray(),
        segRow = segRow.toArray(),
        segFrom = segFrom.toArray(),
        segTo = segTo.toArray(),
        segX = segX.toArray(),
        segLine = segLine.toArray(),
        rowSeg = rowSeg,
        rowLine = rowLine,
        wrapCols = cols,
    )
}

/**
 * Whether row `r + 1` is the rest of row `r`'s sentence rather than a new line.
 *
 * The load-bearing clause is the last one: row `r` had no room left for the
 * first word of row `r + 1`, so the break between them was forced by the margin
 * and not chosen. A line that stopped well short of the margin stopped because
 * the agent wanted it to.
 *
 * Everything before it is there to keep the rule off drawn *structure*, where a
 * wrong join is much worse than a missed one. Blank lines separate paragraphs;
 * box drawing means a border, a divider or a table; and a bullet, a prompt arrow
 * or a numbered item starts something.
 *
 * Indent is the last guard: a continuation keeps the indent of the body it
 * belongs to, so once a paragraph is running, a change of indent is a change of
 * block. Its *first* continuation may indent further, because that is a hanging
 * indent — `※ recap:` at the margin with its body tucked under it — and
 * refusing that join is what leaves the stub line this is all here to avoid.
 */
private fun joinsNext(
    r: Int,
    first: Boolean,
    width: IntArray,
    indent: IntArray,
    wordLen: IntArray,
    structural: BooleanArray,
    opensBlock: BooleanArray,
    margin: Int,
): Boolean = width[r] > 0 && width[r + 1] > 0 &&
    !structural[r] && !structural[r + 1] &&
    !opensBlock[r + 1] &&
    (if (first) indent[r + 1] >= indent[r] else indent[r + 1] == indent[r]) &&
    wordLen[r + 1] > 0 &&
    margin > 20 &&
    width[r] + 1 + wordLen[r + 1] > margin

/** Characters that mean "this line starts something" when they stand alone. */
private const val MARKER_CHARS = "❯›»⏵▸▪•‣-–—*+>#|✔✓✗×◻☐☑○●◦"

/** Whether a row's first word is a bullet, a prompt arrow or a list number. */
private fun opensBlock(word: String): Boolean {
    if (word.isEmpty()) return false
    if (word.length <= 3 && word.all { it in MARKER_CHARS }) return true
    val head = word.dropLast(1)
    return head.isNotEmpty() && head.all { it.isDigit() } && word.last() in ".)"
}

private fun blank(sym: String): Boolean = sym.isEmpty() || sym == " " || sym.isBlank()

/** Box drawing and block elements: a border, a divider, a table or a bar. */
private fun boxDrawing(sym: String): Boolean =
    sym.isNotEmpty() && sym[0] in '─'..'▟'

/** A growable [IntArray]; the layout is rebuilt per frame and should not litter. */
private class IntBuf(capacity: Int = 128) {
    private var a = IntArray(capacity)
    var size: Int = 0
        private set

    operator fun get(i: Int): Int = a[i]
    fun add(v: Int) {
        if (size == a.size) a = a.copyOf(a.size * 2)
        a[size++] = v
    }
    fun clear() { size = 0 }
    fun toArray(): IntArray = a.copyOf(size)
}

/** A live frame, read as [Cells]. */
private class GridCells(private val grid: GridState) : Cells {
    private val widths = contentWidths(grid)
    override val rows: Int get() = grid.rows
    override fun width(row: Int): Int = widths[row]
    override fun sym(row: Int, col: Int): String =
        grid.sym.getOrElse(row * grid.cols + col) { " " }
}

/** How far along each row there is anything to show. */
internal fun contentWidths(grid: GridState): IntArray {
    val cols = grid.cols
    val widths = IntArray(grid.rows)
    for (y in 0 until grid.rows) {
        val base = y * cols
        var width = 0
        for (x in 0 until cols) {
            val i = base + x
            if (i >= grid.sym.size) break
            if (grid.sym[i] != " " || grid.bg[i] != 0L || grid.mod[i] != 0) width = x + 1
        }
        widths[y] = width
    }
    return widths
}

internal fun wrapGrid(grid: GridState, wrapCols: Int): WrapLayout =
    wrapCells(GridCells(grid), wrapCols)


/** Whole text-size steps in a pinch that has travelled [zoom] from where it began. */
internal fun pinchSteps(zoom: Float): Int = when {
    zoom >= PINCH_STEP -> 1
    zoom <= 1f / PINCH_STEP -> -1
    else -> 0
}

/** [steps] up or down from [sp], inside the range the terminal is legible in. */
internal fun stepFontSize(sp: Float, steps: Int): Float =
    (sp + steps * TERMINAL_STEP_SP).coerceIn(TERMINAL_MIN_SP, TERMINAL_MAX_SP)

/**
 * Whole rows of scroll in an accumulated pixel drag.
 *
 * Truncates toward zero on purpose, both ways: the remainder stays on the
 * carry, so a slow drag that never reaches a full row in one frame still
 * scrolls once it has covered one. Positive is a drag *downward*, which reveals
 * what is above — back into history, the direction a wheel goes when it is
 * pushed away from you.
 */
internal fun wholeRows(carry: Float, rowSpan: Float): Int =
    if (rowSpan <= 0f) 0 else (carry / rowSpan).toInt()

/**
 * Keep the wrapped image inside the viewport; never let it be dragged off.
 *
 * The floor is the *bottom* anchor, not zero: a pane is read from its newest
 * line, so the last line of text belongs against the bottom of the window.
 * Content taller than the window scrolls between its top and that anchor;
 * content shorter than the window has nowhere to go and sits on the bottom,
 * rather than clinging to the top with half a window of nothing beneath it.
 */
internal fun clampY(y: Float, contentH: Float, viewH: Float): Float {
    val bottom = viewH - contentH
    return y.coerceIn(bottom, max(bottom, 0f))
}

/**
 * Where the viewport parks when the user has not dragged it.
 *
 * The bottom of the text against the bottom of the window. That is the answer
 * for an agent pane at any text size — smaller text means more lines, not more
 * blank space — and it is the answer when the keyboard opens too, because the
 * anchor is recomputed from the height that is left rather than kept where it
 * was.
 *
 * The cursor overrides it only when the anchor would leave the cursor above the
 * window, which takes a pane whose cursor is nowhere near its end — a
 * full-screen editor rather than an agent's input line.
 */
internal fun restY(cursorLine: Int, cellH: Float, contentH: Float, viewH: Float): Float =
    clampY(max(viewH - contentH, -(cursorLine * cellH)), contentH, viewH)

@OptIn(ExperimentalTextApi::class)
private fun DrawScope.drawWrapped(
    grid: GridState,
    layout: WrapLayout,
    measurer: TextMeasurer,
    style: TextStyle,
    cellW: Float,
    cellH: Float,
    firstLine: Int,
    lastLine: Int,
) {
    val cols = grid.cols
    for (line in firstLine..lastLine) {
        val top = line * cellH
        for (seg in layout.segmentsOf(line)) {
            val runs = buildRuns(
                grid,
                rowBase = layout.row(seg) * cols,
                from = layout.from(seg),
                to = layout.to(seg),
                xBase = layout.x(seg),
            )
            for (run in runs) {
                if (run.bg != Color.Transparent) {
                    drawRect(
                        color = run.bg,
                        topLeft = Offset(run.x * cellW, top),
                        size = Size(run.text.length * cellW, cellH),
                    )
                }
            }
            for (run in runs) {
                if (CellMod.has(run.mod, CellMod.HIDDEN)) continue
                val text = run.text.toString()
                if (text.isBlank()) continue
                drawText(
                    textMeasurer = measurer,
                    text = text,
                    topLeft = Offset(run.x * cellW, top),
                    // A run is one line of cells and must stay one line. Left
                    // to wrap, a run whose box is a rounding error too narrow
                    // breaks at its last space and puts the tail on a second
                    // line — outside a box one cell high, so the word is simply
                    // gone. That is invisible until you compare the screen with
                    // the text, and it silently ate a word a line.
                    softWrap = false,
                    maxLines = 1,
                    // Explicit: without it Compose derives the layout box from
                    // the canvas size minus topLeft, which goes negative
                    // off-screen. A cell of slack keeps the box from being the
                    // binding constraint; the canvas does the real clipping.
                    size = Size(text.length * cellW + cellW, cellH),
                    style = style.copy(
                        color = if (CellMod.has(run.mod, CellMod.DIM)) run.fg.copy(alpha = 0.6f) else run.fg,
                        fontWeight = if (CellMod.has(run.mod, CellMod.BOLD)) FontWeight.Bold else FontWeight.Normal,
                        fontStyle = if (CellMod.has(run.mod, CellMod.ITALIC)) FontStyle.Italic else FontStyle.Normal,
                        textDecoration = when {
                            CellMod.has(run.mod, CellMod.UNDERLINED) -> TextDecoration.Underline
                            CellMod.has(run.mod, CellMod.CROSSED_OUT) -> TextDecoration.LineThrough
                            else -> null
                        },
                    ),
                )
            }
        }
    }

    grid.cursor?.takeIf { it.visible }?.let { cursor ->
        val line = layout.lineOf(cursor.y, cursor.x)
        if (line < firstLine || line > lastLine) return@let
        val x = layout.xOf(cursor.y, cursor.x) * cellW
        val y = line * cellH
        // Difference blending keeps the glyph under the cursor legible whatever
        // its colors are.
        val (w, h, topOffset) = when (cursor.shape) {
            3, 4 -> Triple(cellW, cellH * 0.12f, cellH * 0.88f) // underline
            5, 6 -> Triple(cellW * 0.15f, cellH, 0f)            // bar
            else -> Triple(cellW, cellH, 0f)                    // block
        }
        drawRect(
            color = ShepPalette.accent,
            topLeft = Offset(x, y + topOffset),
            size = Size(w, h),
            blendMode = BlendMode.Difference,
        )
    }
}

/**
 * Collapse one wrapped piece of a row into style runs.
 *
 * `reversed` is resolved here rather than at draw time so that reversed cells
 * still merge with neighbours that share their resulting colors. Run positions
 * are rebased onto [xBase], because a wrapped piece draws where its display
 * line puts it and not at the column it came from.
 */
private fun buildRuns(grid: GridState, rowBase: Int, from: Int, to: Int, xBase: Int): List<Run> {
    val runs = ArrayList<Run>(8)
    var current: Run? = null
    for (x in from until to) {
        val i = rowBase + x
        if (i >= grid.sym.size) break
        val mod = grid.mod[i]
        var fg = PackedColor.unpack(grid.fg[i], ShepPalette.text)
        var bg = PackedColor.unpack(grid.bg[i], Color.Transparent)
        if (CellMod.has(mod, CellMod.REVERSED)) {
            val swapFg = if (bg == Color.Transparent) ShepPalette.panelBg else bg
            val swapBg = fg
            fg = swapFg
            bg = swapBg
        }
        val styleBits = mod and (CellMod.BOLD or CellMod.DIM or CellMod.ITALIC or
            CellMod.UNDERLINED or CellMod.HIDDEN or CellMod.CROSSED_OUT)
        val open = current
        if (open != null && open.fg == fg && open.bg == bg && open.mod == styleBits) {
            open.text.append(grid.sym[i])
        } else {
            open?.let { runs.add(it) }
            current = Run(xBase + x - from, StringBuilder(grid.sym[i]), fg, bg, styleBits)
        }
    }
    current?.let { runs.add(it) }
    return runs
}
