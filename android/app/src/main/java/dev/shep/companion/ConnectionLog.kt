package dev.shep.companion

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

/**
 * One thing that happened to this device's link to the bridge.
 *
 * [host] is the address only — never the URL with its token. This log is the
 * one part of the app written expressly to be read back later, and a bearer
 * credential for a shell does not belong in something that outlives the
 * connection it opened.
 */
data class ConnectionEvent(
    /** Unix milliseconds, from the device's clock. */
    val at: Long,
    val kind: Kind,
    val host: String,
    /** Why, for the two kinds that have a reason. */
    val detail: String? = null,
) {
    enum class Kind(val wire: String, val label: String) {
        Connected("connected", "connected"),
        Failed("failed", "could not connect"),
        Dropped("dropped", "dropped");

        companion object {
            fun fromWire(wire: String): Kind? = entries.firstOrNull { it.wire == wire }
        }
    }
}

/**
 * What this device's link to the bridge has been doing.
 *
 * A phone reaches shep over a tailnet, from a train, on a handset that sleeps
 * — so "it would not connect" is a question about a history, not about right
 * now. The banner can only ever say what is true this second; this says what
 * has been true, which is the thing you actually need when the address moved
 * or the link is flaky.
 *
 * Device-local by design. It records this phone's own view and asks the server
 * nothing, so it still has answers precisely when the connection is the thing
 * that is broken.
 */
object ConnectionLog {
    /** Kept per device, so the app's own prefs file rather than the pairing store. */
    private const val PREFS = "shep"
    private const val KEY = "connection_log"

    /**
     * How many events are kept.
     *
     * Long enough to show a pattern across a few days of ordinary use, short
     * enough that reading and rewriting the whole list on every connection
     * stays free.
     */
    const val CAP = 50

    fun record(context: Context, kind: ConnectionEvent.Kind, url: String, detail: String? = null) {
        val event = ConnectionEvent(System.currentTimeMillis(), kind, hostOf(url), detail)
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val kept = append(decode(prefs.getString(KEY, null)), event, CAP)
        prefs.edit().putString(KEY, encode(kept)).apply()
    }

    /** Newest first — the order the question is asked in. */
    fun entries(context: Context): List<ConnectionEvent> =
        decode(context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, null))
            .asReversed()

    fun clear(context: Context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().remove(KEY).apply()
    }

    /**
     * Just the authority — `ws://10.0.0.5:7431/` becomes `10.0.0.5:7431`.
     *
     * Deliberately string work rather than [android.net.Uri]: a URL that was
     * typed by hand may not parse, and a log that throws while recording a
     * failed connection would lose exactly the entry worth having.
     */
    fun hostOf(url: String): String {
        val afterScheme = url.substringAfter("://", url)
        val authority = afterScheme.substringBefore('/').substringBefore('?')
        // Strip any `user:pass@`, which is where a token would hide.
        val host = authority.substringAfterLast('@')
        return host.ifBlank { url }
    }

    /** Oldest first, capped by dropping from the front. */
    fun append(existing: List<ConnectionEvent>, event: ConnectionEvent, cap: Int): List<ConnectionEvent> {
        val grown = existing + event
        return if (grown.size <= cap) grown else grown.takeLast(cap)
    }

    fun encode(events: List<ConnectionEvent>): String {
        val array = JSONArray()
        events.forEach { event ->
            array.put(
                JSONObject()
                    .put("at", event.at)
                    .put("kind", event.kind.wire)
                    .put("host", event.host)
                    .apply { event.detail?.let { put("detail", it) } },
            )
        }
        return array.toString()
    }

    /**
     * Anything unreadable is dropped rather than thrown.
     *
     * A log is a convenience; a corrupt one must not be able to stop the app
     * connecting, which is the thing the log is about.
     */
    fun decode(raw: String?): List<ConnectionEvent> {
        if (raw.isNullOrBlank()) return emptyList()
        val array = runCatching { JSONArray(raw) }.getOrNull() ?: return emptyList()
        val events = mutableListOf<ConnectionEvent>()
        for (i in 0 until array.length()) {
            val item = array.optJSONObject(i) ?: continue
            val kind = ConnectionEvent.Kind.fromWire(item.optString("kind")) ?: continue
            val at = item.optLong("at")
            if (at <= 0L) continue
            events.add(
                ConnectionEvent(
                    at = at,
                    kind = kind,
                    host = item.optString("host"),
                    detail = if (item.isNull("detail")) null
                    else item.optString("detail").ifEmpty { null },
                ),
            )
        }
        return events
    }
}
