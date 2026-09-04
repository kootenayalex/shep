package dev.shep.companion.net

import dev.shep.companion.terminal.TerminalKey
import org.json.JSONArray
import org.json.JSONObject

/** The `pane.*` request that carries one key when no stream channel is open. */
fun keyRequest(paneId: String, key: TerminalKey): Pair<String, JSONObject> = when (key) {
    is TerminalKey.Text ->
        "pane.send_text" to JSONObject().put("pane_id", paneId).put("text", key.text)
    is TerminalKey.Named ->
        "pane.send_keys" to JSONObject().put("pane_id", paneId).put("keys", JSONArray(listOf(key.name)))
    is TerminalKey.Keys ->
        "pane.send_keys" to JSONObject().put("pane_id", paneId).put("keys", JSONArray(key.names))
}

/**
 * Every keypress on a pane goes through here, whether from the key bar or
 * the soft keyboard.
 *
 * With a stream channel open the key rides that channel: one WebSocket write,
 * no round trip. Before the stream has produced its first frame, and after
 * it has ended or failed, there is no channel — and keys used to be dropped
 * on the floor by an empty handler. Now they go as ordinary `pane.send_text`
 * / `pane.send_keys` requests through [request], in order, so what was typed
 * while "connecting to pane…" was on screen still lands. A write the socket
 * refuses is reported through [onDropped] instead of vanishing.
 */
class InputRouter(
    private val paneId: String,
    private val request: (method: String, params: JSONObject) -> Unit,
    private val onDropped: (String) -> Unit,
) {
    @Volatile var channel: StreamChannel? = null

    fun press(key: TerminalKey) {
        val open = channel
        if (open == null) {
            val (method, params) = keyRequest(paneId, key)
            request(method, params)
            return
        }
        if (!open.send(key)) onDropped("input dropped — the bridge socket is closed")
    }
}
