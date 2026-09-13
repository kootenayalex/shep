package dev.shep.companion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the `overseer.sample` payload and the four model functions the board is
 * built out of, against the Rust side (`OverseerSample` in
 * src/api/schema/overseer.rs, `first_sentence` and `proposals` in
 * src/app/overseer.rs, `overseer_model` in src/ui/overseer.rs).
 *
 * Drift here is silent in the worst way: the board still renders, just with an
 * empty right-hand side, which reads as "the overseer has nothing to say"
 * rather than as "the phone stopped understanding the answer".
 */
class OverseerParseTest {

    private val full = """
    {"sample":{
      "plugin_linked":true,"sampled":true,
      "narrative":["claude is blocked on a permission prompt.","two proposals on the board"],
      "source":"brain","tick_at":"07:08","situation_age_seconds":42,
      "brain_age_seconds":180,"runtime":"claude","tick_in_flight":false,
      "health":[
        {"level":"ok","check":"server","detail":"running"},
        {"level":"warn","check":"disk","detail":"9.8 G free","fix":"free some"},
        {"level":"fail","check":"bridge","detail":"not listening"}
      ],
      "chat":[
        {"at":1757000000,"role":"you","text":"what needs me?"},
        {"at":1757000040,"role":"overseer","text":"workmayt's claude is blocked."}
      ],
      "chat_total":12,"chat_pending":false,
      "session":{"id":"e5f0","started":true}}}
    """.trimIndent()

    @Test
    fun `reads a full sample as the server sends it`() {
        val sample = parseOverseerSample(JSONObject(full))
        assertTrue(sample.pluginLinked)
        assertTrue(sample.sampled)
        assertEquals(2, sample.narrative.size)
        assertEquals("brain", sample.source)
        assertEquals("07:08", sample.tickAt)
        assertEquals(42L, sample.situationAgeSeconds)
        assertEquals(180L, sample.brainAgeSeconds)
        assertEquals("claude", sample.runtime)
        assertFalse(sample.tickInFlight)
        assertEquals(
            listOf(HealthLevel.Ok, HealthLevel.Warn, HealthLevel.Fail),
            sample.health.map { it.level },
        )
        assertEquals("free some", sample.health[1].fix)
        assertNull(sample.health[0].fix)
        assertEquals(listOf(ChatRole.You, ChatRole.Overseer), sample.chat.map { it.role })
        assertEquals(1757000040L, sample.chat[1].at)
        assertEquals(12L, sample.chatTotal)
        assertEquals("e5f0", sample.sessionId)
        assertTrue(sample.sessionStarted)
    }

    /**
     * An overseer that has never run answers with a sample, not with an error.
     * Absent lists are empty and absent facts are null — never a plausible
     * zero, which the header would print as a real tick time.
     */
    @Test
    fun `an empty sample says so rather than inventing facts`() {
        val sample = parseOverseerSample(JSONObject("""{"sample":{}}"""))
        assertFalse(sample.pluginLinked)
        assertFalse(sample.sampled)
        assertEquals(emptyList<String>(), sample.narrative)
        assertEquals("deterministic", sample.source)
        assertNull(sample.tickAt)
        assertNull(sample.runtime)
        assertNull(sample.sessionId)
        assertEquals(emptyList<ChatTurn>(), sample.chat)
        assertEquals(0L, sample.chatTotal)
    }

    /** A payload that is not a sample at all still parses, so the board degrades. */
    @Test
    fun `a missing sample parses as an unsampled one`() {
        val sample = parseOverseerSample(JSONObject("""{"snapshot":{}}"""))
        assertFalse(sample.sampled)
        assertFalse(sample.pluginLinked)
    }

    /**
     * The cases from `first_sentence_skips_the_header` in src/app/overseer.rs,
     * line for line: both surfaces cut the strip's sentence the same way or
     * the phone and the desk quote the overseer differently.
     */
    @Test
    fun `the first sentence skips the header, exactly as the desktop cuts it`() {
        val cases = listOf(
            listOf("# BOARD — now", "first thing. second thing.") to "first thing.",
            listOf("OVERSEER · 07:08", "", "all quiet! nothing owed") to "all quiet!",
            listOf("no header here", "second line") to "no header here",
            listOf("one sentence with no stop") to "one sentence with no stop",
            listOf("# only a header") to null,
            emptyList<String>() to null,
        )
        cases.forEach { (lines, want) ->
            assertEquals(lines.toString(), want, firstSentence(lines))
        }
        // `? ` cuts too, and the earliest mark wins when a line has two.
        assertEquals("why is it blocked?", firstSentence(listOf("why is it blocked? because.")))
    }

    // ── proposals ───────────────────────────────────────────────────────────

