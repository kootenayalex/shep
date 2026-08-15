package dev.shep.companion.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import dev.shep.companion.SessionHost
import dev.shep.companion.SessionTotals
import dev.shep.companion.humanBytes
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType

/**
 * The two-row pulse strip from the top of the desktop session board: what the
 * agents are doing, then what the machine is doing.
 *
 * Both rows scroll horizontally rather than wrapping or truncating — a phone
 * is narrower than a terminal, and a half-shown number is worse than one the
 * user can swipe to.
 */
@Composable
fun DashboardStrip(totals: SessionTotals, host: SessionHost, statusColor: (String) -> Color) {
    Column(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
        verticalArrangement = Arrangement.spacedBy(ShepSpace.tight),
    ) {
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Label("agents ")
            Value(totals.agents.toString())
            if (totals.attention > 0) {
                Spacer(Modifier.width(ShepSpace.small))
                // The one number on this strip that is a call to action.
                Text(
                    "${totals.attention} need you",
                    style = ShepType.meta.copy(
                        color = ShepPalette.red,
                        fontWeight = FontWeight.Bold,
                    ),
                )
            }
            listOf(
                "blocked" to totals.blocked,
                "done" to totals.done,
                "working" to totals.working,
                "idle" to totals.idle,
            ).forEach { (label, count) ->
                Separator()
                Text("$label ", style = ShepType.meta.copy(color = statusColor(label)))
                Value(count.toString())
            }
            if (totals.workspaces > 0) {
                Separator()
                Label("${totals.workspaces} ws · ${totals.tabs} tabs · ${totals.panes} panes")
            }
            if (totals.queuedInput > 0) {
                Separator()
                Text("⇥${totals.queuedInput} queued", style = ShepType.meta.copy(color = ShepPalette.teal))
            }
            totals.pendingTasks?.takeIf { it > 0 }?.let {
                Separator()
                Value("$it tasks")
            }
        }

        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Label("shep ")
            Value(host.version ?: "—")
            Separator()
            Label("load ")
            // An unreadable or unsampled vital prints an em dash. Never a zero:
            // "0% load" and "we don't know" are very different claims.
            if (host.loadPercent != null) {
                Text(
                    "${host.loadPercent}%",
                    style = ShepType.meta.copy(
                        color = when {
                            host.loadPercent >= 100 -> ShepPalette.red
                            host.loadPercent >= 70 -> ShepPalette.yellow
                            else -> ShepPalette.text
                        },
                    ),
                )
                host.cores?.let { Label(" of $it cores") }
            } else {
                Label("—")
            }
            Separator()
            Label("mem ")
            if (host.memoryPercent != null) {
                Text(
                    "${host.memoryPercent}%",
                    style = ShepType.meta.copy(
                        color = when {
                            host.memoryPercent >= 90 -> ShepPalette.red
                            host.memoryPercent >= 75 -> ShepPalette.yellow
                            else -> ShepPalette.text
                        },
                    ),
                )
                if (host.memoryUsedBytes != null && host.memoryTotalBytes != null) {
                    Label(
                        " ${humanBytes(host.memoryUsedBytes)} of " +
                            humanBytes(host.memoryTotalBytes)
                    )
                }
            } else {
                Label("—")
            }
        }
    }
}

@Composable
private fun Label(text: String) {
    Text(text, style = ShepType.meta)
}

@Composable
private fun Value(text: String) {
    Text(text, style = ShepType.meta.copy(color = ShepPalette.text))
}

@Composable
private fun Separator() {
    Text("  ·  ", style = ShepType.meta.copy(color = ShepPalette.surface1))
}
