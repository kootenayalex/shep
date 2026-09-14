package dev.shep.companion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the `overseer.sample` payload and the model functions the board is built
 * out of, against the Rust side (`OverseerSample` in
 * src/api/schema/overseer.rs, `first_sentence` and `narrative_sections` in
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
      "narrative":["claude","is blocked on a permission prompt.","codex","shipped the parser.",
                   "room","one item due today."],
      "sections":[
        {"title":"claude","lines":["is blocked on a permission prompt."]},
        {"title":"codex","lines":["shipped the parser."]},
        {"title":"room","lines":["one item due today."]}
      ],
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
        assertEquals(6, sample.narrative.size)
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
        assertEquals(emptyList<NarrativeSection>(), sample.sections)
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

    // ── sections ────────────────────────────────────────────────────────────

    /**
     * The read of the room arrives cut into sections — one per agent, titled
     * with the same `display_name` `session.overview` sends, then `room` last
     * — and an older server that sends none degrades to one titleless section
     * holding the whole narrative rather than to a blank region.
     */
    @Test
    fun `sections parse with titles and fall back to one prose section when absent`() {
        val sections = parseOverseerSample(JSONObject(full)).sections
        assertEquals(listOf("claude", "codex", "room"), sections.map { it.title })
        assertEquals(listOf("is blocked on a permission prompt."), sections[0].lines)
        assertEquals(listOf("one item due today."), sections[2].lines)

        val older = parseOverseerSample(
            JSONObject("""{"sample":{"sampled":true,"narrative":["all quiet.","nothing owed."]}}"""),
        )
        assertEquals(listOf(NarrativeSection(null, older.narrative)), older.sections)
        assertNull(older.sections.single().title)

        // A board with nothing on it implies no sections at all, so the region
        // says "the overseer has not spoken yet" instead of drawing an empty one.
        val blank = parseOverseerSample(JSONObject("""{"sample":{"sampled":true}}"""))
        assertEquals(emptyList<NarrativeSection>(), blank.sections)
    }

    /** A section with no title is prose; one with no lines is a heading and nothing else. */
    @Test
    fun `a titleless section and an empty one both survive the parse`() {
        val sample = parseOverseerSample(
            JSONObject(
                """{"sample":{"sampled":true,"narrative":["a"],
                   "sections":[{"title":null,"lines":["a"]},{"title":"room","lines":[]}]}}""",
            ),
        )
        assertEquals(listOf(null, "room"), sample.sections.map { it.title })
        assertEquals(emptyList<String>(), sample.sections[1].lines)
    }

    // ── seating the sections under the rows ─────────────────────────────────

    private fun agent(
        paneId: String,
        status: String = "working",
        name: String? = null,
        group: String = "shep",
    ) = AgentRow(
        terminalId = paneId, paneId = paneId, workspaceId = group, workspaceLabel = group,
        agent = "claude", status = status, contextPercent = null, reviewState = "",
        customStatus = null, worktreeRepo = null, isWorktree = false, memoryPercent = null,
        displayName = name,
    )

    private fun section(title: String?, vararg lines: String) =
        NarrativeSection(title, lines.toList())

    /**
     * Each agent's paragraph goes under that agent's row — by display name, or
     * by `name · group` when the tick had to disambiguate two agents with the
     * same name — and `room` is never an agent's, so the region below keeps it.
     */
    @Test
    fun `sections are seated under the row they are about, room excepted`() {
        val rows = listOf(
            agent("p1", name = "claude", group = "workmayt"),
            agent("p2", name = "claude", group = "emberline"),
            agent("p3", name = "codex"),
        )
        val sections = listOf(
            section("claude · workmayt", "blocked on a prompt."),
            section("claude · emberline", "done and unseen."),
            section("codex", "shipped the parser."),
            section(ROOM_SECTION, "one item due today."),
        )
        val seated = seatSections(rows, sections)
        assertEquals(setOf("p1", "p2", "p3"), seated.byPane.keys)
        assertEquals(listOf("blocked on a prompt."), seated.byPane.getValue("p1").lines)
        assertEquals(listOf("shipped the parser."), seated.byPane.getValue("p3").lines)
        assertEquals(listOf(ROOM_SECTION), seated.remaining.map { it.title })
    }

    /**
     * A section whose agent is no longer running stays for the region — with
     * its heading, which is what tells it from the room's own prose — and the
     * room comes first there because the region is named after it.
     */
    @Test
    fun `what no row claims is left for the region, room first`() {
        val sections = listOf(
            section(null, "an older server sent no sections."),
            section("codex", "closed between the tick and now."),
            section(ROOM_SECTION, "nothing owed."),
        )
        val seated = seatSections(listOf(agent("p1", name = "claude")), sections)
        assertTrue(seated.byPane.isEmpty())
        assertEquals(listOf(ROOM_SECTION, null, "codex"), seated.remaining.map { it.title })
    }

    /** One bare-name section cannot be seated under two rows of that name. */
    @Test
    fun `a section is seated at most once`() {
        val rows = listOf(agent("p1", name = "claude"), agent("p2", name = "claude"))
        val seated = seatSections(rows, listOf(section("claude", "only one of you.")))
        assertEquals(listOf("p1"), seated.byPane.keys.toList())
        assertTrue(seated.remaining.isEmpty())
    }

    /** With no board at all there is nothing to seat and nothing left over. */
    @Test
    fun `no sections seats nothing`() {
        val seated = seatSections(listOf(agent("p1")), emptyList())
        assertTrue(seated.byPane.isEmpty())
        assertTrue(seated.remaining.isEmpty())
    }

    /**
     * The agents strip has one line, and since the board now seats each
     * agent's paragraph under its own row, that line is the room's — not
     * whichever agent the tick wrote about first.
     */
    @Test
    fun `the strip quotes the room section when the sample has one`() {
        val sample = parseOverseerSample(JSONObject(full))
        assertEquals("one item due today.", firstSentence(sample))
        // No sections at all: the narrative's own first sentence, as before.
        val older = parseOverseerSample(
            JSONObject("""{"sample":{"sampled":true,"narrative":["all quiet. nothing owed."]}}"""),
        )
        assertEquals("all quiet.", firstSentence(older))
        // A room section with nothing in it falls back rather than blanking
        // the strip: there is a board, so there is something to quote.
        val empty = parseOverseerSample(
            JSONObject(
                """{"sample":{"sampled":true,"narrative":["codex shipped it."],
                   "sections":[{"title":"codex","lines":["codex shipped it."]},
                               {"title":"room","lines":[]}]}}""",
            ),
        )
        assertEquals("codex shipped it.", firstSentence(empty))
    }
}
