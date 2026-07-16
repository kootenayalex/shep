package dev.shep.companion

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import org.json.JSONObject
import org.unifiedpush.android.connector.MessagingReceiver
import org.unifiedpush.android.connector.UnifiedPush

/**
 * A3 push. Flow:
 *   shep `[notifications] exec = "shep bridge notify-push"` POSTs a JSON body to
 *   the phone's ntfy topic → the ntfy app (UnifiedPush distributor) wakes
 *   [PushReceiver] even with our app closed → we render an actionable
 *   notification whose Approve/Deny fire [ActionReceiver] → a short-lived
 *   BridgeClient sends `y`/`n` via `pane.send_keys`. Tapping the body deep-links
 *   into the pane. No persistent foreground socket: we live entirely off push
 *   wake-ups (ANDROID-COMPANION.md battery guardrail).
 */

const val PUSH_CHANNEL_ID = "shep_agent"
private const val PUSH_ACTION = "dev.shep.companion.PANE_ACTION"
private const val PREFS = "shep"

/** UnifiedPush registration + the endpoint→shep handshake. */
object PushManager {
    /**
     * Ensure a distributor is chosen and (re)register, so the endpoint arrives at
     * [PushReceiver.onNewEndpoint]. No-op with a logged reason when no distributor
     * (e.g. the ntfy app) is installed. Returns a human status for the Shep tab.
     */
    fun register(context: Context): String {
        val saved = UnifiedPush.getSavedDistributor(context)
        if (saved.isNullOrEmpty()) {
            val available = UnifiedPush.getDistributors(context)
            when {
                available.isEmpty() ->
                    return "no push distributor — install the ntfy app and point it at your ntfy server"
                available.size == 1 -> UnifiedPush.saveDistributor(context, available.first())
                else -> UnifiedPush.saveDistributor(context, available.first())
            }
        }
        UnifiedPush.registerApp(context)
        return "registering for push…"
    }

    fun unregister(context: Context) {
        UnifiedPush.unregisterApp(context)
    }
}

/** UnifiedPush delivery target. Manifest-registered so it wakes a closed app. */
class PushReceiver : MessagingReceiver() {

    override fun onNewEndpoint(context: Context, endpoint: String, instance: String) {
        // Persist locally (for display) and hand the endpoint to shep so its
        // notify-push hook knows where to POST. Done off the main thread.
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString("push_endpoint", endpoint).apply()
        registerEndpointWithShep(context, endpoint)
    }

    override fun onRegistrationFailed(context: Context, instance: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString("push_status", "registration failed").apply()
    }

    override fun onUnregistered(context: Context, instance: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().remove("push_endpoint").apply()
    }

    override fun onMessage(context: Context, message: ByteArray, instance: String) {
        val json = runCatching { JSONObject(String(message, Charsets.UTF_8)) }.getOrNull() ?: return
        val state = json.optString("state")
        val agent = json.optString("agent").ifEmpty { "agent" }
        val workspace = json.optString("workspace")
        val paneId = json.optString("pane_id")
        val body = json.optString("message").ifEmpty { "needs your attention" }
        showAgentNotification(context, state, agent, workspace, paneId, body)
    }
}

/** Approve/Deny tapped from the notification — send the keystroke over the bridge. */
class ActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != PUSH_ACTION) return
        val verb = intent.getStringExtra("verb") ?: return
        val paneId = intent.getStringExtra("pane_id") ?: return
        val notifId = intent.getIntExtra("notif_id", 0)
        val key = if (verb == "approve") "y" else "n"

        // Network off the main thread; keep the receiver alive until it finishes.
        val pending = goAsync()
        Thread {
            try {
                val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
                val url = prefs.getString("url", null)
                val token = prefs.getString("token", null)
                if (url != null && token != null) {
                    val client = BridgeClient(url, token)
                    if (client.connect(timeoutSeconds = 8) == null) {
                        runCatching {
                            client.call(
                                "pane.send_keys",
                                JSONObject()
                                    .put("pane_id", paneId)
                                    .put("keys", org.json.JSONArray(listOf(key))),
                            )
                        }
                    }
                    client.close()
                }
                NotificationManagerCompat.from(context).cancel(notifId)
            } finally {
                pending.finish()
            }
        }.start()
    }
}

/** Build + post the actionable notification. Approve/Deny only when blocked. */
private fun showAgentNotification(
    context: Context,
    state: String,
    agent: String,
    workspace: String,
    paneId: String,
    body: String,
) {
    ensureChannel(context)
    val notifId = if (paneId.isNotEmpty()) paneId.hashCode() else agent.hashCode()

    val openPending = PendingIntent.getActivity(
        context,
        notifId,
        Intent(context, MainActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            data = Uri.parse("shep://pane?pane=${Uri.encode(paneId)}")
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
        },
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )

    val title = if (workspace.isNotEmpty()) "$agent · $workspace" else agent
    val builder = NotificationCompat.Builder(context, PUSH_CHANNEL_ID)
        .setSmallIcon(android.R.drawable.stat_sys_warning)
        .setContentTitle(title)
        .setContentText(body)
        .setStyle(NotificationCompat.BigTextStyle().bigText(body))
        .setPriority(NotificationCompat.PRIORITY_HIGH)
        .setCategory(NotificationCompat.CATEGORY_CALL)
        .setAutoCancel(true)
        .setContentIntent(openPending)

    if (state == "blocked" && paneId.isNotEmpty()) {
        builder.addAction(
            0, "Approve", actionPending(context, "approve", paneId, notifId),
        )
        builder.addAction(
            0, "Deny", actionPending(context, "deny", paneId, notifId),
        )
    }

    runCatching { NotificationManagerCompat.from(context).notify(notifId, builder.build()) }
}

private fun actionPending(
    context: Context,
    verb: String,
    paneId: String,
    notifId: Int,
): PendingIntent {
    val intent = Intent(context, ActionReceiver::class.java).apply {
        action = PUSH_ACTION
        putExtra("verb", verb)
        putExtra("pane_id", paneId)
        putExtra("notif_id", notifId)
    }
    // Distinct request code per (pane, verb) so extras don't collapse together.
    val requestCode = notifId * 2 + if (verb == "approve") 0 else 1
    return PendingIntent.getBroadcast(
        context,
        requestCode,
        intent,
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )
}

private fun ensureChannel(context: Context) {
    val mgr = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    if (mgr.getNotificationChannel(PUSH_CHANNEL_ID) == null) {
        val channel = NotificationChannel(
            PUSH_CHANNEL_ID,
            "Agent alerts",
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = "An agent is blocked and needs a decision"
            lockscreenVisibility = Notification.VISIBILITY_PUBLIC
        }
        mgr.createNotificationChannel(channel)
    }
}

/** Register the UnifiedPush endpoint with shep over a short-lived bridge call. */
private fun registerEndpointWithShep(context: Context, endpoint: String) {
    val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    val url = prefs.getString("url", null) ?: return
    val token = prefs.getString("token", null) ?: return
    Thread {
        val client = BridgeClient(url, token)
        if (client.connect(timeoutSeconds = 8) == null) {
            runCatching {
                client.call(
                    "push.register",
                    JSONObject()
                        .put("endpoint", endpoint)
                        .put("label", android.os.Build.MODEL ?: "android"),
                )
            }
            prefs.edit().putString("push_status", "push registered").apply()
        }
        client.close()
    }.start()
}
