package dev.shep.companion

import android.content.Context
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import org.json.JSONArray
import org.json.JSONObject

private const val PREFS = "shep"

/**
 * FCM delivery.
 *
 * FCM is the transport that survives the phone being asleep: Play Services holds
 * one OS-privileged socket for every app on the device, and a high-priority
 * message is allowed through Doze. A broker of our own, however well run, is a
 * background service Android is free to kill — which it does, silently, exactly
 * when you have stopped watching the screen and most need to be told something.
 *
 * Messages are data-only by design (see the Rust side's `fcm::send`). A
 * `notification` block would have Android render the notification itself, which
 * drops the Approve/Deny actions and the deep-link — the whole reason the
 * notification is worth having.
 */
class ShepMessagingService : FirebaseMessagingService() {

    /**
     * FCM hands out a new token on install, restore, and whenever it feels like
     * it. The server dedups on the token, so re-registering is always safe.
     */
    override fun onNewToken(token: String) {
        applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString("fcm_token", token).apply()
        FcmManager.registerWithShep(applicationContext, token)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val data = message.data
        if (data.isEmpty()) return
        showShepNotification(
            applicationContext,
            ShepNotification(
                kind = data["kind"].orEmpty(),
                state = data["state"].orEmpty(),
                agent = data["agent"].orEmpty(),
                workspace = data["workspace"].orEmpty(),
                paneId = data["pane_id"].orEmpty(),
                title = data["title"].orEmpty(),
                body = data["message"].orEmpty(),
            ),
        )
    }
}

/** Getting this device's token to shep, and keeping its subscriptions in step. */
object FcmManager {

    /**
     * Ask FCM for this install's token and hand it to shep.
     *
     * Called on app start and from the settings screen. Returns immediately; the
     * outcome lands in prefs for the settings screen to show, because there is
     * nothing useful to block on.
     */
    fun register(context: Context, kinds: Set<NotifyKind>? = null) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs.edit().putString("push_status", "registering…").apply()
        FirebaseMessaging.getInstance().token
            .addOnSuccessListener { token ->
                prefs.edit().putString("fcm_token", token).apply()
                registerWithShep(context, token, kinds)
            }
            .addOnFailureListener { err ->
                prefs.edit()
                    .putString("push_status", "no FCM token: ${err.message}")
                    .apply()
            }
    }

    /** The kinds this device has chosen, defaulting on first run. */
    fun selectedKinds(context: Context): Set<NotifyKind> {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val saved = prefs.getStringSet("push_kinds", null) ?: return NotifyKind.DEFAULTS
        return saved.mapNotNull { NotifyKind.fromWire(it) }.toSet()
    }

    /**
     * Change what this device wants to hear about.
     *
     * The choice is stored server-side as well as locally: muting at the source
     * means a muted kind costs no radio wake at all, and it means the setting
     * survives reinstalling the app.
     */
    fun setKinds(context: Context, kinds: Set<NotifyKind>, onResult: (String) -> Unit) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs.edit().putStringSet("push_kinds", kinds.map { it.wire }.toSet()).apply()
        val token = prefs.getString("fcm_token", null)
        withBridge(context, onResult) { client ->
            val params = JSONObject().put("kinds", JSONArray(kinds.map { it.wire }))
            if (token != null) params.put("token", token)
            client.call("push.set_kinds", params)
            "notifications updated"
        }
    }

    /**
     * Ask shep to send a real notification right now.
     *
     * Push failing is invisible by construction — nothing arrives, and nothing
     * says so. This is the one way to tell "working" from "silently broken"
     * without waiting for an agent to block.
     */
    fun sendTest(context: Context, onResult: (String) -> Unit) {
        withBridge(context, onResult) { client ->
            val result = client.call("push.test", JSONObject())
            val sent = result.optInt("sent")
            val results = result.optJSONArray("results") ?: JSONArray()
            if (sent > 0) {
                "sent to $sent device(s) — it should arrive in a moment"
            } else if (results.length() == 0) {
                "shep has no registered devices — tap re-register first"
            } else {
                // Report what shep actually said rather than a generic failure;
                // the reason is the entire value of this button.
                (0 until results.length())
                    .mapNotNull { results.optJSONObject(it) }
                    .joinToString("; ") {
                        "${it.optString("label")}: ${it.optString("outcome")}"
                    }
            }
        }
    }

    internal fun registerWithShep(
        context: Context,
        token: String,
        kinds: Set<NotifyKind>? = null,
    ) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        withBridge(context, { prefs.edit().putString("push_status", it).apply() }) { client ->
            val params = JSONObject()
                .put("transport", "fcm")
                .put("token", token)
                .put("label", android.os.Build.MODEL ?: "android")
            // Only send kinds when the user has actually chosen: an unqualified
            // re-register must not reset a choice made on another install.
            if (kinds != null) {
                params.put("kinds", JSONArray(kinds.map { it.wire }))
            }
            client.call("push.register", params)
            "push registered"
        }
    }

    /**
     * Run one short-lived bridge call off the main thread, reporting either its
     * result or why it could not run.
     */
    private fun withBridge(
        context: Context,
        onResult: (String) -> Unit,
        body: (BridgeClient) -> String,
    ) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val url = prefs.getString("url", null)
        val token = prefs.getString("token", null)
        if (url == null || token == null) {
            onResult("not paired with shep yet")
            return
        }
        Thread {
            val client = BridgeClient(url, token)
            val error = client.connect(timeoutSeconds = 8)
            val message = if (error != null) {
                "cannot reach shep: $error"
            } else {
                runCatching { body(client) }
                    .getOrElse { "failed: ${it.message}" }
            }
            client.close()
            onResult(message)
        }.start()
    }
}
