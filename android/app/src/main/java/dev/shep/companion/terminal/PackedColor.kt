package dev.shep.companion.terminal

import androidx.compose.ui.graphics.Color
import dev.shep.companion.ui.theme.ShepPalette

/**
 * Decoder for the packed cell colors `pane.stream` sends.
 *
 * The bridge forwards the server's `CellData.fg`/`bg` words untouched (see
 * `color packing` in src/protocol/wire.rs) rather than resolving them to RGB,
 * precisely so that named and indexed colors land in *this* palette. A pane
 * asking for "red" gets shep's red on the phone, the same as on the desktop.
 *
 * Layout — the high byte is a tag (see `color_to_u32` in src/protocol/wire.rs):
 *  - `0x00` named: the low byte is **1-based** — `0x00` itself is Reset, and
 *    `0x01..0x10` are Black through White. Getting this off by one paints
 *    default-colored text as ANSI black, i.e. invisible on a dark terminal.
 *  - `0x01` indexed: low byte is an xterm-256 index
 *  - `0x02` rgb: low 24 bits are RRGGBB
 */
object PackedColor {

    private const val TAG_NAMED = 0x00
    private const val TAG_INDEXED = 0x01
    private const val TAG_RGB = 0x02

    /** Named slot 0: "whatever the surface default is". */
    private const val RESET = 0x00

    /**
     * @param default what a reset/unknown color resolves to — the surface's
     *   foreground for `fg`, its background for `bg`.
     */
    fun unpack(packed: Long, default: Color): Color {
        val tag = ((packed shr 24) and 0xFF).toInt()
        val low = (packed and 0xFFFFFF).toInt()
        return when (tag) {
            TAG_NAMED -> {
                if (low == RESET || low > 0x10) default
                else ShepPalette.ansi16.getOrElse(low - 1) { default }
            }
            TAG_INDEXED -> xterm256(low and 0xFF, default)
            TAG_RGB -> Color(0xFF000000L.toInt() or low)
            else -> default
        }
    }

    /** xterm-256: 16 named, then a 6×6×6 cube, then a 24-step grey ramp. */
    fun xterm256(index: Int, default: Color): Color = when {
        index < 16 -> ShepPalette.ansi16.getOrElse(index) { default }
        index < 232 -> {
            val n = index - 16
            Color(cubeStep(n / 36), cubeStep((n / 6) % 6), cubeStep(n % 6))
        }
        index < 256 -> {
            val level = 8 + (index - 232) * 10
            Color(level, level, level)
        }
        else -> default
    }

    /** The cube is not linear: the first step jumps to 95, then rises by 40. */
    private fun cubeStep(step: Int): Int = if (step == 0) 0 else 55 + step * 40
}
