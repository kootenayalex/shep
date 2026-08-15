package dev.shep.companion.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import dev.shep.companion.AgentRow
import dev.shep.companion.BridgeClient
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.ButtonTone
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback

/** Tint diff lines: green added, red removed, copper hunk headers, dim context. */
fun colorizeDiff(diff: String): AnnotatedString =
    buildAnnotatedString {
        diff.lineSequence().forEach { line ->
            val color = when {
                line.startsWith("+++") || line.startsWith("---") -> ShepPalette.overlay0
                line.startsWith("@@") -> ShepPalette.accent
                line.startsWith("+") -> ShepPalette.green
                line.startsWith("-") -> ShepPalette.red
                else -> ShepPalette.text
            }
            withStyle(SpanStyle(color = color)) { append(line) }
            append("\n")
        }
    }

/**
 * Review & Ship (A5): the workspace's diff via `workspace.diff`, with Request
 * changes (feedback → agent pane + `workspace.set_review_state`) and, for a
 * linked worktree, Ship (`workspace.ship` merge → `worktree.remove` cleanup).
 *
 * Reached through Agents → pane → review. That entry point existed and was
 * visible and tappable and rippled for months while doing nothing at all: the
 * screen was finished, the API methods were finished, and no call site ever
 * passed the callback. [PaneScreen] now owns it exactly as it owns history —
 * both are full-screen pushes over a pane, so neither needs threading up
 * through the nav shell.
 */
