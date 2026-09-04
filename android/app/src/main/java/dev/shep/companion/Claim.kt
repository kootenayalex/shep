package dev.shep.companion

import dev.shep.companion.net.plaintextAllowed
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject

/** How many characters `shep bridge pair` prints. */
const val CLAIM_CODE_LENGTH = 8

/**
 * A computer's name or address, as the pairing URL the bridge listens on.
 *
 * The claim code cannot carry the host — it is eight characters read off a
 * screen — and the phone has no discovery, so the person types the same thing
 * they would `ssh` to. Everything they might reasonably type is accepted: a
 * bare name, a name with a port, an address, a whole `ws://` URL.
 */
fun pairingUrlFromHost(raw: String): String {
    val host = raw.trim().replace(" ", "").trimEnd('/')
    if (host.contains("://")) return normalizeUrl("$host/")
    val hostAndPort = if (host.contains(':') && !host.contains(']')) host else "$host:7431"
    return "ws://$hostAndPort/"
}

/** Everything a person might type back — dashes, spaces, lower case — removed. */
fun normalizeClaimCode(input: String): String =
    input.filter { it.isLetterOrDigit() }.uppercase()

fun isCompleteClaimCode(code: String): Boolean = code.length == CLAIM_CODE_LENGTH

/**
 * Spend a claim code for the bridge's real token.
 *
 * One websocket, one frame, closed: the bridge answers `Authorization: Pair
 * <code>` with the token beside its usual hello and hangs up, so this is not
 * a connection to keep — [MainActivity] hands the token straight to the
 * ordinary Bearer connect that follows.
 */
fun claimToken(url: String, code: String, timeoutSeconds: Long = 8): Result<String> {
    val normalized = normalizeUrl(url)
    if (!plaintextAllowed(normalized)) {
        return Result.failure(
            IllegalArgumentException(
                "that address is not on your own network — the phone and the computer have " +
                    "to share a wi-fi or tailnet",
            ),
        )
    }
    val request = try {
        Request.Builder()
            .url(normalized)
            .header("Authorization", "Pair $code")
            .build()
    } catch (e: IllegalArgumentException) {
        return Result.failure(IllegalArgumentException("that address does not look right"))
    }
    val http = OkHttpClient.Builder().build()
    val latch = CountDownLatch(1)
    var token: String? = null
    var failure: String? = null
    val socket = http.newWebSocket(request, object : WebSocketListener() {
        override fun onMessage(webSocket: WebSocket, text: String) {
            val frame = runCatching { JSONObject(text) }.getOrNull() ?: return
            val claimed = frame.optJSONObject("pair")?.optString("token")
            if (!claimed.isNullOrEmpty()) {
                token = claimed
                latch.countDown()
            }
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            failure = if (response?.code == 401) {
                "that code didn't work — check it and try again"
            } else {
                t.message ?: "could not reach that computer"
            }
            latch.countDown()
        }

        override fun onClosed(webSocket: WebSocket, code2: Int, reason: String) {
            latch.countDown()
        }
    })
    val answered = latch.await(timeoutSeconds, TimeUnit.SECONDS)
    socket.cancel()
    val got = token
    return when {
        got != null -> Result.success(got)
        !answered -> Result.failure(IllegalStateException("no answer from that computer"))
        else -> Result.failure(IllegalStateException(failure ?: "pairing failed"))
    }
}
