package dev.shep.companion.screens

import dev.shep.companion.AgentRow
import dev.shep.companion.parseTree
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The list is a join of the two things the server will say about a session:
 * the tree decides what exists and in what order, the overview decides what
 * each agent is. These pin the join, because getting it wrong is silent —
 * a dropped shell just is not there, and a stale name looks like a name.
 */
class ChannelsTest {

    /** Trimmed from a live `session.snapshot`: two spaces, three panes, one a shell. */
    private val snapshot = """
    {"snapshot":{
      "focused_workspace_id":"w3","focused_tab_id":"w3:t1","focused_pane_id":"w3:p1",
      "workspaces":[
        {"workspace_id":"w1","label":"shiftmayt","number":1,"agent_status":"working",
         "review_state":"none","focused":false,"active_tab_id":"w1:t1",
         "pane_count":2,"tab_count":2},
        {"workspace_id":"w3","label":"shep","number":2,"agent_status":"idle",
         "review_state":"none","focused":true,"active_tab_id":"w3:t1",
         "pane_count":1,"tab_count":1}
      ],
      "tabs":[
        {"tab_id":"w1:t1","workspace_id":"w1","label":"1","number":1,
         "agent_status":"working","focused":false,"pane_count":1},
        {"tab_id":"w1:t2","workspace_id":"w1","label":"docs","number":2,
         "agent_status":"idle","focused":false,"pane_count":1},
        {"tab_id":"w3:t1","workspace_id":"w3","label":"1","number":1,
         "agent_status":"idle","focused":true,"pane_count":1}
      ],
      "panes":[
        {"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","agent":"claude",
         "agent_status":"working","cwd":"/Users/alex/vault/dev/shiftmayt","focused":false},
        {"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1",
         "agent_status":"unknown","cwd":"/Users/alex/vault/dev/shiftmayt","focused":false},
        {"pane_id":"w3:p1","tab_id":"w3:t1","workspace_id":"w3","agent":"claude",
         "agent_status":"idle","cwd":"/Users/alex/vault/dev/shep","focused":true}
      ]
    }}
    """.trimIndent()

    private fun tree() = parseTree(JSONObject(snapshot))

    private fun row(
        paneId: String,
        workspaceId: String = paneId.substringBefore(":"),
        workspaceLabel: String = workspaceId,
        agent: String = "claude",
        status: String = "idle",
        displayName: String? = null,
        reviewState: String = "none",
        queuedInput: Int = 0,
    ) = AgentRow(
        terminalId = "",
        paneId = paneId,
        workspaceId = workspaceId,
        workspaceLabel = workspaceLabel,
        agent = agent,
        status = status,
        contextPercent = null,
        reviewState = reviewState,
        customStatus = null,
        worktreeRepo = null,
        isWorktree = false,
        memoryPercent = null,
        displayName = displayName,
        queuedInput = queuedInput,
    )

    @Test
    fun `sections follow the session's own order, not an attention sort`() {
        val sections = buildChannels(tree(), emptyList())
        assertEquals(listOf("w1", "w3"), sections.map { it.workspaceId })
        assertEquals(listOf("shiftmayt", "shep"), sections.map { it.label })
        assertEquals(listOf("w1:p1", "w1:p2"), sections[0].channels.map { it.paneId })
    }

    /**
     * The reason the tree is the spine. `session.overview` answers with agents,
     * so a plain shell is simply not in it — and dropping shells from the list
     * would make panes you can see on the desktop unreachable from the phone.
     */
    @Test
    fun `a pane with no agent still gets a row`() {
        val shell = buildChannels(tree(), emptyList())[0].channels[1]
        assertEquals("w1:p2", shell.paneId)
        assertEquals("shell", shell.name)
        assertNull(shell.row)
    }

