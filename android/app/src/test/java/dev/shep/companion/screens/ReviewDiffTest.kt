package dev.shep.companion.screens

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ReviewDiffTest {

    @Test
    fun `a stat reads back as files and lines`() {
        val stat = parseDiffStat(
            """
             src/main.rs      | 12 ++++++++----
             android/App.kt   |  4 ++++
             3 files changed, 112 insertions(+), 30 deletions(-)
            """.trimIndent()
        )!!
        assertEquals(3, stat.files)
        assertEquals(112, stat.added)
        assertEquals(30, stat.removed)
        assertEquals(listOf("src/main.rs", "android/App.kt"), stat.perFile.map { it.path })
        assertEquals(FileStat("android/App.kt", added = 4, removed = 0), stat.perFile[1])
        // 12 lines split 8/4 by the bar's marks.
        assertEquals(8, stat.perFile[0].added)
        assertEquals(4, stat.perFile[0].removed)
    }

    @Test
    fun `a one-sided change is not a parse failure`() {
        val added = parseDiffStat("1 file changed, 3 insertions(+)")!!
        assertEquals(3, added.added)
        assertEquals(0, added.removed)
        val removed = parseDiffStat("2 files changed, 5 deletions(-)")!!
        assertEquals(0, removed.added)
        assertEquals(5, removed.removed)
    }

    @Test
    fun `a binary file is counted but not measured`() {
        val stat = parseDiffStat(
            """
             app/icon.png | Bin 0 -> 128 bytes
             1 file changed, 0 insertions(+), 0 deletions(-)
            """.trimIndent()
        )!!
        assertTrue(stat.perFile.single().binary)
        assertEquals(0, stat.perFile.single().added)
    }

    @Test
    fun `a stat this cannot read is null, not an empty change`() {
        assertNull(parseDiffStat(""))
        assertNull(parseDiffStat("something else entirely"))
    }

    @Test
    fun `a diff splits by file, keyed by where the change lands`() {
        val diff = """
            diff --git a/src/main.rs b/src/main.rs
            @@ -1,2 +1,3 @@
            +one
            diff --git a/App.kt b/App.kt
            @@ -4,1 +4,1 @@
            -two
        """.trimIndent()
        val byFile = splitDiffByFile(diff)
        assertEquals(listOf("src/main.rs", "App.kt"), byFile.keys.toList())
        assertTrue(byFile["src/main.rs"]!!.contains("+one"))
        assertTrue(byFile["App.kt"]!!.contains("-two"))
    }

    @Test
    fun `a diff with no header survives whole under one key`() {
        val byFile = splitDiffByFile("+one\n-two")
        assertEquals(listOf(""), byFile.keys.toList())
        assertEquals("+one\n-two", byFile[""])
    }

    @Test
    fun `the summary counts in english`() {
        assertEquals(
            "written by kai · 1 file · 1 line added, 0 removed",
            reviewSummary("kai", DiffStat(1, 1, 0, emptyList())),
        )
        assertEquals(
            "written by kai · 4 files · 112 lines added, 30 removed",
            reviewSummary("kai", DiffStat(4, 112, 30, emptyList())),
        )
    }
}
