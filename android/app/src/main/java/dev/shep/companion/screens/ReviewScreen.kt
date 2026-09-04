package dev.shep.companion.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.material3.minimumInteractiveComponentSize
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import dev.shep.companion.AgentRow
import dev.shep.companion.BridgeClient
import dev.shep.companion.parseSnapshot
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.BackHeader
import dev.shep.companion.ui.components.ButtonTone
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.ExplainLine
import dev.shep.companion.ui.components.ExplainRow
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.NoticeTone
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.ShepCard
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
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
    var rawDiff by remember { mutableStateOf("") }
    var status by remember { mutableStateOf("loading diff…") }
    var notice by remember { mutableStateOf<String?>(null) }
    var noticeTone by remember { mutableStateOf(NoticeTone.Info) }
    var requesting by remember { mutableStateOf(false) }
    var confirmShip by remember { mutableStateOf(false) }
    var shipping by remember { mutableStateOf(false) }
    // `session.overview` cannot say whether a workspace is a worktree — the
    // parser hard-codes both fields to null/false — so a row that arrived from
    // the board would hide "merge it in" on exactly the workspaces that need
    // it. `session.snapshot` does know, and Review is the one screen that
    // cares, so it asks once on the way in.
    var isWorktree by remember { mutableStateOf(row.isWorktree) }
    var worktreeRepo by remember { mutableStateOf(row.worktreeRepo) }
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
            rawDiff = d
            diff = if (d.isEmpty()) AnnotatedString("") else colorizeDiff(d)
            status = if (stat.isEmpty() && d.isEmpty()) "no changes to review" else ""
        }.onFailure { status = "diff failed: ${it.message}" }
        withContext(Dispatchers.IO) {
            runCatching { parseSnapshot(client.call("session.snapshot")) }.getOrNull()
        }?.find { it.workspaceId == row.workspaceId }?.let {
            isWorktree = it.isWorktree
            worktreeRepo = it.worktreeRepo ?: worktreeRepo
        }
    }
    val stats = remember(stat) { parseDiffStat(stat) }
    val hunks = remember(rawDiff) { splitDiffByFile(rawDiff) }

    Column(Modifier.fillMaxSize()) {
        BackHeader("agent", onBack) {
            Column(Modifier.weight(1f)) {
                Text("review · ${row.workspaceLabel}", style = ShepType.agentName)
                Text(
                    worktreeRepo?.let { "$it${if (isWorktree) " · own copy" else ""}" }
                        ?: "the main copy",
                    style = ShepType.meta,
                )
            }
        }
        notice?.let { Notice(it, tone = noticeTone, onDismiss = { notice = null }) }
        ExplainRow("what does merging do?") {
            ExplainLine(
                "own copy",
                "the agent worked in a separate folder, so nothing it did has touched " +
                    "your own files yet.",
            )
            ExplainLine(
                "merge it in",
                "copies its work into your main copy and deletes the separate folder. " +
                    "it refuses if either side has unsaved work.",
            )
            ExplainLine(
                "ask for changes",
                "sends what you type back to the agent and marks this as needing another go.",
            )
        }
        Box(Modifier.weight(1f).fillMaxWidth().background(ShepPalette.panelBg)) {
            if (stat.isEmpty() && diff.text.isEmpty()) {
                EmptyState(status.ifEmpty { "no changes" })
            } else {
                Column(
                    Modifier
                        .fillMaxSize()
                        .verticalScroll(scroll)
                        .padding(ShepSpace.small),
                ) {
                    if (stats == null) {
                        // A stat this build cannot read is still the truth; show it.
                        ShepCard {
                            Text("stat", style = ShepType.sectionLabel)
                            Spacer(Modifier.height(ShepSpace.tight))
                            Text(
                                stat,
                                style = ShepType.codeSmall.copy(color = ShepPalette.overlay0),
                            )
                        }
                        if (diff.text.isNotEmpty()) {
                            Spacer(Modifier.height(ShepSpace.small))
                            Text(diff, style = ShepType.codeSmall)
                        }
                    } else {
                        Text(
                            reviewSummary(row.displayName ?: row.agent, stats),
                            style = ShepType.summary,
                        )
                        Spacer(Modifier.height(ShepSpace.tight))
                        DiffLegend()
                        Spacer(Modifier.height(ShepSpace.small))
                        stats.perFile.forEach { file ->
                            FileDisclosure(file, hunks[file.path] ?: hunks[""])
                        }
                    }
                }
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
                "ask for changes",
                tone = ButtonTone.Quiet,
                modifier = Modifier.weight(1f),
            ) { requesting = true }
            ActionText(
                "approve",
                style = ShepType.action.copy(color = ShepPalette.green),
            ) {
                scope.launch {
                    withContext(Dispatchers.IO) {
                        runCatching {
                            client.call(
                                "workspace.set_review_state",
                                JSONObject()
                                    .put("workspace_id", row.workspaceId)
                                    .put("review_state", "approved"),
                            )
                        }
                    }
                        .onSuccess { notice = "✓ approved"; noticeTone = NoticeTone.Good }
                        .onFailure {
                            notice = "approve failed: ${it.message}"
                            noticeTone = NoticeTone.Bad
                        }
                }
            }
        }
        if (isWorktree) {
            ShepButton(
                if (shipping) "merging…" else "merge it in",
                enabled = !shipping,
                modifier = Modifier.fillMaxWidth(),
            ) { confirmShip = true }
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
                        "✓ sent to the agent"
                    } else {
                        "failed to send feedback"
                    }
                    noticeTone = if (ok) NoticeTone.Good else NoticeTone.Bad
                }
            },
        )
    }

    if (confirmShip) {
        AlertDialog(
            onDismissRequest = { confirmShip = false },
            containerColor = ShepPalette.surfaceDim,
            title = { Text("merge it in?", style = ShepType.sheetTitle) },
            text = {
                Text(
                    "adds ${row.displayName ?: row.agent}'s work to your main copy of " +
                        "${worktreeRepo ?: "this project"}, and removes the separate folder it " +
                        "worked in. refuses if either side has unsaved changes or the two " +
                        "disagree about the same lines. can't be undone.",
                    style = ShepType.bodySmall,
                )
            },
            confirmButton = {
                ActionText(
                    "merge it in",
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
                                .onSuccess { notice = "✓ $it"; noticeTone = NoticeTone.Good; onBack() }
                                .onFailure {
                                    notice = "merge failed: ${it.message}"
                                    noticeTone = NoticeTone.Bad
                                }
                        }
                }
            },
            dismissButton = {
                ActionText("cancel", onClick = { confirmShip = false })
            },
        )
    }
}

