package dev.shep.companion.ui.theme

import androidx.compose.ui.graphics.Color

/**
 * The shep palette, mirroring `Palette::shep()` in src/app/state.rs.
 *
 * These are the desktop TUI's own colors, not an approximation of them — the
 * companion only reads as part of shep because it renders in the same ink.
 *
 * Semantics carried over from the desktop:
 *  - [accent] copper: focus, selection, the working state
 *  - [peach] warning tier: git-behind, memory pressure, "changes requested"
 *  - [red] blocked or destructive ONLY, so red always means "stop"
 *  - [teal] queued input
 *  - [mauve] review requested
 */
object ShepPalette {
    val accent = Color(0xFFE09A55) // copper
    val panelBg = Color(0xFF1D1813)
    val surface0 = Color(0xFF2E2720)
    val surface1 = Color(0xFF3A3128)
    val surfaceDim = Color(0xFF241E17)
    val overlay0 = Color(0xFF8C8278)
    val overlay1 = Color(0xFFA99F92)
    val text = Color(0xFFECE6DF)
    val subtext0 = Color(0xFFC8BFB4)
    val mauve = Color(0xFFA294EE)
    val green = Color(0xFF4FB477)
    val yellow = Color(0xFFD7A23F)
    val red = Color(0xFFE66A5E)
    val blue = Color(0xFF6BA6CC)
    val teal = Color(0xFF63C1B0)
    val peach = Color(0xFFE6A65E)

    /** Page backdrop behind the app surfaces. */
    val rootBg = Color(0xFF120F0C)

    val accentDim = accent.copy(alpha = 0.16f)
    val tealDim = teal.copy(alpha = 0.18f)
    val peachDim = peach.copy(alpha = 0.16f)
    val redDim = red.copy(alpha = 0.16f)
    val greenDim = green.copy(alpha = 0.16f)
    val mauveDim = mauve.copy(alpha = 0.16f)

    /**
     * The 16 ANSI slots a terminal cell can name, resolved into shep's own
     * colors rather than the stock VGA set.
     *
     * This is what makes a streamed pane look like the desktop: the server
     * sends "color 2", and both ends agree that shep's green is what that
     * means. Order is the standard ANSI one (black, red, green, yellow, blue,
     * magenta, cyan, white, then the bright variants).
     */
    val ansi16: List<Color> = listOf(
        panelBg, red, green, yellow, blue, mauve, teal, subtext0,
        overlay0, red, green, yellow, blue, mauve, teal, text,
    )
}
