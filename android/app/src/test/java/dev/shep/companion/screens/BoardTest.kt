package dev.shep.companion.screens

import dev.shep.companion.ChatRole
import dev.shep.companion.ChatTurn
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The three things the board gets wrong if nobody pins them, all of which look
 * like working software from a screenshot: a chat that shows every line twice,
 * a spinner that never stops, and an overseer region that vanishes for good
 * because one request timed out.
 */
class BoardTest {

    private fun you(at: Long, text: String = "what needs me?") =
        ChatTurn(at, ChatRole.You, text)

    private fun overseer(at: Long, text: String = "claude is blocked.") =
        ChatTurn(at, ChatRole.Overseer, text)

    /**
     * Every turn arrives at least twice — as an event, and again in the tail of
     * the next sample — so the merge is what stands between the chat and a
     * screen of doubled lines.
     */
    @Test
    fun `a turn that arrives twice is shown once`() {
        val first = mergeChat(emptyList(), listOf(you(100), overseer(140)))
        val again = mergeChat(first, listOf(you(100), overseer(140)))
        assertEquals(2, again.size)
        assertEquals(listOf(100L, 140L), again.map { it.at })
    }

    /**
     * `(at, role)` is the identity, not `at` alone: a question and its answer
     * can land inside the same second, and they are two turns.
     */
    @Test
    fun `two roles in the same second are two turns`() {
        val merged = mergeChat(listOf(you(100)), listOf(overseer(100)))
        assertEquals(2, merged.size)
        assertEquals(listOf(ChatRole.You, ChatRole.Overseer), merged.map { it.role })
    }

    /** The tail comes back oldest-first however it arrived. */
    @Test
    fun `the chat reads oldest first whatever order it arrived in`() {
        val merged = mergeChat(listOf(overseer(300)), listOf(you(100), you(200)))
        assertEquals(listOf(100L, 200L, 300L), merged.map { it.at })
    }

    /**
     * The event carries `pending`, and the overseer's own turn is what clears
     * it. Without this the spinner sits under the answer it was waiting for.
     */
    @Test
    fun `pending clears on the overseer's turn`() {
        val asked = parseOverseerEvent(
            JSONObject(
                """{"event":"overseer_chat_turn","data":{"type":"overseer_chat_turn",
                   "turn":{"at":100,"role":"you","text":"what needs me?"},"pending":true}}"""
            )
        )
        assertTrue(asked is OverseerEvent.Chat)
        assertTrue((asked as OverseerEvent.Chat).pending)
        assertEquals(ChatRole.You, asked.turn.role)

        val answered = parseOverseerEvent(
            JSONObject(
                """{"event":"overseer_chat_turn","data":{"type":"overseer_chat_turn",
                   "turn":{"at":140,"role":"overseer","text":"claude is blocked."},
                   "pending":false}}"""
            )
        )
        assertTrue(answered is OverseerEvent.Chat)
        assertFalse((answered as OverseerEvent.Chat).pending)
        assertEquals(ChatRole.Overseer, answered.turn.role)
    }

    @Test
    fun `an updated event is a nudge, and anything else is ignored`() {
        assertEquals(
            OverseerEvent.Updated,
            parseOverseerEvent(
                JSONObject(
                    """{"event":"overseer_updated","data":{"type":"overseer_updated",
                       "tick_at":"07:08","source":"brain","situation_age_seconds":4}}"""
                )
            ),
        )
        assertNull(
            parseOverseerEvent(
                JSONObject("""{"event":"pane.moved","data":{"type":"pane_moved"}}""")
            )
        )
        assertNull(parseOverseerEvent(JSONObject("{}")))
    }

    /**
     * The one that actually bit the agents list before it was fixed there:
     * latching the fallback on any error means one bad moment costs the board
     * its overseer regions until the app restarts.
     */
    @Test
    fun `the unsupported fallback does not latch on a timeout`() {
        assertTrue(overseerSupported(true, "overseer.sample timed out"))
        assertTrue(overseerSupported(true, "Software caused connection abort"))
        assertTrue(overseerSupported(true, null))
        // Only the server saying it has never heard of the method.
        assertFalse(overseerSupported(true, "unknown variant `overseer.sample`, expected one of"))
        // And once it does answer, the board comes back.
        assertTrue(overseerSupported(false, null))
        // A transient error while already off leaves it off rather than
        // flapping the whole right-hand side on and off.
        assertFalse(overseerSupported(false, "timed out"))
    }
}
