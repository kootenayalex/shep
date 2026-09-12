package dev.shep.companion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DocketParseTest {
    private fun parse(raw: String): Docket = parseDocket(JSONObject(raw))

    @Test
    fun `reads a docket list as the server sends it`() {
        val docket = parse(
            """
            {"today":"2026-09-11","items":[
              {"id":1,"title":"file the register","kind":"captured","status":"inbox",
               "source":{"file":"/Users/alex/.claude/projects/-Users-alex/memory/MEMORY.md","line":12},
               "notes":"A5 on the board\nsecond line","created":"2026-09-11T01:00:00Z","updated":"2026-09-11T01:00:00Z","overdue":false},
              {"id":2,"title":"rotate the key","kind":"slated","status":"open","due":"2026-09-08",
               "created":"2026-09-01T00:00:00Z","updated":"2026-09-01T00:00:00Z","overdue":true}
            ]}
            """.trimIndent()
        )

        assertEquals("2026-09-11", docket.today)
        assertEquals(2, docket.items.size)
        val first = docket.items[0]
        assertEquals(1L, first.id)
        assertEquals(DocketKind.Captured, first.kind)
        assertEquals(DocketStatus.Inbox, first.status)
        assertEquals("MEMORY.md:12", first.sourceLabel)
        assertEquals("A5 on the board\nsecond line", first.notes)
        assertNull(first.due)
        assertFalse(first.overdue)
        val second = docket.items[1]
        assertEquals(DocketKind.Slated, second.kind)
        assertEquals("2026-09-08", second.due)
        assertTrue(second.overdue)
        assertFalse(second.dueToday)
    }

    /** org.json turns a JSON null into the literal string "null" via optString. */
    @Test
    fun `nulls parse as absent, not the word null`() {
        val item = parse(
            """{"today":"2026-09-11","items":[{"id":3,"title":"bare","kind":"slated","status":"open","due":null,"repeat":null,"source":null,"notes":null,"created":"c","updated":"u"}]}"""
        ).items.single()

        assertNull(item.due)
        assertNull(item.repeat)
        assertNull(item.sourceLabel)
        assertNull(item.notes)
        assertFalse(item.overdue)
    }

    @Test
    fun `due today is judged against the store's today, not the phone's`() {
        val item = parse(
            """{"today":"2026-09-11","items":[{"id":4,"title":"t","kind":"recurring","status":"open","due":"2026-09-11","repeat":"1w","created":"c","updated":"u","overdue":false}]}"""
        ).items.single()

        assertTrue(item.dueToday)
        assertEquals("1w", item.repeat)
        // Only an open item is due today: done keeps its date but not the flag.
        val done = parseDocketItem(
            JSONObject("""{"id":5,"title":"t","kind":"slated","status":"done","due":"2026-09-11","created":"c","updated":"u"}"""),
            "2026-09-11",
        )
        assertFalse(done.dueToday)
    }

    @Test
    fun `an unknown kind or status falls back rather than crashing`() {
        val item = parse(
            """{"today":"2026-09-11","items":[{"id":6,"title":"t","kind":"future","status":"weird","created":"c","updated":"u"}]}"""
        ).items.single()
        assertEquals(DocketKind.Captured, item.kind)
        assertEquals(DocketStatus.Inbox, item.status)
    }

    /** Mirrors `source_tags` in src/ui/board.rs, plus the overseer's `{kind, ref}`. */
    @Test
    fun `source labels match the desktop's`() {
        assertEquals("memory.md:12", docketSourceLabel(JSONObject("""{"file":"/a/b/memory.md","line":12}""")))
        assertEquals("memory.md", docketSourceLabel(JSONObject("""{"file":"/a/b/memory.md"}""")))
        assertEquals("pane p3", docketSourceLabel(JSONObject("""{"pane":"p3","session":"s"}""")))
        assertEquals("situation SHEP", docketSourceLabel(JSONObject("""{"kind":"situation","ref":"SHEP"}""")))
        assertNull(docketSourceLabel(JSONObject("""{"other":1}""")))
        assertNull(docketSourceLabel(null))
    }

    /** Mirrors `DueLabel::text` in src/ui/board.rs. */
    @Test
    fun `date rows read as the desktop's`() {
        assertEquals("overdue 3d", docketDueLabel("2026-09-08", "2026-09-11"))
        assertEquals("due today", docketDueLabel("2026-09-11", "2026-09-11"))
        assertEquals("in 5d", docketDueLabel("2026-09-16", "2026-09-11"))
        assertEquals("—", docketDueLabel(null, "2026-09-11"))
        // A date the phone cannot read is shown rather than hidden.
        assertEquals("someday", docketDueLabel("someday", "2026-09-11"))
    }

    @Test
    fun `the date field takes a real calendar date only`() {
        assertTrue(isDocketDate("2026-10-07"))
        assertFalse(isDocketDate("2026-13-07"))
        assertFalse(isDocketDate("2026-02-30"))
        assertFalse(isDocketDate("07/10/2026"))
        assertFalse(isDocketDate(""))
    }

    /** Mirrors `docket_board_model` in src/ui/board.rs: five lanes, always, discarded nowhere. */
    @Test
    fun `lanes bucket like the desktop board`() {
        val docket = parse(
            """
            {"today":"2026-09-11","items":[
              {"id":1,"title":"a","kind":"captured","status":"inbox","created":"c","updated":"2026-09-11T00:00:01Z"},
              {"id":2,"title":"b","kind":"slated","status":"open","due":"2026-09-08","overdue":true,"created":"c","updated":"u"},
              {"id":3,"title":"c","kind":"slated","status":"open","due":"2026-09-11","created":"c","updated":"u"},
              {"id":4,"title":"d","kind":"slated","status":"open","due":"2026-10-07","created":"c","updated":"u"},
              {"id":5,"title":"e","kind":"slated","status":"open","created":"c","updated":"u"},
              {"id":6,"title":"f","kind":"recurring","status":"open","due":"2026-09-18","repeat":"1w","created":"c","updated":"u"},
              {"id":7,"title":"g","kind":"recurring","status":"open","due":"2026-09-11","repeat":"1d","created":"c","updated":"u"},
              {"id":8,"title":"h","kind":"slated","status":"done","created":"c","updated":"2026-09-10T00:00:00Z"},
              {"id":9,"title":"i","kind":"slated","status":"done","created":"c","updated":"2026-09-11T00:00:00Z"},
              {"id":10,"title":"j","kind":"captured","status":"discarded","created":"c","updated":"u"}
            ]}
            """.trimIndent()
        )

        val lanes = docketLanes(docket).associate { (lane, rows) -> lane to rows.map { it.id } }
        assertEquals(DocketLane.entries.toSet(), lanes.keys)
        assertEquals(listOf(1L), lanes[DocketLane.Inbox])
        // Overdue first, then today's — a recurring item due today is due, not recurring.
        assertEquals(listOf(2L, 3L, 7L), lanes[DocketLane.Due])
        assertEquals(listOf(4L, 5L), lanes[DocketLane.Slated])
        assertEquals(listOf(6L), lanes[DocketLane.Recurring])
        // Newest done first; discarded is not on the board.
        assertEquals(listOf(9L, 8L), lanes[DocketLane.Done])
    }

    @Test
    fun `the done lane keeps only the newest ten`() {
        val items = (1..12).joinToString(",") {
            """{"id":$it,"title":"t","kind":"slated","status":"done","created":"c","updated":"2026-09-${"%02d".format(it)}T00:00:00Z"}"""
        }
        val lanes = docketLanes(parse("""{"today":"2026-09-11","items":[$items]}""")).toMap()
        assertEquals(DOCKET_DONE_LIMIT, lanes.getValue(DocketLane.Done).size)
        assertEquals(12L, lanes.getValue(DocketLane.Done).first().id)
    }
}
