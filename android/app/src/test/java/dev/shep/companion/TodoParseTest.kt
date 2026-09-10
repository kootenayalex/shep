package dev.shep.companion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TodoParseTest {
    private fun parse(raw: String): Todos? = parseTodos(JSONObject(raw))

    @Test
    fun `reads the checklist in the order the server sent it`() {
        val todos = parse(
            """
            {"todos": {"session_id": "abc", "source": "store", "items": [
              {"id":"1","subject":"first","activeForm":"doing first","description":"why","status":"completed","blocks":[],"blockedBy":[]},
              {"id":"2","subject":"second","activeForm":"doing second","description":"","status":"in_progress","blocks":[],"blockedBy":["1"]}
            ]}}
            """.trimIndent()
        )

        assertEquals(2, todos?.items?.size)
        assertEquals("store", todos?.source)
        assertEquals("abc", todos?.sessionId)
        assertEquals("first", todos?.items?.get(0)?.subject)
        assertEquals("completed", todos?.items?.get(0)?.status)
        assertEquals(listOf("1"), todos?.items?.get(1)?.blockedBy)
    }

    /** org.json turns a JSON null into the literal string "null" via optString. */
    @Test
    fun `a session with no id parses as absent, not the word null`() {
        val todos = parse("""{"todos": {"session_id": null, "source": "transcript", "items": []}}""")

        assertNull(todos?.sessionId)
        assertTrue(todos?.items?.isEmpty() == true)
    }

    @Test
    fun `a response without a todos object is not a checklist`() {
        assertNull(parse("""{"transcript": {"turns": []}}"""))
    }

    @Test
    fun `an item missing its optional fields still parses`() {
        val todos = parse("""{"todos": {"items": [{"id":"7","subject":"bare"}]}}""")

        val item = todos?.items?.single()
        assertEquals("7", item?.id)
        assertEquals("pending", item?.status)
        assertEquals("", item?.activeForm)
        assertTrue(item?.blockedBy?.isEmpty() == true)
        assertEquals("transcript", todos?.source)
    }

    @Test
    fun `only unfinished work counts as open`() {
        assertTrue(todoIsOpen("pending"))
        assertTrue(todoIsOpen("in_progress"))
        assertEquals(false, todoIsOpen("completed"))
        assertEquals(false, todoIsOpen("cancelled"))
    }
}
