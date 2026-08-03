package dev.shep.companion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Pins the `session.overview` payload shape against the Rust side
 * (`SessionOverview` in src/api/schema/session.rs).
 *
 * A drift here is silent: the board still renders, just with blank cards and
 * zeroed totals, which reads as "nothing is running" rather than as an error.
 */
class OverviewParseTest {

    private fun payload(agent: String) = JSONObject(
        """
        {"overview":{
          "totals":{"agents":2,"blocked":1,"done":0,"working":1,"idle":0,"attention":1,
                    "workspaces":2,"tabs":3,"panes":5,"queued_input":2,"pending_tasks":4},
          "host":{"version":"0.7.3","load_percent":26,"cores":12,
                  "memory_percent":17,"memory_total_bytes":25769803776,
                  "memory_used_bytes":4380866641},
          "agents":[$agent]}}
        """.trimIndent()
    )

    private val fullAgent = """
        {"pane_id":"w1:p2","workspace_id":"w1","tab_id":"t1","tab_name":"review",
         "pane_number":2,"workspace_label":"shep","branch":"master","name":"claude",
         "display_agent":"opus","agent_status":"blocked","unseen":true,
         "custom_status":"waiting","activity_line":"waiting for approval",
         "context_percent":62,"cwd":"~/vault/dev/shep","state_age_seconds":240,
         "queued_input":2,"focused":true}
    """.trimIndent()

    @Test
    fun `parses totals and host vitals`() {
        val overview = parseOverview(payload(fullAgent))!!
        assertEquals(2, overview.totals.agents)
        assertEquals(1, overview.totals.attention)
        assertEquals(5, overview.totals.panes)
        assertEquals(4, overview.totals.pendingTasks)
        assertEquals(12, overview.host.cores)
        assertEquals(25769803776L, overview.host.memoryTotalBytes)
    }

    @Test
    fun `parses the placement and display facts a card needs`() {
        val row = parseOverview(payload(fullAgent))!!.agents.single()
        assertEquals("claude", row.agent)
        assertEquals("blocked", row.status)
        assertEquals("review", row.tabName)
        assertEquals("opus", row.displayAgent)
        assertEquals("waiting for approval", row.activityLine)
        assertEquals(62, row.contextPercent)
        assertEquals(240L, row.stateAgeSeconds)
        assertEquals(2, row.queuedInput)
    }

    @Test
    fun `location prefers the tab name and drops an absent pane number`() {
        val row = parseOverview(payload(fullAgent))!!.agents.single()
        assertEquals("review·p2", row.location)

        val noPane = """{"pane_id":"w1:p1","tab_name":"docs","agent_status":"idle"}"""
        assertEquals("docs", parseOverview(payload(noPane))!!.agents.single().location)

        val noTab = """{"pane_id":"w1:p1","pane_number":3,"agent_status":"idle"}"""
        assertEquals("p3", parseOverview(payload(noTab))!!.agents.single().location)
    }

    @Test
    fun `absent optional fields stay null rather than becoming zero or empty`() {
        // The strip must be able to tell "unknown" from "zero" — an absent
        // vital renders an em dash, a zero would be a false claim.
        val bare = """{"pane_id":"w1:p1","agent_status":"idle"}"""
        val overview = parseOverview(
            JSONObject("""{"overview":{"totals":{},"host":{},"agents":[$bare]}}""")
        )!!
        assertNull(overview.host.loadPercent)
        assertNull(overview.host.memoryPercent)
        assertNull(overview.host.version)
        assertNull(overview.totals.pendingTasks)
        val row = overview.agents.single()
        assertNull(row.contextPercent)
        assertNull(row.activityLine)
        assertNull(row.location)
        assertEquals(0, row.queuedInput)
    }

    @Test
    fun `server order is preserved so the phone matches the desktop board`() {
        // The server sorts by attention; re-sorting here would let the two
        // views disagree about what matters most.
        val agents = """
            {"pane_id":"a","name":"first","agent_status":"blocked"},
            {"pane_id":"b","name":"second","agent_status":"idle"},
            {"pane_id":"c","name":"third","agent_status":"working"}
        """.trimIndent()
        val rows = parseOverview(payload(agents))!!.agents
        assertEquals(listOf("first", "second", "third"), rows.map { it.agent })
    }

    @Test
    fun `a non-overview payload parses as null so the caller can fall back`() {
        assertNull(parseOverview(JSONObject("""{"snapshot":{"agents":[]}}""")))
    }

    @Test
    fun `totals derived from rows when the server is too old`() {
        val rows = listOf(
            row("blocked", queued = 1),
            row("done"),
            row("working"),
            row("idle"),
        )
        val totals = totalsFromRows(rows)
        assertEquals(4, totals.agents)
        assertEquals(2, totals.attention, )
        assertEquals(1, totals.queuedInput)
        // Session shape and vitals are genuinely unknown from a snapshot.
        assertEquals(0, totals.panes)
        assertNull(totals.pendingTasks)
    }

    private fun row(status: String, queued: Int = 0) = AgentRow(
        terminalId = "t", paneId = status, workspaceId = "w", workspaceLabel = "ws",
        agent = "claude", status = status, contextPercent = null, reviewState = "",
        customStatus = null, worktreeRepo = null, isWorktree = false, memoryPercent = null,
        queuedInput = queued,
    )

    @Test
    fun `human bytes and ages read as magnitudes`() {
        assertEquals("24.0G", humanBytes(25769803776L))
        assertEquals("1.0K", humanBytes(1024))
        assertEquals("512B", humanBytes(512))
        assertEquals("45s", formatAge(45))
        assertEquals("4m", formatAge(240))
        assertEquals("2h", formatAge(7200))
    }
}
