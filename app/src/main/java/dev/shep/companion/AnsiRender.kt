package dev.shep.companion

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle

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

// 16-colour palette tuned for the dark shep background (index 0 = a visible
// grey, not pure black, so "black" text stays legible).
private val ANSI_16 = listOf(
    Color(0xFF5A554E), Color(0xFFD9695F), Color(0xFF9BC177), Color(0xFFE0B085),
    Color(0xFF7FA8C9), Color(0xFFC79BC9), Color(0xFF7FC9C4), Color(0xFFEDE7DF),
    Color(0xFF6C665E), Color(0xFFE98A80), Color(0xFFB4D897), Color(0xFFF0C6A0),
    Color(0xFF9EC2DD), Color(0xFFD9B4DB), Color(0xFF9EDBD6), Color(0xFFFFFFFF),
)

private fun xterm256(n: Int): Color = when {
    n < 16 -> ANSI_16[n.coerceIn(0, 15)]
    n in 16..231 -> {
        val i = n - 16
        val r = i / 36
        val g = (i % 36) / 6
        val b = i % 6
        fun c(v: Int) = if (v == 0) 0 else 55 + v * 40
        Color(c(r), c(g), c(b))
    }
    else -> {
        val v = 8 + (n - 232).coerceIn(0, 23) * 10
        Color(v, v, v)
    }
}

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
