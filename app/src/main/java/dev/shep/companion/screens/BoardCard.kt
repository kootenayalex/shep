package dev.shep.companion.screens

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shep.companion.AgentRow
import dev.shep.companion.formatAge
import dev.shep.companion.ui.theme.ShepPalette

/**
 * One agent, laid out as the desktop board's card: identity line, placement
 * line, status, what the screen is saying, then where it is working plus a
 * context gauge.
 *
 * The lines are the same lines, in the same order, as `render_card` in
 * src/ui/board.rs. That correspondence is the point — this is a companion
 * view of one screen, not a second design.
 *
 * [displayName] is the session-wide-unique name the server decides, not
 * `row.agent`: a screenful of cards all reading "claude" is the thing this
 * card exists to stop, and deciding it server-side is what keeps this card and
 * the desktop board naming the same agent the same way. Long-pressing offers
 * to name it something better.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun BoardCard(
    row: AgentRow,
    statusColor: (String) -> Color,
    displayName: String = row.agent,
    onLongClick: () -> Unit = {},
    onClick: () -> Unit,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(ShepPalette.surface0)
            .combinedClickable(onLongClick = onLongClick, onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        // Line 1 — dot, agent, model, queued badge … location.
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier
                    .size(10.dp)
                    .clip(CircleShape)
                    .background(statusColor(row.status))
            )
            Spacer(Modifier.width(10.dp))
            Text(
                displayName,
                color = ShepPalette.text,
                fontWeight = FontWeight.SemiBold,
                fontSize = 15.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            row.displayAgent?.let {
                Spacer(Modifier.width(8.dp))
                Text(it, color = ShepPalette.teal, fontSize = 12.sp)
            }
            if (row.queuedInput > 0) {
                Spacer(Modifier.width(8.dp))
                Text("⇥${row.queuedInput}", color = ShepPalette.teal, fontSize = 12.sp)
            }
            Spacer(Modifier.weight(1f))
            row.location?.let {
                Text(
                    it,
                    color = ShepPalette.overlay0,
                    fontSize = 12.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }

        // Line 2 — workspace · branch … age.
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                listOfNotNull(row.workspaceLabel.takeIf { it.isNotBlank() }, row.branch)
                    .joinToString(" · "),
                color = ShepPalette.overlay0,
                fontSize = 12.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            row.stateAgeSeconds?.let {
                Spacer(Modifier.width(8.dp))
                Text(formatAge(it), color = ShepPalette.overlay0, fontSize = 12.sp)
            }
        }

        // Line 3 — the agent's own status, in its state color.
        Text(
            row.customStatus ?: row.status,
            color = statusColor(row.status),
            fontSize = 13.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )

        // Line 4 — what the pane is actually saying. Italic because it is a
        // hint read off the screen, not something shep is asserting.
        row.activityLine?.let {
            Text(
                it,
                color = ShepPalette.overlay0,
                fontSize = 12.sp,
                fontStyle = FontStyle.Italic,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }

        // Line 5 — where it is working … context gauge.
        if (row.cwd != null || row.contextPercent != null) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                row.cwd?.let {
                    Text(
                        it,
                        color = ShepPalette.overlay0,
                        fontSize = 11.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                }
                Spacer(Modifier.weight(1f))
                row.contextPercent?.let { ContextGauge(it) }
            }
        }
    }
}

/**
 * The context window as a bar plus its number.
 *
 * A bar because the thing worth seeing across a screenful of cards is *which
 * agent is nearly full*, and a column of bare percentages does not show that.
 * Warms through yellow to red as it fills, matching the desktop.
 */
@Composable
fun ContextGauge(percent: Int) {
    val clamped = percent.coerceIn(0, 100)
    val color = when {
        clamped >= 85 -> ShepPalette.red
        clamped >= 60 -> ShepPalette.yellow
        else -> ShepPalette.overlay0
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier
                .width(44.dp)
                .height(4.dp)
                .clip(RoundedCornerShape(2.dp))
                .background(ShepPalette.surface1)
        ) {
            Box(
                Modifier
                    .fillMaxWidth(clamped / 100f)
                    .height(4.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(color)
            )
        }
        Spacer(Modifier.width(6.dp))
        Text("$clamped%", color = color, fontSize = 11.sp)
    }
}
