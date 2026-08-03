package dev.shep.companion.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.unit.dp

/**
 * Wraps Material only so stray Material components inherit shep's colors rather
 * than the default purple. Screens should reach for [ShepPalette] and [ShepType]
 * directly — the design language here is the desktop TUI's, not Material's.
 */
@Composable
fun ShepTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = ShepPalette.accent,
            onPrimary = ShepPalette.panelBg,
            secondary = ShepPalette.teal,
            background = ShepPalette.panelBg,
            onBackground = ShepPalette.text,
            surface = ShepPalette.surfaceDim,
            onSurface = ShepPalette.text,
            surfaceVariant = ShepPalette.surface0,
            onSurfaceVariant = ShepPalette.subtext0,
            error = ShepPalette.red,
            outline = ShepPalette.surface1,
        ),
        // Matches the prototype's 16/10/6px radii.
        shapes = Shapes(
            large = RoundedCornerShape(16.dp),
            medium = RoundedCornerShape(10.dp),
            small = RoundedCornerShape(6.dp),
        ),
        content = content,
    )
}
