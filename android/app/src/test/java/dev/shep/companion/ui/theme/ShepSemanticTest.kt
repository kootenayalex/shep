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

    /**
     * Mirrors `manual_state_appearance` in src/ui/status.rs: every tier the
     * server can name resolves to a real appearance, and each one ends in the
     * override marker so a hand-set state is never mistaken for a detected one.
     */
    @Test
    fun `every manual tier resolves and wears the override marker`() {
        assertEquals(7, ShepSemantic.MANUAL_TIERS.size)
        ShepSemantic.MANUAL_TIERS.forEach { tier ->
            val it = ShepSemantic.manual(tier, "hand")
            assertEquals(tier, true, it.glyph.endsWith("·"))
            assertEquals("hand", it.label)
            assertNotEquals(ShepPalette.accent, it.color)
        }
        assertEquals("◉·", ShepSemantic.manual("stop", "x").glyph)
        assertEquals(ShepPalette.red, ShepSemantic.manual("stop", "x").color)
        assertEquals("◆·", ShepSemantic.manual("review", "x").glyph)
        assertEquals(ShepPalette.mauve, ShepSemantic.manual("review", "x").color)
        assertEquals("◐·", ShepSemantic.manual("working", "x", tick = 0).glyph)
        // A tier this build has never heard of is absent, not a crash.
        assertEquals("··", ShepSemantic.manual("future-tier", "x").glyph)
    }

    @Test
    fun `a manual glyph is never a detected glyph`() {
        val detected = listOf("blocked", "working", "done", "idle", "?").map { ShepSemantic.agent(it).glyph }
        ShepSemantic.MANUAL_TIERS.forEach { tier ->
            assertEquals(tier, false, ShepSemantic.manual(tier, "x").glyph in detected)
        }
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

    /**
     * Mirrors `task_appearance` in src/ui/status.rs. The shapes are the agent
     * table's, because they mean the same things; only "done" changes tier,
     * since a task has no notion of your having seen it.
     */
    @Test
    fun `task states match the desktop's table`() {
        listOf(
            Triple("blocked", "◉", ShepPalette.red),
            Triple("done", "●", ShepPalette.green),
            Triple("todo", "○", ShepPalette.overlay1),
            Triple("cancelled", "·", ShepPalette.overlay0),
        ).forEach { (state, glyph, color) ->
            val it = ShepSemantic.task(state)
            assertEquals(glyph, it.glyph)
            assertEquals(state, it.label)
            assertEquals(color, it.color)
        }
        val running = ShepSemantic.task("running", tick = 0)
        assertEquals("◐", running.glyph)
        assertEquals(ShepPalette.yellow, running.color)
        assertEquals("running", running.label)
    }

    /**
     * The queue used to draw one filled dot for all five states and differ
     * only in hue — on both surfaces.
     */
    @Test
    fun `every task state has its own glyph`() {
        val glyphs = listOf("todo", "running", "blocked", "done", "cancelled")
            .map { ShepSemantic.task(it).glyph }
        assertEquals(glyphs.size, glyphs.toSet().size)
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
