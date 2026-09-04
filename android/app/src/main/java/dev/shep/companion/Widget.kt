package dev.shep.companion

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.widget.RemoteViews

/**
 * A6 home-screen widget: blocked-agent count + the top blocked agent, in the
 * shep palette. Battery guardrail holds — no persistent socket; updates are
 * pull-on-demand: the system's periodic update (30 min floor), a tap on the
 * ↻ glyph, and a nudge from [PushReceiver.onMessage] whenever a push lands
 * (the moment the count actually changed). Tapping the body deep-links into
 * the top blocked pane, or just opens the app when nothing is blocked.
 */
class ShepWidgetProvider : AppWidgetProvider() {

    override fun onUpdate(context: Context, appWidgetManager: AppWidgetManager, appWidgetIds: IntArray) {
        refreshAll(context)
    }

    override fun onReceive(context: Context, intent: Intent) {
        super.onReceive(context, intent)
        if (intent.action == ACTION_REFRESH) refreshAll(context)
    }

    companion object {
        const val ACTION_REFRESH = "dev.shep.companion.WIDGET_REFRESH"

        /** Re-fetch the snapshot off the main thread and repaint every widget. */
        fun refreshAll(context: Context) {
            val mgr = AppWidgetManager.getInstance(context)
            val ids = mgr.getAppWidgetIds(ComponentName(context, ShepWidgetProvider::class.java))
            if (ids.isEmpty()) return

            val saved = PairingStore.load(context)
            val url = saved?.url
            val token = saved?.token

            Thread {
                // "unpaired" | "offline" | "ok"
                var state = if (url == null || token == null) "unpaired" else "offline"
                var blockedCount = 0
                var topLabel: String? = null
                var topPane: String? = null
                if (state == "offline") {
                    val client = BridgeClient(url!!, token!!)
                    if (client.connect(timeoutSeconds = 6) == null) {
                        runCatching {
                            val rows = parseSnapshot(client.call("session.snapshot"))
                            val blocked = rows.filter { it.status == "blocked" }
                            blockedCount = blocked.size
                            topLabel = blocked.firstOrNull()?.let { "${it.agent} · ${it.workspaceLabel}" }
                            topPane = blocked.firstOrNull()?.paneId
                            state = "ok"
                        }
                        client.close()
                    }
                }
                for (id in ids) {
                    mgr.updateAppWidget(id, buildViews(context, state, blockedCount, topLabel, topPane))
                }
            }.start()
        }

        private fun buildViews(
            context: Context,
            state: String,
            blockedCount: Int,
            topLabel: String?,
            topPane: String?,
        ): RemoteViews {
            val views = RemoteViews(context.packageName, R.layout.widget_blocked)

            val (headline, headlineColor) = when {
                state != "ok" -> state to 0xFF9C948A.toInt()            // subtext
                blockedCount > 0 -> "$blockedCount blocked" to 0xFFD9695F.toInt() // red
                else -> "all clear" to 0xFF9BC177.toInt()               // green
            }
            views.setTextViewText(R.id.widget_headline, headline)
            views.setTextColor(R.id.widget_headline, headlineColor)
            views.setTextViewText(
                R.id.widget_detail,
                when {
                    state == "unpaired" -> "open shep to pair"
                    state == "offline" -> "bridge unreachable"
                    topLabel != null -> topLabel
                    else -> "no agent needs you"
                },
            )

            // Body tap: deep-link the top blocked pane (the A3 notification
            // path), else plain app open. Refresh glyph: re-pull now.
            val launch = if (topPane != null) {
                Intent(context, MainActivity::class.java).apply {
                    action = Intent.ACTION_VIEW
                    data = Uri.parse("shep://pane?pane=${Uri.encode(topPane)}")
                    flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
                }
            } else {
                Intent(context, MainActivity::class.java).apply {
                    flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
                }
            }
            views.setOnClickPendingIntent(
                R.id.widget_body,
                PendingIntent.getActivity(
                    context, 0, launch,
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                ),
            )
            views.setOnClickPendingIntent(
                R.id.widget_refresh,
                PendingIntent.getBroadcast(
                    context, 1,
                    Intent(context, ShepWidgetProvider::class.java).setAction(ACTION_REFRESH),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                ),
            )
            return views
        }
    }
}
