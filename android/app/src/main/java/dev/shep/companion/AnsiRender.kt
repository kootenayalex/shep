package dev.shep.companion

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import dev.shep.companion.terminal.PackedColor
import dev.shep.companion.ui.theme.ShepPalette

/**
 * Read-only ANSI → AnnotatedString renderer for the pane view (A2 decision:
 * a lightweight SGR renderer over embedding Termux's GPLv3 terminal-view).
 *
 * The observe stream is only viewed, never echoed, so this handles the visible
 * subset — SGR colour/weight — and strips cursor-motion CSI and OSC sequences.
 * Backgrounds and blink/italic/underline attributes are intentionally ignored.
 */

private const val ESC = '\u001B'
private const val BEL = '\u0007'

// Scrollback and the live stream render the same pane, so they resolve colours
// through the same table. This file used to carry a second sixteen-colour
// palette of its own, which made a pane's red #D9695F in history and #E66A5E
// live — the same output in two different inks depending on which view you
// happened to be in.
private val ANSI_16 = ShepPalette.ansi16

private fun xterm256(n: Int): Color =
    PackedColor.xterm256(n.coerceIn(0, 255), ShepPalette.text)

fun ansiToAnnotated(raw: String, base: Color): AnnotatedString = buildAnnotatedString {
    var fg: Color? = null
    var bold = false
    val run = StringBuilder()
    val n = raw.length
    var i = 0

    fun flush() {
        if (run.isEmpty()) return
        withStyle(
            SpanStyle(
                color = fg ?: base,
                fontWeight = if (bold) FontWeight.Bold else FontWeight.Normal,
            )
        ) { append(run.toString()) }
        run.setLength(0)
    }

    while (i < n) {
        val c = raw[i]
        if (c == ESC && i + 1 < n) {
            when (raw[i + 1]) {
                '[' -> {
                    // CSI: ESC [ params final-byte
                    var j = i + 2
                    while (j < n && raw[j] !in '@'..'~') j++
                    if (j >= n) { i = n; break }
                    if (raw[j] == 'm') {
                        flush()
                        val paramStr = raw.substring(i + 2, j)
                        val codes = if (paramStr.isEmpty()) listOf(0)
                        else paramStr.split(';').map { it.toIntOrNull() ?: 0 }
                        var k = 0
                        while (k < codes.size) {
                            when (val code = codes[k]) {
                                0 -> { fg = null; bold = false }
                                1 -> bold = true
                                22 -> bold = false
                                in 30..37 -> fg = ANSI_16[code - 30]
                                39 -> fg = null
                                in 90..97 -> fg = ANSI_16[code - 90 + 8]
                                38 -> when {
                                    k + 2 < codes.size && codes[k + 1] == 5 -> {
                                        fg = xterm256(codes[k + 2]); k += 2
                                    }
                                    k + 4 < codes.size && codes[k + 1] == 2 -> {
                                        fg = Color(codes[k + 2], codes[k + 3], codes[k + 4]); k += 4
                                    }
                                }
                                else -> {} // backgrounds, underline, etc. ignored
                            }
                            k++
                        }
                    }
                    i = j + 1
                }
                ']' -> {
                    // OSC: ESC ] ... (BEL | ST)
                    var j = i + 2
                    while (j < n && raw[j] != BEL &&
                        !(raw[j] == ESC && j + 1 < n && raw[j + 1] == '\\')
                    ) j++
                    i = when {
                        j < n && raw[j] == BEL -> j + 1
                        j + 1 < n -> j + 2
                        else -> n
                    }
                }
                else -> i += 2 // other two-byte escapes
            }
        } else {
            run.append(c)
            i++
        }
    }
    flush()
}