    private fun item(
        id: Long,
        updated: String,
        sourceKind: String?,
        status: DocketStatus = DocketStatus.Inbox,
    ) = DocketItem(
        id = id,
        title = "item $id",
        kind = DocketKind.Captured,
        status = status,
        due = null,
        repeat = null,
        sourceLabel = sourceKind?.let { "$it p$id" },
        sourceKind = sourceKind,
        notes = null,
        overdue = false,
        updated = updated,
        dueToday = false,
    )

    /**
     * The desktop's `proposals_are_situation_sourced_newest_first_max_five`
     * (src/app/overseer.rs:1332), rebuilt on the phone's own types.
     */
    @Test
    fun `proposals are situation-sourced, newest first, at most five`() {
        val docket = Docket(
            today = "2026-09-12",
            items = listOf(
                item(1, "2026-09-10T10:00:00Z", "situation"),
                item(2, "2026-09-12T10:00:00Z", "situation"),
                item(3, "2026-09-11T10:00:00Z", null), // a pane source, not the overseer
                item(4, "2026-09-11T10:00:00Z", null),
                item(5, "2026-09-11T12:00:00Z", "situation"),
                item(6, "2026-09-11T11:00:00Z", "situation"),
                item(7, "2026-09-11T09:00:00Z", "situation"),
                item(8, "2026-09-11T08:00:00Z", "situation"),
                item(9, "2026-09-13T00:00:00Z", "situation", status = DocketStatus.Done),
            ),
        )
        assertEquals(listOf(2L, 5L, 6L, 7L, 8L), proposals(docket).map { it.id })
        assertEquals(MAX_PROPOSALS, proposals(docket).size)
    }

    @Test
    fun `a docket with nothing captured proposes nothing`() {
        val docket = Docket("2026-09-12", listOf(item(1, "u", null)))
        assertEquals(emptyList<DocketItem>(), proposals(docket))
    }

    // ── the docket region ───────────────────────────────────────────────────

    /**
     * The board's docket is the due lane whole plus the inbox minus the
     * proposals — a captured item is not listed twice on one screen.
     */
    @Test
    fun `the board's docket counts the lanes and subtracts the proposals`() {
        val due = DocketItem(
            id = 10, title = "rotate the key", kind = DocketKind.Slated,
            status = DocketStatus.Open, due = "2026-09-08", repeat = null,
            sourceLabel = null, sourceKind = null, notes = null, overdue = true,
            updated = "2026-09-01T00:00:00Z", dueToday = false,
        )
        val docket = Docket(
            today = "2026-09-12",
            items = listOf(
                due,
                item(1, "2026-09-12T10:00:00Z", "situation"),
                item(2, "2026-09-11T10:00:00Z", null),
            ),
        )
        val cards = proposals(docket)
        val board = boardDocket(docket, cards)
        assertEquals(listOf(1L), cards.map { it.id })
        assertEquals(1, board.due)
        assertEquals(1, board.overdue)
        assertEquals(2, board.inbox)
        // The due item and the inbox item that is not a proposal; #1 is not
        // listed here because it has a region of its own.
        assertEquals(listOf(10L, 2L), board.rows.map { it.id })
    }

    // ── needs you ───────────────────────────────────────────────────────────

    private fun agent(
        paneId: String,
        status: String,
        said: String? = null,
        ahead: Int? = null,
    ) = AgentRow(
        terminalId = paneId, paneId = paneId, workspaceId = "w", workspaceLabel = "shep",
        agent = "claude", status = status, contextPercent = null, reviewState = "",
        customStatus = null, worktreeRepo = null, isWorktree = false, memoryPercent = null,
        activityLine = said, gitAhead = ahead,
    )

    /**
     * Blocked first, then finished-and-unseen, board order holding inside each
     * — the desktop sorts the same way and for the same reason: the question
     * "who needs me" has an order, and it is urgency.
     */
    @Test
    fun `needs you puts the blocked agents first and carries the unpushed count`() {
        val rows = listOf(
            agent("p1", "done", said = "all tests green", ahead = 3),
            agent("p2", "working"),
            agent("p3", "blocked", said = "? make this edit"),
            agent("p4", "idle"),
            agent("p5", "done", said = "nothing to push"),
        )
        val waiting = needsYou(rows)
        assertEquals(listOf("p3", "p1", "p5"), waiting.map { it.row.paneId })
        assertTrue(waiting[0].blocked)
        assertEquals("? make this edit", waiting[0].detail)
        // A blocked agent has nothing to push; the hint is "answer it".
        assertNull(waiting[0].ahead)
        assertEquals(3, waiting[1].ahead)
        assertNull(waiting[2].ahead)
        // Working and idle agents are not waiting on anybody.
        assertEquals(3, waiting.size)
    }

    /** With no screen line, the row still says something rather than nothing. */
    @Test
    fun `an agent with nothing on its screen falls back to its state word`() {
        assertEquals("blocked", needsYou(listOf(agent("p1", "blocked"))).single().detail)
    }
}
