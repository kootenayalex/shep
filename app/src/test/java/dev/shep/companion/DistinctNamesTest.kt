package dev.shep.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the board's naming rule: every session gets a name no other session
 * answers to, and detail is only spent where it buys a distinction.
 *
 * This is the fix for a screenful of cards all reading "claude", so the thing
 * worth testing is not that names are correct but that they are *different* —
 * and no longer than they need to be.
 */
class DistinctNamesTest {

    private fun row(
        paneId: String,
        agent: String = "claude",
        workspace: String = "shep",
        branch: String? = "master",
        tab: String? = null,
        pane: Int? = null,
    ) = AgentRow(
        terminalId = paneId,
        paneId = paneId,
        workspaceId = paneId.substringBefore(':'),
        workspaceLabel = workspace,
        agent = agent,
        status = "idle",
        contextPercent = null,
        reviewState = "",
        customStatus = null,
        worktreeRepo = null,
        isWorktree = false,
        memoryPercent = null,
        tabName = tab,
        paneNumber = pane,
        branch = branch,
    )

    @Test
    fun `a name that is already unique is left alone`() {
        val rows = listOf(row("w1:p1", agent = "claude"), row("w2:p1", agent = "opencode"))
        val names = distinctNames(rows)
        assertEquals("claude", names["w1:p1"])
        assertEquals("opencode", names["w2:p1"])
    }

    @Test
    fun `collisions grow only as far as they must`() {
        // Same runtime, different repos: one qualifier is enough.
        val rows = listOf(
            row("w1:p1", workspace = "shep"),
            row("w2:p1", workspace = "workmayt"),
        )
        val names = distinctNames(rows)
        assertEquals("claude · shep", names["w1:p1"])
        assertEquals("claude · workmayt", names["w2:p1"])
    }

    @Test
    fun `same repo different branches separate on the branch`() {
        val rows = listOf(
            row("w1:p1", branch = "master"),
            row("w2:p1", branch = "billing/hardening"),
        )
        val names = distinctNames(rows)
        assertEquals("claude · shep · master", names["w1:p1"])
        assertEquals("claude · shep · billing/hardening", names["w2:p1"])
    }

    @Test
    fun `identical sessions fall through to tab then pane`() {
        val rows = listOf(
            row("w1:p1", tab = "review", pane = 1),
            row("w1:p2", tab = "review", pane = 2),
        )
        val names = distinctNames(rows)
        assertEquals("claude · shep · master · review · p1", names["w1:p1"])
        assertEquals("claude · shep · master · review · p2", names["w1:p2"])
    }

    @Test
    fun `a renamed session keeps its name and forces no one else deeper`() {
        val rows = listOf(
            row("w1:p1", agent = "billing fix"),
            row("w2:p1"),
            row("w3:p1", workspace = "workmayt"),
        )
        val names = distinctNames(rows)
        assertEquals("billing fix", names["w1:p1"])
        // The two unnamed ones only needed their workspace to separate.
        assertEquals("claude · shep", names["w2:p1"])
        assertEquals("claude · workmayt", names["w3:p1"])
    }

    @Test
    fun `indistinguishable sessions still get distinct names`() {
        // Nothing separates these — no tab, no pane number, same repo/branch.
        // The pane id is the last resort, and it must actually be used.
        val rows = listOf(row("w1:p1"), row("w1:p2"))
        val names = distinctNames(rows)
        assertEquals(2, names.values.toSet().size)
        assertTrue(names.getValue("w1:p1").endsWith("w1:p1"))
        assertTrue(names.getValue("w1:p2").endsWith("w1:p2"))
    }

    @Test
    fun `every session on a busy board gets a name and they are all different`() {
        val rows = (1..12).map { index ->
            row(
                "w$index:p1",
                agent = if (index % 3 == 0) "opencode" else "claude",
                workspace = if (index % 2 == 0) "shep" else "workmayt",
                branch = "master",
                tab = "t${index % 4}",
                pane = index % 2,
            )
        }
        val names = distinctNames(rows)
        assertEquals(rows.size, names.size)
        assertEquals(rows.size, names.values.toSet().size)
    }
}
