package dev.shep.companion.net

import dev.shep.companion.BridgeClient
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.Dispatchers
import org.json.JSONArray
import org.json.JSONObject

/** One line from a `pane.stream` channel. */
sealed interface StreamEvent {
    data class Size(val cols: Int, val rows: Int) : StreamEvent
    data class Frame(val json: JSONObject) : StreamEvent
    /** Liveness tick during a quiet pane — distinguishes idle from wedged. */
    data object Ping : StreamEvent
    data class Ended(val reason: String?) : StreamEvent
    data class Failed(val message: String) : StreamEvent
}

/**
 * A live view of one pane, plus the input path back into it.
 *
 * Output arrives as cell-grid frames; input goes out on the same channel as
 * `{"ch":N,"data":{...}}`, so typing costs one WebSocket write instead of a
 * request round trip.
 */
class StreamChannel(
    private val client: BridgeClient,
    private val ch: Long,
) {
    fun sendText(text: String) {
        if (text.isEmpty()) return
        client.sendData(ch, JSONObject().put("text", text))
    }

    fun sendKeys(vararg keys: String) {
        if (keys.isEmpty()) return
        client.sendData(ch, JSONObject().put("keys", JSONArray(keys.toList())))
    }

    fun close() = client.closeChannel(ch)
}

/**
 * Open a `pane.stream` and emit its lines.
 *
 * Parsing runs off the main thread: a full frame is thousands of JSON array
 * elements and org.json is not fast.
 */
fun BridgeClient.paneStream(paneId: String): Flow<Pair<StreamChannel, StreamEvent>> = callbackFlow {
    var channel: StreamChannel? = null
    val ch = openChannel(
        "pane.stream",
        JSONObject().put("pane_id", paneId),
        object : BridgeClient.ChannelListener {
            override fun onLine(line: JSONObject) {
                val stream = channel ?: return
                // An API-shaped error (e.g. an older bridge with no pane.stream)
                // arrives as a normal line rather than a channel error.
                line.optJSONObject("error")?.let {
                    trySend(stream to StreamEvent.Failed(it.optString("message", "stream failed")))
                    return
                }
                val event = when (line.optString("type")) {
                    "size" -> StreamEvent.Size(line.optInt("w"), line.optInt("h"))
                    "frame" -> StreamEvent.Frame(line)
                    "ping" -> StreamEvent.Ping
                    "end" -> StreamEvent.Ended(line.optString("reason").ifEmpty { null })
                    else -> return
                }
                trySend(stream to event)
            }

            override fun onClosed(error: String?) {
                val stream = channel
                if (stream != null) {
                    trySend(
                        stream to if (error != null) StreamEvent.Failed(error)
                        else StreamEvent.Ended(null)
                    )
                }
                close()
            }
        },
    )
    channel = StreamChannel(this@paneStream, ch)

    awaitClose { closeChannel(ch) }
}.flowOn(Dispatchers.Default)
