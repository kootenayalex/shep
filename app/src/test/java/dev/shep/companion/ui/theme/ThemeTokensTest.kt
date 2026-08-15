package dev.shep.companion.ui.theme

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The design system, enforced by grep.
 *
 * This is the same trick that keeps the desktop's palette clean: shep's Rust UI
 * has zero inline colour literals across eighteen files, so retheming is a
 * one-struct edit. The companion had no such rule and drifted — 126 inline font
 * sizes across twelve values, 296 spacings across twenty-three, nine corner
 * radii, and a `MaterialTheme.shapes` that was configured and then never read
 * by anything.
 *
 * A test rather than a lint rule because it needs no tooling, runs in the same
 * `./gradlew test` everything else does, and fails with the offending line.
 */
class ThemeTokensTest {

    private val sourceRoot = File("src/main/java/dev/shep/companion")

    /** Every source file except the token definitions themselves. */
    private fun callSites(): List<File> = sourceRoot.walkTopDown()
        .filter { it.isFile && it.extension == "kt" }
        .filterNot { it.path.contains("ui/theme") }
        .toList()

    private fun offenders(pattern: Regex): List<String> = callSites().flatMap { file ->
        file.readLines().withIndex()
            .filter { (_, line) -> pattern.containsMatchIn(line) }
            // One genuine exception exists and says so on the line itself: a
            // 1dp `AndroidView` that owns the IME connection and is meant to be
            // invisible. An escape hatch that has to be written down is fine;
            // a rule with no exceptions would just be deleted the first time
            // someone hit a real one.
            .filterNot { (_, line) -> line.contains("not-a-token") }
            .map { (index, line) -> "${file.name}:${index + 1}: ${line.trim()}" }
    }

    @Test
    fun `the source set is not empty`() {
        // Guards the rest of this file: a wrong working directory would make
        // every assertion below pass by finding nothing at all.
        assertTrue("no sources found under $sourceRoot", callSites().size > 10)
    }

    /**
     * Font sizes come from [ShepType].
     *
     * `baseFontSizeSp.sp` and friends are fine — they convert a value that was
     * already decided somewhere else. It is the literal `13.sp` that puts a
     * design decision where nobody will find it again.
     */
    @Test
    fun `no font size literals outside the theme`() {
        val found = offenders(Regex("""\b\d+(\.\d+)?\.sp\b"""))
        assertEquals("use a ShepType role instead:\n" + found.joinToString("\n"), emptyList<String>(), found)
    }

    /** Spacing comes from [ShepSpace] or [ShepSize]. */
    @Test
    fun `no dimension literals outside the theme`() {
        val found = offenders(Regex("""\b\d+(\.\d+)?\.dp\b"""))
        assertEquals("use a ShepSpace or ShepSize token instead:\n" + found.joinToString("\n"), emptyList<String>(), found)
    }

    /** Corners come from [ShepShape], so a chip is the same chip everywhere. */
    @Test
    fun `no shapes outside the theme`() {
        val found = offenders(Regex("""RoundedCornerShape\(|CircleShape"""))
        assertEquals("use a ShepShape token instead:\n" + found.joinToString("\n"), emptyList<String>(), found)
    }

    /**
     * Colours come from [ShepPalette].
     *
     * Already true when this test was written, and worth pinning before it
     * stops being: the app carried two rival colour vocabularies for months
     * (`ShepColors` and `ShepPalette`) and the same role rendered as two
     * different hexes depending on which file you were looking at.
     */
    @Test
    fun `no colour literals outside the theme`() {
        val found = offenders(Regex("""Color\(0x[0-9A-Fa-f]{8}\)"""))
        assertEquals("use a ShepPalette token instead:\n" + found.joinToString("\n"), emptyList<String>(), found)
    }

    /**
     * Prose is sans; everything else is mono.
     *
     * `FontFamily.Monospace` resolves to Droid Sans Mono on Android, which is
     * not the typeface the desktop draws in — the whole reason JetBrains Mono
     * is bundled. Reaching for the platform one silently un-does that.
     */
    @Test
    fun `nothing reaches for the platform monospace`() {
        val found = offenders(Regex("""FontFamily\.Monospace"""))
        assertEquals("use ShepType, which is JetBrains Mono:\n" + found.joinToString("\n"), emptyList<String>(), found)
    }
}
