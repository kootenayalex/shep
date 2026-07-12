package dev.shep.companion

import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * Client for the `shep bridge` WebSocket relay.
 *
 * One socket, many channels: each API call is `{"ch":N,"req":{...}}` out and a
 * stream of `{"ch":N,"line":{...}}` frames back, ended by `{"ch":N,"eof":true}`
 * or `{"ch":N,"error":"..."}`. See src/cli/bridge.rs in the shep repo.
 */
class BridgeClient(private val url: String, private val token: String) {

    interface ChannelListener {
        fun onLine(line: JSONObject)
        fun onClosed(error: String?)
    }

    private val http = OkHttpClient.Builder()
        .pingInterval(20, TimeUnit.SECONDS)
        .build()
    private val nextChannel = AtomicLong(1)
    private val channels = ConcurrentHashMap<Long, ChannelListener>()
    private val nextRequestId = AtomicLong(1)
    private val open = AtomicBoolean(false)
    @Volatile private var socket: WebSocket? = null
    @Volatile var serverVersion: String? = null
        private set
    @Volatile var onDisconnect: ((String?) -> Unit)? = null

    val isOpen: Boolean get() = open.get()

    /** Connect and wait for the hello frame. Returns null on success, else the error. */
    fun connect(timeoutSeconds: Long = 8): String? {
        val helloLatch = CountDownLatch(1)
        var failure: String? = null
        // Hand-typed URLs crash Request.Builder (bad scheme, stray spaces,
        // keyboard autocapitalization) — normalize, then report instead of
        // throwing.
        val normalized = normalizeUrl(url)
        val request = try {
            Request.Builder()
                .url(normalized)
                .header("Authorization", "Bearer $token")
                .build()
        } catch (e: IllegalArgumentException) {
            return "invalid URL: $normalized"
        }
        socket = http.newWebSocket(request, object : WebSocketListener() {
            override fun onMessage(webSocket: WebSocket, text: String) {
                val frame = runCatching { JSONObject(text) }.getOrNull() ?: return
                if (frame.has("hello")) {
                    serverVersion = frame.getJSONObject("hello").optString("server_version")
                    open.set(true)
                    helloLatch.countDown()
                    return
                }
                val ch = frame.optLong("ch", -1)
                if (ch < 0) return
                val listener = channels[ch] ?: return
                when {
                    frame.has("line") -> listener.onLine(frame.getJSONObject("line"))
                    frame.optBoolean("eof") -> {
                        channels.remove(ch)
                        listener.onClosed(null)
                    }
                    frame.has("error") -> {
                        channels.remove(ch)
                        listener.onClosed(frame.optString("error"))
                    }
                }
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                failure = if (response?.code == 401) "unauthorized — check the token"
                else t.message ?: "connection failed"
                open.set(false)
                helloLatch.countDown()
                failAllChannels(failure)
                onDisconnect?.invoke(failure)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                open.set(false)
                failAllChannels("connection closed")
                onDisconnect?.invoke(null)
            }
        })
        if (!helloLatch.await(timeoutSeconds, TimeUnit.SECONDS)) {
            socket?.cancel()
            return "timed out connecting to $url"
        }
        return if (open.get()) null else (failure ?: "connection failed")
    }

    fun close() {
        open.set(false)
        socket?.close(1000, "bye")
        failAllChannels("closed")
    }

    private fun failAllChannels(reason: String?) {
        val pending = channels.values.toList()
        channels.clear()
        pending.forEach { it.onClosed(reason) }
    }

    /** Open a relay channel for one API request; lines stream to [listener]. */
    fun openChannel(method: String, params: JSONObject, listener: ChannelListener): Long {
        val ch = nextChannel.getAndIncrement()
        channels[ch] = listener
        val request = JSONObject()
            .put("id", "app-${nextRequestId.getAndIncrement()}")
            .put("method", method)
            .put("params", params)
        val sent = socket?.send(JSONObject().put("ch", ch).put("req", request).toString()) ?: false
        if (!sent) {
            channels.remove(ch)
            listener.onClosed("not connected")
        }
        return ch
    }

    fun closeChannel(ch: Long) {
        channels.remove(ch)
        socket?.send(JSONObject().put("ch", ch).put("close", true).toString())
    }

    /**
     * Single request → single response helper. Returns the `result` object or
     * throws with the API error message.
     */
    fun call(method: String, params: JSONObject = JSONObject(), timeoutSeconds: Long = 10): JSONObject {
        val latch = CountDownLatch(1)
        var line: JSONObject? = null
        var closedError: String? = null
        val ch = openChannel(method, params, object : ChannelListener {
            override fun onLine(l: JSONObject) {
                if (line == null) {
                    line = l
                    latch.countDown()
                }
            }
            override fun onClosed(error: String?) {
                closedError = error
                latch.countDown()
            }
        })
        if (!latch.await(timeoutSeconds, TimeUnit.SECONDS)) {
            closeChannel(ch)
            throw BridgeException("$method timed out")
        }
        val response = line
            ?: throw BridgeException(closedError ?: "$method returned no response")
        response.optJSONObject("error")?.let {
            throw BridgeException(it.optString("message", "api error"))
        }
        return response.optJSONObject("result") ?: JSONObject()
    }
}

class BridgeException(message: String) : Exception(message)

/** Forgiving pairing input: trim, lowercase the scheme, default to ws://. */
fun normalizeUrl(raw: String): String {
    var url = raw.trim().replace(" ", "")
    val schemeEnd = url.indexOf("://")
    url = if (schemeEnd > 0) {
        url.take(schemeEnd).lowercase() + url.substring(schemeEnd)
    } else {
        "ws://$url"
    }
    return url
}
