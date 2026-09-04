package dev.shep.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ClaimCodeTest {

    @Test
    fun `a computer is whatever the person would ssh to`() {
        assertEquals("ws://mini:7431/", pairingUrlFromHost("mini"))
        assertEquals("ws://mini:7431/", pairingUrlFromHost("  mini  "))
        assertEquals("ws://10.0.0.27:7432/", pairingUrlFromHost("10.0.0.27:7432"))
        assertEquals("ws://mini.ts.net:7431/", pairingUrlFromHost("mini.ts.net"))
    }

    @Test
    fun `a whole url is left as one`() {
        assertEquals("ws://mini:7431/", pairingUrlFromHost("ws://mini:7431"))
        assertEquals("wss://mini/", pairingUrlFromHost("wss://mini/"))
    }

    @Test
    fun `a code survives the trip through a person`() {
        assertEquals("7K4M9QP2", normalizeClaimCode("7k4m-9qp2"))
        assertEquals("7K4M9QP2", normalizeClaimCode(" 7K4M 9QP2 "))
        assertTrue(isCompleteClaimCode("7K4M9QP2"))
        assertFalse(isCompleteClaimCode("7K4M9QP"))
        assertFalse(isCompleteClaimCode(""))
    }
}
