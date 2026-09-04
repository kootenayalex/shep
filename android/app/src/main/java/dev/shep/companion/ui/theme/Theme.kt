package dev.shep.companion.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable

/**
 * Wraps Material so stray Material components inherit shep's colours, shapes
 * and — the part that was missing — shep's typeface.
 *
 * Screens reach for [ShepPalette], [ShepType] and [ShepSpace] directly; the
 * design language here is the desktop TUI's, not Material's. But a bare `Text`,
 * a `Button`'s label and an `OutlinedTextField`'s placeholder all resolve
 * through [Typography], and with the default one they rendered in Roboto 16 —
 * on a screen whose whole point is to look like a terminal. Mapping the slots
 * below means a call site that forgets to name a style still lands in mono.
 *
 * Mono is the default and sans is opt-in, not the other way round: nearly every
 * string in this app is shep talking about itself, and the exceptions (agent
 * output, task prompts, memory entries) are few enough to say so explicitly.
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
        typography = Typography(
            displayLarge = ShepType.hero,
            displayMedium = ShepType.hero,
            displaySmall = ShepType.screenTitle,
            headlineLarge = ShepType.screenTitle,
            headlineMedium = ShepType.screenTitle,
            headlineSmall = ShepType.sheetTitle,
            titleLarge = ShepType.sheetTitle,
            titleMedium = ShepType.agentName,
            titleSmall = ShepType.itemName,
            // bodyLarge is what a `Text` with no style resolves to.
            bodyLarge = ShepType.field,
            bodyMedium = ShepType.state,
            bodySmall = ShepType.meta,
            labelLarge = ShepType.button,
            labelMedium = ShepType.chip,
            labelSmall = ShepType.badge,
        ),
        shapes = Shapes(
            large = ShepShape.sheet,
            medium = ShepShape.card,
            small = ShepShape.button,
        ),
        content = content,
    )
}
