package dev.shep.companion.ui.theme

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The state vocabulary from `docs/DESIGN-LANGUAGE.md`, spelled out.
 *
 * The desktop has the same table in `state_vocabulary_matches_the_design_language`
 * (src/ui/status.rs). If you change one, change the other and the doc — the two
 * surfaces disagreeing about what yellow means is exactly the bug this pins,
 * and it went unnoticed for months because nothing compared them.
 */
class ShepSemanticTest {

    @Test
    fun `agent states match the design language`() {
        listOf(
            Triple("blocked", "◉", ShepPalette.red),
            Triple("done", "●", ShepPalette.blue),
            Triple("idle", "○", ShepPalette.green),
        ).forEach { (status, glyph, color) ->
            val it = ShepSemantic.agent(status)
            assertEquals(glyph, it.glyph)
            assertEquals(status, it.label)
            assertEquals(color, it.color)
        }
        // Working animates, so pin the ink and the first frame separately.
        val working = ShepSemantic.agent("working", tick = 0)
        assertEquals("◐", working.glyph)
        assertEquals(ShepPalette.yellow, working.color)
    }

    /**
     * Copper is focus and selection. A working agent sharing it with the row you
     * have selected is why this moved; the phone used to use copper for working
     * while the desktop used yellow.
     */
    @Test
    fun `working does not borrow the focus colour`() {
        assertNotEquals(ShepPalette.accent, ShepSemantic.agentColor("working"))
        assertEquals(ShepPalette.yellow, ShepSemantic.agentColor("working"))
    }

    @Test
    fun `an unrecognised state is absent, not an error`() {
        val it = ShepSemantic.agent("something-new")
        assertEquals("·", it.glyph)
        assertEquals(ShepPalette.overlay0, it.color)
        // Blank falls back to a word, because a card with no status reads as idle.
        assertEquals("idle", ShepSemantic.agent("").label)
    }

    /**
     * Colour is never the only channel: a monochrome themed icon and a
     * colour-blind reader both have to be able to tell these apart.
     */
    @Test
    fun `every state has its own glyph`() {
        val glyphs = listOf("blocked", "working", "done", "idle", "?")
            .map { ShepSemantic.agent(it).glyph }
        assertEquals(glyphs.size, glyphs.toSet().size)
    }

    @Test
    fun `the spinner rotates at the desktop's cadence`() {
        // spinnerFrame divides by eight, so eight steps is one frame and thirty-two
        // is a full turn of the four-frame circle.
        assertEquals(spinnerFrame(0), spinnerFrame(7))
        assertNotEquals(spinnerFrame(0), spinnerFrame(8))
        assertEquals(spinnerFrame(0), spinnerFrame(32))
        // Negative ticks cannot happen, but modulo on a negative would crash.
        assertEquals(spinnerFrame(0), spinnerFrame(-32))
    }

    @Test
    fun `task states match the desktop's table`() {
        assertEquals(ShepPalette.red, ShepSemantic.task("blocked"))
        assertEquals(ShepPalette.yellow, ShepSemantic.task("running"))
        assertEquals(ShepPalette.green, ShepSemantic.task("done"))
        assertEquals(ShepPalette.overlay0, ShepSemantic.task("cancelled"))
        assertEquals(ShepPalette.overlay1, ShepSemantic.task("todo"))
    }

    @Test
    fun `review badges do not borrow a state colour`() {
        assertNull(ShepSemantic.reviewBadge(null))
        assertNull(ShepSemantic.reviewBadge("none"))
        // Mauve, not yellow: yellow is the working tier.
        assertEquals("◆" to ShepPalette.mauve, ShepSemantic.reviewBadge("needs_review"))
        assertEquals("↺" to ShepPalette.peach, ShepSemantic.reviewBadge("changes_requested"))
        assertEquals("✓" to ShepPalette.green, ShepSemantic.reviewBadge("approved"))
    }

    /** `✓` is the approved badge and nothing else. It used to be idle's glyph too. */
    @Test
    fun `the approved tick is not also a state`() {
        val tick = ShepSemantic.reviewBadge("approved")!!.first
        listOf("blocked", "working", "done", "idle", "?").forEach {
            assertNotEquals(tick, ShepSemantic.agent(it).glyph)
        }
    }

    /**
     * Scrollback and the live stream resolve colours through one table. They
     * used to have two, so the same pane's red was #D9695F in history and
     * #E66A5E live.
     */
    @Test
    fun `ansi black is legible rather than the background`() {
        assertEquals(ShepPalette.ansiBlack, ShepPalette.ansi16[0])
        assertNotEquals(ShepPalette.panelBg, ShepPalette.ansi16[0])
    }
}
