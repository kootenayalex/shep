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

/**
 * Push notifications.
 *
 * shep's `[notifications] exec = "shep bridge notify-push"` sends a flat set of
 * string fields to every registered device over FCM, which delivers them to
 * [ShepMessagingService]. From there [showShepNotification] either posts one
 * notification per agent (a newer event for the same agent replaces the older
 * one — the `tag` field is the pane id) or, for `op = clear`, withdraws it:
 * the pane was looked at, on the desk or on another device.
 *
 * Approve/Deny fire [ActionReceiver], which sends `y`/`n` through a short-lived
 * [BridgeClient] via `pane.send_keys`; tapping the body deep-links into the
 * pane, which in turn tells shep the pane was seen. No persistent foreground
 * socket — the app lives entirely off push wake-ups (the ANDROID-COMPANION.md
 * battery guardrail).
 */

/**
 * The kinds of thing shep notifies about, mirroring `crate::config::NotifyKind`
 * one for one: what the server can put in `SHEP_NOTIFY_KIND`, the phone can
 * name, so a subscription chosen here always means something there.
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
    Review(
        "review", "review", "shep_review", "Ready for review",
        "A group is ready to review", NotificationManager.IMPORTANCE_DEFAULT,
    ),
    Task(
        "task", "task", "shep_task", "Task queue",
        "A queued task changed state", NotificationManager.IMPORTANCE_LOW,
    ),
    Working(
        "working", "working", "shep_working", "Agent working",
        "An agent started working", NotificationManager.IMPORTANCE_LOW,
    ),
    Idle(
        "idle", "idle", "shep_idle", "Agent idle",
        "An agent went quiet without finishing a run", NotificationManager.IMPORTANCE_LOW,
    ),
    Unknown(
        "unknown", "unknown", "shep_unknown", "Agent state unknown",
        "shep lost track of what an agent is doing", NotificationManager.IMPORTANCE_LOW,
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
                val saved = PairingStore.load(context)
                if (saved != null) {
                    val client = BridgeClient(saved.url, saved.token)
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
    /** What the device keys the notification on; the pane id from shep. */
    val tag: String = "",
    /** `show` (default, and what a server without ops sends) or `clear`. */
    val op: String = OP_SHOW,
) {
    val isClear: Boolean get() = op == OP_CLEAR

    companion object {
        const val OP_SHOW = "show"
        const val OP_CLEAR = "clear"

        /** Decode the flat string fields of a push message; absent keys are empty. */
        fun fromFields(field: (String) -> String?): ShepNotification = ShepNotification(
            kind = field("kind").orEmpty(),
            state = field("state").orEmpty(),
            agent = field("agent").orEmpty(),
            workspace = field("workspace").orEmpty(),
            paneId = field("pane_id").orEmpty(),
            title = field("title").orEmpty(),
            body = field("message").orEmpty(),
            tag = field("tag").orEmpty(),
            op = field("op")?.takeIf { it.isNotEmpty() } ?: OP_SHOW,
        )
    }
}

/**
 * The Android notification id for one agent. One id per agent — not per
 * (agent, kind) — so a "done" that follows a "blocked" replaces it rather than
 * stacking beside it, and a clear knows exactly what to take down.
 */
fun notificationIdFor(tag: String, paneId: String = "", agent: String = ""): Int =
    tag.ifEmpty { paneId.ifEmpty { agent.ifEmpty { "agent" } } }.hashCode()

/** What [showShepNotification] will do, decided without touching Android. */
sealed class NotificationPlan {
    abstract val id: Int

    data class Cancel(override val id: Int) : NotificationPlan()

    data class Post(
        override val id: Int,
        val kind: NotifyKind,
        val title: String,
        val body: String,
        /** Approve/Deny are offered only when an agent is waiting on an answer. */
        val offerAnswer: Boolean,
    ) : NotificationPlan()
}

/**
 * Turn a message into a plan. Approve/Deny appear only when an agent is
 * actually waiting on an answer — offering them on a "task done" would send a
 * keystroke nobody asked for.
 */
fun planNotification(notification: ShepNotification): NotificationPlan {
    val agent = notification.agent.ifEmpty { "agent" }
    val id = notificationIdFor(notification.tag, notification.paneId, agent)
    if (notification.isClear) return NotificationPlan.Cancel(id)
    val kind = NotifyKind.fromWire(notification.kind)
        // A server too old to send a kind only ever sent blocked transitions.
        ?: NotifyKind.Blocked
    val body = notification.body.ifEmpty {
        when (kind) {
            NotifyKind.Blocked -> "needs your attention"
            NotifyKind.Done -> "finished"
            NotifyKind.Review -> "ready for review"
            NotifyKind.Task -> "task queue changed"
            NotifyKind.Working -> "working"
            NotifyKind.Idle -> "went quiet"
            NotifyKind.Unknown -> "state unknown"
        }
    }
    // Task and review events name themselves ("task #4 done"); agent events are
    // identified by who and where instead.
    val title = notification.title.ifEmpty {
        if (notification.workspace.isNotEmpty()) "$agent · ${notification.workspace}" else agent
    }
    return NotificationPlan.Post(
        id = id,
        kind = kind,
        title = title,
        body = body,
        offerAnswer = kind == NotifyKind.Blocked && notification.paneId.isNotEmpty(),
    )
}

/** Build and post (or withdraw) the notification for one message. */
fun showShepNotification(context: Context, notification: ShepNotification) {
    when (val plan = planNotification(notification)) {
        is NotificationPlan.Cancel -> {
            runCatching { NotificationManagerCompat.from(context).cancel(plan.id) }
        }
        is NotificationPlan.Post -> postNotification(context, notification.paneId, plan)
    }
    // The blocked set may have just changed either way — repaint the widget.
    ShepWidgetProvider.refreshAll(context)
}

private fun postNotification(context: Context, paneId: String, plan: NotificationPlan.Post) {
    val kind = plan.kind
    val notifId = plan.id
    ensureChannel(context, kind)

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

    val builder = NotificationCompat.Builder(context, kind.channelId)
        .setSmallIcon(R.drawable.ic_notification)
        .setContentTitle(plan.title)
        .setContentText(plan.body)
        .setStyle(NotificationCompat.BigTextStyle().bigText(plan.body))
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

    if (plan.offerAnswer) {
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
