package dev.shep.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The words under an agent's name.
 *
 * The age is NOT one of them — it renders as a sibling element so it can line
 * up under the state word — so every expectation here is the sentence alone.
 *
 * Two of these assertions are not about English at all: the now-line renders
 * above two controls Maestro finds by index and by full-match regex, so a
 * wording change that happens to say `live`, or that reads `<word> blocked`,
 * silently retargets flows 07 and 13 at something else on the screen.
 */
class NowLineTest {

    @Test
    fun `a label set by hand wins over the detected state`() {
        assertEquals(
            "shipping",
            nowLine("working", manualLabel = "shipping", activityLine = "npm run build"),
        )
    }

    @Test
    fun `blocked says who it is waiting for`() {
        assertEquals(
            "waiting for you",
            nowLine("blocked", null, null),
        )
        assertEquals(
            "waiting for you — allow edit to src/main.rs?",
            nowLine("blocked", null, "  ⠋  allow edit to src/main.rs?  "),
        )
    }

    @Test
    fun `working quotes the line it is on`() {
        assertEquals("working · cargo test", nowLine("working", null, "> cargo test"))
        assertEquals("working", nowLine("working", null, "   "))
    }

    @Test
    fun `done reads as an invitation`() {
        assertEquals(
            "finished — ready for you to look at",
            nowLine("done", null, null),
        )
    }

    @Test
    fun `idle and an unknown state both survive`() {
        assertEquals("idle", nowLine("idle", null, null))
        assertEquals("starting", nowLine("starting", null, null))
    }

    @Test
    fun `a long activity line is cut rather than wrapped`() {
        val line = nowLine("working", null, "x".repeat(200))
        assertTrue(line, line.endsWith("…"))
        assertTrue(line, line.length < 80)
    }

    @Test
    fun `it never full-matches one of Maestro's anchors`() {
        // Maestro matches an element's whole text, so these are full matches,
        // not substrings: `live` is flow 07's out toggle and `\S+ blocked` is
        // flow 13's row. The now-line renders above both.
        val blockedAnchor = Regex("""\S+ blocked""")
        for (status in listOf("blocked", "working", "done", "idle", "live")) {
            for (activity in listOf(null, "live", "◉ blocked")) {
                val line = nowLine(status, null, activity)
                assertFalse(line, line == "live")
                assertFalse(line, blockedAnchor.matches(line))
            }
        }
    }
}
