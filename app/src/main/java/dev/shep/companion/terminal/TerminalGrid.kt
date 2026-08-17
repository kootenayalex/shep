package dev.shep.companion.terminal

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
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

/**
 * Renders a streamed pane's cell grid.
 *
 * Drawn on a Canvas as style runs rather than as composables per cell: a row is
 * typically 3-8 runs, so a full screen is a few hundred draw calls instead of
 * thousands of layout nodes.
 *
 * Zoom is a [androidx.compose.ui.graphics.graphicsLayer]-style scale applied to
 * the whole canvas, so pinching is free and never re-measures text. The default
 * **fills the window** rather than fitting the pane's whole width: a 167x54
 * pane fitted to a phone's width draws at about 6px per cell and still leaves
 * nearly half the height empty, which is unreadable text *and* a wasted screen.
 * Filling means the wide side runs off the edge — drag to follow it, pinch or
 * double-tap to see the pane whole.
 *
 * The viewport *follows the cursor* unless the user has panned away. That is
 * what makes the pane usable with the keyboard up: the IME halves the visible
 * height, and without following, the line being typed sits below the fold and
 * the only way back to it is to zoom all the way out. Opening or closing the
 * keyboard also resumes following, so the answer to "where did my prompt go" is
 * always "on screen".
 */
@OptIn(ExperimentalTextApi::class)
@Composable
fun TerminalGrid(
    grid: GridState,
    modifier: Modifier = Modifier,
    baseFontSizeSp: Float = ShepType.TERMINAL_BASE_SP,
    onTap: () -> Unit = {},
    onScrollRows: (Int) -> Unit = {},
) {
    // `pointerInput(Unit)` captures its lambda once, so a callback read
    // directly inside it is the one from the first composition — which is how
    // tapping the grid kept opening the keyboard after the input mode changed.
    val tap by rememberUpdatedState(onTap)
    val scrollRows by rememberUpdatedState(onScrollRows)
    val measurer = rememberTextMeasurer()
    val style = remember(baseFontSizeSp) {
        TextStyle(fontFamily = JetBrainsMono, fontSize = baseFontSizeSp.sp)
    }
    // Monospace: one measurement generalizes to every cell.
    val cell = remember(style) { measurer.measure("M", style) }
    val cellW = cell.size.width.toFloat().coerceAtLeast(1f)
    val cellH = cell.size.height.toFloat().coerceAtLeast(1f)
    // How much of the viewport the keyboard is currently taking. The pane
    // screen pads itself by the IME, so the height this composable is given
    // halves when the keyboard opens — and a default scale computed from that
    // would resize every glyph on the screen every time you started typing.
    // Added back, the default is the one the resting window deserves. Minus the
    // navigation bars, which the root already padded away: `imePadding` only
    // applies what is left after that, and adding the whole inset back here
    // overshoots by a nav bar's worth of height.
    val density = LocalDensity.current
    val imeHeight = (
        WindowInsets.ime.getBottom(density) - WindowInsets.navigationBars.getBottom(density)
        ).coerceAtLeast(0).toFloat()

    var scale by remember { mutableFloatStateOf(0f) }
    // Null means "following the cursor". A pan pins it; the viewport changing
    // size (the keyboard) releases it again.
    var panned by remember { mutableStateOf<Offset?>(null) }
    // Drag the clamp could not absorb, in pixels, waiting to add up to a whole
    // row. Without it a slow drag is a run of sub-row movements that each
    // truncate to zero and the pane never scrolls at all.
    var scrollCarry by remember { mutableFloatStateOf(0f) }

    BoxWithConstraints(modifier) {
        val viewW = constraints.maxWidth.toFloat()
        val viewH = constraints.maxHeight.toFloat()
        val gridW = max(grid.cols, 1) * cellW
        val gridH = max(grid.rows, 1) * cellH
        // Whole pane visible; the floor for a pinch, and where a double-tap
        // goes when you want to see all of it at once.
        val fitScale = if (gridW > 0f) viewW / gridW else 1f
        // What it opens at: the window full of text.
        val resting = coverScale(gridW, gridH, viewW, viewH + imeHeight)
        val effective = if (scale <= 0f) resting else scale

        // The IME opening or closing is the case this whole mechanism exists
        // for, and it arrives as a height change.
        LaunchedEffect(viewH) { panned = null }

        val description = remember(grid.revision.intValue) { grid.plainText() }

        Canvas(
            modifier = Modifier
                .fillMaxSize()
                .semantics { contentDescription = description }
                .pointerInput(Unit) {
                    detectTapGestures(
                        onTap = { tap() },
                        // Snap between the two sizes worth having — the
                        // window full of text, and the whole pane at once —
                        // and hand the viewport back to the cursor either way.
                        onDoubleTap = {
                            scale = if (scale > fitScale * 1.05f) fitScale else resting
                            panned = null
                        },
                    )
                }
                .pointerInput(Unit) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        val before = if (scale <= 0f) resting else scale
                        scale = (before * zoom).coerceIn(fitScale.coerceAtMost(1f), 4f)
                        // Start from wherever the follower had us, so grabbing
                        // the grid never makes it jump.
                        val from = panned ?: followOffset(
                            grid.cursor, cellW, cellH, scale, gridW, gridH, viewW, viewH,
                        )
                        val wanted = Offset(from.x + pan.x, from.y + pan.y)
                        val to = clampOffset(
                            wanted, gridW * scale, gridH * scale, viewW, viewH,
                        )
                        // Vertical drag the grid had nowhere to go with. Zoomed
                        // in that is only the part past an edge; at the resting
                        // scale the grid is exactly as tall as the window, so it
                        // is the whole drag. Either way it is the gesture asking
                        // for content that is not on the screen, which is what
                        // scrolling is.
                        scrollCarry += wanted.y - to.y
                        val rowSpan = cellH * scale
                        val rows = wholeRows(scrollCarry, rowSpan)
                        if (rows != 0) {
                            scrollCarry -= rows * rowSpan
                            scrollRows(rows)
                        }
                        panned = to
                    }
                },
        ) {
            // Reading revision here (not in composition) means a new frame
            // repaints without recomposing the tree.
            @Suppress("UNUSED_EXPRESSION")
            grid.revision.intValue
            if (grid.isEmpty) return@Canvas

            // Resolved per frame rather than in composition: following has to
            // track the cursor at terminal update rates, and recomposing that
            // often would undo the whole flat-array design.
            val offset = panned?.let {
                clampOffset(it, gridW * effective, gridH * effective, viewW, viewH)
            } ?: followOffset(grid.cursor, cellW, cellH, effective, gridW, gridH, viewW, viewH)

            // Only draw rows that can actually land on screen. Beyond the perf
            // win, drawing a row far below the viewport asks Compose for a
            // negative height constraint, which throws.
            val rowSpan = cellH * effective
            val firstRow = ((-offset.y) / rowSpan).toInt().coerceIn(0, max(grid.rows - 1, 0))
            val lastRow = (((-offset.y) + viewH) / rowSpan).toInt()
                .coerceIn(firstRow, max(grid.rows - 1, 0))

            withTransform({
                translate(offset.x, offset.y)
                scale(effective, effective, pivot = Offset.Zero)
            }) {
                drawGrid(grid, measurer, style, cellW, cellH, firstRow, lastRow)
            }
        }
    }
}

