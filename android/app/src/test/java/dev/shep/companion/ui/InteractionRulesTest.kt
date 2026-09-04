package dev.shep.companion.ui

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The two touch rules, enforced by grep, for the same reason `ThemeTokensTest`
 * enforces the tokens: both were broken in eight places each, in eight
 * hand-rolled copies of the same three components, and neither shows up in a
 * screenshot. A square ripple over a rounded chip and a 5-inch-tall button that
 * only registers on its middle 18dp both look completely fine in a still.
 *
 * These are text rules over Compose modifier chains, so they are approximate by
 * construction. They are calibrated to catch the shapes the app actually got
 * wrong, not to be a Compose type-checker.
 */
class InteractionRulesTest {

    private val sourceRoot = File("src/main/java/dev/shep/companion")

    /** Modifier chains, flattened so a multi-line chain reads as one string. */
    private fun chains(): List<Pair<String, String>> = sourceRoot.walkTopDown()
        .filter { it.isFile && it.extension == "kt" }
        .flatMap { file ->
            // Collapse whitespace so `.background(x)\n  .clickable{}` matches.
            val flat = file.readText().replace(Regex("""\s*\n\s*"""), "")
            Regex("""Modifier\s*(\.[A-Za-z]\w*\([^\n]*?\))+""")
                .findAll(flat)
                .map { file.name to it.value }
        }
        .toList()

    @Test
    fun `there are modifier chains to check`() {
        assertTrue("found ${chains().size}", chains().size > 20)
    }

    /**
     * A rounded background must be clipped before anything can ripple on it.
     *
     * `.background(colour, shape).clickable{}` draws the ripple against the
     * *layout* bounds, which are square. Every terminal key, both mode chips
     * and the queue/send buttons flashed a rectangle on touch.
     */
    @Test
    fun `nothing ripples outside its own corners`() {
        val offenders = chains().filter { (_, chain) ->
            val clickable = chain.indexOf(".clickable")
            if (clickable < 0) return@filter false
            val before = chain.take(clickable)
            // A shaped background before the click, with no clip in between.
            val shaped = Regex("""\.background\([^)]*Shep(Shape|Palette)[^)]*Shape""")
                .containsMatchIn(before)
            shaped && !before.contains(".clip(")
        }
        assertEquals(
            "clip before clickable, or the ripple is square:\n" +
                offenders.joinToString("\n") { "${it.first}: ${it.second.take(120)}" },
            emptyList<Pair<String, String>>(),
            offenders,
        )
    }

    /**
     * Padding belongs inside the touch target, not outside it.
     *
     * `.clickable{}.padding(...)` grows the element *around* the hit area, so
     * the padding is decoration and the target stays the size of the text.
     * `ActionText`, `ShepButton` and `ShepChip` put a `defaultMinSize` between
     * the two, which is what makes the padding count.
     */
    @Test
    fun `a tappable thing is at least as big as its padding suggests`() {
        val offenders = chains().filter { (_, chain) ->
            val clickable = chain.indexOf(".clickable")
            if (clickable < 0) return@filter false
            val after = chain.substring(clickable)
            after.contains(".padding(") &&
                !chain.contains(".defaultMinSize(") &&
                !chain.contains(".minimumInteractiveComponentSize()")
        }
        assertEquals(
            "add minimumInteractiveComponentSize() (or a defaultMinSize) to the chain, " +
                "or reach for ActionText / ShepButton / ShepChip:\n" +
                offenders.joinToString("\n") { "${it.first}: ${it.second.take(120)}" },
            emptyList<Pair<String, String>>(),
            offenders,
        )
    }
}
