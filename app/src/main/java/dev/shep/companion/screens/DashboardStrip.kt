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
 * Written to fit a phone's width rather than to be swiped. Both rows still
 * scroll horizontally as a backstop for a narrow device or a large font scale,
 * but a strip you have to scroll to finish reading is one you stop reading, so
 * nothing here is spelled out at terminal length: counts that are zero are left
 * off entirely, and the machine's vitals are abbreviated.
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
                    style = ShepType.metaSmall.copy(
                        color = ShepPalette.red,
                        fontWeight = FontWeight.Bold,
                    ),
                )
            }
            // Only states that are actually happening. `blocked 0 · done 0` is
            // three quarters of this row on a quiet session, and it pushed the
            // states that were happening off the right edge.
            listOf(
                "blocked" to totals.blocked,
                "done" to totals.done,
                "working" to totals.working,
                "idle" to totals.idle,
            ).filter { (_, count) -> count > 0 }.forEach { (label, count) ->
                Separator()
                Text("$label ", style = ShepType.metaSmall.copy(color = statusColor(label)))
                Value(count.toString())
            }
            if (totals.queuedInput > 0) {
                Separator()
                Text(
                    "⇥${totals.queuedInput} queued",
                    style = ShepType.metaSmall.copy(color = ShepPalette.teal),
                )
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
                    style = ShepType.metaSmall.copy(
                        color = when {
                            host.loadPercent >= 100 -> ShepPalette.red
                            host.loadPercent >= 70 -> ShepPalette.yellow
                            else -> ShepPalette.text
                        },
                    ),
                )
                host.cores?.let { Label("/${it}c") }
            } else {
                Label("—")
            }
            Separator()
            Label("mem ")
            if (host.memoryPercent != null) {
                Text(
                    "${host.memoryPercent}%",
                    style = ShepType.metaSmall.copy(
                        color = when {
                            host.memoryPercent >= 90 -> ShepPalette.red
                            host.memoryPercent >= 75 -> ShepPalette.yellow
                            else -> ShepPalette.text
                        },
                    ),
                )
                if (host.memoryUsedBytes != null && host.memoryTotalBytes != null) {
                    Label(
                        " ${humanBytes(host.memoryUsedBytes)}/" +
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
    Text(text, style = ShepType.metaSmall)
}

@Composable
private fun Value(text: String) {
    Text(text, style = ShepType.metaSmall.copy(color = ShepPalette.text))
}

@Composable
private fun Separator() {
    Text(" · ", style = ShepType.metaSmall.copy(color = ShepPalette.surface1))
}