    @Test
    fun `the overview decides the name and the state`() {
        val sections = buildChannels(
            tree(),
            listOf(row("w1:p1", displayName = "Orbit", status = "blocked")),
        )
        val agent = sections[0].channels[0]
        assertEquals("Orbit", agent.name)
        assertEquals("blocked", agent.status)
        // The tree's own status is the fallback, not the answer.
        assertEquals("unknown", sections[0].channels[1].status)
    }

    /** The tab is kept per row so "go to" still has something to focus. */
    @Test
    fun `each row remembers which tab it lives in`() {
        val sections = buildChannels(tree(), emptyList())
        assertEquals(listOf("w1:t1", "w1:t2"), sections[0].channels.map { it.tabId })
    }

    /**
     * A snapshot that failed while the overview answered must still produce a
     * list. It loses the shells and the ordering, which is a worse list — but
     * an empty screen is not a list at all.
     */
    @Test
    fun `without a tree it groups what the overview gave`() {
        val sections = buildChannels(
            emptyList(),
            listOf(
                row("w1:p1", workspaceLabel = "shiftmayt", displayName = "Orbit"),
                row("w3:p1", workspaceLabel = "shep"),
            ),
        )
        assertEquals(listOf("shiftmayt", "shep"), sections.map { it.label })
        assertEquals("Orbit", sections[0].channels[0].name)
    }

    @Test
    fun `an empty session is an empty list, not a section of nothing`() {
        assertTrue(buildChannels(emptyList(), emptyList()).isEmpty())
    }

    /**
     * Latching the overview off on any error is what made one dropped packet
     * permanently downgrade a connection to the thinner snapshot. Only the
     * server saying it does not have the method should do that.
     */
    @Test
    fun `only a real unknown-method error latches the fallback`() {
        assertTrue(looksUnsupported("unknown variant `session.overview`, expected one of ..."))
        assertTrue(looksUnsupported("unknown method"))
        assertFalse(looksUnsupported("session.overview timed out"))
        assertFalse(looksUnsupported("Software caused connection abort"))
        assertFalse(looksUnsupported(null))
    }

    /** What the space heading counts, now that nothing is filtered out of view. */
    @Test
    fun `attention is blocked plus finished-and-unseen`() {
        assertTrue(needsAttention("blocked"))
        assertTrue(needsAttention("done"))
        assertFalse(needsAttention("working"))
        assertFalse(needsAttention("idle"))
        assertFalse(needsAttention("unknown"))
    }

    /**
     * The list shows every pane. A default of "attention" answered "what is
     * running?" with an empty screen whenever nothing was blocked, which is
     * most of the time.
     */
    @Test
    fun `every pane is on the list whatever its state`() {
        val sections = buildChannels(
            tree(),
            listOf(
                row("w1:p1", status = "working"),
                row("w3:p1", status = "idle"),
            ),
        )
        assertEquals(3, sections.sumOf { it.channels.size })
        assertEquals(0, sections.sumOf { s -> s.channels.count { needsAttention(it.status) } })
    }

    @Test
    fun `prototype filters keep blocked working review and queued attention visible`() {
        val channels = listOf(
            Channel("w:p1", "blocked", "blocked", row("w:p1", status = "blocked"), null, null),
            Channel("w:p2", "working", "working", row("w:p2", status = "working"), null, null),
            Channel("w:p3", "review", "idle", row("w:p3", reviewState = "needs_review"), null, null),
            Channel("w:p4", "queued", "idle", row("w:p4", queuedInput = 2), null, null),
            Channel("w:p5", "idle", "idle", row("w:p5"), null, null),
        )

        assertEquals(4, channels.count { matchesAgentFilter(it, AgentFilter.Attention) })
        assertEquals(1, channels.count { matchesAgentFilter(it, AgentFilter.Review) })
        assertEquals(1, channels.count { matchesAgentFilter(it, AgentFilter.Queued) })
        assertEquals(5, channels.count { matchesAgentFilter(it, AgentFilter.All) })
    }
}
