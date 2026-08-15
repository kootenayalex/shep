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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.withTransform
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shep.companion.ui.theme.JetBrainsMono
import dev.shep.companion.ui.theme.ShepPalette
import kotlin.math.max
import kotlin.math.min

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
 * scale fits the pane's full width — the point of the companion is to mirror
 * what the desktop shows, not to reflow it.
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
    baseFontSizeSp: Float = 13f,
    onTap: () -> Unit = {},
) {
    val measurer = rememberTextMeasurer()
    val style = remember(baseFontSizeSp) {
        TextStyle(fontFamily = JetBrainsMono, fontSize = baseFontSizeSp.sp)
    }
    // Monospace: one measurement generalizes to every cell.
    val cell = remember(style) { measurer.measure("M", style) }
    val cellW = cell.size.width.toFloat().coerceAtLeast(1f)
    val cellH = cell.size.height.toFloat().coerceAtLeast(1f)

    var scale by remember { mutableFloatStateOf(0f) }
    // Null means "following the cursor". A pan pins it; the viewport changing
    // size (the keyboard) releases it again.
    var panned by remember { mutableStateOf<Offset?>(null) }

    BoxWithConstraints(modifier) {
        val viewW = constraints.maxWidth.toFloat()
        val viewH = constraints.maxHeight.toFloat()
        val gridW = max(grid.cols, 1) * cellW
        val gridH = max(grid.rows, 1) * cellH
        // Whole width visible: the mirror default.
        val fitScale = if (gridW > 0f) viewW / gridW else 1f
        val effective = if (scale <= 0f) fitScale else scale

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
                        onTap = { onTap() },
                        // Snap between "see everything" and native size, and
                        // hand the viewport back to the cursor either way.
                        onDoubleTap = {
                            scale = if (scale > fitScale * 1.05f) fitScale else 1f
                            panned = null
                        },
                    )
                }
                .pointerInput(Unit) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        val before = if (scale <= 0f) fitScale else scale
                        scale = (before * zoom).coerceIn(fitScale.coerceAtMost(1f), 4f)
                        // Start from wherever the follower had us, so grabbing
                        // the grid never makes it jump.
                        val from = panned ?: followOffset(
                            grid.cursor, cellW, cellH, scale, gridW, gridH, viewW, viewH,
                        )
                        panned = clampOffset(
                            Offset(from.x + pan.x, from.y + pan.y),
                            gridW * scale, gridH * scale, viewW, viewH,
                        )
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
