package dev.shep.companion.terminal

import androidx.compose.ui.graphics.Color
import dev.shep.companion.ui.theme.ShepPalette
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Pins the decoder against `color_to_u32` in the shep repo (src/protocol/wire.rs).
 *
 * A drift here is invisible rather than loud: the terminal still renders, just
 * in the wrong colors. The off-by-one on named colors originally painted every
 * default-colored character in ANSI black, which on a dark terminal meant the
 * text simply wasn't there.
 */
class PackedColorTest {

    private val fgDefault = Color(0xFFABCDEF)
    private val bgDefault = Color.Transparent

    @Test
    fun `reset resolves to the surface default, not black`() {
        assertEquals(fgDefault, PackedColor.unpack(0x00000000, fgDefault))
        assertEquals(bgDefault, PackedColor.unpack(0x00000000, bgDefault))
    }

    @Test
    fun `named colors are one-based`() {
        // 0x01 is Black, 0x02 Red … 0x10 White.
        assertEquals(ShepPalette.ansi16[0], PackedColor.unpack(0x00000001, fgDefault))
        assertEquals(ShepPalette.ansi16[1], PackedColor.unpack(0x00000002, fgDefault))
        assertEquals(ShepPalette.ansi16[15], PackedColor.unpack(0x00000010, fgDefault))
    }

    @Test
    fun `named colors resolve into the shep palette`() {
        // Red must be shep's red — this is what makes a streamed pane look like
        // the desktop rather than a stock terminal.
        assertEquals(ShepPalette.red, PackedColor.unpack(0x00000002, fgDefault))
        assertEquals(ShepPalette.green, PackedColor.unpack(0x00000003, fgDefault))
    }

    @Test
    fun `out of range named index falls back to the default`() {
        assertEquals(fgDefault, PackedColor.unpack(0x00000011, fgDefault))
        assertEquals(fgDefault, PackedColor.unpack(0x000000FF, fgDefault))
    }

    @Test
    fun `rgb keeps its exact components`() {
        // The shep copper, as the server would pack it.
        assertEquals(Color(0xFFE09A55), PackedColor.unpack(0x02E09A55, fgDefault))
        assertEquals(Color(0xFF000000), PackedColor.unpack(0x02000000, fgDefault))
        assertEquals(Color(0xFFFFFFFF), PackedColor.unpack(0x02FFFFFF, fgDefault))
    }

    @Test
    fun `indexed below sixteen uses the palette`() {
        assertEquals(ShepPalette.ansi16[0], PackedColor.unpack(0x01000000, fgDefault))
        assertEquals(ShepPalette.ansi16[9], PackedColor.unpack(0x01000009, fgDefault))
    }

    @Test
    fun `indexed cube is not linear`() {
        // xterm's 6x6x6 cube jumps to 95 then steps by 40.
        assertEquals(Color(0, 0, 0), PackedColor.xterm256(16, fgDefault))
        assertEquals(Color(95, 0, 0), PackedColor.xterm256(16 + 36, fgDefault))
        assertEquals(Color(255, 255, 255), PackedColor.xterm256(231, fgDefault))
    }

    @Test
    fun `indexed grey ramp runs from eight upward`() {
        assertEquals(Color(8, 8, 8), PackedColor.xterm256(232, fgDefault))
        assertEquals(Color(238, 238, 238), PackedColor.xterm256(255, fgDefault))
    }

    @Test
    fun `unknown tag falls back rather than throwing`() {
        assertEquals(fgDefault, PackedColor.unpack(0x7F123456, fgDefault))
    }
}
