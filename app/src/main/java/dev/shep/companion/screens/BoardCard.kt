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
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextOverflow
import dev.shep.companion.AgentRow
import dev.shep.companion.formatAge
import dev.shep.companion.ui.components.Meter
import dev.shep.companion.ui.components.StateGlyph
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType

/**
 * One agent, laid out as the desktop board's card: identity line, placement
 * line, status, what the screen is saying, then where it is working plus a
 * context gauge.
 *
 * The lines are the same lines, in the same order, as `render_card` in
 * src/ui/board.rs. That correspondence is the point — this is a companion
 * view of one screen, not a second design. It now reads in the same typeface
 * too: every line here was Roboto until this pass, on the one surface the app
 * exists to show.
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
    modifier: Modifier = Modifier,
    displayName: String = row.agent,
    onLongClick: () -> Unit = {},
    onClick: () -> Unit,
) {
    Column(
        modifier
            .fillMaxWidth()
            .clip(ShepShape.card)
            .background(ShepPalette.surface0)
            .combinedClickable(onLongClick = onLongClick, onClick = onClick)
            .padding(ShepSpace.card),
        verticalArrangement = Arrangement.spacedBy(ShepSpace.hair),
    ) {
        // Line 1 — state, agent, model, queued badge … location.
        //
        // The identity group is one weighted child and the location is not, so
        // the location measures at its natural width and the name gets every
        // pixel that is left. It used to be a weighted name beside a weighted
        // spacer, which splits the row fifty-fifty however short the name is —
        // so an agent called "claude · workmayt" ellipsised at "workm…" with
        // half a card of empty space beside it.
        Row(verticalAlignment = Alignment.CenterVertically) {
            Row(
                Modifier.weight(1f),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                StateGlyph(row.status)
                Spacer(Modifier.width(ShepSpace.small))
                Text(
                    displayName,
                    style = ShepType.agentName,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                row.displayAgent?.let {
                    Spacer(Modifier.width(ShepSpace.small))
                    Text(
                        it,
                        style = ShepType.meta.copy(color = ShepPalette.teal),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (row.queuedInput > 0) {
                    Spacer(Modifier.width(ShepSpace.small))
                    Text("⇥${row.queuedInput}", style = ShepType.badge.copy(color = ShepPalette.teal))
                }
            }
            row.location?.let {
                Spacer(Modifier.width(ShepSpace.small))
                Text(it, style = ShepType.meta, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }

        // Line 2 — workspace · branch … age.
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                listOfNotNull(row.workspaceLabel.takeIf { it.isNotBlank() }, row.branch)
                    .joinToString(" · "),
                style = ShepType.meta,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            row.stateAgeSeconds?.let {
                Spacer(Modifier.width(ShepSpace.small))
                Text(formatAge(it), style = ShepType.meta)
            }
        }

        // Line 3 — the agent's own status, in its state color.
        Text(
            row.customStatus ?: row.status,
            style = ShepType.state.copy(color = statusColor(row.status)),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )

        // Line 4 — what the pane is actually saying. Italic because it is a
        // hint read off the screen, not something shep is asserting.
        row.activityLine?.let {
            Text(
                it,
                style = ShepType.meta.copy(fontStyle = FontStyle.Italic),
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
                        style = ShepType.metaSmall,
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
        Meter(
            fraction = clamped / 100f,
            color = color,
            height = ShepSize.gaugeHeight,
            modifier = Modifier.width(ShepSize.gaugeWidth),
        )
        Spacer(Modifier.width(ShepSpace.snug))
        Text("$clamped%", style = ShepType.badge.copy(color = color))
    }
}
