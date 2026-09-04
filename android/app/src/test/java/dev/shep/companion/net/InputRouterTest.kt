package dev.shep.companion.net

import dev.shep.companion.BridgeClient
import dev.shep.companion.terminal.TerminalKey
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Keys typed before the stream is up, or after it is gone, used to hit an
 * empty handler and vanish. These pin the two routes and the failure report.
 */
class InputRouterTest {

    private val sent = mutableListOf<Pair<String, JSONObject>>()
    private val dropped = mutableListOf<String>()
    private val router = InputRouter(
        paneId = "w1:p2",
        request = { method, params -> sent += method to params },
        onDropped = { dropped += it },
    )

    @Test
    fun `without a stream channel text goes as pane send_text`() {
        router.press(TerminalKey.Text("ls"))
        val (method, params) = sent.single()
        assertEquals("pane.send_text", method)
        assertEquals("w1:p2", params.getString("pane_id"))
        assertEquals("ls", params.getString("text"))
        assertTrue(dropped.isEmpty())
    }

    @Test
    fun `without a stream channel a named key goes as pane send_keys`() {
        router.press(TerminalKey.Named("enter"))
        val (method, params) = sent.single()
        assertEquals("pane.send_keys", method)
        assertEquals(listOf("enter"), params.getJSONArray("keys").let { a -> List(a.length()) { a.getString(it) } })
    }

    @Test
    fun `a burst of keys is one request carrying all of them`() {
        router.press(TerminalKey.Keys(listOf("backspace", "backspace", "delete")))
        val (method, params) = sent.single()
        assertEquals("pane.send_keys", method)
        assertEquals(3, params.getJSONArray("keys").length())
    }

    /** A channel on a socket that is not open refuses the write; the user is told. */
    @Test
    fun `a refused stream write is reported not swallowed`() {
        router.channel = StreamChannel(BridgeClient("ws://10.0.2.2:7432/", "t"), 1)
        router.press(TerminalKey.Text("x"))
        assertTrue(sent.isEmpty())
        assertEquals(1, dropped.size)
        assertTrue(dropped[0].contains("dropped"))
    }

    @Test
    fun `request shapes match the socket api`() {
        val (m1, p1) = keyRequest("w1:p1", TerminalKey.Text("hi"))
        assertEquals("pane.send_text", m1)
        assertNull(p1.opt("keys"))
        val (m2, p2) = keyRequest("w1:p1", TerminalKey.Named("tab"))
        assertEquals("pane.send_keys", m2)
        assertNull(p2.opt("text"))
    }
}
