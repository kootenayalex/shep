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
 * Push notifications, over either transport.
 *
 * shep's `[notifications] exec = "shep bridge notify-push"` sends a flat set of
 * string fields to every registered device. FCM delivers them to
 * [ShepMessagingService]; UnifiedPush delivers the same fields as a JSON body
 * to [PushReceiver]. Both hand off to [showShepNotification], so there is one
 * notification design and one place it can go wrong.
 *
 * From there: Approve/Deny fire [ActionReceiver], which sends `y`/`n` through a
 * short-lived [BridgeClient] via `pane.send_keys`; tapping the body deep-links
 * into the pane. No persistent foreground socket — the app lives entirely off
 * push wake-ups (the ANDROID-COMPANION.md battery guardrail).
 */

/**
 * The kinds of thing shep notifies about, mirroring `crate::config::NotifyKind`.
 *
 * Each gets its own Android channel so the system's own per-channel controls
 * work — silencing "done" while keeping "blocked" audible is a thing people
 * want, and Android already has that UI. The in-app toggles are a different
 * lever: they stop the *server* sending, which the phone's own settings cannot.
 */
enum class NotifyKind(
    val wire: String,
    val label: String,
    val channelId: String,
    val channelName: String,
    val description: String,
    val importance: Int,
) {
    Blocked(
        "blocked", "blocked", "shep_agent", "Agent blocked",
        "An agent is waiting on a decision", NotificationManager.IMPORTANCE_HIGH,
    ),
    Done(
        "done", "done", "shep_done", "Run finished",
        "An agent finished what it was doing", NotificationManager.IMPORTANCE_DEFAULT,
    ),
    Task(
        "task", "task", "shep_task", "Task queue",
        "A queued task changed state", NotificationManager.IMPORTANCE_LOW,
    ),
    Review(
        "review", "review", "shep_review", "Ready for review",
        "A space is ready to review", NotificationManager.IMPORTANCE_DEFAULT,
    );

    companion object {
        fun fromWire(wire: String): NotifyKind? = entries.find { it.wire == wire }

        /** What a device subscribes to when nobody has chosen yet. */
        val DEFAULTS = setOf(Blocked, Done, Review)
    }
}

/** Kept for the pre-kinds notification channel, which is now Blocked's. */
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
        showShepNotification(
            context,
            ShepNotification(
                kind = json.optString("kind"),
                state = json.optString("state"),
                agent = json.optString("agent"),
                workspace = json.optString("workspace"),
                paneId = json.optString("pane_id"),
                title = json.optString("title"),
                body = json.optString("message"),
            ),
        )
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

/** One notification's worth of fields, as shep sends them. */
data class ShepNotification(
    val kind: String,
    val state: String,
    val agent: String,
    val workspace: String,
    val paneId: String,
    val title: String,
    val body: String,
)

/**
 * Build and post the notification. Approve/Deny appear only when an agent is
 * actually waiting on an answer — offering them on a "task done" would send a
 * keystroke nobody asked for.
 */
fun showShepNotification(context: Context, notification: ShepNotification) {
    val kind = NotifyKind.fromWire(notification.kind)
        // A server too old to send a kind only ever sent blocked transitions.
        ?: NotifyKind.Blocked
    val agent = notification.agent.ifEmpty { "agent" }
    val paneId = notification.paneId
    val body = notification.body.ifEmpty {
        when (kind) {
            NotifyKind.Blocked -> "needs your attention"
            NotifyKind.Done -> "finished"
            NotifyKind.Task -> "task queue changed"
            NotifyKind.Review -> "ready for review"
        }
    }
    ensureChannel(context, kind)
    // Keyed per pane per kind, so a "done" does not overwrite the "blocked"
    // still waiting for an answer on the same pane.
    val identity = if (paneId.isNotEmpty()) paneId else agent
    val notifId = (identity + "/" + kind.wire).hashCode()

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

    // Task and review events name themselves ("task #4 done"); agent events are
    // identified by who and where instead.
    val title = notification.title.ifEmpty {
        if (notification.workspace.isNotEmpty()) "$agent · ${notification.workspace}" else agent
    }
    val builder = NotificationCompat.Builder(context, kind.channelId)
        .setSmallIcon(R.drawable.ic_notification)
        .setContentTitle(title)
        .setContentText(body)
        .setStyle(NotificationCompat.BigTextStyle().bigText(body))
        .setPriority(
            if (kind == NotifyKind.Blocked) {
                NotificationCompat.PRIORITY_HIGH
            } else {
                NotificationCompat.PRIORITY_DEFAULT
            },
        )
        .setCategory(
            if (kind == NotifyKind.Blocked) {
                NotificationCompat.CATEGORY_CALL
            } else {
                NotificationCompat.CATEGORY_STATUS
            },
        )
        .setAutoCancel(true)
        .setContentIntent(openPending)

    if (kind == NotifyKind.Blocked && paneId.isNotEmpty()) {
        builder.addAction(
            0, "Approve", actionPending(context, "approve", paneId, notifId),
        )
        builder.addAction(
            0, "Deny", actionPending(context, "deny", paneId, notifId),
        )
    }

    runCatching { NotificationManagerCompat.from(context).notify(notifId, builder.build()) }
    // The blocked set may have just changed — repaint the widget.
    ShepWidgetProvider.refreshAll(context)
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

private fun ensureChannel(context: Context, kind: NotifyKind) {
    val mgr = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    if (mgr.getNotificationChannel(kind.channelId) == null) {
        val channel = NotificationChannel(
            kind.channelId,
            kind.channelName,
            kind.importance,
        ).apply {
            description = kind.description
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
