package dev.shep.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The notification plan is decided without Android, so the rules that matter —
 * one notification per agent, a clear takes it down, every server kind is
 * understood — are pinned here rather than discovered on a phone.
 */
class PushTest {

    private fun message(vararg fields: Pair<String, String>): ShepNotification {
        val data = fields.toMap()
        return ShepNotification.fromFields { data[it] }
    }

    @Test
    fun `a newer event for the same agent bumps the older one`() {
        val blocked = planNotification(
            message("tag" to "w1:p7", "pane_id" to "w1:p7", "kind" to "blocked", "agent" to "claude"),
        )
        val done = planNotification(
            message("tag" to "w1:p7", "pane_id" to "w1:p7", "kind" to "done", "agent" to "claude"),
        )
        assertTrue(blocked is NotificationPlan.Post)
        assertTrue(done is NotificationPlan.Post)
        assertEquals("same agent, same slot", blocked.id, done.id)

        val other = planNotification(
            message("tag" to "w1:p8", "pane_id" to "w1:p8", "kind" to "blocked", "agent" to "claude"),
        )
        assertNotEquals("different agents keep their own slots", blocked.id, other.id)
    }

    @Test
    fun `a clear cancels the agent's notification and posts nothing`() {
        val shown = planNotification(
            message("tag" to "w1:p7", "pane_id" to "w1:p7", "kind" to "blocked", "op" to "show"),
        )
        val cleared = planNotification(
            message("tag" to "w1:p7", "pane_id" to "w1:p7", "kind" to "blocked", "op" to "clear"),
        )
        assertTrue(cleared is NotificationPlan.Cancel)
        assertEquals("the clear takes down exactly what was shown", shown.id, cleared.id)
    }

    @Test
    fun `a message without op or tag is a show keyed by the pane`() {
        val legacy = message("pane_id" to "w1:p7", "kind" to "blocked", "agent" to "claude")
        assertFalse(legacy.isClear)
        val plan = planNotification(legacy)
        assertTrue(plan is NotificationPlan.Post)
        assertEquals(notificationIdFor("w1:p7"), plan.id)
    }

    @Test
    fun `only a blocked agent is offered an answer`() {
        val blocked = planNotification(message("pane_id" to "w1:p7", "kind" to "blocked"))
        val done = planNotification(message("pane_id" to "w1:p7", "kind" to "done"))
        assertTrue((blocked as NotificationPlan.Post).offerAnswer)
        assertFalse((done as NotificationPlan.Post).offerAnswer)
    }

    /** Parity with `crate::config::NotifyKind::label` on the server. */
    @Test
    fun `every server kind resolves on the phone`() {
        val serverKinds = listOf("idle", "working", "blocked", "unknown", "done", "task", "review")
        for (wire in serverKinds) {
            assertNotNull("kind $wire", NotifyKind.fromWire(wire))
        }
        assertEquals(serverKinds.size, NotifyKind.entries.size)
    }
}