/**
 * The scale a pane opens at: never smaller than its width fit, grown toward
 * filling the height, and stopped at the size the font was measured for.
 *
 * Fitting the whole grid means fitting its *width*, because a terminal is much
 * wider than it is tall and a phone is the other way round. For the panes that
 * matter — a full-screen tab reports 167x54 — that draws about 6px a cell,
 * which is unreadable, *and* leaves nearly half the window blank. So the height
 * gets to pull the scale up, and the far right of the pane runs off the edge:
 * mostly the border an agent draws around its own output, and a drag, a pinch
 * or a double-tap all go and get it.
 *
 * It stops at 1f because past that the text is bigger than the size chosen as
 * comfortable, and columns would be cropped to pay for it. A short pane — a
 * 60x21 shell — reaches its width fit and stops there with room to spare, which
 * is right: the text is already large enough, so there is nothing to buy.
 */
internal fun coverScale(gridW: Float, gridH: Float, viewW: Float, viewH: Float): Float {
    if (gridW <= 0f || gridH <= 0f || viewW <= 0f || viewH <= 0f) return 1f
    return max(viewW / gridW, (viewH / gridH).coerceAtMost(1f))
}

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

/** Keep the grid inside the viewport; never let it be dragged off-screen. */
internal fun clampOffset(
    offset: Offset,
    scaledW: Float,
    scaledH: Float,
    viewW: Float,
    viewH: Float,
): Offset = Offset(
    offset.x.coerceIn(-max(0f, scaledW - viewW), 0f),
    offset.y.coerceIn(-max(0f, scaledH - viewH), 0f),
)

/**
 * Where to park the viewport so the cursor is visible.
 *
 * Anchors on the cursor's *row* with a couple of rows of headroom below it —
 * for an agent TUI the cursor sits in the input box, so this keeps what you are
 * typing on screen along with as much of the conversation above it as fits.
 * When the whole grid fits, this is the top-left corner, i.e. the old
 * behaviour.
 */
internal fun followOffset(
    cursor: Cursor?,
    cellW: Float,
    cellH: Float,
    scale: Float,
    gridW: Float,
    gridH: Float,
    viewW: Float,
    viewH: Float,
): Offset {
    val scaledW = gridW * scale
    val scaledH = gridH * scale
    if (cursor == null) return clampOffset(Offset.Zero, scaledW, scaledH, viewW, viewH)
    val rowSpan = cellH * scale
    val colSpan = cellW * scale
    // Two rows of breathing room below the cursor, clamped by what exists.
    val wantY = (cursor.y + 1) * rowSpan + rowSpan * 2 - viewH
    val wantX = (cursor.x + 1) * colSpan + colSpan * 4 - viewW
    return clampOffset(Offset(-max(0f, wantX), -max(0f, wantY)), scaledW, scaledH, viewW, viewH)
}

@OptIn(ExperimentalTextApi::class)
private fun DrawScope.drawGrid(
    grid: GridState,
    measurer: TextMeasurer,
    style: TextStyle,
    cellW: Float,
    cellH: Float,
    firstRow: Int,
    lastRow: Int,
) {
    val cols = grid.cols
    for (y in firstRow..lastRow) {
        val rowBase = y * cols
        val runs = buildRuns(grid, rowBase, cols)
        val top = y * cellH

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
                // Explicit: without it Compose derives the layout box from the
                // canvas size minus topLeft, which goes negative off-screen.
                size = Size(text.length * cellW, cellH),
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

    grid.cursor?.takeIf { it.visible }?.let { cursor ->
        val x = cursor.x * cellW
        val y = cursor.y * cellH
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
 * Collapse a row into style runs.
 *
 * `reversed` is resolved here rather than at draw time so that reversed cells
 * still merge with neighbours that share their resulting colors.
 */
private fun buildRuns(grid: GridState, rowBase: Int, cols: Int): List<Run> {
    val runs = ArrayList<Run>(8)
    var current: Run? = null
    for (x in 0 until cols) {
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
            current = Run(x, StringBuilder(grid.sym[i]), fg, bg, styleBits)
        }
    }
    current?.let { runs.add(it) }
    return runs
}
