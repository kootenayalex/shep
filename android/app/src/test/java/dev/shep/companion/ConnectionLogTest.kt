package dev.shep.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The parsing half of the connection log, which is everything except the two
 * lines that touch a `Context`.
 */
class ConnectionLogTest {

    private fun event(at: Long, kind: ConnectionEvent.Kind = ConnectionEvent.Kind.Connected) =
        ConnectionEvent(at = at, kind = kind, host = "10.0.0.5:7431")

    /**
     * The token is a bearer credential for a shell, and this is the one part of
     * the app written to outlive the connection it describes.
     */
    @Test
    fun `the host is kept and anything that could carry a token is not`() {
        assertEquals("10.0.0.5:7431", ConnectionLog.hostOf("ws://10.0.0.5:7431/"))
        assertEquals("100.83.179.75:7431", ConnectionLog.hostOf("ws://100.83.179.75:7431/"))
        // A query string is where a token would ride.
        assertEquals("host:7431", ConnectionLog.hostOf("ws://host:7431/?token=sekrit"))
        // So is userinfo.
        assertEquals("host:7431", ConnectionLog.hostOf("wss://user:sekrit@host:7431/"))
    }

    /** A hand-typed URL may not parse; recording must still produce something. */
    @Test
    fun `an unparseable url still yields a host line`() {
        assertEquals("nonsense", ConnectionLog.hostOf("nonsense"))
        assertEquals("", ConnectionLog.hostOf(""))
    }

    @Test
    fun `a round trip preserves every field`() {
        val events = listOf(
            ConnectionEvent(1_700_000_000_000, ConnectionEvent.Kind.Connected, "a:1"),
            ConnectionEvent(1_700_000_001_000, ConnectionEvent.Kind.Failed, "b:2", "unauthorized"),
            ConnectionEvent(1_700_000_002_000, ConnectionEvent.Kind.Dropped, "c:3", null),
        )
        assertEquals(events, ConnectionLog.decode(ConnectionLog.encode(events)))
    }

    /** The cap drops the oldest, so the newest events are the ones kept. */
    @Test
    fun `appending past the cap keeps the newest`() {
        var kept = emptyList<ConnectionEvent>()
        for (i in 1..10) kept = ConnectionLog.append(kept, event(i.toLong()), cap = 4)
        assertEquals(4, kept.size)
        assertEquals(listOf(7L, 8L, 9L, 10L), kept.map { it.at })
    }

    /**
     * A corrupt log must not be able to stop the app connecting — which is the
     * thing the log exists to explain.
     */
    @Test
    fun `unreadable content decodes to nothing rather than throwing`() {
        assertTrue(ConnectionLog.decode(null).isEmpty())
        assertTrue(ConnectionLog.decode("").isEmpty())
        assertTrue(ConnectionLog.decode("{not json").isEmpty())
        assertTrue(ConnectionLog.decode("{\"an\":\"object\"}").isEmpty())
    }

    /** One bad row loses that row, not the rest of the history. */
    @Test
    fun `rows that cannot be read are skipped individually`() {
        val raw = """
            [{"at":1,"kind":"connected","host":"a:1"},
             {"at":2,"kind":"invented","host":"b:2"},
             {"kind":"dropped","host":"c:3"},
             {"at":4,"kind":"dropped","host":"d:4","detail":"closed"}]
        """.trimIndent()
        val decoded = ConnectionLog.decode(raw)
        assertEquals(listOf(1L, 4L), decoded.map { it.at })
        assertNull(decoded.first().detail)
        assertEquals("closed", decoded.last().detail)
    }
}
