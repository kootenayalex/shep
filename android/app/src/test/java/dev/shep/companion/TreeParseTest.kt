package dev.shep.companion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * `session.snapshot` is three flat lists plus ids; the groups screen needs the
 * tree they describe. These pin the join, using the shape a real server sends.
 */
class TreeParseTest {

    /** Captured from a live `session.snapshot`, trimmed to the fields in use. */
    private val snapshot = """
    {"snapshot":{
      "focused_workspace_id":"w3","focused_tab_id":"w3:t1","focused_pane_id":"w3:p1",
      "workspaces":[
        {"workspace_id":"w1","label":"notify test","number":1,"agent_status":"working",
         "review_state":"needs_review","focused":false,"active_tab_id":"w1:t1",
         "pane_count":2,"tab_count":2},
        {"workspace_id":"w3","label":"shep","number":2,"agent_status":"idle",
         "review_state":"none","focused":true,"active_tab_id":"w3:t1",
         "pane_count":1,"tab_count":1,
         "worktree":{"repo_name":"shep","is_linked_worktree":true,
                     "repo_key":"k","repo_root":"/r","checkout_path":"/c"}}
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
         "agent_status":"working","cwd":"/Users/alex/vault/dev/shep","focused":false},
        {"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1",
         "agent_status":"unknown","cwd":"/Users/alex/vault/dev/shep","focused":false},
        {"pane_id":"w3:p1","tab_id":"w3:t1","workspace_id":"w3","label":"reviewer",
         "agent":"claude","agent_status":"idle","cwd":"/tmp/x","focused":true}
      ]
    }}
    """.trimIndent()

    private fun tree() = parseTree(JSONObject(snapshot))

    @Test
    fun `builds the group tab pane tree`() {
        val groups = tree()
        assertEquals(2, groups.size)
        assertEquals(listOf("w1", "w3"), groups.map { it.workspaceId })
        assertEquals(listOf(2, 1), groups.map { it.tabs.size })
        assertEquals(listOf("w1:p1"), groups[0].tabs[0].panes.map { it.paneId })
        assertEquals(listOf("w1:p2"), groups[0].tabs[1].panes.map { it.paneId })
    }

    /** Server order is session order — the phone must not re-sort the session. */
    @Test
    fun `preserves the order the server sent`() {
        val tabs = tree()[0].tabs
        assertEquals(listOf("w1:t1", "w1:t2"), tabs.map { it.tabId })
        assertEquals(listOf("1", "docs"), tabs.map { it.label })
    }

    @Test
    fun `carries review state and worktree facts`() {
        val groups = tree()
        assertEquals("needs_review", groups[0].reviewState)
        assertFalse(groups[0].isWorktree)
        assertNull(groups[0].worktreeRepo)
        assertEquals("shep", groups[1].worktreeRepo)
        assertTrue(groups[1].isWorktree)
        assertTrue(groups[1].focused)
    }

    /**
     * A pane's manual label is the name someone chose for it, so it wins over
     * the detected runtime; a pane with neither is a shell, and says so.
     */
    @Test
    fun `pane name prefers the label it was given`() {
        val groups = tree()
        assertEquals("claude", groups[0].tabs[0].panes[0].agent)
        assertNull(groups[0].tabs[1].panes[0].agent)
        assertEquals("reviewer", groups[1].tabs[0].panes[0].agent)
    }

    /**
     * The server refuses to close a group's last tab, so the screen has to know
     * which groups those are rather than offering a button that only errors.
     */
    @Test
    fun `knows when a group has no closable tab`() {
        val groups = tree()
        assertFalse(groups[0].hasOnlyOneTab)
        assertTrue(groups[1].hasOnlyOneTab)
    }

    @Test
    fun `an empty or malformed snapshot yields no groups`() {
        assertEquals(emptyList<GroupNode>(), parseTree(JSONObject("{}")))
        assertEquals(emptyList<GroupNode>(), parseTree(JSONObject("""{"snapshot":{}}""")))
    }
}
