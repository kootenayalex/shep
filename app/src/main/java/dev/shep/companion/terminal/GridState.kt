package dev.shep.companion.terminal

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONArray
import org.json.JSONObject

data class Cursor(val x: Int, val y: Int, val visible: Boolean, val shape: Int)

/**
 * The cell grid a streamed pane renders from.
 *
 * Deliberately built out of flat primitive arrays rather than a list of cell
 * objects: a full 120x34 frame is ~4000 cells, and allocating that per frame at
 * terminal update rates would thrash. For the same reason the arrays are plain
 * (not Compose state) — the single [revision] counter is the only observable,
 * and [TerminalGrid] reads it inside its draw lambda so a new frame repaints
 * without recomposing anything.
 */
class GridState {
    var cols by mutableIntStateOf(0)
        private set
    var rows by mutableIntStateOf(0)
        private set
    var cursor by mutableStateOf<Cursor?>(null)
        private set

    /** Bumped on every applied frame; the sole repaint trigger. */
    val revision = mutableIntStateOf(0)

    var sym: Array<String> = emptyArray()
        private set
    var fg: LongArray = LongArray(0)
        private set
    var bg: LongArray = LongArray(0)
        private set
    var mod: IntArray = IntArray(0)
        private set
    var link: IntArray = IntArray(0)
        private set

    var links: List<String> = emptyList()
        private set

    val isEmpty: Boolean get() = cols == 0 || rows == 0

    private fun resize(w: Int, h: Int) {
        if (w == cols && h == rows && sym.isNotEmpty()) return
        cols = w
        rows = h
        val size = w * h
        sym = Array(size) { " " }
        fg = LongArray(size)
        bg = LongArray(size)
        mod = IntArray(size)
        link = IntArray(size) { -1 }
    }

    /**
     * Apply one `{"type":"frame"}` line. Full frames reset the grid first;
     * incremental ones patch only the cells they carry.
     */
    fun apply(frame: JSONObject) {
        val w = frame.optInt("w", cols)
        val h = frame.optInt("h", rows)
        val full = frame.optBoolean("full", false)
        if (full || w != cols || h != rows) {
            resize(w, h)
            if (full) {
                sym.fill(" ")
                fg.fill(0); bg.fill(0); mod.fill(0); link.fill(-1)
            }
        }
        frame.optJSONArray("links")?.let { arr ->
            links = (0 until arr.length()).map { arr.optString(it) }
        }
        val cells = frame.optJSONArray("cells") ?: JSONArray()
        val size = sym.size
        for (i in 0 until cells.length()) {
            val cell = cells.optJSONArray(i) ?: continue
            val idx = cell.optInt(0, -1)
            if (idx < 0 || idx >= size) continue
            sym[idx] = cell.optString(1, " ").ifEmpty { " " }
            fg[idx] = cell.optLong(2, 0)
            bg[idx] = cell.optLong(3, 0)
            mod[idx] = cell.optInt(4, 0)
            link[idx] = if (cell.isNull(5)) -1 else cell.optInt(5, -1)
        }
        cursor = frame.optJSONObject("cursor")?.let {
            Cursor(
                x = it.optInt("x"),
                y = it.optInt("y"),
                visible = it.optBoolean("visible", true),
                shape = it.optInt("shape", 0),
            )
        }
        revision.intValue++
    }

    fun clear() {
        sym = emptyArray(); fg = LongArray(0); bg = LongArray(0)
        mod = IntArray(0); link = IntArray(0)
        cols = 0; rows = 0; cursor = null; links = emptyList()
        revision.intValue++
    }

    /**
     * The grid as plain text.
     *
     * Canvas-drawn glyphs are invisible to both TalkBack and Maestro, so this
     * backs the pane's accessibility description — it is what makes the live
     * terminal readable and testable at all.
     */
    fun plainText(): String {
        if (isEmpty) return ""
        return (0 until rows).joinToString("\n") { y ->
            (0 until cols)
                .joinToString("") { x -> sym.getOrElse(y * cols + x) { " " } }
                .trimEnd()
        }.trimEnd()
    }
}

/** Modifier bits, matching ratatui's `Modifier` plus shep's underline-style extension. */
object CellMod {
    const val BOLD = 1 shl 0
    const val DIM = 1 shl 1
    const val ITALIC = 1 shl 2
    const val UNDERLINED = 1 shl 3
    const val REVERSED = 1 shl 6
    const val HIDDEN = 1 shl 7
    const val CROSSED_OUT = 1 shl 8

    fun has(mod: Int, bit: Int) = (mod and bit) != 0
}
