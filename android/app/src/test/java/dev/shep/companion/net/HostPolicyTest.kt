package dev.shep.companion.net

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The bearer token goes out in the clear only to addresses that are ours:
 * loopback, the emulator's host, RFC 1918, the tailnet's CGNAT block, and
 * names that cannot leave the local network. Everything else needs wss://.
 */
class HostPolicyTest {

    @Test
    fun `tls is always allowed`() {
        assertTrue(plaintextAllowed("wss://shep.example.com/"))
        assertTrue(plaintextAllowed("wss://1.2.3.4:7431/"))
    }

    @Test
    fun `plaintext to private addresses is allowed`() {
        listOf(
            "ws://10.0.2.2:7432/",
            "ws://127.0.0.1:7432/",
            "ws://localhost:7432",
            "ws://10.0.0.27:7431/",
            "ws://192.168.1.10:7431/",
            "ws://172.16.5.5:7431/",
            "ws://172.31.255.1/",
            "ws://100.83.179.75:7431/",
            "ws://100.127.0.1/",
            "ws://169.254.1.1/",
            "ws://macmini:7431/",
            "ws://macmini.local:7431/",
            "ws://macmini.tail1234.ts.net:7431/",
            "ws://[::1]:7431/",
            "ws://[fd7a:115c:a1e0::1]:7431/",
            "ws://[fe80::1%25wlan0]:7431/",
        ).forEach { assertTrue(it, plaintextAllowed(it)) }
    }

    @Test
    fun `plaintext to public addresses is refused`() {
        listOf(
            "ws://1.2.3.4:7431/",
            "ws://shep.example.com/",
            "ws://172.32.0.1/",
            "ws://100.128.0.1/",
            "ws://11.0.0.1/",
            "ws://[2001:db8::1]/",
            "ws://8.8.8.8/",
            "ftp://10.0.0.1/",
            "ws://",
            "nonsense",
        ).forEach { assertFalse(it, plaintextAllowed(it)) }
    }

    @Test
    fun `the host is read past credentials ports and brackets`() {
        assertTrue(plaintextAllowed("ws://user:pw@10.0.0.5:7431/path?x=1"))
        assertFalse(plaintextAllowed("ws://user:pw@8.8.8.8:7431/path"))
        assertTrue(plaintextAllowed("ws://[fc00::5]:7431"))
    }
}
