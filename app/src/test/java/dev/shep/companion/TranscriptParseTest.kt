package dev.shep.companion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The wire shape of `pane.transcript`, pinned against the Rust encoder.
 *
 * The interleaving is the part worth a test: a reply is prose and tool calls in
 * the order they happened, and flattening that back into "text, then tools"
 * would silently lose which sentence each call belongs to.
 */
class TranscriptParseTest {

    private fun parse(json: String) = parseTranscript(JSONObject(json))

    @Test
    fun `keeps prose and tool calls in source order`() {
        val transcript = parse(
            """
            {"transcript": {
              "agent": "claude",
              "session_id": "abc",
              "source": "reported",
              "truncated": false,
              "turns": [
                {"role": "user", "ts": "t1", "text": "run the tests"},
                {"role": "assistant", "ts": "t2", "text": "On it.\nAll green.",
                 "thinking": "",
                 "blocks": [
                   {"kind": "text", "text": "On it."},
                   {"kind": "tool", "name": "Bash", "summary": "just check",
                    "result": {"ok": true, "preview": "2763 passed"}},
                   {"kind": "text", "text": "All green."}
                 ]}
              ]
            }}
            """.trimIndent(),
        )!!

        assertEquals("reported", transcript.source)
        assertEquals(2, transcript.turns.size)
        assertEquals("user", transcript.turns[0].role)

        val blocks = transcript.turns[1].blocks
        assertEquals(3, blocks.size)
        assertEquals("On it.", (blocks[0] as Block.Prose).text)
        val tool = (blocks[1] as Block.Tool).call
        assertEquals("Bash", tool.name)
        assertEquals("just check", tool.summary)
        assertEquals(true, tool.ok)
        assertEquals("2763 passed", tool.preview)
        assertEquals("All green.", (blocks[2] as Block.Prose).text)
    }

    @Test
    fun `a tool still running has no verdict`() {
        val transcript = parse(
            """
            {"transcript": {"source": "only", "truncated": false, "turns": [
              {"role": "assistant", "ts": "t", "text": "", "thinking": "",
               "blocks": [{"kind": "tool", "name": "Bash", "summary": "sleep 60",
                           "result": null}]}
            ]}}
            """.trimIndent(),
        )!!
        // null is "outstanding", which the UI shows as … rather than ✓ or ✗ —
        // conflating it with failure would make every in-flight call look broken.
        assertNull((transcript.turns[0].blocks[0] as Block.Tool).call.ok)
    }

    @Test
    fun `drops blank prose blocks`() {
        val transcript = parse(
            """
            {"transcript": {"source": "matched", "truncated": true, "turns": [
              {"role": "assistant", "ts": "t", "text": "hi", "thinking": "",
               "blocks": [{"kind": "text", "text": "   "},
                          {"kind": "text", "text": "hi"}]}
            ]}}
            """.trimIndent(),
        )!!
        assertTrue(transcript.truncated)
        assertEquals(1, transcript.turns[0].blocks.size)
    }

    @Test
    fun `a response without a transcript is not a transcript`() {
        assertNull(parse("""{"other": {}}"""))
    }
}