@Composable
fun ReviewScreen(client: BridgeClient, row: AgentRow, onBack: () -> Unit) {
    var stat by remember { mutableStateOf("") }
    var diff by remember { mutableStateOf(AnnotatedString("")) }
    var status by remember { mutableStateOf("loading diff…") }
    var notice by remember { mutableStateOf<String?>(null) }
    var requesting by remember { mutableStateOf(false) }
    var confirmShip by remember { mutableStateOf(false) }
    var shipping by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val scroll = rememberScrollState()
    val haptics = LocalHapticFeedback.current

    LaunchedEffect(row.workspaceId) {
        withContext(Dispatchers.IO) {
            runCatching {
                client.call("workspace.diff", JSONObject().put("workspace_id", row.workspaceId), 20)
            }
        }.onSuccess {
            stat = it.optString("stat")
            val d = it.optString("diff")
            diff = if (d.isEmpty()) AnnotatedString("") else colorizeDiff(d)
            status = if (stat.isEmpty() && d.isEmpty()) "no changes to review" else ""
        }.onFailure { status = "diff failed: ${it.message}" }
    }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(ShepPalette.surfaceDim)
                .padding(horizontal = ShepSpace.small, vertical = ShepSpace.tight),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ActionText(
                "‹",
                style = ShepType.screenTitle.copy(color = ShepPalette.accent),
                description = "back to the pane",
                onClick = onBack,
            )
            Spacer(Modifier.width(ShepSpace.tight))
            Column(Modifier.weight(1f)) {
                Text("review · ${row.workspaceLabel}", style = ShepType.agentName)
                Text(
                    row.worktreeRepo?.let { "$it${if (row.isWorktree) " · worktree" else ""}" }
                        ?: "working tree",
                    style = ShepType.meta,
                )
            }
        }
        notice?.let { Notice(it, onDismiss = { notice = null }) }
        if (stat.isNotEmpty()) {
            Text(
                stat,
                style = ShepType.codeSmall.copy(color = ShepPalette.overlay0),
                modifier = Modifier
                    .fillMaxWidth()
                    .background(ShepPalette.surfaceDim)
                    .padding(ShepSpace.small),
            )
        }
        Box(Modifier.weight(1f).fillMaxWidth().background(ShepPalette.panelBg)) {
            if (diff.text.isEmpty()) {
                EmptyState(status.ifEmpty { "no changes" })
            } else {
                Text(
                    diff,
                    style = ShepType.codeSmall,
                    modifier = Modifier
                        .fillMaxSize()
                        .verticalScroll(scroll)
                        .padding(ShepSpace.small),
                )
            }
        }
        Row(
            Modifier
                .fillMaxWidth()
                .background(ShepPalette.surfaceDim)
                .padding(ShepSpace.small),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ShepButton(
                "request changes",
                tone = ButtonTone.Quiet,
                modifier = Modifier.weight(1f),
            ) { requesting = true }
            if (row.isWorktree) {
                ShepButton(
                    if (shipping) "shipping…" else "ship ⑂",
                    enabled = !shipping,
                    modifier = Modifier.weight(1f),
                ) { confirmShip = true }
            }
        }
    }

    if (requesting) {
        RequestChangesSheet(
            onDismiss = { requesting = false },
            onSubmit = { feedback ->
                requesting = false
                scope.launch {
                    val ok = withContext(Dispatchers.IO) {
                        runCatching {
                            // Feedback into the agent pane, then flag the workspace.
                            client.call(
                                "agent.send",
                                JSONObject().put("target", row.paneId).put("text", feedback),
                            )
                            client.call(
                                "workspace.set_review_state",
                                JSONObject()
                                    .put("workspace_id", row.workspaceId)
                                    .put("review_state", "changes_requested"),
                            )
                        }.isSuccess
                    }
                    notice = if (ok) {
                        "changes requested — sent to the agent"
                    } else {
                        "failed to send feedback"
                    }
                }
            },
        )
    }

    if (confirmShip) {
        AlertDialog(
            onDismissRequest = { confirmShip = false },
            containerColor = ShepPalette.surfaceDim,
            title = { Text("Ship this worktree?", style = ShepType.sheetTitle) },
            text = {
                Text(
                    "Merge ${row.worktreeRepo ?: "this worktree"}'s branch into its base " +
                        "checkout, then remove the worktree. Refuses if either side is dirty " +
                        "or the merge conflicts. This can't be undone.",
                    style = ShepType.bodySmall,
                )
            },
            confirmButton = {
                ActionText(
                    "Merge & ship",
                    style = ShepType.action.copy(color = ShepPalette.accent),
                ) {
                        // A merge that cannot be undone. It gets a tick.
                        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                        confirmShip = false
                        shipping = true
                        scope.launch {
                            val result = withContext(Dispatchers.IO) {
                                runCatching {
                                    val shipped = client.call(
                                        "workspace.ship",
                                        JSONObject().put("workspace_id", row.workspaceId),
                                        30,
                                    )
                                    // Cleanup: remove the now-merged worktree.
                                    runCatching {
                                        client.call(
                                            "worktree.remove",
                                            JSONObject().put("workspace_id", row.workspaceId),
                                            30,
                                        )
                                    }
                                    shipped.optString("message", "shipped")
                                }
                            }
                            shipping = false
                            result
                                .onSuccess { notice = "✓ $it"; onBack() }
                                .onFailure { notice = "ship failed: ${it.message}" }
                        }
                }
            },
            dismissButton = {
                ActionText("Cancel", onClick = { confirmShip = false })
            },
        )
    }
}

@Composable
fun RequestChangesSheet(onDismiss: () -> Unit, onSubmit: (String) -> Unit) {
    var text by remember { mutableStateOf("") }
    ShepSheet(title = "request changes", onDismiss = onDismiss) {
        Text(
            "goes straight into the agent's pane and flags the workspace.",
            style = ShepType.bodySmall.copy(color = ShepPalette.overlay0),
        )
        Spacer(Modifier.height(ShepSpace.medium))
        OutlinedTextField(
            value = text,
            onValueChange = { text = it },
            placeholder = { Text("what needs to change…", style = ShepType.fieldLabel) },
            textStyle = ShepType.body,
            modifier = Modifier.fillMaxWidth(),
            maxLines = 6,
        )
        Spacer(Modifier.height(ShepSpace.medium))
        ShepButton(
            "send to agent",
            onClick = { onSubmit(text.trim()) },
            enabled = text.isNotBlank(),
            modifier = Modifier.fillMaxWidth(),
        )
    }
}
