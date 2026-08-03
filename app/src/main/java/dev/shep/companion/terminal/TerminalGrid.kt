package dev.shep.companion.terminal

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
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
    var offset by remember { mutableStateOf(Offset.Zero) }

    BoxWithConstraints(modifier) {
        val viewW = constraints.maxWidth.toFloat()
        val viewH = constraints.maxHeight.toFloat()
        val gridW = max(grid.cols, 1) * cellW
        val gridH = max(grid.rows, 1) * cellH
        // Whole width visible: the mirror default.
        val fitScale = if (gridW > 0f) viewW / gridW else 1f
        val effective = if (scale <= 0f) fitScale else scale

        val description = remember(grid.revision.intValue) { grid.plainText() }

        Canvas(
            modifier = Modifier
                .fillMaxSize()
                .semantics { contentDescription = description }
                .pointerInput(Unit) {
                    detectTapGestures(
                        onTap = { onTap() },
                        // Snap between "see everything" and native size.
                        onDoubleTap = {
                            scale = if (scale > fitScale * 1.05f) fitScale else 1f
                            offset = Offset.Zero
                        },
                    )
                }
                .pointerInput(Unit) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        val next = (if (scale <= 0f) fitScale else scale) * zoom
                        scale = next.coerceIn(fitScale.coerceAtMost(1f), 4f)
                        val scaledW = gridW * scale
                        val scaledH = gridH * scale
                        // Clamp so the grid can never be dragged off-screen.
                        val maxX = max(0f, scaledW - viewW)
                        val maxY = max(0f, scaledH - viewH)
                        offset = Offset(
                            (offset.x + pan.x).coerceIn(-maxX, 0f),
                            (offset.y + pan.y).coerceIn(-maxY, 0f),
                        )
                    }
                },
        ) {
            // Reading revision here (not in composition) means a new frame
            // repaints without recomposing the tree.
            @Suppress("UNUSED_EXPRESSION")
            grid.revision.intValue
            if (grid.isEmpty) return@Canvas

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
