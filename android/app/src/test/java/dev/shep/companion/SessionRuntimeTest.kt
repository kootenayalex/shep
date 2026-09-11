package dev.shep.companion

import dev.shep.companion.screens.SessionRuntime
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * What a new session is actually launched as.
 *
 * The runtime is started by typing its argv into a fresh shell, so a wrong
 * word here is a command line a person did not ask for — which, for the bypass
 * flag, means an agent that edits and runs without asking.
 */
class SessionRuntimeTest {

    /** The exact spelling `claude --help` gives. A near miss is not a flag. */
    @Test
    fun `claude carries the real skip-permissions flag`() {
        assertEquals("--dangerously-skip-permissions", SessionRuntime.Claude.bypassFlag)
    }

    /**
     * Guessing another runtime's flag either fails the launch or is swallowed,
     * leaving a session whose supervision is not what the toggle claimed. No
     * flag is the honest answer until one is verified.
     */
    @Test
    fun `no other runtime claims a bypass flag`() {
        assertNull(SessionRuntime.Opencode.bypassFlag)
        assertNull(SessionRuntime.Grok.bypassFlag)
        assertNull(SessionRuntime.Terminal.bypassFlag)
    }

    /** A terminal is the absence of an agent, so there is nothing to bypass. */
    @Test
    fun `a runtime with a flag is one that is actually launched`() {
        SessionRuntime.entries.filter { it.bypassFlag != null }.forEach {
            assertNotNull(it.bypassFlag)
            assert(it.argv.isNotEmpty()) { "${it.label} has a flag but nothing to pass it to" }
        }
    }

    /** The flag is appended, so the runtime's own command line stays intact. */
    @Test
    fun `the flag appends to argv rather than replacing it`() {
        val runtime = SessionRuntime.Claude
        val withBypass = runtime.argv + listOfNotNull(runtime.bypassFlag)
        assertEquals(listOf("claude", "--dangerously-skip-permissions"), withBypass)
        assertEquals(listOf("claude"), runtime.argv)
    }
}
