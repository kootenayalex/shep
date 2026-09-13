package dev.shep.companion.ui.theme

import androidx.compose.ui.graphics.Color
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

    /**
     * The docket table, all six rows. Desktop twin: `docket_appearance` in
     * src/ui/status.rs. Overdue is peach and `!` — a warning, not a stop — and
     * due today borrows the working tier's yellow because it is the thing
     * happening now.
     */
    @Test
    fun `docket states match the design language`() {
        data class Row(val status: String, val overdue: Boolean, val today: Boolean, val glyph: String, val label: String, val color: Color)
        listOf(
            Row("inbox", false, false, "○", "inbox", ShepPalette.overlay1),
            Row("open", true, false, "!", "overdue", ShepPalette.peach),
            Row("open", false, true, "●", "due today", ShepPalette.yellow),
            Row("open", false, false, "●", "open", ShepPalette.overlay1),
            Row("done", false, false, "●", "done", ShepPalette.green),
            Row("discarded", false, false, "·", "discarded", ShepPalette.overlay0),
        ).forEach { row ->
            val it = ShepSemantic.docket(row.status, overdue = row.overdue, dueToday = row.today)
            assertEquals(row.label, row.glyph, it.glyph)
            assertEquals(row.label, it.label)
            assertEquals(row.label, row.color, it.color)
        }
        // Overdue never borrows the blocked agent's red.
        assertNotEquals(ShepPalette.red, ShepSemantic.docket("open", overdue = true).color)
        // Inbox has no date, so the flags cannot promote it out of the hollow ring.
        assertEquals("○", ShepSemantic.docket("inbox", overdue = true, dueToday = true).glyph)
    }

    /**
     * The overseer's mark, from `docs/DESIGN-LANGUAGE.md:175`. `✦` and mauve,
     * and never `◆` — that is the needs-review badge, and the two can sit on
     * the same board.
     */
    @Test
    fun `the overseer's mark is its own`() {
        assertEquals("✦", ShepSemantic.overseer.glyph)
        assertEquals(ShepPalette.mauve, ShepSemantic.overseer.color)
        assertNotEquals(ShepSemantic.reviewBadge("needs_review")!!.first, ShepSemantic.overseer.glyph)
        // Nor is it an agent state: the overseer is not one of the things on
        // the board, it is the thing talking about them.
        listOf("blocked", "working", "done", "idle", "?").forEach {
            assertNotEquals(ShepSemantic.overseer.glyph, ShepSemantic.agent(it).glyph)
        }
    }

    /**
     * The three health rows, from `docs/DESIGN-LANGUAGE.md:175-178` and the
     * desktop's `health_facts` (src/ui/overseer.rs). A warning is peach, never
     * red; a failed check does borrow the stop tier whole, because a bridge
     * that is not listening stops you exactly the way a blocked agent does.
     */
    @Test
    fun `health findings match the design language`() {
        assertEquals("✓" to ShepPalette.green, ShepSemantic.health("ok").let { it.glyph to it.color })
        assertEquals("⚠" to ShepPalette.peach, ShepSemantic.health("warn").let { it.glyph to it.color })
        assertEquals("◉" to ShepPalette.red, ShepSemantic.health("fail").let { it.glyph to it.color })
        // Never colour alone: three levels, three glyphs.
        val glyphs = listOf("ok", "warn", "fail").map { ShepSemantic.health(it).glyph }
        assertEquals(glyphs.size, glyphs.toSet().size)
        // A level this build has never heard of is absent, not a crash.
        assertEquals(ShepPalette.overlay0, ShepSemantic.health("future-level").color)
    }

    /**
     * The gauge is a meter, not a state. It used to go yellow at 60 and red at
     * 85 — spending the working and stop tiers on a number — which
     * `docs/DESIGN-LANGUAGE.md:170-173` calls the mistake and `gauge_color`
     * (src/ui/gauge.rs) fixed at the desk.
     */
    @Test
    fun `gauge never spends a state tier`() {
        val tiers = listOf(ShepPalette.red, ShepPalette.yellow, ShepPalette.blue, ShepPalette.green)
        (0..100).forEach { percent ->
            val ink = ShepSemantic.gauge(percent)
            tiers.forEach { assertNotEquals("at $percent%", it, ink) }
        }
        assertEquals(ShepPalette.overlay0, ShepSemantic.gauge(0))
        assertEquals(ShepPalette.overlay0, ShepSemantic.gauge(79))
        assertEquals(ShepPalette.peach, ShepSemantic.gauge(80))
        assertEquals(ShepPalette.peach, ShepSemantic.gauge(100))
    }

    /**
     * The tally's order, from the titlebar's right slot (`right_run` in
     * src/ui/chrome.rs): urgency first, so a glance that only reaches the
     * first fact reaches the right one.
     */
    @Test
    fun `the tally reads in the desktop's order`() {
        assertEquals(listOf("blocked", "working", "done", "idle"), ShepSemantic.TALLY_ORDER)
        // Every entry is a real state with a glyph of its own.
        val glyphs = ShepSemantic.TALLY_ORDER.map { ShepSemantic.agent(it).glyph }
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