@Composable
fun RequestChangesSheet(onDismiss: () -> Unit, onSubmit: (String) -> Unit) {
    var text by remember { mutableStateOf("") }
    ShepSheet(title = "ask for changes", onDismiss = onDismiss) {
        Text(
            "goes straight to the agent, and marks this as needing another go.",
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

/** What the two colours in a file row mean, said once at the top. */
@Composable
private fun DiffLegend() {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier
                .width(ShepSize.legendBarWidth)
                .height(ShepSize.gaugeHeight)
                .clip(ShepShape.bar)
                .background(ShepPalette.green),
        )
        Spacer(Modifier.width(ShepSpace.tight))
        Text("added", style = ShepType.metaSmall)
        Spacer(Modifier.width(ShepSpace.small))
        Box(
            Modifier
                .width(ShepSize.legendBarWidth)
                .height(ShepSize.gaugeHeight)
                .clip(ShepShape.bar)
                .background(ShepPalette.red),
        )
        Spacer(Modifier.width(ShepSpace.tight))
        Text("removed", style = ShepType.metaSmall)
    }
}

/**
 * One changed file: its name and size always, its code on request.
 *
 * The whole diff used to arrive as one unbroken block, which is readable on a
 * desktop and is a wall on a phone. A file at a time is the unit a person
 * actually decides about.
 */
@Composable
private fun FileDisclosure(file: FileStat, hunk: String?) {
    var open by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier
                .fillMaxWidth()
                .minimumInteractiveComponentSize()
                .clickable(enabled = hunk != null) { open = !open }
                .padding(vertical = ShepSpace.tight),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text(file.path, style = ShepType.bodySmall)
                Text(
                    if (file.binary) {
                        "not text — nothing to read here"
                    } else {
                        "+${file.added} −${file.removed}"
                    },
                    style = ShepType.metaSmall,
                )
            }
            if (hunk != null) {
                Text(if (open) "hide the code" else "show the code", style = ShepType.explainLabel)
            }
        }
        if (open && hunk != null) {
            Text(colorizeDiff(hunk), style = ShepType.codeSmall)
        }
    }
}
