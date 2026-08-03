package dev.shep.companion.ui.theme

import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import dev.shep.companion.R

/**
 * Type for a terminal companion: monospace carries the identity.
 *
 * Anything that is shep's own vocabulary — agent names, pane ids, states,
 * badges, diffs, the terminal itself — is mono. Prose is sans. JetBrains Mono is
 * bundled rather than using [FontFamily.Monospace], which on Android resolves to
 * Droid Sans Mono and reads visibly wrong against the desktop TUI.
 */
val JetBrainsMono = FontFamily(
    Font(R.font.jetbrains_mono_regular, FontWeight.Normal),
    Font(R.font.jetbrains_mono_bold, FontWeight.Bold),
)

object ShepType {
    val mono = TextStyle(fontFamily = JetBrainsMono)
    val sans = TextStyle(fontFamily = FontFamily.Default)

    /** Lowercase section label, e.g. "agents" / "stat". */
    val viewTitle = mono.copy(
        fontSize = 12.sp,
        letterSpacing = 1.sp,
        color = ShepPalette.overlay0,
    )
    val agentName = mono.copy(fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
    val agentState = mono.copy(fontSize = 11.sp)
    val paneId = mono.copy(fontSize = 11.sp, color = ShepPalette.overlay0)
    val hint = mono.copy(fontSize = 12.sp)
    val chip = mono.copy(fontSize = 12.sp)
    val badge = mono.copy(fontSize = 11.sp, fontWeight = FontWeight.Bold)
    val key = mono.copy(fontSize = 13.sp, fontWeight = FontWeight.Bold)
    val kv = mono.copy(fontSize = 12.sp)
    val body = sans.copy(fontSize = 14.sp)
    val wordmark = mono.copy(
        fontSize = 16.sp,
        fontWeight = FontWeight.ExtraBold,
        letterSpacing = 0.5.sp,
    )
}
